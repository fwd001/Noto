//! 行映射与仓储内部件：列清单、FTS 维护、outbox 入队、meta 读写。
//!
//! 这里放的是"只有一个写入口"所需的底层件，全部接收 `&Connection`（可以是事务），
//! 因此 `Store` 的每个公共写方法都能把 `notes` / `note_revisions` / 派生列 /
//! `notes_fts` / `sync_operations` 关进**同一个事务**（I5）。

use crate::error::StoreError;
use crate::types::{Attachment, Folder, Note, NoteListRow, OpKind};
use notera_core::{EntityId, EntityKind, Rev};
use rusqlite::OptionalExtension;
use rusqlite::{params, Connection, Row};

/// `notes` 的读取列清单（顺序与 `note_from_row` 严格对应）。
/// 注意：列表投影走 [`LIST_COLS`]，那里**没有 `doc`**（DATA-MODEL §13）。
pub(crate) const NOTE_COLS: &str = "id, folder_id, doc, doc_format, title, plain_text, summary, \
     char_count, block_count, has_attachment, pinned, color, rev, sync_rev, sync_hash, remote_rev, \
     content_hash, created_at, updated_at, deleted_at, purged_at";

/// 列表投影：绝不 `SELECT doc`（性能约束，DATA-MODEL §13）。
pub(crate) const LIST_COLS: &str = "n.id, n.folder_id, f.name, n.title, n.summary, n.char_count, \
     n.block_count, n.has_attachment, n.pinned, n.color, n.rev, n.sync_rev, n.remote_rev, \
     n.content_hash, n.created_at, n.updated_at, n.deleted_at, n.purged_at, (n.rev <> n.sync_rev)";

pub(crate) const FOLDER_COLS: &str = "id, parent_id, name, color, system_kind, sort_order, rev, \
     sync_rev, sync_hash, remote_rev, content_hash, created_at, updated_at, deleted_at, purged_at";

// --------------------------------------------------------------- 基础转换 ---

/// 行映射里的业务错误要借道 `rusqlite::Error` 才能穿过 `query_map`；
/// 上层 `?` 会把它包回 `StoreError::Sql`，信息不丢。
pub(crate) fn into_sql(e: StoreError) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
}

pub(crate) fn kind_tag(kind: EntityKind) -> &'static str {
    match kind {
        EntityKind::Note => "note",
        EntityKind::Folder => "folder",
        EntityKind::Attachment => "attachment",
    }
}

pub(crate) fn kind_from_tag(s: &str) -> Result<EntityKind, StoreError> {
    Ok(match s {
        "note" => EntityKind::Note,
        "folder" => EntityKind::Folder,
        "attachment" => EntityKind::Attachment,
        other => return Err(StoreError::Constraint(format!("未知 entity_type: {other}"))),
    })
}

/// 行映射内部用：错误借道 `rusqlite::Error`。
fn map_id(s: &str) -> rusqlite::Result<EntityId> {
    EntityId::parse(s).map_err(|e| into_sql(StoreError::Constraint(format!("库内 id 不是合法 UUID: {e}"))))
}

pub(crate) fn parse_id(s: &str) -> Result<EntityId, StoreError> {
    EntityId::parse(s).map_err(|_| StoreError::Constraint(format!("库内 id 不是合法 UUID: {s}")))
}

fn rev_of(row: &Row, i: usize) -> rusqlite::Result<Rev> {
    let v: i64 = row.get(i)?;
    Ok(Rev(v.max(0) as u64))
}

fn bool_of(row: &Row, i: usize) -> rusqlite::Result<bool> {
    let v: i64 = row.get(i)?;
    Ok(v != 0)
}

// ------------------------------------------------------------------ Note ---

pub(crate) fn note_from_row(row: &Row) -> rusqlite::Result<Note> {
    let doc_text: String = row.get(2)?;
    let doc: serde_json::Value = serde_json::from_str(&doc_text)
        .map_err(|e| into_sql(StoreError::Constraint(format!("notes.doc 不是合法 JSON: {e}"))))?;
    Ok(Note {
        id: map_id(&row.get::<_, String>(0)?)?,
        folder_id: map_id(&row.get::<_, String>(1)?)?,
        doc,
        doc_format: row.get::<_, i64>(3)? as u16,
        title: row.get(4)?,
        plain_text: row.get(5)?,
        summary: row.get(6)?,
        char_count: row.get::<_, i64>(7)? as u32,
        block_count: row.get::<_, i64>(8)? as u32,
        has_attachment: bool_of(row, 9)?,
        pinned: bool_of(row, 10)?,
        color: row.get(11)?,
        rev: rev_of(row, 12)?,
        sync_rev: rev_of(row, 13)?,
        sync_hash: row.get(14)?,
        remote_rev: rev_of(row, 15)?,
        content_hash: row.get(16)?,
        created_at: row.get(17)?,
        updated_at: row.get(18)?,
        deleted_at: row.get(19)?,
        purged_at: row.get(20)?,
    })
}

pub(crate) fn read_note(conn: &Connection, id: &EntityId) -> Result<Option<Note>, StoreError> {
    let sql = format!("SELECT {NOTE_COLS} FROM notes WHERE id = ?1");
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query_map([id.as_str()], note_from_row)?;
    match rows.next() {
        Some(r) => Ok(Some(r?)),
        None => Ok(None),
    }
}

/// 读 `notes.rowid`（FTS external-content 的连接键）。
pub(crate) fn note_rowid(conn: &Connection, id: &EntityId) -> Result<Option<i64>, StoreError> {
    Ok(conn
        .query_row("SELECT rowid FROM notes WHERE id = ?1", [id.as_str()], |r| r.get(0))
        .optional()?)
}

pub(crate) fn list_from_row(row: &Row) -> rusqlite::Result<NoteListRow> {
    Ok(NoteListRow {
        id: map_id(&row.get::<_, String>(0)?)?,
        folder_id: map_id(&row.get::<_, String>(1)?)?,
        folder_name: row.get(2)?,
        title: row.get(3)?,
        summary: row.get(4)?,
        char_count: row.get::<_, i64>(5)? as u32,
        block_count: row.get::<_, i64>(6)? as u32,
        has_attachment: bool_of(row, 7)?,
        pinned: bool_of(row, 8)?,
        color: row.get(9)?,
        rev: rev_of(row, 10)?,
        sync_rev: rev_of(row, 11)?,
        remote_rev: rev_of(row, 12)?,
        content_hash: row.get(13)?,
        created_at: row.get(14)?,
        updated_at: row.get(15)?,
        deleted_at: row.get(16)?,
        purged_at: row.get(17)?,
        dirty: bool_of(row, 18)?,
    })
}

// ---------------------------------------------------------------- Folder ---

pub(crate) fn folder_from_row(row: &Row) -> rusqlite::Result<Folder> {
    let parent: Option<String> = row.get(1)?;
    Ok(Folder {
        id: map_id(&row.get::<_, String>(0)?)?,
        parent_id: match parent {
            Some(p) => Some(map_id(&p)?),
            None => None,
        },
        name: row.get(2)?,
        color: row.get(3)?,
        system_kind: row.get(4)?,
        sort_order: row.get(5)?,
        rev: rev_of(row, 6)?,
        sync_rev: rev_of(row, 7)?,
        sync_hash: row.get(8)?,
        remote_rev: rev_of(row, 9)?,
        content_hash: row.get(10)?,
        created_at: row.get(11)?,
        updated_at: row.get(12)?,
        deleted_at: row.get(13)?,
        purged_at: row.get(14)?,
    })
}

pub(crate) fn read_folder(conn: &Connection, id: &EntityId) -> Result<Option<Folder>, StoreError> {
    let sql = format!("SELECT {FOLDER_COLS} FROM folders WHERE id = ?1");
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query_map([id.as_str()], folder_from_row)?;
    match rows.next() {
        Some(r) => Ok(Some(r?)),
        None => Ok(None),
    }
}

// ----------------------------------------------------------- Attachment ----

pub(crate) const ATTACHMENT_COLS: &str = "sha256, size, media_type, filename, width, height, \
     duration_ms, local_state, remote_state, created_at, verified_at, deleted_at";

/// 同一批列，但带表别名前缀。
///
/// `attachments` 与 `note_attachments` 都有 `sha256` 列，联表时用裸列清单会直接
/// 报 `ambiguous column name: sha256`（实测踩过）。单表读用 `ATTACHMENT_COLS`，
/// 联表读必须用本函数。
pub(crate) fn attachment_cols(alias: &str) -> String {
    ATTACHMENT_COLS
        .split(", ")
        .map(|c| format!("{alias}.{}", c.trim()))
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) fn attachment_from_row(row: &Row) -> rusqlite::Result<Attachment> {
    Ok(Attachment {
        sha256: row.get(0)?,
        size: row.get(1)?,
        media_type: row.get(2)?,
        filename: row.get(3)?,
        width: row.get(4)?,
        height: row.get(5)?,
        duration_ms: row.get(6)?,
        local_state: row.get(7)?,
        remote_state: row.get(8)?,
        created_at: row.get(9)?,
        verified_at: row.get(10)?,
        deleted_at: row.get(11)?,
        note_id: map_id(&row.get::<_, String>(12)?)?,
        block_id: row.get(13)?,
        role: row.get(14)?,
        position: row.get(15)?,
    })
}

// ------------------------------------------------------ FTS5 手工维护 -----
//
// external content 表没有触发器（DATA-MODEL §5.3）：更新必须先按**旧值**删索引行、
// 再按**新值**插入，且两步都在写事务里（I5）。

pub(crate) fn fts_delete(conn: &Connection, rowid: i64, title: &str, plain_text: &str) -> Result<(), StoreError> {
    conn.execute(
        "INSERT INTO notes_fts(notes_fts, rowid, title, plain_text) VALUES ('delete', ?1, ?2, ?3)",
        params![rowid, title, plain_text],
    )?;
    Ok(())
}

pub(crate) fn fts_insert(conn: &Connection, rowid: i64, title: &str, plain_text: &str) -> Result<(), StoreError> {
    conn.execute(
        "INSERT INTO notes_fts(rowid, title, plain_text) VALUES (?1, ?2, ?3)",
        params![rowid, title, plain_text],
    )?;
    Ok(())
}

// ------------------------------------------------------------------ meta ---

pub(crate) fn meta_get(conn: &Connection, key: &str) -> Result<Option<String>, StoreError> {
    Ok(conn
        .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get::<_, String>(0))
        .optional()?)
}

pub(crate) fn meta_set(conn: &Connection, key: &str, value: &str) -> Result<(), StoreError> {
    conn.execute(
        "INSERT INTO meta(key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

pub(crate) fn meta_i64(conn: &Connection, key: &str, default: i64) -> Result<i64, StoreError> {
    let raw = meta_get(conn, key)?;
    Ok(raw.and_then(|s| s.parse::<i64>().ok()).unwrap_or(default))
}

// -------------------------------------------------------------- outbox ----

pub(crate) fn enabled_accounts(conn: &Connection) -> Result<Vec<String>, StoreError> {
    let mut stmt = conn.prepare("SELECT id FROM sync_accounts WHERE enabled = 1 ORDER BY id")?;
    let v = stmt.query_map([], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
    Ok(v)
}

/// 待办要写给**所有**账户（含未启用的哨兵账户）：
/// "提交即入 outbox，不等网络"（DATA-MODEL §12）+ I8；是否取件由同步侧决定。
/// 参与 outbox 扇出的账户。
///
/// 规则：已启用的账户 + **本地哨兵账户**。哨兵是 `enabled=0` 的（未配置真实远端时
/// 待办只落库不外发，作为本地写入的留痕，见 store.rs 初始化），所以不能按
/// `enabled = 1` 一刀切过滤掉它。
/// 真正该排除的是"用户后来禁用的真实账户" —— 否则引擎永不消费它，
/// 它的 outbox 行会无界堆积。
pub(crate) fn all_accounts(conn: &Connection) -> Result<Vec<String>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT id FROM sync_accounts WHERE enabled = 1 OR id = ?1 ORDER BY id",
    )?;
    let sentinel = crate::types::LOCAL_ACCOUNT_ID;
    let v = stmt
        .query_map([sentinel], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(v)
}

/// `dedupe_key` = **账户** + 实体 + 动作 + 载荷 rev。
///
/// 账户必须在键里。`sync_operations.dedupe_key` 是唯一索引，而 outbox 按账户扇出：
/// 少了 account，第二个账户的 INSERT 会撞上第一个账户已建的同一个键、被
/// `ON CONFLICT DO UPDATE` 吸收掉，那一行仍属于第一个账户 —— 结果是
/// "本地写入只同步到其中一台服务器"，且完全静默（本仓库实测踩过）。
pub(crate) fn dedupe_key(account: &str, kind: EntityKind, id: &str, op: OpKind, rev: Option<Rev>) -> String {
    let r = rev.map(|r| r.to_string()).unwrap_or_else(|| "-".to_string());
    format!("{account}:{}:{id}:{}:{r}", kind_tag(kind), op.as_str())
}

/// 入队一条待办（UUID 键实体：note / folder）。
pub(crate) fn enqueue(
    conn: &Connection,
    now: &str,
    kind: EntityKind,
    id: &EntityId,
    op: OpKind,
    payload_rev: Option<Rev>,
    sha256: Option<&str>,
) -> Result<(), StoreError> {
    enqueue_key(conn, now, kind, id.as_str(), op, payload_rev, sha256)
}

/// 按**原始键**入队（附件的键是 64hex 的 sha256，不是 UUID）。
/// 同时把同实体同动作的更早 pending/inflight 记录标 `superseded`
/// —— 远端只需要最终态，旧 rev 的载荷已无意义。
pub(crate) fn enqueue_key(
    conn: &Connection,
    now: &str,
    kind: EntityKind,
    entity_key: &str,
    op: OpKind,
    payload_rev: Option<Rev>,
    sha256: Option<&str>,
) -> Result<(), StoreError> {
    for account in all_accounts(conn)? {
        if let Some(rev) = payload_rev {
            conn.execute(
                "UPDATE sync_operations
                    SET state = 'superseded', updated_at = ?2
                  WHERE account_id = ?1 AND entity_type = ?3 AND entity_id = ?4 AND op = ?5
                    AND state IN ('pending','inflight')
                    AND (payload_rev IS NULL OR payload_rev < ?6)",
                params![account, now, kind_tag(kind), entity_key, op.as_str(), rev.get() as i64],
            )?;
        }
        let key = dedupe_key(&account, kind, entity_key, op, payload_rev);
        conn.execute(
            "INSERT INTO sync_operations
               (account_id, dedupe_key, entity_type, entity_id, op, payload_rev, sha256,
                state, attempts, created_at, updated_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,'pending',0,?8,?8)
             ON CONFLICT(dedupe_key) DO UPDATE SET
               state = 'pending', sha256 = excluded.sha256, updated_at = excluded.updated_at",
            params![
                account,
                key,
                kind_tag(kind),
                entity_key,
                op.as_str(),
                payload_rev.map(|r| r.get() as i64),
                sha256,
                now
            ],
        )?;
    }
    Ok(())
}

/// 把某实体所有未完成的上行动作标 `superseded`（永久删除后不得再上传旧内容）。
pub(crate) fn supersede_pending(conn: &Connection, now: &str, kind: EntityKind, id: &EntityId) -> Result<(), StoreError> {
    conn.execute(
        "UPDATE sync_operations
            SET state = 'superseded', updated_at = ?3
          WHERE entity_type = ?1 AND entity_id = ?2 AND state IN ('pending','inflight')",
        params![kind_tag(kind), id.as_str(), now],
    )?;
    Ok(())
}

// -------------------------------------------------------------- revisions --

// 形参就是 `note_revisions` 的列：包一层结构体不会少一个字段，只会多一处搬运。
#[allow(clippy::too_many_arguments)]
pub(crate) fn insert_revision(
    conn: &Connection,
    note_id: &EntityId,
    rev: Rev,
    doc_json: &str,
    content_hash: &str,
    origin: crate::types::RevOrigin,
    device: &str,
    now: &str,
) -> Result<(), StoreError> {
    conn.execute(
        "INSERT INTO note_revisions(note_id, rev, doc, content_hash, origin, device_id, created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7)
         ON CONFLICT(note_id, rev) DO UPDATE SET doc = excluded.doc, content_hash = excluded.content_hash",
        params![note_id.as_str(), rev.get() as i64, doc_json, content_hash, origin.as_str(), device, now],
    )?;
    Ok(())
}

pub(crate) fn revision_doc(conn: &Connection, note_id: &EntityId, rev: Rev) -> Result<Option<serde_json::Value>, StoreError> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT doc FROM note_revisions WHERE note_id = ?1 AND rev = ?2",
            params![note_id.as_str(), rev.get() as i64],
            |r| r.get::<_, String>(0),
        )
        .optional()?;
    let Some(raw) = raw else { return Ok(None) };
    let v: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| StoreError::Constraint(format!("note_revisions.doc 非法 JSON: {e}")))?;
    Ok(Some(v))
}

/// 派生列里的 `has_attachment`：doc 内的附件块 **或** `note_attachments` 链接（§7.1）。
pub(crate) fn has_linked_attachment(conn: &Connection, note_id: &EntityId) -> Result<bool, StoreError> {
    let n: i64 = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM note_attachments WHERE note_id = ?1)",
        [note_id.as_str()],
        |r| r.get(0),
    )?;
    Ok(n != 0)
}

