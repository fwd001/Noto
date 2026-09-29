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

/// `Store::resolved_divergences` 的一行：实体 + 用户当时看到的对面那一版编号（计划层 P19 的输入）。
pub type ResolvedDivergence = (EntityKind, EntityId, u64);

/// 契约里的 `EntityId` 只装 UUID；附件是内容寻址（64hex），此时用 nil UUID 占位。
const NIL_ID: &str = "00000000-0000-0000-0000-000000000000";

/// nil UUID：表示"该行的键不是 UUID，请用 [`SyncOperation::entity_key`] /
/// [`RemoteIndexEntry::sha256`]"。绝不用随机 ID 冒充。
fn nil_id() -> EntityId {
    EntityId::parse(NIL_ID)
        .unwrap_or_else(|_| EntityId::parse(NIL_ID).expect("nil UUID 必须可解析"))
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
    pub fn register_account(
        &self,
        id: &str,
        label: &str,
        base_url: &str,
    ) -> Result<(), StoreError> {
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
            let n = tx.execute(
                "UPDATE sync_accounts SET enabled = ?2 WHERE id = ?1",
                params![id, enabled as i64],
            )?;
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
            let n = tx.execute(
                "UPDATE sync_accounts SET cap_mask = ?2, caps_probed_at = ?3 WHERE id = ?1",
                params![id, mask as i64, at],
            )?;
            if n == 0 {
                return Err(StoreError::Constraint(format!("账户不存在: {id}")));
            }
            Ok(())
        })
    }

    /// `None` = 从未探测过（调用方应使用保守默认，而不是当成"全不支持"）。
    pub fn account_caps(&self, id: &str) -> Result<Option<u32>, StoreError> {
        let conn = self.read()?;
        let v = conn.query_row(
            "SELECT cap_mask FROM sync_accounts WHERE id = ?1",
            [id],
            |r| r.get::<_, Option<i64>>(0),
        );
        match v {
            Ok(Some(m)) => Ok(Some(m.max(0) as u32)),
            Ok(None) => Ok(None),
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                Err(StoreError::Constraint(format!("账户不存在: {id}")))
            }
            Err(e) => Err(StoreError::from(e)),
        }
    }

    /// 上次 §5 探测的时刻（`None` = 从未探测）。存的是字符串，"多久算过期"
    /// 是同步侧的策略，不放进存储层。
    pub fn account_caps_probed_at(&self, id: &str) -> Result<Option<String>, StoreError> {
        let conn = self.read()?;
        let v = conn.query_row(
            "SELECT caps_probed_at FROM sync_accounts WHERE id = ?1",
            [id],
            |r| r.get::<_, Option<String>>(0),
        );
        match v {
            Ok(t) => Ok(t),
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                Err(StoreError::Constraint(format!("账户不存在: {id}")))
            }
            Err(e) => Err(StoreError::from(e)),
        }
    }

    pub fn account_ids(&self) -> Result<Vec<String>, StoreError> {
        let conn = self.read()?;
        rows::enabled_accounts(&conn)
    }

    pub fn account_exists(&self, id: &str) -> Result<bool, StoreError> {
        let conn = self.read()?;
        Ok(conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_accounts WHERE id = ?1)",
            [id],
            |r| r.get::<_, i64>(0),
        )? != 0)
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

    /// 已同步头：`rev == sync_rev` 且未删除的实体 → (类型, id, rev, 全哈希)。
    ///
    /// `dirty_entities` 只报脏行，**干净的设备在它里面是空的** —— 引擎因此会把整份
    /// 清单上的每一条都当成"本机没有的新增"重下一遍，每轮的请求预算全花在重复劳动上，
    /// 真正缺的那几条永远排不到（实测：260 条的库，第二台设备卡在 196/260 且再也不动）。
    /// 这一份视图就是给引擎跳过"已经有了的那一版"用的。
    pub fn synced_heads(&self) -> Result<Vec<(EntityKind, EntityId, Rev, String)>, StoreError> {
        let conn = self.read()?;
        let mut out = Vec::new();
        for (kind, sql) in [
            (EntityKind::Note, "SELECT id, rev, content_hash FROM notes WHERE rev = sync_rev AND deleted_at IS NULL"),
            (EntityKind::Folder, "SELECT id, rev, content_hash FROM folders WHERE rev = sync_rev AND deleted_at IS NULL"),
        ] {
            let mut stmt = conn.prepare(sql)?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?)))?;
            for row in rows {
                let (id, rev, hash) = row?;
                // id 解析不了说明库里混进了非本系统生成的行：跳过它最坏是"这一条照样重下一次"，
                // 不能让它把整轮同步顶死。
                if let Ok(parsed) = EntityId::parse(&id) {
                    out.push((kind, parsed, Rev(rev as u64), hash));
                }
            }
        }
        Ok(out)
    }

    /// 分段内容哈希缓存（按账户存：`{分段名 → hash12}`）。
    ///
    /// 引擎据此决定"这一段的基线还要不要再下一遍"（`LocalPort::cached_segment_hashes`）。
    /// 只有**核对过**的内容才会被写进来 —— 写方是引擎，这里只负责跨进程留住它。
    /// 读不进来一律当"没有缓存"：代价是多下一次分段，绝不能把整轮同步顶死。
    /// 按账户分开存：换服务器之后同名分段（`seg-0000`）是**另一份内容**，混用就会
    /// 让新账户跳过一份本机其实没有的基线。
    pub fn segment_hashes(
        &self,
        account: &str,
    ) -> Result<std::collections::BTreeMap<String, String>, StoreError> {
        let key = format!("segment_hashes:{account}");
        let conn = self.read()?;
        let raw = match rows::meta_get(&conn, &key)? {
            Some(v) => v,
            None => return Ok(std::collections::BTreeMap::new()),
        };
        Ok(serde_json::from_str(&raw).unwrap_or_default())
    }

    pub fn set_segment_hashes(
        &self,
        account: &str,
        map: &std::collections::BTreeMap<String, String>,
    ) -> Result<(), StoreError> {
        let account = account.to_string();
        let key = format!("segment_hashes:{account}");
        let json = serde_json::to_string(map)
            .map_err(|e| StoreError::Constraint(format!("分段哈希缓存写不出去: {e}")))?;
        self.write_tx(|tx, _now| rows::meta_set(tx, &key, &json))
    }

    /// 脏集 = `rev != sync_rev`（DATA-MODEL §4.2），外加"永久删除尚未被远端确认"的墓碑。
    ///
    /// 硬性要求 6：有 `purged=1` 墓碑的记录**不会**被当作新增列入脏集（否则设备 B 会把
    /// 已永久删除的笔记重新上传 → 复活）。`account` 不存在时只返回实体表判脏结果。
    pub fn dirty_entities(&self, account: &str) -> Result<Vec<DirtyEntity>, StoreError> {
        let conn = self.read()?;
        let mut out: Vec<DirtyEntity> = Vec::new();
        let account_known: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_accounts WHERE id = ?1)",
            [account],
            |r| r.get::<_, i64>(0),
        )? != 0;

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
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, Option<String>>(3)?,
                ))
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
    pub fn outbox_take(
        &self,
        account: &str,
        limit: usize,
    ) -> Result<Vec<SyncOperation>, StoreError> {
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
    /// 只动 pending/inflight：`failed` **不在这里被改写** —— 这一条是"这一轮的活干完了"的结清，
    /// 把一支 failed 写成 done 就等于替用户宣布"那次失败成功了"。
    /// （顺带更正这句注释以前写的"`failed` 行归退避逻辑管"：并没有一条后台退避在重试 outbox 行 ——
    /// 文本引擎按 `local_views` 规划、附件轮按 `attachments` 的状态挑活，`outbox_take` 在生产里没有
    /// 调用方。failed 真正的出路是三条：下一次变更把它重新入队（`rows::enqueue` 的
    /// `ON CONFLICT(dedupe_key) DO UPDATE SET state='pending'`）、附件那侧由
    /// [`Self::settle_satisfied_attachment_ops`] / [`Self::mark_attachments_quarantined`] 收口、
    /// 或用户点界面上的重试动作。）
    ///
    /// `kind` 刻意是 `EntityKind` 而不是字符串：`entity_type` 列写的是长标记
    /// （note/folder/attachment），而同步线上飘的是短标记（n/f/a）。这里收字符串的话，
    /// 传错词汇编译能过、UPDATE 匹配 0 行、待办静静停在 inflight —— 实测就这么坏过。
    pub fn outbox_settle(
        &self,
        account: &str,
        kind: EntityKind,
        id: &str,
        rev: i64,
        st: OpState,
    ) -> Result<bool, StoreError> {
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
        let placeholders = list
            .iter()
            .enumerate()
            .map(|(i, _)| format!("?{}", i + 2))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT COUNT(*) FROM sync_operations WHERE account_id = ?1 AND state IN ({placeholders})"
        );
        let mut stmt = conn.prepare(&sql)?;
        let binds: Vec<&dyn rusqlite::ToSql> = std::iter::once(&account as &dyn rusqlite::ToSql)
            .chain(list.iter().map(|s| s as &dyn rusqlite::ToSql))
            .collect();
        Ok(stmt.query_row(binds.as_slice(), |r| r.get::<_, i64>(0))? as u32)
    }

    // ------------------------------------------------- 远端游标 / 确认点 ---

    /// push 成功：`sync_rev ← rev ; sync_hash ← hash`（DATA-MODEL §4.2）。
    ///
    /// 实体行已不存在但存在 `purged=1` 墓碑时，把确认写进 `sync_remote_index`
    /// （否则永久删除的传播没有可记录的确认点，墓碑会永远算作脏）。
    pub fn mark_synced(
        &self,
        kind: EntityKind,
        id: &EntityId,
        rev: Rev,
        hash: &str,
    ) -> Result<(), StoreError> {
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
    pub fn set_remote_rev(
        &self,
        kind: EntityKind,
        id: &EntityId,
        remote_rev: Rev,
        hash12: &str,
    ) -> Result<(), StoreError> {
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
    pub fn remote_index_replace(
        &self,
        account: &str,
        entries: &[RemoteIndexEntry],
    ) -> Result<(), StoreError> {
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
            // I2 那一档"已观测到的远端头部"就地跟着视图更新（ADR-0021 D1）。
            // 这一列此前在生产里**永远是 0**：引擎只发 `SetRemote{kind:"manifest"/"seq"}`，
            // 实体那一档零调用点，于是 `next_rev(local, remote_rev)` 恒等于 `local + 1`，
            // 而 DATA-MODEL §0 与 `next_rev` 的注释都写着"取大者 +1，编号不会相等"。
            // 单调 `MAX`：这一列的语义是"本机见过的对面最高编号"，回退它等于取消那条不变式。
            // 只动 `remote_rev`，**不动 `updated_at`**（"删后又改"的判据要看它），也不动 `sync_rev`（那是三方合并的 base）。
            let mut bump_note = tx.prepare(
                "UPDATE notes SET remote_rev = MAX(remote_rev, ?2) WHERE id = ?1",
            )?;
            let mut bump_folder = tx.prepare(
                "UPDATE folders SET remote_rev = MAX(remote_rev, ?2) WHERE id = ?1",
            )?;
            let mut stmt = tx.prepare(
                "INSERT INTO sync_remote_index (account_id, kind, entity_id, rev, hash12, size, deleted, purged, seg, deleted_at, updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            )?;
            for e in &entries {
                let key = match e.kind {
                    EntityKind::Attachment => e.sha256.clone().ok_or_else(|| {
                        StoreError::Constraint("附件清单条目必须给出 sha256 作为键".into())
                    })?,
                    _ => e.id.as_str().to_string(),
                };
                match e.kind {
                    EntityKind::Note => {
                        bump_note.execute(params![key, e.rev.get() as i64])?;
                    }
                    EntityKind::Folder => {
                        bump_folder.execute(params![key, e.rev.get() as i64])?;
                    }
                    // 附件以 sha256 寻址，实体行上没有 remote_rev 这一档（见 set_remote_rev_tx）
                    EntityKind::Attachment => {}
                }
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
                    e.deleted_at,
                    now
                ])?;
            }
            drop(stmt);
            Ok(())
        })
    }

    pub fn remote_index_lookup(
        &self,
        account: &str,
        kind: EntityKind,
        key: &str,
    ) -> Result<Option<RemoteIndexEntry>, StoreError> {
        let account = account.to_string();
        let key = key.to_string();
        let conn = self.read()?;
        let mut stmt = conn.prepare(
            "SELECT kind, entity_id, rev, hash12, size, deleted, purged, seg, deleted_at FROM sync_remote_index
              WHERE account_id = ?1 AND kind = ?2 AND entity_id = ?3",
        )?;
        let out = stmt
            .query_map(
                params![account.as_str(), rows::kind_tag(kind), key.as_str()],
                |r| {
                    let tag = r.get::<_, String>(0)?;
                    let id = r.get::<_, String>(1)?;
                    Ok((
                        tag,
                        id,
                        r.get::<_, i64>(2)?,
                        r.get::<_, Option<String>>(3)?,
                        r.get::<_, Option<i64>>(4)?,
                        r.get::<_, i64>(5)?,
                        r.get::<_, i64>(6)?,
                        r.get::<_, Option<String>>(7)?,
                        r.get::<_, Option<String>>(8)?,
                    ))
                },
            )?
            .next()
            .transpose()?;
        let Some((tag, id, rev, hash12, size, deleted, purged, seg, deleted_at)) = out else {
            return Ok(None);
        };
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
            deleted_at,
        }))
    }

    /// 整表读出某个账户的远端视图。
    ///
    /// 这张表的建表注释就写着"清单的本地缓存：让每轮同步免于全量下载"，但此前生产路径
    /// 上没有读取者也没有写入者 —— 引擎每轮的远端视图只活在进程里，于是落后超过窗口时
    /// 每一轮都要把整份基线分段重下一遍。有了它（加上 0007 的 `deleted_at`），视图才真的
    /// 可以持久，`cached_segment_hashes` 那类"跳过下载"的判据也才成立：跳过之前得有个
    /// 地方把条目读回来，否则省下的下载就是丢的数据。
    pub fn remote_index_list(&self, account: &str) -> Result<Vec<RemoteIndexEntry>, StoreError> {
        let account = account.to_string();
        let conn = self.read()?;
        let mut stmt = conn.prepare(
            "SELECT kind, entity_id, rev, hash12, size, deleted, purged, seg, deleted_at
               FROM sync_remote_index WHERE account_id = ?1 ORDER BY kind, entity_id",
        )?;
        let rows = stmt.query_map([account.as_str()], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<i64>>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, i64>(6)?,
                r.get::<_, Option<String>>(7)?,
                r.get::<_, Option<String>>(8)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (tag, id, rev, hash12, size, deleted, purged, seg, deleted_at) = row?;
            let kind = rows::kind_from_tag(&tag)?;
            out.push(RemoteIndexEntry {
                kind,
                id: id_or_nil(&id),
                rev: Rev(rev.max(0) as u64),
                hash12,
                size,
                deleted: deleted != 0,
                purged: purged != 0,
                seg,
                sha256: (kind == EntityKind::Attachment).then_some(id),
                deleted_at,
            });
        }
        Ok(out)
    }

    // ------------------------------------------------------------ 附件态 ---

    /// 附件生命周期迁移（DATA-MODEL §8）：`local_state` / `remote_state` 由同步侧推进。
    ///
    /// 写成本地 `available` 会**顺带清掉 `deleted_at`**：那一列是 GC 的"已隔离、等宽限期"标记，
    /// 而本机重新有了这份经过校验的字节就意味着它又活过来了 —— 留着它，两个队列与体检
    /// 都看不见这一行（三条口径都带 `deleted_at IS NULL`），于是这份字节永远排不进上传。
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
                        verified_at = CASE WHEN ?2 = 'available' THEN COALESCE(verified_at, ?4) ELSE verified_at END,
                        deleted_at = CASE WHEN ?2 = 'available' THEN NULL ELSE deleted_at END
                  WHERE sha256 = ?1",
                params![sha256, local_state, remote_state, self.now()],
            )?;
            if n == 0 {
                return Err(StoreError::Constraint(format!("附件不存在: {sha256}")));
            }
            Ok(())
        })
    }

    /// 批量回填磁盘体检的**实测**尺寸，**一次写事务做完**（与
    /// [`Store::set_attachments_locally_missing`] 同一个形状）。体检已经把整份字节读完并复算过
    /// sha256，哈希相符就意味着"盘上这个长度"就是这份 blob 的真实尺寸。
    ///
    /// 这是唯一允许把 `size` **往小**改的一路，而且是必需的：`upsert_attachment_row` 的
    /// 冲突规则是 `MAX(attachments.size, excluded.size)` —— 一个"不知道"（0）不许冲掉已知值，
    /// 于是正文里偏大的声明值永远纠不掉。不纠的代价不是显示错一个数，而是那一行在此后的
    /// **每一轮**附件轮里都被整份读进内存再哈希一遍（单条上界 32 MiB），偏大的尺寸还会
    /// 继续排进上传预算（`attachment_jobs` 按 `size` 排序）。
    ///
    /// 为什么是批量而不是逐行：G4 把体检的**降级**批量了，尺寸回填当时漏下 —— 它还是每条
    /// 一次提交。2026-09-28 量出来这不是理论账：`attachment_gc_scale` 那一格里，一轮慢路读
    /// 1 KiB×200 与读 64 KiB×100（IO 差 30 倍）耗时都是 ~320 ms，而夹具里同样条数的
    /// **写提交**成本是 100 次 106 ms / 1000 次 1151 ms（≈1.1 ms/次）—— 那一轮的钱主要
    /// 花在提交上，不在哈希上。刚导入 / 刚升级过的库可以有成百上千条声明尺寸是偏的，
    /// 逐行提交就是成百上千次排队占住写锁，而用户那次保存正排在这把锁后面。
    ///
    /// 只改 `size` 那一列：回填只证明"盘上实测长度是这个数"，状态、远端态、`deleted_at`
    /// 一个字都不动。返回**实际写成**的条数（库里没这条的不计入），调用方靠
    /// "返回数 < 传入数"吵一声，这里不静默替它吞掉。
    pub fn set_attachment_sizes(&self, sizes: &[(String, i64)]) -> Result<usize, StoreError> {
        if sizes.is_empty() {
            return Ok(0);
        }
        let sizes = sizes.to_vec();
        self.write_tx(|tx, _now| {
            let mut stmt = tx.prepare("UPDATE attachments SET size = ?2 WHERE sha256 = ?1")?;
            let mut done = 0usize;
            for (sha, size) in &sizes {
                done += stmt.execute(params![sha, size])?;
            }
            Ok(done)
        })
    }

    /// 批量把"账上 `available` 而本机已经没有"的行降级，**一次写事务做完**。
    ///
    /// 为什么不逐行调 [`Store::set_attachment_states`]：整个 `attachments/` 目录被删
    /// （换盘没搬完、杀毒整目录隔离、误 `rm`）时，一轮体检会攒出成百上千条 —— 逐行提交
    /// 就是那么多次提交排队占住写锁，而用户那次保存正排在这把锁后面（§48 缺口 G4）。
    ///
    /// 只碰 `local_state`，而且只碰**点名传进来的**、当前确实是 `available` 的行：
    /// * 远端态一字不动。体检只证明"本机没有"，远端有没有是另一回事；顺手写 `absent`
    ///   等于把这张图判死 —— 下载队列的口径是 `present`/`unknown`，从此再也不去问一次。
    /// * 不写成 `WHERE local_state='available'` 那种整表形式：那会把盘上明明好好的文件
    ///   一起降级，而调用方只核对过点名的这一批。
    ///
    /// 返回**实际降级**的条数：库里没有这条、或它本来已经不是 `available` 的都不计入。
    /// 调用方靠"返回数 < 传入数"吵一声，这里不静默替它吞掉。
    pub fn set_attachments_locally_missing(&self, shas: &[String]) -> Result<usize, StoreError> {
        if shas.is_empty() {
            return Ok(0);
        }
        let shas = shas.to_vec();
        self.write_tx(|tx, _now| {
            let mut stmt = tx.prepare(
                "UPDATE attachments SET local_state = 'missing'
                  WHERE sha256 = ?1 AND local_state = 'available' AND deleted_at IS NULL",
            )?;
            let mut done = 0usize;
            for sha in &shas {
                done += stmt.execute(params![sha])?;
            }
            Ok(done)
        })
    }

    /// GC（§8）在账上的隔离判据：`local_state` + `deleted_at`。
    ///
    /// 为什么要单独读这一对：`deleted_at` 今天**只由 GC 写**（隔离），而两个队列与磁盘体检
    /// 都按 `deleted_at IS NULL` 过滤，所以「这一行被 GC 认领了没有」既看不见在 `local_state`
    /// 里，也看不见在任何队列里。界面与手动动作要分清"本机没有（还没下）"与"本机没有（被隔离）"，
    /// 靠的就是这一列。行不存在时回 `None`。
    pub fn attachment_quarantine_state(&self, sha256: &str) -> Option<(String, Option<String>)> {
        let sha = sha256.to_string();
        let conn = self.read().ok()?;
        conn.query_row(
            "SELECT local_state, deleted_at FROM attachments WHERE sha256 = ?1",
            [sha.as_str()],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)),
        )
        .optional()
        .ok()
        .flatten()
    }

    /// 可以进入隔离的附件行（§8 的 GC 第一步）。
    ///
    /// 三个条件每一个都在挡一类数据丢失，缺一个都不算"安全的那一半"：
    /// * `NOT EXISTS note_attachments` —— 引用计数**只由链接表派生**（§8，不存 `ref_count` 列）。
    ///   回收站里的笔记行还在，它的链接也还在 → 仍算引用 → 不收（用户随时可能还原）。
    ///   只有笔记被**永久删除**（`notes` 行被 CASCADE 带走链接）才归零。
    /// * `remote_state='present'` —— 只收"服务器已经确认有副本"的行。没传上去过的字节是
    ///   本机独家的一份，而隔离会把它同时从上传队列里摘掉（那条队列也带 `deleted_at IS NULL`），
    ///   于是"省磁盘"就变成了"判死一份独家副本"。数据安全排在性能前面，这一类今天不收：
    ///   它会照常走上传队列，传成功了才变成 GC 的候选。
    /// * `local_state='available'` —— 只有"账上说本机有"的行才谈得上回收字节。
    ///
    /// `limit` 是每轮工作量的上界（与磁盘体检同一条理由：§48 G4）。按 `sha256` 稳定排序，
    /// 而被认领的行会**立刻**离开候选集（`deleted_at` 有值 + 不再是 `available`），所以取前
    /// `limit` 条不会饿死后面的行。
    pub fn gc_quarantine_candidates(&self, limit: usize) -> Result<Vec<String>, StoreError> {
        let conn = self.read()?;
        let mut stmt = conn.prepare(
            "SELECT a.sha256 FROM attachments a
              WHERE a.local_state = 'available' AND a.remote_state = 'present'
                AND a.deleted_at IS NULL
                AND NOT EXISTS (SELECT 1 FROM note_attachments na WHERE na.sha256 = a.sha256)
              ORDER BY a.sha256 LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// 把点名的行**隔离**（账上那一步）：`local_state='missing'` + `deleted_at=now`，
    /// 并关掉它们还挂着的附件待办 —— 全部在**一个写事务**里。
    ///
    /// 为什么引用检查要写在 `UPDATE ... WHERE` 里而不是让调用方先查一遍候选集：候选集是
    /// 上一句 SQL 读出来的，"用户在这一瞬还原了那条笔记 / 另一台设备的记录刚把同一份字节
    /// 引用进来"就落在那两条语句之间。写在同一条语句里，检查与写入才是原子的。
    ///
    /// 为什么远端态一字不动：隔离只关于**本机**。写成 `absent` 等于宣布"服务器上没有"，
    /// 那一行从此不在下载队列的口径里，撤销期结束后也再没有谁会去取它。
    ///
    /// 为什么待办要一起关掉：这一行已经不在两个队列的取活范围里，永远没人再结它 ——
    /// 留下的是一条永远不掉下去的"待同步"计数（§18 要求这个数诚实）。
    ///
    /// 为什么这里连 `failed` 一起结，而引擎那条 [`Self::outbox_settle`] 不许动 `failed`：
    /// 那条是"这一轮做完了"的结清，`failed` 归退避重试管，替它改写结论就是把用户的失败吞掉；
    /// 而这里的前提是**这一行被 GC 认领了**，它既离开下载队列也离开上传队列（两边的 SQL 都带
    /// `deleted_at IS NULL`），那条 `failed` 从此不会有任何消费者 —— 虚高的计数就是这么来的。
    /// 范围也卡在这：只对 `mark` 真落到那一行的 sha 执行，仍被引用的行那条 `failed` 一字不动。
    ///
    /// 返回**实际隔离**的条数：库里没有、仍被引用、本来已隔离的都不计入。调用方靠
    /// "返回数 < 传入数"吵一声，这里不静默替它吞掉。
    pub fn mark_attachments_quarantined(&self, shas: &[String]) -> Result<usize, StoreError> {
        if shas.is_empty() {
            return Ok(0);
        }
        let shas = shas.to_vec();
        self.write_tx(|tx, now| {
            let mut mark = tx.prepare(
                "UPDATE attachments
                    SET local_state = 'missing', deleted_at = ?2
                  WHERE sha256 = ?1 AND local_state = 'available' AND deleted_at IS NULL
                    AND NOT EXISTS (SELECT 1 FROM note_attachments na WHERE na.sha256 = attachments.sha256)",
            )?;
            let mut settle = tx.prepare(
                "UPDATE sync_operations SET state = 'done', updated_at = ?2
                  WHERE entity_type = 'attachment' AND entity_id = ?1
                    AND state IN ('pending','inflight','failed')",
            )?;
            let mut done = 0usize;
            for sha in &shas {
                if mark.execute(params![sha, now])? == 1 {
                    settle.execute(params![sha, now])?;
                    done += 1;
                }
            }
            Ok(done)
        })
    }

    /// 已经隔离、且**过了宽限期**、且仍然零引用的行（GC 第二步的清单）。
    ///
    /// `cutoff` 由调用方给（不是一个"天数"常量藏在 SQL 里），理由有两个：
    /// * 宽限期是策略，判据是事实 —— host 那一层算 `now - 30 天`，测试就能用
    ///   "远古 / 未来"两个值把到期与否变成确定性的断言，而不是等真时钟走过 30 天。
    /// * `Timestamp` 是定宽 RFC 3339 毫秒 UTC，**字典序即时间序**（notera-core 那条注释），
    ///   所以这里可以直接比字符串。
    ///
    /// 引用在这里问第二遍：从隔离到真删中间隔着整个宽限期，那一头笔记被还原（链接回来 +
    /// `deleted_at` 被登记那条路清成 NULL）是正常事件，不是异常。这一句把"到期"与"仍无人引用"
    /// 同时成立才放行；`purge_attachment_rows` 里那条 `ON DELETE RESTRICT` 是机器兜底。
    ///
    /// `remote_state='present'` 也在这里问第二遍，但**它的角色不是"再确认一次"** —— 真正的
    /// 再确认是 host 那一轮销毁之前发的 HEAD。这一句管的是**别每轮重问**：一轮 HEAD 判下来
    /// 会把结论写回这一格（服务器说没有 → `absent`；用户说"服务器那份是坏的" → `error`），
    /// 写了之后就离开候选集，20 秒一轮的常驻循环因此不会反复去敲同一扇门（§27/§28 不许空转）。
    /// `unknown` 也一并排除 —— 那是"用户刚点了重试、还没人问过服务器"那一格，判死不合资格。
    pub fn gc_ready_to_purge(&self, cutoff: &str, limit: usize) -> Result<Vec<String>, StoreError> {
        let conn = self.read()?;
        let mut stmt = conn.prepare(
            "SELECT a.sha256 FROM attachments a
              WHERE a.deleted_at IS NOT NULL AND a.deleted_at < ?1
                AND a.remote_state = 'present'
                AND NOT EXISTS (SELECT 1 FROM note_attachments na WHERE na.sha256 = a.sha256)
              ORDER BY a.deleted_at, a.sha256 LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![cutoff, limit as i64], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// 把销毁清单里那些行从账上**彻底抹掉**，返回真删掉了哪些 sha。
    ///
    /// 调用方只能按返回值去删隔离区里的文件 —— 顺序是"先删行、后删字节"，反过来一旦中间
    /// 断电就留下"账上还在、字节没了"的笔记（那张图永远取不回来）。按这个顺序，最坏的残留
    /// 是"行没了、字节还多占一份"，那是一条可以安全重跑的幂等清理，而不是数据损坏。
    ///
    /// `WHERE NOT EXISTS` 先自己问一遍引用，`note_attachments.sha256` 上的 `ON DELETE RESTRICT`
    /// 再兜一层：判据哪天写错了，SQLite 会把整笔事务顶回来（错误原样抛给调用方，不吞）。
    pub fn purge_attachment_rows(&self, shas: &[String]) -> Result<Vec<String>, StoreError> {
        if shas.is_empty() {
            return Ok(Vec::new());
        }
        let shas = shas.to_vec();
        self.write_tx(|tx, now| {
            let mut del = tx.prepare(
                "DELETE FROM attachments WHERE sha256 = ?1
                  AND NOT EXISTS (SELECT 1 FROM note_attachments na WHERE na.sha256 = attachments.sha256)",
            )?;
            let mut settle = tx.prepare(
                "UPDATE sync_operations SET state = 'done', updated_at = ?2
                  WHERE entity_type = 'attachment' AND entity_id = ?1
                    AND state IN ('pending','inflight')",
            )?;
            let mut purged = Vec::new();
            for sha in &shas {
                if del.execute(params![sha])? == 1 {
                    // 行都没了，挂在它上面的待办永远满足不了 —— 一起结掉（同上一条理由：
                    // "待同步"计数必须诚实）。
                    settle.execute(params![sha, now])?;
                    purged.push(sha.clone());
                }
            }
            Ok(purged)
        })
    }

    /// 撤销一个隔离标记（只清 `deleted_at`，两个状态位一字不动）。
    ///
    /// 为什么需要这一条：被 GC 认领过的行在**三个**后台口径里都是隐形的（两个队列与磁盘
    /// 体检都带 `deleted_at IS NULL`）。用户明确要求"把这份取回来"时，如果本机隔离区里那份
    /// 已经不可用（位腐、被磁盘清理删掉），就必须把标记撤掉这一行才重新排得进下载队列 ——
    /// 否则按钮点下去什么也不会发生，而那条待办永远关不掉（"待同步"计数永久虚高，§18）。
    /// 撤掉的代价正是用户要的东西：宽限期到此为止。
    pub fn release_attachment_quarantine(&self, sha256: &str) -> Result<(), StoreError> {
        let sha = sha256.to_string();
        self.write_tx(|tx, _now| {
            let n = tx.execute(
                "UPDATE attachments SET deleted_at = NULL WHERE sha256 = ?1",
                params![sha],
            )?;
            if n == 0 {
                return Err(StoreError::Constraint(format!("附件不存在: {sha}")));
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

    /// 待下载的附件：本地没有内容，而"服务器上有"这件事**还没被否定**。
    ///
    /// 所以口径是 `present` **或** `unknown`，不只是 `present`：外来笔记登记进来的行起步就是
    /// `unknown`（谁也没确认过），如果只等 `present`，那这台设备就永远不去取那个 blob ——
    /// 而唯一的确认办法恰恰是去问服务器一次。404 走的是既有的一条路：标 `absent`，就此收手
    /// （`run_attachment_round`），因此不会变成每轮重复的空转。
    pub fn attachment_downloads(&self, limit: usize) -> Result<Vec<AttachmentJob>, StoreError> {
        self.attachment_jobs(limit, "missing", &["present", "unknown"])
    }

    /// §27「本地附件缺失」的体检候选：账上 `available`、远端 `present`、盘上却没有的行。
    ///
    /// 为什么只扫 `remote_state='present'` 这一格：`available` 的行不进下载队列，所以
    /// "本地没了 + 远端有"这一格**没有任何别的代码看它** —— 不救就是永久坏掉。
    /// 其余三格各有归属，别抢：
    /// * `unknown`/`absent` + `available`：**上传队列本来就看得见它**（那条队列的远端口径
    ///   含 unknown/absent/error），会走"读本地字节 → 传"；本地文件不在时它把行标成 `error`
    ///   挂着（见 `run_attachment_round` 的上传分支），等用户把文件放回来再传 —— 这正是
    ///   "还没上过服务器的独家字节"该有的待遇，降级成 `missing` 反而会把它推进一张注定 404 的下载单。
    /// * `partial`/`missing`/`error`：下载队列已经覆盖。
    ///   `deleted_at IS NULL` 与两个队列同口径 —— 但今天它**不起作用**：没有生产代码往
    ///   `attachments.deleted_at` 写值（只有重置为 NULL 那两处），所以"回收站里等的东西不必救"
    ///   目前是意图而不是行为。
    ///
    /// `limit` 是**每轮工作量的上界**（§48 G4）：候选集不分页的话，整个目录被删时一轮要发
    /// 出 N 次 `stat` 再攒 N 条降级，全挤在常驻循环那一格里。排序按 `sha256` 稳定，而降级
    /// 出来的行会立刻离开候选集（不再是 `available`），所以每一轮取前 `limit` 条**不会饿死**
    /// 后面的行 —— 下一轮它们自然浮上来。
    pub fn attachment_repair_candidates(
        &self,
        limit: usize,
    ) -> Result<Vec<(String, i64)>, StoreError> {
        let conn = self.read()?;
        let mut stmt = conn.prepare(
            "SELECT sha256, size FROM attachments
              WHERE local_state = 'available' AND remote_state = 'present'
                AND deleted_at IS NULL ORDER BY sha256 LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    fn attachment_jobs(
        &self,
        limit: usize,
        local: &str,
        remote_in: &[&str],
    ) -> Result<Vec<AttachmentJob>, StoreError> {
        let placeholders = remote_in.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT sha256, size, media_type, remote_state FROM attachments
              WHERE local_state IN ('{local}','partial','error')
                AND remote_state IN ({ph})
                AND deleted_at IS NULL
              ORDER BY size ASC LIMIT ?",
            local = if local == "available" {
                "available"
            } else {
                "missing"
            },
            ph = placeholders
        );
        let conn = self.read()?;
        let mut stmt = conn.prepare(&sql)?;
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = remote_in
            .iter()
            .map(|s| Box::new(s.to_string()) as _)
            .collect();
        args.push(Box::new(limit as i64));
        let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|a| a.as_ref()).collect();
        let rows = stmt.query_map(rusqlite::params_from_iter(refs), |r| {
            Ok(AttachmentJob {
                sha256: r.get(0)?,
                size: r.get::<_, i64>(1)?,
                media_type: r.get(2)?,
                remote_state: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// 记下"远端有、本地还没有"的附件（清单/记录里读到引用时调用）。
    /// 只登记元数据与 `local_state='missing'`，**绝不**创建空 blob 占位。
    pub fn register_remote_attachment(
        &self,
        sha256: &str,
        size: i64,
        media_type: &str,
    ) -> Result<(), StoreError> {
        let sha = sha256.to_string();
        let media = if media_type.trim().is_empty() {
            "application/octet-stream".to_string()
        } else {
            media_type.to_string()
        };
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

    /// 关掉"已经没有活可干"的附件待办行。
    ///
    /// 为什么需要：同一份字节流被反复引用时会重复入队，而上传/下载队列是按
    /// `attachments` 的**状态**挑活的 —— 状态已经满足的那些行根本不会被取出来，
    /// 于是永远没人去关它。留着的表现就是设置页的"待同步"计数永久虚高（§18 要求诚实）。
    /// 方向必须分开判：`upload` 看服务器有没有、`download` 看本地有没有；
    /// 用 OR 混在一起会把"本地还缺着"的下载单也顺手关掉，那才是真的丢数据。
    ///
    /// **`failed` 为什么也在这里收**（而不是"归退避重试管"）：这句注释以前写的是退避重试，
    /// 实情是**没有任何后台消费者会再去碰一支 failed 的附件待办** —— 文本引擎只按 `local_views`
    /// 规划（附件不走那条路），附件轮只按 `attachments` 的状态挑活，`outbox_take` 在生产里没有
    /// 调用方。而 `outbox_pending` 的口径含 `failed`。所以"状态已经满足了还留着一支 failed"
    /// 就是一笔永远不会掉的虚高。
    /// **反过来那一半同样要守住**：状态**没**满足的 failed 一条都不许动 —— 那是"这一项确实还没
    /// 同步上去"，用户应当看到（§27 的收手形状靠界面上那颗「重新上传本机这份」接手）。
    pub fn settle_satisfied_attachment_ops(&self) -> Result<u32, StoreError> {
        self.write_tx(|tx, now| {
            let n = tx.execute(
                "UPDATE sync_operations SET state = 'done', updated_at = ?1
                  WHERE entity_type = 'attachment' AND state IN ('pending','inflight','failed')
                    AND EXISTS (
                      SELECT 1 FROM attachments a
                       WHERE a.sha256 = sync_operations.sha256
                         AND ((sync_operations.op = 'upload' AND a.remote_state = 'present')
                           OR (sync_operations.op = 'download' AND a.local_state = 'available')))",
                params![now],
            )?;
            Ok(n as u32)
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

    /// 附件登记的媒体类型（显示用）。行不存在时给 `None`，让调用方决定占位。
    pub fn attachment_media_type(&self, sha256: &str) -> Result<Option<String>, StoreError> {
        let sha = sha256.to_string();
        let conn = self.read()?;
        Ok(conn
            .query_row(
                "SELECT media_type FROM attachments WHERE sha256 = ?1",
                [sha.as_str()],
                |r| r.get::<_, String>(0),
            )
            .optional()?)
    }

    /// 落在这些文件夹里的笔记所引用的附件 sha（按文件夹导出时只带上这些字节）。
    /// 一次集合查询，不是"每篇笔记问一遍"。
    pub fn attachment_shas_in_folders(
        &self,
        folder_ids: &[String],
    ) -> Result<Vec<String>, StoreError> {
        if folder_ids.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.read()?;
        let marks = folder_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT DISTINCT na.sha256 FROM note_attachments na JOIN notes n ON n.id = na.note_id WHERE n.folder_id IN ({marks}) ORDER BY na.sha256"
        );
        let binds: Vec<&dyn rusqlite::ToSql> = folder_ids
            .iter()
            .map(|s| s as &dyn rusqlite::ToSql)
            .collect();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(binds.as_slice(), |r| r.get::<_, String>(0))?;
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

    /// 收件箱里是否已经有一张**同样的**未处理卡片（同实体、同一对哈希）。
    ///
    /// 去重用的。`UpdateDelete` / `DeleteUpdate`（P11）这类判定引擎**故意**不自动收敛
    /// （保留哪一边要用户决定），所以只要用户没处理，每一轮都会再判出同一件事 ——
    /// 每轮多一张卡片是骚扰，每轮多造一篇"本地副本"更是往用户库里塞垃圾（实测真发生过）。
    /// 只对"完全同一对哈希"去重：任意一侧又改了，那就是新事实，该再进一张。
    pub fn open_conflict_exists(
        &self,
        kind: EntityKind,
        id: &EntityId,
        local_hash: &str,
        remote_hash: &str,
    ) -> Result<bool, StoreError> {
        let conn = self.read()?;
        Ok(conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_conflicts
              WHERE entity_type = ?1 AND entity_id = ?2 AND state = 'open'
                AND local_hash = ?3 AND remote_hash = ?4)",
            params![rows::kind_tag(kind), id.as_str(), local_hash, remote_hash],
            |r| r.get::<_, i64>(0),
        )? == 1)
    }

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
            rows.map(|row| rows::parse_id(&row?))
                .collect::<Result<_, _>>()?
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

    /// **本机干净、而持久远端视图已经越过本机确认点**的那些实体（计划层的第二组输入）。
    ///
    /// 为什么必须有这一条查询：脏集（`dirty_entities`）按定义只报 `rev <> sync_rev`，于是
    /// "本机这一行已经追平、对面又往前走了一步"的那种行**根本进不了本轮计划** —— 计划里那一格
    /// 就只有远端视图，`decide(None, 远端删除)` 于是答 P2b"本地没有这一条，无事可做"，
    /// 可本地**确实有**这一条。实测形状（判据 `a_delete_reaches_a_device_that_had_nothing_pending`）：
    /// A 删除并公告，B 空闲、`remote_rev` 已经是删除那一版，主行却仍活在正常列表、徽标"已同步"。
    /// 编辑那一侧侥幸没坏，是因为 `decide(None, 活版本)` 走 P2 Pull，落库时按 id 更新到了既有行上。
    ///
    /// 口径：只要"干净行 + 远端视图 rev 严格大于本机确认点"就算候选，删除态/活版本都要
    /// （谁更活、要不要问用户，由计划层看着 l/r 判，不在这儿预断）。
    /// 已永久删除（`tombstones.purged=1`）的一律排除 —— 防复活优先。
    pub fn remote_moved_entities(&self, account: &str) -> Result<Vec<DirtyEntity>, StoreError> {
        let conn = self.read()?;
        let mut out = Vec::new();
        for (table, kind) in [("notes", EntityKind::Note), ("folders", EntityKind::Folder)] {
            let mut stmt = conn.prepare(&format!(
                "SELECT e.id, e.rev, e.content_hash, e.sync_rev FROM {table} e
                   JOIN sync_remote_index r
                     ON r.account_id = ?1 AND r.kind = ?2 AND r.entity_id = e.id
                  WHERE e.rev = e.sync_rev
                    AND r.rev > e.sync_rev
                    AND NOT EXISTS(SELECT 1 FROM tombstones t
                                    WHERE t.entity_type = ?2 AND t.entity_id = e.id AND t.purged = 1)
                  ORDER BY e.id"
            ))?;
            let rows = stmt.query_map(params![account, rows::kind_tag(kind)], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            })?;
            for row in rows {
                let (id, rev, hash, sync_rev) = row?;
                out.push(DirtyEntity {
                    kind,
                    id: rows::parse_id(&id)?,
                    rev: Rev(rev.max(0) as u64),
                    content_hash: hash,
                    sync_rev: Rev(sync_rev.max(0) as u64),
                    why: DirtyWhy::RemoteMoved,
                });
            }
        }
        Ok(out)
    }

    /// 用户**已经对哪一份分歧表过态**：实体 → 当时对面那一版的编号（P19 的输入）。
    ///
    /// 只数 `state='resolved'`：`dismissed` 是"现在不想决定"，不该换来让路。
    /// 每个实体取**最大**的那个已裁决远端编号 —— 对面再往前走（编号变了）就对不上，
    /// 冲突照旧重开，所以这条判据不会变成"永久静音"。
    /// 词汇一律走 `kind_tag` / `kind_from_tag` 这一对，两侧都不写字面量。
    /// 返回 `(已理解的清单, 看不懂而被跳过的行数)` —— 本 crate 没有日志依赖，
    /// 所以"跳过"这件事交给调用方说出来，绝不允许静默少算（那正是本仓踩过三次的形状）。
    pub fn resolved_divergences(&self) -> Result<(Vec<ResolvedDivergence>, usize), StoreError> {
        let conn = self.read()?;
        let mut stmt = conn.prepare(
            "SELECT entity_type, entity_id, MAX(remote_rev) FROM sync_conflicts
              WHERE state = 'resolved' GROUP BY entity_type, entity_id",
        )?;
        let raw: Vec<(String, String, i64)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .filter_map(|x| x.ok())
            .collect();
        let mut out = Vec::with_capacity(raw.len());
        let mut skipped = 0usize;
        for (tag, id, rev) in raw {
            match (rows::kind_from_tag(&tag), EntityId::parse(&id)) {
                (Ok(kind), Ok(id)) => out.push((kind, id, rev.max(0) as u64)),
                _ => skipped += 1,
            }
        }
        Ok((out, skipped))
    }

    /// 用户/引擎裁决后关闭一条冲突（`resolution` ∈ kept_both|local|remote|merged|manual）。
    pub fn resolve_conflict(&self, conflict_id: i64, resolution: &str) -> Result<(), StoreError> {
        let resolution = resolution.to_string();
        if !matches!(
            resolution.as_str(),
            "kept_both" | "local" | "remote" | "merged" | "manual"
        ) {
            return Err(StoreError::Constraint(format!(
                "未知 resolution: {resolution}"
            )));
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
            let n = tx.execute(
                "UPDATE sync_conflicts SET state='dismissed' WHERE id=?1",
                [conflict_id],
            )?;
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
fn envelope(
    kind: EntityKind,
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
    m.insert(
        "enc".into(),
        serde_json::json!({ "alg": "none", "hash_alg": "sha256" }),
    );
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
        return Err(StoreError::Constraint(format!(
            "笔记 {} 的 doc 不是 JSON 对象：拒绝构造残缺记录",
            n.id
        )));
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

impl Store {
    /// 把"服务器那一版"的原始记录字节挂到**该实体最新的未裁决冲突**上（迁移 0008）。
    ///
    /// 只挑 `state='open'` 里 id 最大的那一行：一轮里同一条笔记可能登记过多条冲突，
    /// 用户看到的是最新那条；往已裁决的行上写会造出"处理完了却被改动"的假象。
    ///
    /// 返回 0 行是**合法结果**（冲突刚好已被用户裁决），调用方按"这次没挂上"处理 ——
    /// 不许为了拿个返回值去新建一行，也不许因此丢弃冲突本身（卡片退回显示哈希 + 说明）。
    pub fn conflict_attach_payload(
        &self,
        account_id: &str,
        kind: EntityKind,
        id: &EntityId,
        wire: &[u8],
    ) -> Result<u32, StoreError> {
        let wire = String::from_utf8_lossy(wire).into_owned();
        let acct = account_id.to_string();
        let tag = rows::kind_tag(kind).to_string();
        let id = id.as_str().to_string();
        self.write_tx(move |tx, _now| {
            let n = tx.execute(
                "UPDATE sync_conflicts SET remote_wire = ?1
                  WHERE rowid = (
                    SELECT rowid FROM sync_conflicts
                     WHERE account_id = ?2 AND entity_type = ?3 AND entity_id = ?4
                       AND state = 'open'
                     ORDER BY id DESC LIMIT 1)",
                params![wire, acct, tag, id],
            )?;
            Ok(n as u32)
        })
    }
}

fn conflicts_where(conn: &Connection, cond: &str) -> Result<Vec<ConflictRow>, StoreError> {
    let sql = format!(
        "SELECT id, account_id, entity_type, entity_id, base_rev, local_rev, remote_rev, local_hash,
                remote_hash, auto_merged, copy_note_id, state, resolution, created_at, resolved_at,
                remote_wire
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
                state: ConflictState::parse(&r.get::<_, String>(11)?)
                    .unwrap_or(ConflictState::Open),
                resolution: r.get(12)?,
                created_at: r.get(13)?,
                resolved_at: r.get(14)?,
                remote_wire: r.get(15)?,
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
