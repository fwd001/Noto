//! 同步引擎的持久化协作面：脏集枚举、outbox、远端游标、冲突收件箱、账户登记。
//!
//! 存储层依然**不知道同步**（ARCHITECTURE-MAP §2）：这里只有"读写这些表"的能力，
//! 没有轮次、退避、租约语义。所有写入仍走单写者 + 单事务（I5）。

use crate::error::StoreError;
use crate::rows;
use crate::store::Store;
use crate::types::*;
use notera_core::{EntityId, EntityKind, Rev};
use rusqlite::{params, Connection, OptionalExtension};

/// 契约里的 `EntityId` 只装 UUID；附件是内容寻址（64hex），此时用 nil UUID 占位。
const NIL_ID: &str = "00000000-0000-0000-0000-000000000000";

/// nil UUID：表示"该行的键不是 UUID，请用 [`SyncOperation::entity_key`] /
/// [`RemoteIndexEntry::sha256`]"。绝不用随机 ID 冒充。
fn nil_id() -> EntityId {
    EntityId::parse(NIL_ID).unwrap_or_else(|_| EntityId::parse(NIL_ID).expect("nil UUID 必须可解析"))
}

/// 只在 `entity_key` 确实是 UUID 时填充 `id_`。
fn id_or_nil(s: &str) -> EntityId {
    EntityId::parse(s).unwrap_or_else(|_| nil_id())
}

/// `sync_state` 的镜像行（同步引擎的权威游标；存储层不解释语义）。
#[derive(Clone, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct SyncStateRow {
    pub account_id: String,
    pub root_id: Option<String>,
    pub phase: String,
    pub seq_applied: i64,
    pub manifest_etag: Option<String>,
    pub lease_token: Option<String>,
    pub lease_expires_at: Option<String>,
    pub last_round_at: Option<String>,
    pub last_success_at: Option<String>,
    pub next_attempt_at: Option<String>,
    pub consecutive_failures: i64,
    pub needs_relist: bool,
    pub last_error_code: Option<String>,
    pub last_error_at: Option<String>,
}

impl Store {
    // -------------------------------------------------------------- 账户 ---

    /// 登记/更新一个远端账户（outbox 的 `account_id` 是 NOT NULL FK，必须先有账户行）。
    pub fn register_account(&self, id: &str, label: &str, base_url: &str) -> Result<(), StoreError> {
        let device = self.device_id().to_string();
        self.write_tx(|tx, now| {
            tx.execute(
                "INSERT INTO sync_accounts (id, label, base_url, root_prefix, auth_kind, tls_policy, device_id, enabled, created_at)
                 VALUES (?1,?2,?3,'/.notes','basic','strict',?4,1,?5)
                 ON CONFLICT(id) DO UPDATE SET label=excluded.label, base_url=excluded.base_url",
                params![id, label, base_url, device, now],
            )?;
            tx.execute(
                "INSERT INTO sync_state (account_id, phase) VALUES (?1,'unconfigured') ON CONFLICT(account_id) DO NOTHING",
                [id],
            )?;
            Ok(())
        })
    }

    pub fn set_account_enabled(&self, id: &str, enabled: bool) -> Result<(), StoreError> {
        self.write_tx(|tx, now| {
            let n = tx.execute("UPDATE sync_accounts SET enabled = ?2 WHERE id = ?1", params![id, enabled as i64])?;
            if n == 0 {
                return Err(StoreError::Constraint(format!("账户不存在: {id}")));
            }
            let _ = now;
            Ok(())
        })
    }

    /// 记下 §5 的探测结果。`mask` 是 `notera_webdav::Caps::mask()` 的值 ——
    /// 存储层不认识那些位，只负责原样存回去。
    pub fn set_account_caps(&self, id: &str, mask: u32) -> Result<(), StoreError> {
        let at = self.now();
        self.write_tx(|tx, _| {
            let n = tx.execute("UPDATE sync_accounts SET cap_mask = ?2, caps_probed_at = ?3 WHERE id = ?1", params![id, mask as i64, at])?;
            if n == 0 {
                return Err(StoreError::Constraint(format!("账户不存在: {id}")));
            }
            Ok(())
        })
    }

    /// `None` = 从未探测过（调用方应使用保守默认，而不是当成"全不支持"）。
    pub fn account_caps(&self, id: &str) -> Result<Option<u32>, StoreError> {
        let conn = self.read()?;
        let v = conn.query_row("SELECT cap_mask FROM sync_accounts WHERE id = ?1", [id], |r| r.get::<_, Option<i64>>(0));
        match v {
            Ok(Some(m)) => Ok(Some(m.max(0) as u32)),
            Ok(None) => Ok(None),
            Err(rusqlite::Error::QueryReturnedNoRows) => Err(StoreError::Constraint(format!("账户不存在: {id}"))),
            Err(e) => Err(StoreError::from(e)),
        }
    }

    /// 上次 §5 探测的时刻（`None` = 从未探测）。存的是字符串，"多久算过期"
    /// 是同步侧的策略，不放进存储层。
    pub fn account_caps_probed_at(&self, id: &str) -> Result<Option<String>, StoreError> {
        let conn = self.read()?;
        let v = conn.query_row("SELECT caps_probed_at FROM sync_accounts WHERE id = ?1", [id], |r| r.get::<_, Option<String>>(0));
        match v {
            Ok(t) => Ok(t),
            Err(rusqlite::Error::QueryReturnedNoRows) => Err(StoreError::Constraint(format!("账户不存在: {id}"))),
            Err(e) => Err(StoreError::from(e)),
        }
    }

    pub fn account_ids(&self) -> Result<Vec<String>, StoreError> {
        let conn = self.read()?;
        Ok(rows::enabled_accounts(&conn)?)
    }

    pub fn account_exists(&self, id: &str) -> Result<bool, StoreError> {
        let conn = self.read()?;
        Ok(conn
            .query_row("SELECT EXISTS(SELECT 1 FROM sync_accounts WHERE id = ?1)", [id], |r| r.get::<_, i64>(0))?
            != 0)
    }

    pub fn sync_state(&self, account: &str) -> Result<Option<SyncStateRow>, StoreError> {
        let account = account.to_string();
        let conn = self.read()?;
        let row = conn
            .query_row(
                "SELECT account_id, root_id, phase, seq_applied, manifest_etag, lease_token, lease_expires_at,
                        last_round_at, last_success_at, next_attempt_at, consecutive_failures, needs_relist,
                        last_error_code, last_error_at
                   FROM sync_state WHERE account_id = ?1",
            [account.as_str()],
            |r| {
                Ok(SyncStateRow {
                    account_id: r.get(0)?,
                    root_id: r.get(1)?,
                    phase: r.get(2)?,
                    seq_applied: r.get(3)?,
                    manifest_etag: r.get(4)?,
                    lease_token: r.get(5)?,
                    lease_expires_at: r.get(6)?,
                    last_round_at: r.get(7)?,
                    last_success_at: r.get(8)?,
                    next_attempt_at: r.get(9)?,
                    consecutive_failures: r.get(10)?,
                    needs_relist: r.get::<_, i64>(11)? != 0,
                    last_error_code: r.get(12)?,
                    last_error_at: r.get(13)?,
                })
            },
        )
        .optional()?;
        Ok(row)
    }

    /// 整行覆盖式写回（同步引擎持有权威状态，存储层只落盘）。
    pub fn set_sync_state(&self, s: &SyncStateRow) -> Result<(), StoreError> {
        let s = s.clone();
        self.write_tx(|tx, _now| {
            let n = tx.execute(
                "UPDATE sync_state SET root_id=?2, phase=?3, seq_applied=?4, manifest_etag=?5, lease_token=?6,
                        lease_expires_at=?7, last_round_at=?8, last_success_at=?9, next_attempt_at=?10,
                        consecutive_failures=?11, needs_relist=?12, last_error_code=?13, last_error_at=?14
                  WHERE account_id=?1",
                params![
                    s.account_id, s.root_id, s.phase, s.seq_applied, s.manifest_etag, s.lease_token,
                    s.lease_expires_at, s.last_round_at, s.last_success_at, s.next_attempt_at,
                    s.consecutive_failures, s.needs_relist as i64, s.last_error_code, s.last_error_at
                ],
            )?;
            if n == 0 {
                return Err(StoreError::Constraint(format!("账户不存在: {}", s.account_id)));
            }
            Ok(())
        })
    }

    // ------------------------------------------------------------- 脏集 ---

    /// 脏集 = `rev != sync_rev`（DATA-MODEL §4.2），外加"永久删除尚未被远端确认"的墓碑。
    ///
    /// 硬性要求 6：有 `purged=1` 墓碑的记录**不会**被当作新增列入脏集（否则设备 B 会把
    /// 已永久删除的笔记重新上传 → 复活）。`account` 不存在时只返回实体表判脏结果。
    pub fn dirty_entities(&self, account: &str) -> Result<Vec<DirtyEntity>, StoreError> {
        let conn = self.read()?;
        let mut out: Vec<DirtyEntity> = Vec::new();
        let account_known: bool = conn
            .query_row("SELECT EXISTS(SELECT 1 FROM sync_accounts WHERE id = ?1)", [account], |r| r.get::<_, i64>(0))?
            != 0;

        collect_dirty(
            &conn,
            &mut out,
            EntityKind::Note,
            "SELECT n.id, n.rev, n.content_hash, n.sync_rev, n.deleted_at FROM notes n
              WHERE n.rev <> n.sync_rev
                AND NOT EXISTS(SELECT 1 FROM tombstones t
                                 WHERE t.entity_type='note' AND t.entity_id=n.id AND t.purged=1)
              ORDER BY n.id",
        )?;
        collect_dirty(
            &conn,
            &mut out,
            EntityKind::Folder,
            "SELECT f.id, f.rev, f.content_hash, f.sync_rev, f.deleted_at FROM folders f
              WHERE f.rev <> f.sync_rev
                AND NOT EXISTS(SELECT 1 FROM tombstones t
                                 WHERE t.entity_type='folder' AND t.entity_id=f.id AND t.purged=1)
              ORDER BY f.id",
        )?;

        if account_known {
            let mut stmt = conn.prepare(
                "SELECT t.entity_id, t.entity_type, t.rev, t.content_hash FROM tombstones t
                  WHERE t.purged = 1
                    AND COALESCE((SELECT r.rev FROM sync_remote_index r
                                   WHERE r.account_id = ?1 AND r.kind = t.entity_type
                                     AND r.entity_id = t.entity_id AND r.purged = 1), -1) < t.rev
                  ORDER BY t.entity_type, t.entity_id",
            )?;
            let rows = stmt.query_map([account], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, Option<String>>(3)?))
            })?;
            for row in rows {
                let (id, tag, rev, hash) = row?;
                let kind = rows::kind_from_tag(&tag)?;
                if kind == EntityKind::Attachment {
                    continue; // 附件走独立队列（SYNC-PROTOCOL §13），不在实体脏集里
                }
                out.push(DirtyEntity {
                    kind,
                    id: rows::parse_id(&id)?,
                    rev: Rev(rev.max(0) as u64),
                    content_hash: hash.unwrap_or_default(),
                    sync_rev: Rev(0),
                    why: DirtyWhy::Purged,
                });
            }
        }
        Ok(out)
    }

    // ----------------------------------------------------------- outbox ---

    /// 取一批待办并置 `inflight`（`attempts+1`）。单事务，崩溃后可重取。
    pub fn outbox_take(&self, account: &str, limit: usize) -> Result<Vec<SyncOperation>, StoreError> {
        let account = account.to_string();
        self.write_tx(|tx, now| {
            let mut stmt = tx.prepare(
                "SELECT id, dedupe_key, account_id, entity_type, entity_id, op, payload_rev, sha256, state, attempts
                   FROM sync_operations
                  WHERE account_id = ?1
                    AND state IN ('pending','failed')
                    AND (next_retry_at IS NULL OR next_retry_at <= ?2)
                  ORDER BY id LIMIT ?3",
            )?;
            let picked = stmt
                .query_map(params![account.as_str(), now, limit as i64], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, String>(5)?,
                        r.get::<_, Option<i64>>(6)?,
                        r.get::<_, Option<String>>(7)?,
                        r.get::<_, i64>(9)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            drop(stmt);
            let mut out = Vec::with_capacity(picked.len());
            for (id, dedupe, tag, entity_id, op, payload_rev, sha256, attempts) in picked {
                tx.execute(
                    "UPDATE sync_operations SET state='inflight', attempts = attempts + 1, updated_at = ?2 WHERE id = ?1",
                    params![id, now],
                )?;
                out.push(SyncOperation {
                    id,
                    dedupe_key: dedupe,
                    kind: rows::kind_from_tag(&tag)?,
                    id_: id_or_nil(&entity_id),
                    op: OpKind::parse(&op).ok_or_else(|| StoreError::Constraint(format!("未知 op: {op}")))?,
                    payload_rev: payload_rev.map(|v| Rev(v.max(0) as u64)),
                    sha256,
                    state: OpState::Inflight,
                    attempts: (attempts + 1).max(0) as u32,
                    entity_key: entity_id,
                    account_id: account.clone(),
                });
            }
            Ok(out)
        })
    }

    pub fn outbox_state(
        &self,
        id: i64,
        st: OpState,
        err: Option<&str>,
        next_retry: Option<&str>,
    ) -> Result<(), StoreError> {
        let st = st.as_str().to_string();
        let err = err.map(|s| s.to_string());
        let next_retry = next_retry.map(|s| s.to_string());
        self.write_tx(|tx, now| {
            let n = tx.execute(
                "UPDATE sync_operations
                    SET state = ?2, last_error = COALESCE(?3, last_error),
                        next_retry_at = ?4, updated_at = ?5
                  WHERE id = ?1",
                params![id, st, err, next_retry, now],
            )?;
            if n == 0 {
                return Err(StoreError::Constraint(format!("outbox 行不存在: {id}")));
            }
            Ok(())
        })
    }

    /// 结清某个实体某 rev 的待办（引擎手里只有"kind/id/rev"，没有行号也没有 dedupe_key，
    /// 所以定位只能按这三样）。返回 `false` = 没有匹配的待办行：调用方要能区分
    /// "结清了"和"没找到"，否则 outbox 会安静地停在 inflight，队列计数永远不掉。
    /// 只动 pending/inflight：`failed` 行归退避逻辑管，`done`/`superseded` 不该被改写。
    ///
    /// `kind` 刻意是 `EntityKind` 而不是字符串：`entity_type` 列写的是长标记
    /// （note/folder/attachment），而同步线上飘的是短标记（n/f/a）。这里收字符串的话，
    /// 传错词汇编译能过、UPDATE 匹配 0 行、待办静静停在 inflight —— 实测就这么坏过。
    pub fn outbox_settle(&self, account: &str, kind: EntityKind, id: &str, rev: i64, st: OpState) -> Result<bool, StoreError> {
        let account = account.to_string();
        let id = id.to_string();
        let st = st.as_str().to_string();
        self.write_tx(|tx, now| {
            let n = tx.execute(
                "UPDATE sync_operations SET state = ?5, updated_at = ?6
                  WHERE account_id = ?1 AND entity_type = ?2 AND entity_id = ?3 AND payload_rev = ?4
                    AND state IN ('pending','inflight')",
                params![account, rows::kind_tag(kind), id, rev, st, now],
            )?;
            Ok(n > 0)
        })
    }

    pub fn outbox_len(&self, account: &str, states: &[OpState]) -> Result<u32, StoreError> {
        let conn = self.read()?;
        let list: Vec<String> = if states.is_empty() {
            vec!["pending".into(), "inflight".into(), "failed".into()]
        } else {
            states.iter().map(|s| s.as_str().to_string()).collect()
        };
        let placeholders = list.iter().enumerate().map(|(i, _)| format!("?{}", i + 2)).collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT COUNT(*) FROM sync_operations WHERE account_id = ?1 AND state IN ({placeholders})"
        );
        let mut stmt = conn.prepare(&sql)?;
        let binds: Vec<&dyn rusqlite::ToSql> =
            std::iter::once(&account as &dyn rusqlite::ToSql)
                .chain(list.iter().map(|s| s as &dyn rusqlite::ToSql))
                .collect();
        Ok(stmt.query_row(binds.as_slice(), |r| r.get::<_, i64>(0))? as u32)
    }

    // ------------------------------------------------- 远端游标 / 确认点 ---

    /// push 成功：`sync_rev ← rev ; sync_hash ← hash`（DATA-MODEL §4.2）。
    ///
    /// 实体行已不存在但存在 `purged=1` 墓碑时，把确认写进 `sync_remote_index`
    /// （否则永久删除的传播没有可记录的确认点，墓碑会永远算作脏）。
    pub fn mark_synced(&self, kind: EntityKind, id: &EntityId, rev: Rev, hash: &str) -> Result<(), StoreError> {
        let id = id.clone();
        let hash = hash.to_string();
        self.write_tx(|tx, now| {
            if kind == EntityKind::Attachment {
                // 附件是内容寻址（sha256 主键），无法用 EntityId 表达 —— 用专用入口。
                return Err(StoreError::Constraint(
                    "附件以 sha256 寻址：请用 Store::set_attachment_states".into(),
                ));
            }
            let table = if kind == EntityKind::Note { "notes" } else { "folders" };
            let cur_rev: Option<i64> = tx
                .query_row(&format!("SELECT rev FROM {table} WHERE id = ?1"), [id.as_str()], |r| r.get(0))
                .optional()?;
            match cur_rev {
                Some(cur) => {
                    if (rev.get() as i64) > cur {
                        return Err(StoreError::Rejected(format!(
                            "mark_synced({kind:?} {id}) 的 rev {rev} 高于本地头部 {cur}：确认点不可能领先于内容"
                        )));
                    }
                    tx.execute(
                        &format!("UPDATE {table} SET sync_rev = ?2, sync_hash = ?3 WHERE id = ?1"),
                        params![id.as_str(), rev.get() as i64, hash],
                    )?;
                }
                None => {
                    // 实体行可能已被永久删除：确认点落到清单缓存，否则墓碑永远算脏。
                    let t: Option<i64> = tx
                        .query_row(
                            "SELECT rev FROM tombstones WHERE entity_type=?1 AND entity_id=?2 AND purged=1",
                            params![rows::kind_tag(kind), id.as_str()],
                            |r| r.get(0),
                        )
                        .optional()?;
                    match t {
                        Some(tomb_rev) => {
                            Self::confirm_purge_in_index(tx, now, kind, id.as_str(), rev, None)?;
                            if (rev.get() as i64) < tomb_rev {
                                return Err(StoreError::Rejected(format!(
                                    "mark_synced 的 rev {rev} 低于墓碑 rev {tomb_rev}"
                                )));
                            }
                        }
                        None => return Err(StoreError::not_found(kind, id.clone())),
                    }
                }
            }
            Ok(())
        })
    }

    /// 清单里记录的服务器头部：`remote_rev`（Lamport 的"观测"来源，I2 用它取 max）。
    pub fn set_remote_rev(&self, kind: EntityKind, id: &EntityId, remote_rev: Rev, hash12: &str) -> Result<(), StoreError> {
        let id = id.clone();
        let hash12 = hash12.to_string();
        self.write_tx(|tx, now| {
            if !Self::set_remote_rev_tx(tx, now, kind, &id, remote_rev, &hash12)?
                && Self::tombstone_of(tx, kind, &id)?.is_none()
            {
                return Err(StoreError::not_found(kind, id.clone()));
            }
            Ok(())
        })
    }

    /// 事务级实现（`apply_remote` 的 `SetRemote` 复用，避免嵌套事务）。
    /// 返回是否命中实体行。
    pub(crate) fn set_remote_rev_tx(
        tx: &Connection,
        now: &str,
        kind: EntityKind,
        id: &EntityId,
        remote_rev: Rev,
        hash12: &str,
    ) -> Result<bool, StoreError> {
        let table = match kind {
            EntityKind::Note => "notes",
            EntityKind::Folder => "folders",
            // 附件以 sha256 为主键，UUID 契约表达不了（见 Store::set_attachment_states）
            EntityKind::Attachment => {
                return Err(StoreError::Constraint(
                    "附件以 sha256 寻址：请用 Store::set_attachment_states".into(),
                ))
            }
        };
        let n = tx.execute(
            &format!("UPDATE {table} SET remote_rev = ?2 WHERE id = ?1"),
            params![id.as_str(), remote_rev.get() as i64],
        )?;
        for account in rows::all_accounts(tx)? {
            tx.execute(
                "INSERT INTO sync_remote_index (account_id, kind, entity_id, rev, hash12, deleted, purged, updated_at)
                 VALUES (?1,?2,?3,?4,?5,0,0,?6)
                 ON CONFLICT(account_id, kind, entity_id) DO UPDATE SET
                   rev = excluded.rev, hash12 = excluded.hash12, updated_at = excluded.updated_at",
                params![account, rows::kind_tag(kind), id.as_str(), remote_rev.get() as i64, hash12, now],
            )?;
        }
        Ok(n > 0)
    }

    pub(crate) fn confirm_purge_in_index(
        tx: &Connection,
        now: &str,
        kind: EntityKind,
        entity_id: &str,
        rev: Rev,
        hash12: Option<&str>,
    ) -> Result<(), StoreError> {
        for account in rows::all_accounts(tx)? {
            tx.execute(
                "INSERT INTO sync_remote_index (account_id, kind, entity_id, rev, hash12, deleted, purged, updated_at)
                 VALUES (?1,?2,?3,?4,?5,1,1,?6)
                 ON CONFLICT(account_id, kind, entity_id) DO UPDATE SET
                   rev = MAX(sync_remote_index.rev, excluded.rev),
                   purged = 1, deleted = 1, updated_at = excluded.updated_at",
                params![account, rows::kind_tag(kind), entity_id, rev.get() as i64, hash12, now],
            )?;
        }
        Ok(())
    }

    /// 清单缓存整表替换（每账户一次）。单事务：中途失败不会留下半份清单。
    pub fn remote_index_replace(&self, account: &str, entries: &[RemoteIndexEntry]) -> Result<(), StoreError> {
        let account = account.to_string();
        let entries = entries.to_vec();
        self.write_tx(|tx, now| {
            let known: i64 = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM sync_accounts WHERE id = ?1)",
                [account.as_str()],
                |r| r.get(0),
            )?;
            if known == 0 {
                return Err(StoreError::Constraint(format!("账户不存在: {account}")));
            }
            tx.execute("DELETE FROM sync_remote_index WHERE account_id = ?1", [account.as_str()])?;
            let mut stmt = tx.prepare(
                "INSERT INTO sync_remote_index (account_id, kind, entity_id, rev, hash12, size, deleted, purged, seg, updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            )?;
            for e in &entries {
                let key = match e.kind {
                    EntityKind::Attachment => e.sha256.clone().ok_or_else(|| {
                        StoreError::Constraint("附件清单条目必须给出 sha256 作为键".into())
                    })?,
                    _ => e.id.as_str().to_string(),
                };
                stmt.execute(params![
                    account.as_str(),
                    rows::kind_tag(e.kind),
                    key,
                    e.rev.get() as i64,
                    e.hash12,
                    e.size,
                    e.deleted as i64,
                    e.purged as i64,
                    e.seg,
                    now
                ])?;
            }
            drop(stmt);
            Ok(())
        })
    }

    pub fn remote_index_lookup(&self, account: &str, kind: EntityKind, key: &str) -> Result<Option<RemoteIndexEntry>, StoreError> {
        let account = account.to_string();
        let key = key.to_string();
        let conn = self.read()?;
        let mut stmt = conn.prepare(
            "SELECT kind, entity_id, rev, hash12, size, deleted, purged, seg FROM sync_remote_index
              WHERE account_id = ?1 AND kind = ?2 AND entity_id = ?3",
        )?;
        let out = stmt
            .query_map(params![account.as_str(), rows::kind_tag(kind), key.as_str()], |r| {
                let tag = r.get::<_, String>(0)?;
                let id = r.get::<_, String>(1)?;
                Ok((tag, id, r.get::<_, i64>(2)?, r.get::<_, Option<String>>(3)?, r.get::<_, Option<i64>>(4)?,
                    r.get::<_, i64>(5)?, r.get::<_, i64>(6)?, r.get::<_, Option<String>>(7)?))
            })?
            .next()
            .transpose()?;
        let Some((tag, id, rev, hash12, size, deleted, purged, seg)) = out else { return Ok(None) };
        let kind = rows::kind_from_tag(&tag)?;
        Ok(Some(RemoteIndexEntry {
            kind,
            id: id_or_nil(&id),
            rev: Rev(rev.max(0) as u64),
            hash12,
            size,
            deleted: deleted != 0,
            purged: purged != 0,
            seg,
            sha256: (kind == EntityKind::Attachment).then_some(id),
        }))
    }

    // ------------------------------------------------------------ 附件态 ---

    /// 附件生命周期迁移（DATA-MODEL §8）：`local_state` / `remote_state` 由同步侧推进。
    pub fn set_attachment_states(
        &self,
        sha256: &str,
        local_state: Option<&str>,
        remote_state: Option<&str>,
    ) -> Result<(), StoreError> {
        let sha256 = sha256.to_string();
        let local_state = local_state.map(|s| s.to_string());
        let remote_state = remote_state.map(|s| s.to_string());
        self.write_tx(|tx, _now| {
            if let Some(l) = &local_state {
                if !matches!(l.as_str(), "missing" | "available" | "partial" | "error") {
                    return Err(StoreError::Constraint(format!("local_state 非法: {l}")));
                }
            }
            if let Some(r) = &remote_state {
                if !matches!(r.as_str(), "unknown" | "absent" | "present" | "error") {
                    return Err(StoreError::Constraint(format!("remote_state 非法: {r}")));
                }
            }
            let n = tx.execute(
                "UPDATE attachments
                    SET local_state = COALESCE(?2, local_state),
                        remote_state = COALESCE(?3, remote_state),
                        verified_at = CASE WHEN ?2 = 'available' THEN COALESCE(verified_at, ?4) ELSE verified_at END
                  WHERE sha256 = ?1",
                params![sha256, local_state, remote_state, self.now()],
            )?;
            if n == 0 {
                return Err(StoreError::Constraint(format!("附件不存在: {sha256}")));
            }
            Ok(())
        })
    }

    /// 附件的下载待办（`local_state='missing'` 且远端 `present` → 入队，UI 显示占位）。
    pub fn enqueue_download(&self, sha256: &str) -> Result<(), StoreError> {
        let sha = sha256.to_string();
        self.write_tx(|tx, now| {
            let exists: i64 = tx.query_row("SELECT EXISTS(SELECT 1 FROM attachments WHERE sha256=?1)", [sha.as_str()], |r| r.get(0))?;
            if exists == 0 {
                return Err(StoreError::Constraint(format!("附件不存在: {sha}")));
            }
            for account in rows::all_accounts(tx)? {
                tx.execute(
                    "INSERT INTO sync_operations
                       (account_id, dedupe_key, entity_type, entity_id, op, payload_rev, sha256, state, attempts, created_at, updated_at)
                     VALUES (?1,?2,'attachment',?3,'download',NULL,?3,'pending',0,?4,?4)
                     ON CONFLICT(dedupe_key) DO UPDATE SET state='pending', updated_at=excluded.updated_at",
                    params![account, format!("{account}:attachment:{sha}:download:-"), sha.as_str(), now],
                )?;
            }
            Ok(())
        })
    }

    /// 待上传的附件：本地有内容、远端还没确认存在。按体积升序 ——
    /// 小附件先走完，移动网络上更快看到"同步上了"。
    pub fn attachment_uploads(&self, limit: usize) -> Result<Vec<AttachmentJob>, StoreError> {
        self.attachment_jobs(limit, "available", &["unknown", "absent", "error"])
    }

    /// 待下载的附件：清单说远端有，本地却没有内容。
    pub fn attachment_downloads(&self, limit: usize) -> Result<Vec<AttachmentJob>, StoreError> {
        self.attachment_jobs(limit, "missing", &["present"])
    }

    fn attachment_jobs(&self, limit: usize, local: &str, remote_in: &[&str]) -> Result<Vec<AttachmentJob>, StoreError> {
        let placeholders = remote_in.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT sha256, size, media_type FROM attachments
              WHERE local_state IN ('{local}','partial','error')
                AND remote_state IN ({ph})
                AND deleted_at IS NULL
              ORDER BY size ASC LIMIT ?",
            local = if local == "available" { "available" } else { "missing" },
            ph = placeholders
        );
        let conn = self.read()?;
        let mut stmt = conn.prepare(&sql)?;
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = remote_in.iter().map(|s| Box::new(s.to_string()) as _).collect();
        args.push(Box::new(limit as i64));
        let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|a| a.as_ref()).collect();
        let rows = stmt.query_map(rusqlite::params_from_iter(refs), |r| {
            Ok(AttachmentJob { sha256: r.get(0)?, size: r.get::<_, i64>(1)?, media_type: r.get(2)? })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }


    /// 记下"远端有、本地还没有"的附件（清单/记录里读到引用时调用）。
    /// 只登记元数据与 `local_state='missing'`，**绝不**创建空 blob 占位。
    pub fn register_remote_attachment(&self, sha256: &str, size: i64, media_type: &str) -> Result<(), StoreError> {
        let sha = sha256.to_string();
        let media = if media_type.trim().is_empty() { "application/octet-stream".to_string() } else { media_type.to_string() };
        self.write_tx(|tx, now| {
            tx.execute(
                "INSERT INTO attachments (sha256, size, media_type, local_state, remote_state, created_at)
                 VALUES (?1,?2,?3,'missing','present',?4)
                 ON CONFLICT(sha256) DO UPDATE SET
                   size = MAX(attachments.size, excluded.size),
                   remote_state = 'present',
                   deleted_at = NULL",
                params![sha, size, media, now],
            )?;
            Ok(())
        })
    }

    /// 附件传输结束后把它的 outbox 行落到 done/failed。
    /// 状态机以 `local_state/remote_state` 为准，outbox 行只是"有活要干"的账；
    /// 不关掉它们，`outbox_pending` 会一直虚高，看起来像同步卡住。
    pub fn finish_attachment_ops(&self, sha256: &str, ok: bool) -> Result<(), StoreError> {
        let sha = sha256.to_string();
        self.write_tx(|tx, now| {
            tx.execute(
                "UPDATE sync_operations SET state = ?2, updated_at = ?3
                  WHERE entity_type = 'attachment' AND entity_id = ?1
                    AND state IN ('pending','inflight')",
                params![sha, if ok { "done" } else { "failed" }, now],
            )?;
            Ok(())
        })
    }

    /// 附件在本地的引用计数（由 `note_attachments` 派生，**不存 ref_count 列**，§8）。
    pub fn attachment_refs(&self, sha256: &str) -> Result<u32, StoreError> {
        let sha = sha256.to_string();
        let conn = self.read()?;
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM note_attachments na JOIN notes n ON n.id = na.note_id WHERE na.sha256 = ?1",
            [sha.as_str()],
            |r| r.get::<_, i64>(0),
        )? as u32)
    }

    /// 本地**已知**的附件 sha 清单（导出用）。以库为准，不以目录遍历为准：
    /// blob 落在 `<attachments>/<2hex>/<sha>` 的两层结构里，"扫一层目录"这种写法
    /// 只会看到 2 字符的分片目录名，于是导出的 ZIP 里一个附件都没有，而报告说成功
    /// （实测踩过）。库里没有的行不属于任何笔记，不该混进用户的备份。
    pub fn local_attachment_shas(&self) -> Result<Vec<String>, StoreError> {
        let conn = self.read()?;
        let mut stmt = conn.prepare("SELECT sha256 FROM attachments ORDER BY sha256")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        rows.collect::<Result<_, _>>().map_err(Into::into)
    }

    // ------------------------------------------------- push 侧：记录 wire ---

    /// 笔记记录的 wire 字节（DATA-MODEL §11 / SYNC-PROTOCOL §3）：同步引擎 PUT 的就是这份。
    ///
    /// `hash` 取行上的 `content_hash`（= `sha256(canonical(doc))`，I5 保证），所以
    /// [`Store::apply_remote`] 的 I6 闸门复核的就是同一份字节（往返测试据此钉死）。
    /// 行不存在（或已 purged）时只可能给出**墓碑公告**（`payload: null`）—— 否则
    /// "永久删除"永远传不出去，那是静默同步失败（SYNC-PROTOCOL §8）。
    pub fn note_envelope_wire(&self, id: &EntityId) -> Result<Option<Vec<u8>>, StoreError> {
        let id = id.clone();
        let device = self.device.to_string();
        let env = self.with_read(|conn| -> Result<Option<serde_json::Value>, StoreError> {
            match rows::read_note(conn, &id)? {
                Some(n) if n.purged_at.is_none() => Ok(Some(note_wire(&n, &device)?)),
                _ => tombstone_wire(conn, EntityKind::Note, &id, &device),
            }
        })?;
        env.as_ref().map(wire_bytes).transpose()
    }

    /// 见 [`Store::note_envelope_wire`]（文件夹版）。
    pub fn folder_envelope_wire(&self, id: &EntityId) -> Result<Option<Vec<u8>>, StoreError> {
        let id = id.clone();
        let device = self.device.to_string();
        let env = self.with_read(|conn| -> Result<Option<serde_json::Value>, StoreError> {
            match rows::read_folder(conn, &id)? {
                Some(f) if f.purged_at.is_none() => Ok(Some(folder_wire(&f, &device))),
                _ => tombstone_wire(conn, EntityKind::Folder, &id, &device),
            }
        })?;
        env.as_ref().map(wire_bytes).transpose()
    }

    // ----------------------------------------------------------- 冲突收件箱 -----

    pub fn record_conflict(&self, c: &ConflictRecord) -> Result<i64, StoreError> {
        let c = c.clone();
        self.write_tx(|tx, now| {
            tx.execute(
                "INSERT INTO sync_conflicts
                   (account_id, entity_type, entity_id, base_rev, local_rev, remote_rev, local_hash,
                    remote_hash, auto_merged, copy_note_id, state, created_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,'open',?11)",
                params![
                    c.account_id,
                    rows::kind_tag(c.kind),
                    c.id.as_str(),
                    c.base_rev.get() as i64,
                    c.local_rev.get() as i64,
                    c.remote_rev.get() as i64,
                    c.local_hash,
                    c.remote_hash,
                    c.auto_merged as i64,
                    c.copy_note_id.as_ref().map(|i| i.as_str().to_string()),
                    now
                ],
            )?;
            Ok(tx.last_insert_rowid())
        })
    }

    /// 全库记录（导出用）。信封与上传时**同一套构造函数** —— 导出的东西必须能被
    /// 同步层原样吃回去，否则"导出→清空→导入"会走一条和同步不同的写入路径，
    /// 那正是"防复活"最容易漏的地方。
    ///
    /// `include_trash=false` 时软删的笔记仍会出现（它带着 `deleted_at`），
    /// 因为删掉的事实本身就是内容的一部分；只有 purged 的靠墓碑公告。
    pub fn all_records(&self) -> Result<Vec<serde_json::Value>, StoreError> {
        let device = self.device.to_string();
        let mut out = Vec::new();
        for folder in self.list_folders()? {
            out.push(folder_wire(&folder, &device));
        }
        let ids: Vec<EntityId> = {
            let conn = self.read()?;
            let mut stmt = conn.prepare("SELECT id FROM notes ORDER BY id")?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            rows.map(|row| rows::parse_id(&row?)).collect::<Result<_, _>>()?
        };
        for id in ids {
            if let Some(note) = self.get_note(&id)? {
                out.push(note_wire(&note, &device)?);
            }
        }
        let conn = self.read()?;
        let mut stmt = conn.prepare("SELECT entity_type, entity_id FROM tombstones WHERE purged = 1 ORDER BY entity_type, entity_id")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        for row in rows {
            let (tag, id) = row?;
            let kind = rows::kind_from_tag(&tag)?;
            if kind == EntityKind::Attachment {
                continue;
            }
            if let Some(env) = tombstone_wire(&conn, kind, &rows::parse_id(&id)?, &device)? {
                out.push(env);
            }
        }
        Ok(out)
    }

    pub fn open_conflicts(&self) -> Result<Vec<ConflictRow>, StoreError> {
        let conn = self.read()?;
        conflicts_where(&conn, "state = 'open'")
    }

    /// 用户/引擎裁决后关闭一条冲突（`resolution` ∈ kept_both|local|remote|merged|manual）。
    pub fn resolve_conflict(&self, conflict_id: i64, resolution: &str) -> Result<(), StoreError> {
        let resolution = resolution.to_string();
        if !matches!(resolution.as_str(), "kept_both" | "local" | "remote" | "merged" | "manual") {
            return Err(StoreError::Constraint(format!("未知 resolution: {resolution}")));
        }
        self.write_tx(|tx, now| {
            let n = tx.execute(
                "UPDATE sync_conflicts SET state='resolved', resolution=?2, resolved_at=?3 WHERE id=?1",
                params![conflict_id, resolution, now],
            )?;
            if n == 0 {
                return Err(StoreError::Constraint(format!("冲突不存在: {conflict_id}")));
            }
            Ok(())
        })
    }

    pub fn dismiss_conflict(&self, conflict_id: i64) -> Result<(), StoreError> {
        self.write_tx(|tx, _now| {
            let n = tx.execute("UPDATE sync_conflicts SET state='dismissed' WHERE id=?1", [conflict_id])?;
            if n == 0 {
                return Err(StoreError::Constraint(format!("冲突不存在: {conflict_id}")));
            }
            Ok(())
        })
    }
}

// ------------------------------------------------------- 记录信封（§11）---

/// 无正文可哈希时的占位（只用于墓碑公告：`apply` 对 purged 记录不校验 hash）。
fn null_hash() -> String {
    format!("sha256:{}", "0".repeat(64))
}

fn wire_bytes(v: &serde_json::Value) -> Result<Vec<u8>, StoreError> {
    // 信封本身不参与哈希路径（`hash` 字段才是权威），普通序列化即可；
    // 但绝不返回"半个 JSON"—— 那会把远端好记录覆盖成垃圾。
    serde_json::to_vec(v).map_err(|e| StoreError::Constraint(format!("信封无法序列化: {e}")))
}

/// 信封外壳（DATA-MODEL §11 / SYNC-PROTOCOL §3 的字段集，逐字同名）。
///
/// `sync_rev` 是设备态、按 §6 不参与同步判定；带着它只为与规范示例一致，
/// `apply_remote` 不读该字段。墓碑公告没有行，故省略。
#[allow(clippy::too_many_arguments)]
fn envelope(    kind: EntityKind,
    id: &EntityId,
    rev: Rev,
    sync_rev: Option<Rev>,
    hash: &str,
    updated_at: &str,
    device: &str,
    deleted_at: Option<&str>,
    purged: bool,
    payload: serde_json::Value,
) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    m.insert("protocol".into(), serde_json::json!(1));
    m.insert("kind".into(), serde_json::json!(kind.dir()));
    m.insert("id".into(), serde_json::json!(id.as_str()));
    m.insert("rev".into(), serde_json::json!(rev.get()));
    if let Some(s) = sync_rev {
        m.insert("sync_rev".into(), serde_json::json!(s.get()));
    }
    m.insert("hash".into(), serde_json::json!(hash));
    m.insert("updated_at".into(), serde_json::json!(updated_at));
    m.insert("device".into(), serde_json::json!(device));
    m.insert("deleted_at".into(), serde_json::json!(deleted_at));
    m.insert("purged".into(), serde_json::json!(purged));
    m.insert("enc".into(), serde_json::json!({ "alg": "none", "hash_alg": "sha256" }));
    m.insert("payload".into(), payload);
    m.insert("ct".into(), serde_json::Value::Null);
    serde_json::Value::Object(m)
}

/// 笔记的 `payload` = canonical doc ⊕ §6 的同步字段（folder_id / pinned / color）。
///
/// 这些额外键不影响 `hash`：richtext 的 `Document` 只认 `v`/`content`，归一时丢弃
/// 未知顶层键，所以 `sha256(canonical(payload)) == notes.content_hash` 仍然成立。
fn note_wire(n: &Note, device: &str) -> Result<serde_json::Value, StoreError> {
    let mut payload = n.doc.clone();
    let Some(map) = payload.as_object_mut() else {
        return Err(StoreError::Constraint(format!("笔记 {} 的 doc 不是 JSON 对象：拒绝构造残缺记录", n.id)));
    };
    map.insert("folder_id".into(), serde_json::json!(n.folder_id.as_str()));
    map.insert("pinned".into(), serde_json::json!(n.pinned));
    map.insert("color".into(), serde_json::json!(n.color));
    Ok(envelope(
        EntityKind::Note,
        &n.id,
        n.rev,
        Some(n.sync_rev),
        &n.content_hash,
        &n.updated_at,
        device,
        n.deleted_at.as_deref(),
        false,
        payload,
    ))
}

/// 文件夹 `payload` 的键集与 `store::folder_hash` 的入参一致 →
/// `hash == sha256(canonical(payload))` 逐字成立（§3 约束表）。
fn folder_wire(f: &Folder, device: &str) -> serde_json::Value {
    let payload = serde_json::json!({
        "name": f.name,
        "parent_id": f.parent_id.as_ref().map(|p| p.as_str().to_string()),
        "color": f.color,
        "sort_order": f.sort_order,
        "system_kind": f.system_kind,
    });
    envelope(
        EntityKind::Folder,
        &f.id,
        f.rev,
        Some(f.sync_rev),
        &f.content_hash,
        &f.updated_at,
        device,
        f.deleted_at.as_deref(),
        false,
        payload,
    )
}

/// 永久删除的公告记录：`payload: null`（§3 末段）。
/// 没有 purged 墓碑就没有可上传的东西 —— 返回 None 让引擎跳过，而不是编造一份。
fn tombstone_wire(
    conn: &Connection,
    kind: EntityKind,
    id: &EntityId,
    device: &str,
) -> Result<Option<serde_json::Value>, StoreError> {
    let Some(t) = Store::tombstone_of(conn, kind, id)? else {
        return Ok(None);
    };
    if !t.purged {
        return Ok(None);
    }
    let hash = t.content_hash.clone().unwrap_or_else(null_hash);
    Ok(Some(envelope(
        kind,
        id,
        t.rev,
        None,
        &hash,
        &t.deleted_at,
        device,
        Some(t.deleted_at.as_str()),
        true,
        serde_json::Value::Null,
    )))
}

fn conflicts_where(conn: &Connection, cond: &str) -> Result<Vec<ConflictRow>, StoreError> {
    let sql = format!(
        "SELECT id, account_id, entity_type, entity_id, base_rev, local_rev, remote_rev, local_hash,
                remote_hash, auto_merged, copy_note_id, state, resolution, created_at, resolved_at
           FROM sync_conflicts WHERE {cond} ORDER BY created_at DESC, id DESC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let out = stmt
        .query_map([], |r| {
            Ok(ConflictRow {
                conflict_id: r.get(0)?,
                account_id: r.get(1)?,
                kind: rows::kind_from_tag(&r.get::<_, String>(2)?).map_err(rows::into_sql)?,
                id: rows::parse_id(&r.get::<_, String>(3)?).map_err(rows::into_sql)?,
                base_rev: Rev(r.get::<_, i64>(4)?.max(0) as u64),
                local_rev: Rev(r.get::<_, i64>(5)?.max(0) as u64),
                remote_rev: Rev(r.get::<_, i64>(6)?.max(0) as u64),
                local_hash: r.get(7)?,
                remote_hash: r.get(8)?,
                auto_merged: r.get::<_, i64>(9)? != 0,
                copy_note_id: r
                    .get::<_, Option<String>>(10)?
                    .map(|s| rows::parse_id(&s).map_err(rows::into_sql))
                    .transpose()?,
                state: ConflictState::parse(&r.get::<_, String>(11)?).unwrap_or(ConflictState::Open),
                resolution: r.get(12)?,
                created_at: r.get(13)?,
                resolved_at: r.get(14)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(out)
}

fn collect_dirty(
    conn: &Connection,
    out: &mut Vec<DirtyEntity>,
    kind: EntityKind,
    sql: &str,
) -> Result<(), StoreError> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, i64>(3)?,
            r.get::<_, Option<String>>(4)?,
        ))
    })?;
    for row in rows {
        let (id, rev, hash, sync_rev, deleted_at) = row?;
        let why = if deleted_at.is_some() {
            DirtyWhy::Deleted
        } else if sync_rev == 0 {
            DirtyWhy::NeverPushed
        } else {
            DirtyWhy::Edited
        };
        out.push(DirtyEntity {
            kind,
            id: rows::parse_id(&id)?,
            rev: Rev(rev.max(0) as u64),
            content_hash: hash,
            sync_rev: Rev(sync_rev.max(0) as u64),
            why,
        });
    }
    Ok(())
}
