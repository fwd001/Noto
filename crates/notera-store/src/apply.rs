//! `apply_remote` —— 同步引擎把远端事实落库的唯一入口（**单事务**，I5/I6）。
//!
//! 五条硬规则在这里落地：
//! * **I6 闸门**：信封的 `hash` 必须等于 `sha256(canonical(payload))`，且 `payload` 必须能通过
//!   richtext 的 normalize+validate；`payload`/`ct` 必须恰好一个非空。任一不成立 → 整批 `Err`
//!   并回滚，权威表一行都不写。
//! * **不复活**（I1/I3/ADR-0006）：带 `purged=1` 墓碑的实体被 upsert 一律拒绝。
//! * **rev 不回退**（I2）：`env.rev < 本地 rev` 拒绝；相等且哈希相同 = 幂等跳过。
//! * **未同步的本地内容不被远端删除摧毁**（C1/C4）：`Tombstone`/`Purge` 遇到脏实体直接拒绝，
//!   由同步层改道冲突收件箱。
//! * 拉回生效的字段推进按 DATA-MODEL §4.2：`rev ← remote_rev ; doc ← 远端内容 ; sync_rev ← rev`。

use crate::derive;
use crate::error::StoreError;
use crate::rows;
use crate::store::{folder_hash, CurNote, Edit, Store};
use crate::types::*;
use notera_core::{EntityId, EntityKind, Rev};
use rusqlite::{params, Connection, OptionalExtension};

/// v1 协议号（SYNC-PROTOCOL §2/§3）。高于它的记录本程序读不懂 → 拒绝写入。
const MAX_PROTOCOL: u64 = 1;

/// 解析后的记录信封（DATA-MODEL §11 的字段集）。
pub(crate) struct Env {
    /// 已校验等于目标表的 kind（保留用于诊断输出）。
    #[allow(dead_code)]
    pub kind: String,
    pub id: EntityId,
    pub rev: Rev,
    pub hash: String,
    pub deleted_at: Option<String>,
    pub purged: bool,
    pub device: Option<String>,
    pub payload: serde_json::Value,
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl Env {
    /// 取字段：先看 payload（包裹形态），再看信封顶层。
    fn field(&self, key: &str) -> Option<&serde_json::Value> {
        self.payload
            .as_object()
            .and_then(|m| m.get(key))
            .or_else(|| self.extra.get(key))
    }
}

pub(crate) fn env_of(v: &serde_json::Value, want: &str) -> Result<Env, StoreError> {
    let obj = v
        .as_object()
        .ok_or_else(|| StoreError::Rejected(format!("{want} 信封不是 JSON 对象")))?;
    let protocol = obj.get("protocol").and_then(|p| p.as_u64()).unwrap_or(1);
    if protocol > MAX_PROTOCOL {
        return Err(StoreError::Rejected(format!(
            "协议版本 {protocol} 高于本程序支持 {MAX_PROTOCOL}，不写库"
        )));
    }
    let kind = obj
        .get("kind")
        .and_then(|k| k.as_str())
        .ok_or_else(|| StoreError::Rejected("信封缺少 kind".into()))?
        .to_string();
    if kind != want {
        return Err(StoreError::Rejected(format!(
            "信封 kind={kind} 与目标表 {want} 不符"
        )));
    }
    let id_raw = obj
        .get("id")
        .and_then(|i| i.as_str())
        .ok_or_else(|| StoreError::Rejected("信封缺少 id".into()))?;
    let id =
        EntityId::parse(id_raw).map_err(|e| StoreError::Rejected(format!("信封 id 非法: {e}")))?;
    let rev = obj
        .get("rev")
        .and_then(|r| r.as_u64())
        .ok_or_else(|| StoreError::Rejected(format!("{kind} {id} 信封缺少 rev")))?;
    let hash = obj
        .get("hash")
        .and_then(|h| h.as_str())
        .ok_or_else(|| StoreError::Rejected(format!("{kind} {id} 信封缺少 hash")))?
        .to_string();
    let payload = obj
        .get("payload")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let ct = obj.get("ct").cloned().unwrap_or(serde_json::Value::Null);
    let purged = obj.get("purged").and_then(|p| p.as_bool()).unwrap_or(false);
    let alg = obj
        .get("enc")
        .and_then(|e| e.get("alg"))
        .and_then(|a| a.as_str())
        .unwrap_or("none")
        .to_string();
    // SYNC-PROTOCOL §3：payload 与 ct 恰好一个非空；purged 记录是墓碑公告（两者皆空）
    if !purged {
        match (payload.is_null(), ct.is_null()) {
            (true, true) | (false, false) => {
                return Err(StoreError::Rejected(format!(
                    "{kind} {id}: payload 与 ct 必须恰好一个非空"
                )))
            }
            (false, true) => {}
            _ => {
                return Err(StoreError::Rejected(format!(
                    "{kind} {id}: 加密载荷必须在同步层解密后以 enc.alg=none 传入"
                )))
            }
        }
        if alg != "none" {
            return Err(StoreError::Rejected(format!(
                "{kind} {id}: enc.alg={alg}，存储层不解密"
            )));
        }
    }
    let deleted_at = obj
        .get("deleted_at")
        .and_then(|d| d.as_str())
        .or_else(|| {
            payload
                .as_object()
                .and_then(|m| m.get("deleted_at"))
                .and_then(|d| d.as_str())
        })
        .map(|s| s.to_string());
    let device = obj
        .get("device")
        .and_then(|d| d.as_str())
        .map(|s| s.to_string());
    Ok(Env {
        kind,
        id,
        rev: Rev(rev),
        hash,
        deleted_at,
        purged,
        device,
        payload,
        extra: obj.clone(),
    })
}

impl Store {
    /// 一批远端动作在**单事务**内落地：任一被拒绝 → 整批回滚（无部分写入）。
    pub fn apply_remote(&self, ops: &[ApplyOp]) -> Result<ApplyReport, StoreError> {
        let ops = ops.to_vec();
        self.write_tx(|tx, now| {
            let mut report = ApplyReport::default();
            for op in &ops {
                self.apply_one(tx, op, now, &mut report)?;
            }
            Ok(report)
        })
    }

    fn apply_one(
        &self,
        tx: &Connection,
        op: &ApplyOp,
        now: &str,
        rep: &mut ApplyReport,
    ) -> Result<(), StoreError> {
        match op {
            ApplyOp::UpsertNote { env } => {
                self.apply_note(tx, env_of(env, "note")?, now, rep, false)
            }
            ApplyOp::AdoptConflict { env } => {
                self.apply_note(tx, env_of(env, "note")?, now, rep, true)
            }
            ApplyOp::UpsertFolder { env } => {
                self.apply_folder(tx, env_of(env, "folder")?, now, rep)
            }
            ApplyOp::SetRemote {
                kind,
                id,
                rev,
                hash12,
            } => {
                if Self::set_remote_rev_tx(tx, now, *kind, id, *rev, hash12)? {
                    rep.applied += 1;
                } else {
                    rep.skipped += 1;
                }
                Ok(())
            }
            ApplyOp::Tombstone { kind, id, rev } => {
                self.apply_tombstone(tx, *kind, id, *rev, now, rep)
            }
            ApplyOp::Purge { kind, id } => self.apply_purge(tx, *kind, id, now, rep),
        }
    }

    /// `adopt = true` 只允许由 [`ApplyOp::AdoptConflict`] 传入：冲突采纳远端正文。
    /// 它与普通 upsert 的差别只有一处 —— 允许 `rev` 相等而内容不同（见那条注释）。
    fn apply_note(
        &self,
        tx: &Connection,
        env: Env,
        now: &str,
        rep: &mut ApplyReport,
        adopt: bool,
    ) -> Result<(), StoreError> {
        if env.purged {
            return self.apply_purge(tx, EntityKind::Note, &env.id, now, rep);
        }
        // 复活闸门（I1/I3）：被永久删除过的 id 永不接受 upsert
        if let Some(t) = Self::tombstone_of(tx, EntityKind::Note, &env.id)? {
            if t.purged {
                return Err(StoreError::Rejected(format!(
                    "拒绝复活：笔记 {} 已有 purged 墓碑（rev {}）",
                    env.id, t.rev
                )));
            }
        }
        let doc_value = note_doc(&env)?;
        let prepared = derive::prepare(&doc_value)?;
        if !derive::hash_matches(&env.hash, &prepared.content_hash) {
            return Err(StoreError::Rejected(format!(
                "笔记 {} 信封哈希不符（I6）：声明 {}，实算 {}",
                env.id,
                env.hash,
                prepared.content_hash.as_str()
            )));
        }
        // doc 是外来笔记里附件引用的唯一来源：与笔记同一事务登记（DATA-MODEL §8）
        let refs = prepared.attachments.clone();
        let folder_id: Option<EntityId> = match env.field("folder_id") {
            None => None,
            Some(v) if v.is_null() => None,
            Some(v) => Some(
                EntityId::parse(v.as_str().ok_or_else(|| {
                    StoreError::Rejected(format!("笔记 {} 的 folder_id 不是字符串", env.id))
                })?)
                .map_err(|e| {
                    StoreError::Rejected(format!("笔记 {} 的 folder_id 非法: {e}", env.id))
                })?,
            ),
        };
        let pinned = env.field("pinned").and_then(|v| v.as_bool());
        // 区分"字段缺失"（不动）与"字段为 null"（清空）
        let color: Option<Option<String>> = env
            .field("color")
            .map(|v| v.as_str().map(|s| s.to_string()));
        let device = env
            .device
            .clone()
            .unwrap_or_else(|| self.device.to_string());

        match Self::load_cur_opt(tx, &env.id)? {
            None => {
                if env.rev.get() == 0 {
                    return Err(StoreError::Rejected(format!("笔记 {} 的 rev 为 0", env.id)));
                }
                let folder = match &folder_id {
                    Some(f) => f.clone(),
                    None => Self::default_folder_id(tx)?,
                };
                let exists: i64 = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM folders WHERE id = ?1)",
                    [folder.as_str()],
                    |r| r.get(0),
                )?;
                if exists == 0 {
                    return Err(StoreError::Rejected(format!(
                        "笔记 {} 指向不存在的文件夹 {folder}（I6：不写悬空记录）",
                        env.id
                    )));
                }
                tx.execute(
                    "INSERT INTO notes
                       (id, folder_id, doc, doc_format, pinned, color, title, plain_text, summary,
                        char_count, block_count, has_attachment, rev, sync_rev, sync_hash, remote_rev,
                        content_hash, created_at, updated_at, deleted_at, purged_at, created_device, updated_device)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?13,?14,?13,?14,?15,?15,?16,NULL,?17,?17)",
                    params![
                        env.id.as_str(), folder.as_str(), prepared.doc_json, prepared.doc_version as i64,
                        pinned.unwrap_or(false) as i64, color.flatten(), prepared.title, prepared.plain_text,
                        prepared.summary, prepared.char_count as i64, prepared.block_count as i64,
                        prepared.doc_has_attachment as i64, env.rev.get() as i64, prepared.content_hash.as_str(),
                        now, env.deleted_at.clone(), device
                    ],
                )?;
                let rowid = Self::note_rowid_or_err(tx, &env.id)?;
                rows::fts_insert(tx, rowid, &prepared.title, &prepared.plain_text)?;
                rows::insert_revision(
                    tx,
                    &env.id,
                    env.rev,
                    &prepared.doc_json,
                    prepared.content_hash.as_str(),
                    RevOrigin::Remote,
                    &device,
                    now,
                )?;
                // 新建分支不走 `commit_edit`，链接表在这里登记；更新分支由 `commit_edit` 登记。
                self.register_doc_attachments(tx, &env.id, &refs, now)?;
                rep.applied += 1;
                rep.notes_written += 1;
            }
            Some(cur) => {
                if env.rev.get() < cur.note.rev.get() {
                    return Err(StoreError::Rejected(format!(
                        "笔记 {} 远端 rev {} 低于本地 {}：拒绝回退（I2）",
                        env.id, env.rev, cur.note.rev
                    )));
                }
                if env.rev == cur.note.rev {
                    if env.hash == cur.note.content_hash {
                        rep.skipped += 1; // 幂等重放
                        return Ok(());
                    }
                    // 同一个 rev 却是两份内容：普通 upsert 一律拒绝（那是服务器侧异常，
                    // 覆盖谁都没有依据）。唯一例外是**冲突采纳** —— 两台设备从同一个确认点
                    // 各自推到同一个 rev，是分布式写作的正常结果，而本机那一份在调用之前
                    // 已经存成副本笔记了（CONFLICT-RESOLUTION §6.1），所以这里换正文不丢任何东西。
                    if !adopt {
                        return Err(StoreError::Rejected(format!(
                            "笔记 {} 同 rev {} 内容不同：服务器侧异常，不覆盖本地",
                            env.id, env.rev
                        )));
                    }
                }
                let edit = Edit {
                    doc: Some(prepared),
                    expected_rev: None,
                    folder_id,
                    pinned,
                    color,
                    deleted_at: Some(env.deleted_at.clone()),
                    origin: RevOrigin::Remote,
                    enqueue: false,
                    force_rev: Some(env.rev),
                    confirm_sync: true,
                    remote_rev_to: Some(env.rev),
                };
                self.commit_edit(tx, &cur, &edit, now)?;
                rep.applied += 1;
                rep.notes_written += 1;
            }
        }
        Ok(())
    }

    fn apply_folder(
        &self,
        tx: &Connection,
        env: Env,
        now: &str,
        rep: &mut ApplyReport,
    ) -> Result<(), StoreError> {
        if env.purged {
            return self.apply_purge(tx, EntityKind::Folder, &env.id, now, rep);
        }
        if let Some(t) = Self::tombstone_of(tx, EntityKind::Folder, &env.id)? {
            if t.purged {
                return Err(StoreError::Rejected(format!(
                    "拒绝复活：文件夹 {} 已有 purged 墓碑",
                    env.id
                )));
            }
        }
        let name = env
            .field("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| StoreError::Rejected(format!("文件夹 {} 缺 name", env.id)))?
            .to_string();
        let color = env
            .field("color")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let sort_order = env
            .field("sort_order")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let parent: Option<EntityId> = match env.field("parent_id") {
            None => None,
            Some(v) if v.is_null() => None,
            Some(v) => Some(
                EntityId::parse(v.as_str().ok_or_else(|| {
                    StoreError::Rejected(format!("文件夹 {} 的 parent_id 不是字符串", env.id))
                })?)
                .map_err(|e| {
                    StoreError::Rejected(format!("文件夹 {} 的 parent_id 非法: {e}", env.id))
                })?,
            ),
        };
        if let Some(p) = &parent {
            let exists: i64 = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM folders WHERE id = ?1)",
                [p.as_str()],
                |r| r.get(0),
            )?;
            if exists == 0 {
                return Err(StoreError::Rejected(format!(
                    "文件夹 {} 指向不存在的父 {p}（I6：不写悬空记录）",
                    env.id
                )));
            }
        }
        let hash = folder_hash(&name, &parent, &color, sort_order, &None);
        let device = env
            .device
            .clone()
            .unwrap_or_else(|| self.device.to_string());
        match rows::read_folder(tx, &env.id)? {
            None => {
                tx.execute(
                    "INSERT INTO folders
                       (id, parent_id, name, color, system_kind, sort_order, rev, sync_rev, sync_hash,
                        remote_rev, content_hash, created_at, updated_at, deleted_at, purged_at, created_device, updated_device)
                     VALUES (?1,?2,?3,?4,NULL,?5,?6,?6,?7,?6,?7,?8,?8,?9,NULL,?10,?10)",
                    params![
                        env.id.as_str(), parent.as_ref().map(|p| p.as_str().to_string()), name, color, sort_order,
                        env.rev.get() as i64, hash, now, env.deleted_at, device
                    ],
                )?;
            }
            Some(cur) => {
                if env.rev.get() < cur.rev.get() {
                    return Err(StoreError::Rejected(format!(
                        "文件夹 {} 远端 rev {} 低于本地 {}：拒绝回退（I2）",
                        env.id, env.rev, cur.rev
                    )));
                }
                // **父不许把树弯成环。** 本机那一支（`move_folder`）一直查成环，远端这一支过去
                // 只查"父存在不存在"—— 于是两台设备各做一次"单独看合法"的移动就能合出一个环：
                // A 把甲移到乙下面，B 把乙移到甲下面，两边一追平，每台都收到对面那一支。
                // 后果有两个，都不报错：① 这两个子夹从根走不到了（侧栏里没有，里面的笔记也看不见）；
                // ② 用户下一次移动任何文件夹都要跑那条递归 CTE，有环时它不返回。
                // 处置：认对面那一版的**其余字段**与 rev（保证这一行能 settle、不每轮重推），
                // 唯独**不写父**，保留本机现在的父。两边因此会在"这两个夹子的位置"上各自保留
                // 自己那一版 —— 这是有意的：位置分叉看得见、可修（用户再移一次），
                // 环与卡死看不见、也修不了。
                let parent = match &parent {
                    Some(p) if *p == env.id || Self::descendant_ids(tx, &env.id)?.contains(p) => {
                        cur.parent_id.clone()
                    }
                    other => other.clone(),
                };
                let hash = folder_hash(&name, &parent, &color, sort_order, &None);
                if env.rev == cur.rev && hash == cur.content_hash {
                    rep.skipped += 1;
                    return Ok(());
                }
                tx.execute(
                    "UPDATE folders SET name=?2, parent_id=?3, color=?4, sort_order=?5, rev=?6, sync_rev=?6,
                        sync_hash=?7, remote_rev=?6, content_hash=?7, updated_at=?8, updated_device=?9, deleted_at=?10
                      WHERE id=?1",
                    params![
                        env.id.as_str(), name, parent.as_ref().map(|p| p.as_str().to_string()), color, sort_order,
                        env.rev.get() as i64, hash, now, device, env.deleted_at
                    ],
                )?;
            }
        }
        rep.applied += 1;
        rep.folders_written += 1;
        Ok(())
    }

    /// 远端删除事实：本地保留行 + `deleted_at`，并写下墓碑（删除是状态，I3）。
    fn apply_tombstone(
        &self,
        tx: &Connection,
        kind: EntityKind,
        id: &EntityId,
        rev: Rev,
        now: &str,
        rep: &mut ApplyReport,
    ) -> Result<(), StoreError> {
        if kind == EntityKind::Attachment {
            return Err(StoreError::Constraint(
                "附件以 sha256 寻址：请用 Store::set_attachment_states".into(),
            ));
        }
        let table = if kind == EntityKind::Note {
            "notes"
        } else {
            "folders"
        };
        let head: Option<(i64, i64)> = tx
            .query_row(
                &format!("SELECT rev, sync_rev FROM {table} WHERE id = ?1"),
                [id.as_str()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((local_rev, sync_rev)) = head {
            if (rev.get() as i64) < local_rev {
                if local_rev != sync_rev {
                    // C4：删除 vs 本地未推送的修改 → 必须走冲突收件箱，不得静默删除
                    return Err(StoreError::Rejected(format!(
                        "{kind:?} {id} 本地 rev {local_rev}（已确认 {sync_rev}）高于远端删除 rev {rev}：交由冲突解决"
                    )));
                }
                rep.skipped += 1; // 本地已比远端新：幂等跳过
                return Ok(());
            }
            tx.execute(
                &format!("UPDATE {table} SET deleted_at = COALESCE(deleted_at, ?2), rev = ?3, sync_rev = ?3 WHERE id = ?1"),
                params![id.as_str(), now, rev.get() as i64],
            )?;
        }
        self.write_tombstone(tx, kind, id, rev, false, None, None, now)?;
        rep.applied += 1;
        rep.tombstones_written += 1;
        Ok(())
    }

    /// 远端永久删除：写 `tombstones(purged=1)` + 删本地行（墓碑不 GC，I3）。
    fn apply_purge(
        &self,
        tx: &Connection,
        kind: EntityKind,
        id: &EntityId,
        now: &str,
        rep: &mut ApplyReport,
    ) -> Result<(), StoreError> {
        if kind == EntityKind::Attachment {
            return Err(StoreError::Constraint(
                "附件以 sha256 寻址：请用 Store::set_attachment_states".into(),
            ));
        }
        let table = if kind == EntityKind::Note {
            "notes"
        } else {
            "folders"
        };
        let head: Option<(i64, i64, Option<String>)> = tx
            .query_row(
                &if kind == EntityKind::Note {
                    "SELECT rev, sync_rev, title FROM notes WHERE id = ?1".to_string()
                } else {
                    "SELECT rev, sync_rev, name FROM folders WHERE id = ?1".to_string()
                },
                [id.as_str()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let prev: Option<i64> = tx
            .query_row(
                "SELECT rev FROM tombstones WHERE entity_type=?1 AND entity_id=?2",
                params![rows::kind_tag(kind), id.as_str()],
                |r| r.get(0),
            )
            .optional()?;
        let mut rev = Rev(prev.unwrap_or(0).max(0) as u64);
        let title;
        if let Some((local_rev, sync_rev, snap)) = head {
            if local_rev != sync_rev {
                // C1：用户输入过的内容不因远端永久删除而静默消失
                return Err(StoreError::Rejected(format!(
                    "{kind:?} {id} 本地有未确认修改（rev {local_rev} ≠ sync_rev {sync_rev}）：拒绝永久删除，交由冲突解决"
                )));
            }
            rev = notera_core::next_rev(rev, Rev(local_rev.max(0) as u64));
            title = snap;
        } else {
            if rev.get() == 0 {
                // 既无本地行也无墓碑：仍要留下删除事实（I3），rev 起点用 1。
                rev = notera_core::next_rev(Rev::ZERO, Rev::ZERO);
            }
            title = tx
                .query_row(
                    "SELECT title_snap FROM tombstones WHERE entity_type=?1 AND entity_id=?2",
                    params![rows::kind_tag(kind), id.as_str()],
                    |r| r.get::<_, Option<String>>(0),
                )
                .optional()?
                .flatten();
        }
        rows::supersede_pending(tx, now, kind, id)?;
        self.write_tombstone(tx, kind, id, rev, true, None, title, now)?;
        let n = tx.execute(&format!("DELETE FROM {table} WHERE id = ?1"), [id.as_str()])?;
        rep.applied += 1;
        rep.tombstones_written += 1;
        rep.rows_removed += n;
        Ok(())
    }

    /// 冲突卡片上"用我这一版"真正要做的事：**把正文与副本互换**。
    ///
    /// §6.1 采纳之后正文是服务器那一版、本机那一版活在副本笔记里；用户点这颗按钮就是要
    /// 把这个方向反过来。互换（而不是"一边覆盖另一边"）的理由：两版始终各有一处存放，
    /// 这个决定因此不可能吃掉任何人的输入 —— 而副本是普通笔记，会照常同步给别人。
    ///
    /// 前提用事实核对，不靠调用方的自觉：卡片必须是未处理的**笔记**冲突、当时存下了副本，
    /// 而且正文现在的内容确实就是当初采纳进来的那一版（`content_hash == remote_hash`）。
    /// 任一条件不成立就返回 `Ok(false)` —— 那时"正文用我这一版"要么已经成立，要么这按钮
    /// 对这个卡片没有内容可换，调用方据此只把卡片关掉即可（绝不静默改别的东西）。
    pub fn swap_conflict_sides(&self, conflict_id: i64) -> Result<bool, StoreError> {
        let card = self.with_read(|c| {
            c.query_row(
                "SELECT entity_id, copy_note_id, remote_hash FROM sync_conflicts
                  WHERE id = ?1 AND state = 'open' AND entity_type = 'note' AND copy_note_id IS NOT NULL",
                [conflict_id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)),
            )
            .optional()
            .map_err(StoreError::from)
        })?;
        let Some((live_raw, copy_raw, remote_hash)) = card else {
            return Ok(false);
        };
        let live = EntityId::parse(&live_raw).map_err(|e| StoreError::Rejected(e.to_string()))?;
        let copy = EntityId::parse(&copy_raw).map_err(|e| StoreError::Rejected(e.to_string()))?;
        self.write_tx(|tx, now| {
            let a = Self::load_cur(tx, &live)?;
            // 行上存的是清单里那一份哈希（12 位短形式），本地行上是全哈希 —— 必须按同一条
            // 容忍规则比，用 `!=` 会让互换永远不发生。
            if !notera_core::same_content_hash(&remote_hash, a.note.content_hash.as_str()) {
                return Ok(false);
            }
            let b = Self::load_cur(tx, &copy)?;
            let (a_doc, b_doc) = (a.doc_json.clone(), b.doc_json.clone());
            // 两条都要 enqueue：正文换了内容就必须重新公告，否则别的设备看到的还是旧的那一版
            let swap = |target: &CurNote, doc: String| {
                self.commit_edit(
                    tx,
                    target,
                    &Edit {
                        doc: Some(derive::prepare_text(&doc)?),
                        origin: RevOrigin::Local,
                        enqueue: true,
                        ..Edit::default()
                    },
                    now,
                )
            };
            swap(&b, a_doc)?;
            swap(&a, b_doc)?;
            tx.execute(
                "UPDATE sync_conflicts SET state = 'resolved', resolution = 'local', resolved_at = ?2 WHERE id = ?1",
                params![conflict_id, now],
            )?;
            Ok(true)
        })
    }

    pub(crate) fn note_rowid_or_err(tx: &Connection, id: &EntityId) -> Result<i64, StoreError> {
        rows::note_rowid(tx, id)?.ok_or_else(|| StoreError::not_found(EntityKind::Note, id.clone()))
    }
}

/// 从信封里取笔记正文：`payload` 本身就是 Document，或被包了一层 `doc`。
fn note_doc(env: &Env) -> Result<serde_json::Value, StoreError> {
    let map = env
        .payload
        .as_object()
        .ok_or_else(|| StoreError::Rejected(format!("笔记 {} 的 payload 不是对象", env.id)))?;
    if map.contains_key("content") {
        return Ok(env.payload.clone());
    }
    if let Some(d) = map.get("doc").or_else(|| map.get("payload")) {
        return Ok(d.clone());
    }
    Err(StoreError::Rejected(format!(
        "笔记 {} 的 payload 里没有 Document（缺 content）",
        env.id
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope(
        kind: &str,
        id: &EntityId,
        rev: u64,
        payload: serde_json::Value,
    ) -> serde_json::Value {
        serde_json::json!({
            "protocol": 1, "kind": kind, "id": id.as_str(), "rev": rev,
            "hash": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
            "updated_at": "2026-09-25T00:00:00.000Z", "deleted_at": null, "purged": false,
            "enc": {"alg": "none", "hash_alg": "sha256"}, "payload": payload, "ct": null
        })
    }

    #[test]
    fn env_rejects_shape_violations_before_any_write() {
        let id = EntityId::new();
        // payload 与 ct 同时非空 → 违反 SYNC-PROTOCOL §3
        let mut e = envelope("note", &id, 1, serde_json::json!({"v":1,"content":[]}));
        e["ct"] = serde_json::json!("cipher");
        assert!(env_of(&e, "note").is_err());
        // kind 与目标不符
        let e2 = envelope("folder", &id, 1, serde_json::json!({"name":"x"}));
        assert!(env_of(&e2, "note").is_err());
        // 协议版本过高
        let mut e3 = envelope("note", &id, 1, serde_json::json!({"v":1,"content":[]}));
        e3["protocol"] = serde_json::json!(99);
        assert!(env_of(&e3, "note").is_err());
    }

    #[test]
    fn env_reads_document_and_wrapped_shapes() {
        let id = EntityId::new();
        let doc = serde_json::json!({"v": notera_richtext::DOC_FORMAT, "content": []});
        let plain = envelope("note", &id, 3, doc.clone());
        let env = env_of(&plain, "note").expect("合法信封");
        assert_eq!(env.rev, Rev(3));
        assert_eq!(note_doc(&env).unwrap(), doc);
        let wrapped = envelope(
            "note",
            &id,
            3,
            serde_json::json!({"doc": doc, "folder_id": "x", "pinned": true}),
        );
        assert_eq!(note_doc(&env_of(&wrapped, "note").unwrap()).unwrap(), doc);
    }
}
