//! `Store` —— 权威表的**唯一写入口** + 读侧投影（DATA-MODEL §12、ARCHITECTURE-MAP §3）。
//!
//! 结构：
//! * `write: Mutex<Connection>`：专用写连接，`Mutex` 即"单写者序列化"。
//! * `readers`：读连接池（WAL 允许读者与写者并发）。
//! * 每个写方法 = 一个事务：`notes` + 派生列 + `notes_fts` + `note_revisions` +
//!   `sync_operations` 一起成功或一起回滚（I5）。共享实现是 [`Store::commit_edit`]。
//!
//! rev 只能由 `notera_core::next_rev(local, remote_rev)` 推进（I2），本文件不出现手写 `+1`。

use crate::derive::{self, Prepared};
use crate::error::StoreError;
use crate::migrate::{self, MigrateReport};
use crate::pool::{ReadGuard, Readers};
use crate::rows;
use crate::search;
use crate::types::*;
use notera_core::{
    hash_json, next_rev, Clock, ContentHash, DeviceId, EntityId, EntityKind, InvariantViolation,
    Rev, SystemClock, Timestamp,
};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const DB_FILE_NAME: &str = "notera.sqlite";
pub const ATTACHMENTS_DIR_NAME: &str = "attachments";
/// GC 的隔离区（DATA-MODEL §8）：零引用的 blob 先**挪**到这里，不删。
/// 与 `attachments/` 同级、同样的 `<2hex>/<sha>` 两层结构，所以同一份字节在两侧的路径只差根目录。
pub const QUARANTINE_DIR_NAME: &str = "attachments-quarantine";
const META_DEVICE_ID: &str = "device_id";
const META_INSTALL_ID: &str = "install_id";
const META_CACHED_ROOT: &str = "cached_root_id";
const META_SEARCH_GEN: &str = "search_generation";
const META_CREATED_AT: &str = "store_created_at";

/// `notes` 行的写前镜像：canonical `doc` 文本 + rowid（FTS external-content 的连接键）。
pub(crate) struct CurNote {
    pub note: Note,
    pub doc_json: String,
    pub rowid: i64,
}

/// `commit_edit` 的变化集。`None` 表示"该列不动"。
#[derive(Clone, Debug)]
pub(crate) struct Edit {
    pub doc: Option<Prepared>,
    pub folder_id: Option<EntityId>,
    pub pinned: Option<bool>,
    pub color: Option<Option<String>>,
    pub deleted_at: Option<Option<String>>,
    pub expected_rev: Option<Rev>,
    pub origin: RevOrigin,
    /// 是否入 outbox（本地写 = true；`apply_remote` 拉回 = false）。
    pub enqueue: bool,
    /// 直接指定新 rev（pull 生效：`rev ← remote_rev`），否则走 `next_rev`。
    pub force_rev: Option<Rev>,
    /// pull 生效时把 `sync_rev` 推到新 rev（DATA-MODEL §4.2）。
    pub confirm_sync: bool,
    /// 同步观测到的远端头部（Lamport 的"观测"半边，参与下一次 `next_rev`）。
    pub remote_rev_to: Option<Rev>,
}

impl Default for Edit {
    fn default() -> Self {
        Edit {
            doc: None,
            folder_id: None,
            pinned: None,
            color: None,
            deleted_at: None,
            expected_rev: None,
            origin: RevOrigin::Local,
            enqueue: true,
            force_rev: None,
            confirm_sync: false,
            remote_rev_to: None,
        }
    }
}

pub struct Store {
    pub(crate) paths: StorePaths,
    pub(crate) device: DeviceId,
    pub(crate) write: Mutex<Connection>,
    readers: Readers,
    clock: SystemClock,
    migration: MigrateReport,
    startup_violations: Vec<InvariantViolation>,
}

/// `Store` 内部持连接池，无法 derive(Debug)。但测试与日志需要一个
/// 可打印的句柄身份，因此手写摘要实现（不触碰连接，绝不 panic）。
impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store")
            .field("db", &self.paths().db)
            .field("attachments", &self.paths().attachments)
            .field("device", &self.device_id())
            .finish_non_exhaustive()
    }
}

impl Store {
    /// 建目录 → PRAGMA → 迁移 → 引导（默认本 / 哨兵账户 / meta）→ 启动自检。
    ///
    /// `user_version` 高于本程序支持值时返回 [`StoreError::ReadOnly`]（ADR-0012）：
    /// 不迁移、不写回、不改文件。
    pub fn open(dir: &Path, device_id: DeviceId) -> Result<Store, StoreError> {
        std::fs::create_dir_all(dir)?;
        // 待恢复标记必须在打开库之前落地，这样后面的启动自检检查的是恢复后的数据。
        crate::backup::apply_pending_restore(dir)?;
        let attachments = dir.join(ATTACHMENTS_DIR_NAME);
        std::fs::create_dir_all(&attachments)?;
        let db = dir.join(DB_FILE_NAME);

        let mut conn = crate::pool::open_write_conn(&db)?;
        let report = migrate::migrate(&mut conn, &db)?;
        let device = bootstrap(&mut conn, &device_id)?;

        let mut store = Store {
            paths: StorePaths {
                db: db.clone(),
                attachments,
            },
            device,
            write: Mutex::new(conn),
            readers: Readers::new(db),
            clock: SystemClock,
            migration: report,
            startup_violations: Vec::new(),
        };

        // 派生索引自愈（DATA-MODEL §7.3）。
        //
        // 注意：external-content FTS5 的 `count(*)` 报的是**内容表**的行数，索引即使是空的
        // 也照样相等（实测）—— 所以"行数比对"根本发现不了脱钩（例如 0003 刚建好索引、
        // 库里已有老笔记）。唯一的可靠探针是抽样比较 MATCH 与 LIKE 的命中集合。
        if !store.verify_search().is_empty() {
            store.rebuild_search()?;
        }
        let violations = store.verify();
        store.startup_violations = violations;
        Ok(store)
    }

    pub fn device_id(&self) -> DeviceId {
        self.device.clone()
    }

    pub fn paths(&self) -> &StorePaths {
        &self.paths
    }

    pub fn db_path(&self) -> &Path {
        &self.paths.db
    }

    pub fn attachments_dir(&self) -> &Path {
        &self.paths.attachments
    }

    /// 本次 open 实际应用的迁移。`applied` 为空 = 幂等重开，未动 schema。
    pub fn migration_report(&self) -> &MigrateReport {
        &self.migration
    }

    /// `open()` 时启动自检的发现（不 panic，交给 host 决定是否只读/提示）。
    pub fn startup_violations(&self) -> &[InvariantViolation] {
        &self.startup_violations
    }

    // ------------------------------------------------------------ 事务壳 ---

    /// 单写者 + 单事务。闭包返回 `Err` 即整体回滚（WAL + synchronous=FULL）。
    pub(crate) fn write_tx<T>(
        &self,
        f: impl FnOnce(&Connection, &str) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let mut guard = self.write.lock().unwrap_or_else(|p| p.into_inner());
        let now = self.now();
        let tx = guard.transaction()?;
        let out = f(&tx, &now)?;
        tx.commit()?;
        // 「写本地」这一刀刻意落在 commit **之后**：这才是"事务已经在了、
        // 但进程还没来得及往下走"的那个真实窗口（唯一写入口，所以每条写路径都覆盖）。
        notera_core::crash_point("after_local_write");
        Ok(out)
    }

    pub(crate) fn read(&self) -> Result<ReadGuard, StoreError> {
        self.readers.get()
    }

    /// 全库统一的时间源（墙上时间只用于展示与诊断，永不参与新旧判定）。
    /// 公开它是为了让导出文件名/manifest 与库内时间戳出自同一个时钟，不再各引一份 chrono。
    pub fn now(&self) -> String {
        self.clock.now().as_str().to_string()
    }

    // -------------------------------------------------------- 笔记（写） ---

    /// 新建笔记：`doc` 先过 richtext 校验（I6），再在单事务里写
    /// `notes` + 派生列 + `notes_fts` + `note_revisions` + outbox（I5）。
    pub fn create_note(
        &self,
        folder_id: &EntityId,
        doc: serde_json::Value,
    ) -> Result<Note, StoreError> {
        let prepared = derive::prepare(&doc)?;
        let id = EntityId::new();
        let device = self.device.to_string();
        self.write_tx(|tx, now| {
            let folder = rows::read_folder(tx, folder_id)?
                .ok_or_else(|| StoreError::not_found(EntityKind::Folder, folder_id.clone()))?;
            if folder.deleted_at.is_some() {
                return Err(StoreError::Constraint(format!("文件夹 {} 已在回收站", folder.id)));
            }
            let rev = next_rev(Rev::ZERO, Rev::ZERO);
            let has_attach = prepared.doc_has_attachment;
            tx.execute(
                "INSERT INTO notes
                   (id, folder_id, doc, doc_format, pinned, color, title, plain_text, summary,
                    char_count, block_count, has_attachment, rev, sync_rev, sync_hash, remote_rev,
                    content_hash, created_at, updated_at, deleted_at, purged_at, created_device, updated_device)
                 VALUES (?1,?2,?3,?4,0,NULL,?5,?6,?7,?8,?9,?10,?11,0,NULL,0,?12,?13,?13,NULL,NULL,?14,?14)",
                params![
                    id.as_str(),
                    folder_id.as_str(),
                    prepared.doc_json,
                    prepared.doc_version as i64,
                    prepared.title,
                    prepared.plain_text,
                    prepared.summary,
                    prepared.char_count as i64,
                    prepared.block_count as i64,
                    has_attach as i64,
                    rev.get() as i64,
                    prepared.content_hash.as_str(),
                    now,
                    device
                ],
            )?;
            let rowid = rows::note_rowid(tx, &id)?
                .ok_or_else(|| StoreError::not_found(EntityKind::Note, id.clone()))?;
            rows::fts_insert(tx, rowid, &prepared.title, &prepared.plain_text)?;
            rows::insert_revision(
                tx,
                &id,
                rev,
                &prepared.doc_json,
                prepared.content_hash.as_str(),
                RevOrigin::Local,
                &device,
                now,
            )?;
            rows::enqueue(
                tx,
                now,
                EntityKind::Note,
                &id,
                OpKind::Upsert,
                Some(rev),
                Some(prepared.content_hash.as_str()),
            )?;
            self.register_doc_attachments(tx, &id, &prepared.attachments, now)?;
            rows::read_note(tx, &id)?.ok_or_else(|| StoreError::not_found(EntityKind::Note, id.clone()))
        })
    }

    /// 编辑笔记。`expected_rev` 与当前 `rev` 不符 → [`StoreError::StaleEdit`]，且不写任何数据。
    pub fn edit_note(
        &self,
        id: &EntityId,
        doc: serde_json::Value,
        expected_rev: Rev,
    ) -> Result<Note, StoreError> {
        let prepared = derive::prepare(&doc)?;
        self.write_tx(|tx, now| {
            let cur = Self::load_cur(tx, id)?;
            Self::assert_editable(&cur.note)?;
            if prepared.content_hash.as_str() == cur.note.content_hash {
                // 内容未变 → 不推进 rev（I2 只约束"产生内容变化的提交"）。
                return Ok(cur.note);
            }
            let edit = Edit {
                doc: Some(prepared),
                expected_rev: Some(expected_rev),
                ..Default::default()
            };
            self.commit_edit(tx, &cur, &edit, now)
        })
    }

    pub fn set_note_folder(&self, id: &EntityId, folder: &EntityId) -> Result<Note, StoreError> {
        let target = folder.clone();
        self.write_tx(|tx, now| {
            let cur = Self::load_cur(tx, id)?;
            Self::assert_editable(&cur.note)?;
            let f = rows::read_folder(tx, &target)?
                .ok_or_else(|| StoreError::not_found(EntityKind::Folder, target.clone()))?;
            if f.deleted_at.is_some() {
                return Err(StoreError::Constraint(format!(
                    "文件夹 {} 已在回收站",
                    f.id
                )));
            }
            if cur.note.folder_id == target {
                return Ok(cur.note);
            }
            let edit = Edit {
                folder_id: Some(target),
                ..Default::default()
            };
            self.commit_edit(tx, &cur, &edit, now)
        })
    }

    pub fn set_note_pinned(&self, id: &EntityId, pinned: bool) -> Result<Note, StoreError> {
        self.write_tx(|tx, now| {
            let cur = Self::load_cur(tx, id)?;
            Self::assert_editable(&cur.note)?;
            if cur.note.pinned == pinned {
                return Ok(cur.note);
            }
            let edit = Edit {
                pinned: Some(pinned),
                ..Default::default()
            };
            self.commit_edit(tx, &cur, &edit, now)
        })
    }

    /// 软删 = "最近删除"。删除是**记录内容**（ADR-0006），传播形态是带 `deleted_at` 的 upsert。
    pub fn delete_note(&self, id: &EntityId) -> Result<(), StoreError> {
        self.write_tx(|tx, now| {
            let cur = Self::load_cur(tx, id)?;
            if cur.note.deleted_at.is_some() {
                return Ok(()); // 幂等
            }
            let edit = Edit {
                deleted_at: Some(Some(now.to_string())),
                ..Default::default()
            };
            self.commit_edit(tx, &cur, &edit, now)?;
            Ok(())
        })
    }

    pub fn restore_note(&self, id: &EntityId) -> Result<(), StoreError> {
        self.write_tx(|tx, now| {
            let cur = Self::load_cur(tx, id)?;
            if cur.note.deleted_at.is_none() {
                return Ok(()); // 幂等
            }
            let edit = Edit {
                deleted_at: Some(None),
                origin: RevOrigin::Restored,
                ..Default::default()
            };
            self.commit_edit(tx, &cur, &edit, now)?;
            Ok(())
        })
    }

    /// 永久删除：写 `tombstones(purged=1)` + 删 `notes` 行 + 入 `purge` 待办。
    /// 墓碑**不自动 GC**（I3/ADR-0006），所以删除事实比数据活得更久、可跨设备传播。
    pub fn purge_note(&self, id: &EntityId) -> Result<(), StoreError> {
        self.write_tx(|tx, now| {
            let cur = match Self::load_cur_opt(tx, id)? {
                Some(c) => c,
                None => {
                    if Self::tombstone_of(tx, EntityKind::Note, id)?.is_some() {
                        return Ok(()); // 已 purge：幂等
                    }
                    return Err(StoreError::not_found(EntityKind::Note, id.clone()));
                }
            };
            let rev = next_rev(cur.note.rev, cur.note.remote_rev);
            // 未完成的上行动作全部作废：绝不让旧内容再被当成"新增"上传。
            rows::supersede_pending(tx, now, EntityKind::Note, id)?;
            self.write_tombstone(
                tx,
                EntityKind::Note,
                id,
                rev,
                true,
                Some(cur.note.content_hash.clone()),
                Some(cur.note.title.clone()),
                now,
            )?;
            rows::enqueue(
                tx,
                now,
                EntityKind::Note,
                id,
                OpKind::Purge,
                Some(rev),
                Some(&cur.note.content_hash),
            )?;
            // CASCADE 带走 note_revisions / note_attachments 链接；blob 由后台按引用计数回收。
            tx.execute("DELETE FROM notes WHERE id = ?1", [id.as_str()])?;
            Ok(())
        })
    }

    // ------------------------------------------------------ 文件夹（写） ---

    pub fn create_folder(
        &self,
        parent: Option<&EntityId>,
        name: &str,
    ) -> Result<Folder, StoreError> {
        let name = validate_name(name)?;
        let parent = parent.cloned();
        let id = EntityId::new();
        let device = self.device.to_string();
        self.write_tx(|tx, now| {
            if let Some(p) = &parent {
                let f = rows::read_folder(tx, p)?
                    .ok_or_else(|| StoreError::not_found(EntityKind::Folder, p.clone()))?;
                if f.deleted_at.is_some() {
                    return Err(StoreError::Constraint(format!("父文件夹 {} 已在回收站", f.id)));
                }
            }
            let rev = next_rev(Rev::ZERO, Rev::ZERO);
            let hash = folder_hash(&name, &parent, &None, 0, &None);
            tx.execute(
                "INSERT INTO folders
                   (id, parent_id, name, color, system_kind, sort_order, rev, sync_rev, sync_hash,
                    remote_rev, content_hash, created_at, updated_at, deleted_at, purged_at, created_device, updated_device)
                 VALUES (?1,?2,?3,NULL,NULL,0,?4,0,NULL,0,?5,?6,?6,NULL,NULL,?7,?7)",
                params![id.as_str(), parent.as_ref().map(|p| p.as_str().to_string()), name, rev.get() as i64, hash, now, device],
            )?;
            rows::enqueue(tx, now, EntityKind::Folder, &id, OpKind::Upsert, Some(rev), Some(&hash))?;
            rows::read_folder(tx, &id)?.ok_or_else(|| StoreError::not_found(EntityKind::Folder, id.clone()))
        })
    }

    pub fn rename_folder(&self, id: &EntityId, name: &str) -> Result<Folder, StoreError> {
        let name = validate_name(name)?;
        self.write_tx(|tx, now| {
            let cur = rows::read_folder(tx, id)?
                .ok_or_else(|| StoreError::not_found(EntityKind::Folder, id.clone()))?;
            Self::assert_folder_writable(&cur)?;
            if cur.name == name {
                return Ok(cur);
            }
            self.commit_folder_edit(tx, &cur, Some(name), None, now)
        })
    }

    /// 移动文件夹。**必须检测成环**：新父不得是自己，也不得是自己的后代。
    pub fn move_folder(
        &self,
        id: &EntityId,
        new_parent: Option<&EntityId>,
    ) -> Result<Folder, StoreError> {
        let new_parent = new_parent.cloned();
        self.write_tx(|tx, now| {
            let cur = rows::read_folder(tx, id)?
                .ok_or_else(|| StoreError::not_found(EntityKind::Folder, id.clone()))?;
            Self::assert_folder_writable(&cur)?;
            if cur.parent_id == new_parent {
                return Ok(cur);
            }
            if let Some(p) = &new_parent {
                if p == id {
                    return Err(StoreError::Constraint(
                        "拒绝成环：文件夹不能成为自己的子文件夹".into(),
                    ));
                }
                let target = rows::read_folder(tx, p)?
                    .ok_or_else(|| StoreError::not_found(EntityKind::Folder, p.clone()))?;
                if target.deleted_at.is_some() {
                    return Err(StoreError::Constraint(format!(
                        "目标文件夹 {} 已在回收站",
                        target.id
                    )));
                }
                if Self::descendant_ids(tx, id)?.contains(p) {
                    return Err(StoreError::Constraint(format!(
                        "拒绝成环：{} 是 {} 的后代",
                        p, id
                    )));
                }
            }
            self.commit_folder_edit(tx, &cur, None, Some(new_parent), now)
        })
    }

    /// 删除文件夹：**不级联删笔记**（ADR-0006）。
    /// 子文件夹上移一级、笔记移入默认本，只为文件夹本身写一条墓碑；全程单事务。
    pub fn delete_folder(&self, id: &EntityId) -> Result<(), StoreError> {
        self.write_tx(|tx, now| {
            let cur = rows::read_folder(tx, id)?
                .ok_or_else(|| StoreError::not_found(EntityKind::Folder, id.clone()))?;
            Self::assert_folder_writable(&cur)?;
            if cur.deleted_at.is_some() {
                return Ok(()); // 幂等
            }
            let default_folder = Self::default_folder_id(tx)?;
            if cur.id == default_folder {
                return Err(StoreError::Constraint("默认本是 system_kind='default'，不可删除".into()));
            }
            // 1) 笔记移入默认本：folder_id 是同步内容 → 必须推进 rev 并入队（I2/I5）
            for nid in Self::note_ids_in(tx, id)? {
                let note = Self::load_cur(tx, &nid)?;
                let edit = Edit { folder_id: Some(default_folder.clone()), ..Default::default() };
                self.commit_edit(tx, &note, &edit, now)?;
            }
            // 2) 子文件夹上移一级
            for cid in Self::child_folder_ids(tx, id)? {
                let child = rows::read_folder(tx, &cid)?
                    .ok_or_else(|| StoreError::not_found(EntityKind::Folder, cid.clone()))?;
                self.commit_folder_edit(tx, &child, None, Some(cur.parent_id.clone()), now)?;
            }
            // 3) 文件夹自身：软删 + 仅一条墓碑
            let rev = next_rev(cur.rev, cur.remote_rev);
            let hash = folder_hash(&cur.name, &cur.parent_id, &cur.color, cur.sort_order, &cur.system_kind);
            tx.execute(
                "UPDATE folders SET deleted_at = ?2, rev = ?3, content_hash = ?4, updated_at = ?2, updated_device = ?5
                  WHERE id = ?1",
                params![id.as_str(), now, rev.get() as i64, hash, self.device.to_string()],
            )?;
            self.write_tombstone(tx, EntityKind::Folder, id, rev, false, Some(hash.clone()), Some(cur.name.clone()), now)?;
            rows::enqueue(tx, now, EntityKind::Folder, id, OpKind::Upsert, Some(rev), Some(&hash))?;
            Ok(())
        })
    }

    // -------------------------------------------------------- 附件（写） ---

    /// sha256 内容寻址落盘（先 `.part` 再 rename），并在同一事务里更新
    /// `attachments` / `note_attachments` / 派生列 / outbox（`upload`）。
    pub fn attach_blob(
        &self,
        note_id: &EntityId,
        bytes: &[u8],
        media_type: &str,
        filename: Option<&str>,
        block_id: &str,
    ) -> Result<Attachment, StoreError> {
        if bytes.is_empty() {
            return Err(StoreError::Constraint("空 blob 不允许挂载".into()));
        }
        if block_id.trim().is_empty() {
            return Err(StoreError::Constraint(
                "block_id 不能为空（附件必须指向 doc 中的块）".into(),
            ));
        }
        if media_type.trim().is_empty() {
            return Err(StoreError::Constraint("media_type 不能为空".into()));
        }
        let sha = notera_crypto::sha256_hex(bytes);
        ContentHash::from_hex(&sha).map_err(|e| StoreError::Constraint(e.to_string()))?;
        let role = if media_type.starts_with("image/") {
            "inline"
        } else {
            "file"
        };
        let target = blob_path(&self.paths.attachments, &sha);
        if !target.exists() {
            write_atomic(&target, bytes)?;
        }
        let note_id = note_id.clone();
        self.write_tx(|tx, now| {
            let cur = Self::load_cur(tx, &note_id)?;
            Self::assert_editable(&cur.note)?;
            // `deleted_at=NULL`：GC 认领过的行如果又被挂回正文，它就不该继续排在销毁清单里。
            // 这一列今天是"已隔离"的唯一凭据（宽限期按它算），留着旧值等于"笔记在用这张图，
            // 而账上说它已经作废"。
            tx.execute(
                "INSERT INTO attachments (sha256, size, media_type, filename, local_state, remote_state, created_at)
                 VALUES (?1,?2,?3,?4,'available','unknown',?5)
                 ON CONFLICT(sha256) DO UPDATE SET local_state='available', size=excluded.size, verified_at=excluded.created_at, deleted_at=NULL",
                params![sha, bytes.len() as i64, media_type, filename, now],
            )?;
            let position: i64 = tx.query_row(
                "SELECT COUNT(*) FROM note_attachments WHERE note_id = ?1",
                [note_id.as_str()],
                |r| r.get(0),
            )?;
            tx.execute(
                "INSERT INTO note_attachments (note_id, sha256, block_id, role, position)
                 VALUES (?1,?2,?3,?4,?5)
                 ON CONFLICT(note_id, block_id) DO UPDATE SET sha256=excluded.sha256, role=excluded.role",
                params![note_id.as_str(), sha, block_id, role, position],
            )?;
            // 附件的 outbox 键是 sha256（内容寻址），不是笔记 UUID
            rows::enqueue_key(tx, now, EntityKind::Attachment, &sha, OpKind::Upload, None, Some(&sha))?;
            let cur = Self::load_cur(tx, &note_id)?;
            if cur.note.has_attachment {
                // 派生值没变 → 不抖动 rev（I2 只管内容变化）。
                return Ok(());
            }
            self.commit_edit(tx, &cur, &Edit::default(), now)?;
            Ok(())
        })?;
        // 重新挂载同一份字节 = 这一行又活了。隔离区里那份（如果有）此刻是纯多余：
        // 正式位置上刚落下的是**调用方手上那份字节**，而 sha 就是它的内容寻址名。
        self.release_quarantine_copy(&sha);
        self.with_read(|c| attachment_for(c, &note_id, &sha))
    }

    /// 清掉隔离区里那份已经多余的同名拷贝（尽力而为，失败不吵）。
    ///
    /// 为什么删得掉不心疼，凭据是**目录的准入**而不是"内容寻址所以内容一样"那句：这个目录里的文件
    /// 只可能由 GC 的 `quarantine_move` 写入，而它的候选集要求 `remote_state='present'` —— 服务器上有
    /// 一份。所以这里删掉的**永远不会是唯一的一份**（本机坏字节走的是 `<blobs>/<sha>.corrupt`，
    /// 不在这个目录里，不会被这一句碰到）。留着它的代价是同一份字节占两处，而下一轮 GC 再认领这一行
    /// 时会被同名覆盖顺带抹平。这里不是"删用户数据失败也吞"。
    fn release_quarantine_copy(&self, sha256: &str) {
        // 大多数字节从没被隔离过，"没有这个文件"是常态而不是失败。
        let _ = std::fs::remove_file(self.quarantine_path(sha256));
    }

    /// 收下远端发来的 blob：**先校验 sha256 再落盘**，内容不符就拒收。
    /// 内容寻址意味着"错了还写进去"会污染所有引用同一 sha 的笔记。
    pub fn ingest_blob(&self, sha256: &str, bytes: &[u8]) -> Result<(), StoreError> {
        let sha = sha256.to_string();
        if bytes.is_empty() {
            return Err(StoreError::Constraint("空 blob 不允许收下".into()));
        }
        let got = notera_crypto::sha256_hex(bytes);
        if got != sha {
            self.set_attachment_states(&sha, Some("error"), None).ok();
            return Err(StoreError::Constraint(format!(
                "blob 哈希不符：期望 {sha} 实际 {got}"
            )));
        }
        let target = crate::store::blob_path(&self.paths.attachments, &sha);
        if !target.exists() {
            crate::store::write_atomic(&target, bytes)?;
        }
        // 整份字节已经复算过哈希并落在正式位置 → 隔离区里同一 sha 的那一份没有主人了。
        self.release_quarantine_copy(&sha);
        self.set_attachment_states(&sha, Some("available"), Some("present"))
    }

    /// 从**盘上的备份包**收下一个附件：先校验 sha256，再落盘，再登记行。
    ///
    /// 与 [`Store::ingest_blob`]（同步下载那条路）的两点差别都是要命的：
    /// * 包里的字节不是从服务器拿的，所以远端态只能是 `unknown`。写成 `present` 等于对
    ///   队列宣布"服务器已经有了"，于是这台设备永远不会把它补传上去 —— 还原出来的附件
    ///   在服务器上不存在，第三台设备也就永远拿不到它。
    /// * 目标库里可能**根本没有这一行**：`ingest_blob` 走的是 UPDATE，影响 0 行就报
    ///   `附件不存在`，整包还原失败（实测踩过：带附件的备份包一个都导不进去）。
    pub fn restore_blob(&self, sha256: &str, bytes: &[u8]) -> Result<(), StoreError> {
        if bytes.is_empty() {
            return Err(StoreError::Constraint("空 blob 不允许收下".into()));
        }
        let got = notera_crypto::sha256_hex(bytes);
        if got != sha256 {
            return Err(StoreError::Constraint(format!(
                "blob 哈希不符：期望 {sha256} 实际 {got}"
            )));
        }
        let target = blob_path(&self.paths.attachments, sha256);
        if !target.exists() {
            write_atomic(&target, bytes)?;
        }
        // 备份包里这份已经按 sha256 校验并落好了正式位置 → 隔离区那份是同一内容的第二副本。
        self.release_quarantine_copy(sha256);
        let sha = sha256.to_string();
        let size = bytes.len() as i64;
        self.write_tx(move |tx, now| {
            Self::upsert_attachment_row(tx, now, &sha, size, None, None, "available")
        })
    }

    /// 登记 / 更新一行 `attachments` 元数据，**由调用方的事务驱动**（还原与"从记录派生引用"共用）。
    ///
    /// 为什么不能"文件写盘了就算数"：`attachments` 行是附件在库里的唯一户口 —— 引用计数、
    /// 上传/下载队列、按文件夹导出的附件集合全都只看这张表。行不在，blob 就是一片孤儿文件。
    ///
    /// `media_type`/`filename` 传 `None` 表示"不知道"：保留已有值，绝不拿空值覆盖已知的。
    /// `remote_state` 只在**新建**行时写 `'unknown'`，冲突时一字不动。
    pub(crate) fn upsert_attachment_row(
        tx: &Connection,
        now: &str,
        sha256: &str,
        size: i64,
        media_type: Option<&str>,
        filename: Option<&str>,
        local_state: &str,
    ) -> Result<(), StoreError> {
        // size 取两者较大：块属性/清单报过的数不许被"不知道"（0）冲掉，否则上传预算会把
        // 一个大附件当成小附件排进本轮。
        tx.execute(
            "INSERT INTO attachments (sha256, size, media_type, filename, local_state, remote_state, created_at, verified_at)
             VALUES (?1,?2,COALESCE(?3,'application/octet-stream'),?4,?5,'unknown',?6,NULLIF(?7,''))
             ON CONFLICT(sha256) DO UPDATE SET
               size = MAX(attachments.size, excluded.size),
               media_type = COALESCE(?3, attachments.media_type),
               filename = COALESCE(attachments.filename, ?4),
               local_state = ?5,
               verified_at = COALESCE(attachments.verified_at, NULLIF(?7,'')),
               deleted_at = NULL",
            params![sha256, size, media_type, filename, local_state, now, if local_state == "available" { now } else { "" }],
        )?;
        Ok(())
    }

    /// 把一条**外来的**笔记（同步拉回 / 备份包导入）doc 里的附件引用登记入库。
    ///
    /// 这一步以前是缺席的：`register_remote_attachment` 在生产代码里零调用，于是第二台设备
    /// 收到带图片的笔记之后 —— 没有 `attachments` 行 → 没有下载任务 → 图片永远停在占位；
    /// 引用计数恒为 0 → GC 可以把还在用的 blob 删掉；按文件夹导出的附件集合也是空的。
    ///
    /// 和笔记写在**同一事务**里，口径与 I5 一致：doc 变了，由它派生的东西一起变。
    pub(crate) fn register_doc_attachments(
        &self,
        tx: &Connection,
        note_id: &EntityId,
        refs: &[notera_richtext::BlockAttachment],
        now: &str,
    ) -> Result<(), StoreError> {
        for (idx, r) in refs.iter().enumerate() {
            let media = r.media_type.as_deref().filter(|m| !m.trim().is_empty());
            let local = if self.blob_path(&r.sha256).exists() {
                "available"
            } else {
                "missing"
            };
            Self::upsert_attachment_row(
                tx,
                now,
                &r.sha256,
                r.size.unwrap_or(0),
                media,
                r.filename.as_deref(),
                local,
            )?;
            let role = match r.role.as_deref() {
                Some(x @ ("inline" | "file")) => x,
                // 块上没写角色就按媒体类型推（与 `attach_blob` 同一条判据）
                _ => {
                    if media.is_some_and(|m| m.starts_with("image/")) {
                        "inline"
                    } else {
                        "file"
                    }
                }
            };
            tx.execute(
                "INSERT INTO note_attachments (note_id, sha256, block_id, role, position)
                 VALUES (?1,?2,?3,?4,?5)
                 ON CONFLICT(note_id, block_id) DO UPDATE SET sha256=excluded.sha256, role=excluded.role",
                params![note_id.as_str(), r.sha256, r.block_id, role, idx as i64],
            )?;
        }
        Ok(())
    }

    pub(crate) fn with_read<T>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let conn = self.read()?;
        f(&conn)
    }

    /// blob 的落盘路径（内容寻址，无 path 列）。
    /// 读回附件的两个状态位。**库里没有这一行**时回 `("absent","absent")` —— 这是唯一的
    /// "没登记"哨兵（`local_state` 的四个取值里没有 `absent`，见 0001 迁移的 CHECK），
    /// 调用方可以直接按那一对判"这行不存在"。
    /// 生产调用者是 `App::retry_attachment` / `App::reupload_attachment`（先看清那对状态
    /// 再决定撤哪一半）与测试诊断；**队列本身不看它**，挑活走 `attachment_uploads` /
    /// `attachment_downloads` 的 SQL 口径。
    pub fn attachment_for_state(&self, sha256: &str) -> (String, String) {
        let sha = sha256.to_string();
        self.with_read(|c| {
            c.query_row(
                "SELECT local_state, remote_state FROM attachments WHERE sha256 = ?1",
                [sha.as_str()],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(StoreError::from)
        })
        .ok()
        .flatten()
        .unwrap_or_else(|| ("absent".into(), "absent".into()))
    }

    pub fn blob_path(&self, sha256: &str) -> PathBuf {
        blob_path(&self.paths.attachments, sha256)
    }

    /// 下载中途落在磁盘上的那半截。**只有它存在，跨轮次的续传才有地方放**。
    /// 与正式 blob 同目录、加 `.part` 后缀：清理时一眼认得出，也不会被当成附件本体。
    pub fn blob_part_path(&self, sha256: &str) -> PathBuf {
        let mut p = self.blob_path(sha256);
        p.as_mut_os_string().push(".part");
        p
    }

    /// GC 隔离区的根目录（与 `attachments/` 同级）。
    ///
    /// **不在 `open()` 里预建**：只有真要回收东西时才建，冷启动一个 syscall 都不多
    /// （PERF-01 那条 1000 ms 预算含 WebView2，不该为一段几乎不跑的逻辑付钱）。
    pub fn quarantine_dir(&self) -> PathBuf {
        let parent = self.paths.attachments.parent().unwrap_or(Path::new("."));
        parent.join(QUARANTINE_DIR_NAME)
    }

    /// 某份字节在隔离区里的位置：与正式位置**只差根目录**，因为两侧共用
    /// `<2hex>/<sha>` 那套结构 —— 于是"挪过去 / 挪回来"都是同一条 `blob_path` 换个起点，
    /// 不必再造第二套寻址规则（那是 §39 禁止的重复状态机）。
    pub fn quarantine_path(&self, sha256: &str) -> PathBuf {
        blob_path(&self.quarantine_dir(), sha256)
    }

    // ---------------------------------------------------------- 内部：编辑 ---

    pub(crate) fn load_cur(tx: &Connection, id: &EntityId) -> Result<CurNote, StoreError> {
        Self::load_cur_opt(tx, id)?
            .ok_or_else(|| StoreError::not_found(EntityKind::Note, id.clone()))
    }

    pub(crate) fn load_cur_opt(
        tx: &Connection,
        id: &EntityId,
    ) -> Result<Option<CurNote>, StoreError> {
        let sql = format!("SELECT {}, rowid FROM notes WHERE id = ?1", rows::NOTE_COLS);
        let row = tx.query_row(&sql, [id.as_str()], |r| {
            let doc_json: String = r.get(2)?;
            let note = rows::note_from_row(r)?;
            let rowid: i64 = r.get(21)?;
            Ok(CurNote {
                note,
                doc_json,
                rowid,
            })
        });
        match row {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(StoreError::Sql(e)),
        }
    }

    fn assert_editable(note: &Note) -> Result<(), StoreError> {
        if note.deleted_at.is_some() {
            // "打开中的笔记不得带 deleted_at"（DATA-MODEL §5.1 notes 表注释）
            return Err(StoreError::Constraint(format!(
                "笔记 {} 在回收站中，请先恢复再编辑",
                note.id
            )));
        }
        Ok(())
    }

    fn assert_folder_writable(f: &Folder) -> Result<(), StoreError> {
        if f.system_kind.is_some() {
            return Err(StoreError::Constraint(format!(
                "内置文件夹 {} 不可改名/移动/删除",
                f.id
            )));
        }
        if f.deleted_at.is_some() {
            return Err(StoreError::Constraint(format!(
                "文件夹 {} 已在回收站",
                f.id
            )));
        }
        Ok(())
    }

    /// 唯一的笔记写入实现：`notes`（含派生列）+ `notes_fts` + `note_revisions` + outbox。
    /// 调用方必须已在写事务内（I5）。
    pub(crate) fn commit_edit(
        &self,
        tx: &Connection,
        cur: &CurNote,
        edit: &Edit,
        now: &str,
    ) -> Result<Note, StoreError> {
        if let Some(exp) = edit.expected_rev {
            if exp != cur.note.rev {
                return Err(StoreError::stale_edit(
                    cur.note.id.clone(),
                    exp,
                    cur.note.rev,
                    &self.device,
                ));
            }
        }
        let new_rev = match edit.force_rev {
            Some(r) => {
                if r.get() < cur.note.rev.get() {
                    return Err(StoreError::Rejected(format!(
                        "拒绝回退 rev：远端 {} < 本地 {}",
                        r, cur.note.rev
                    )));
                }
                r
            }
            None => {
                let r = next_rev(cur.note.rev, cur.note.remote_rev);
                notera_core::assert_rev_advances(cur.note.rev, r)
                    .map_err(|v| StoreError::Constraint(v.detail))?;
                r
            }
        };

        let folder_id = edit
            .folder_id
            .clone()
            .unwrap_or_else(|| cur.note.folder_id.clone());
        let (
            doc_json,
            doc_format,
            title,
            plain_text,
            summary,
            char_count,
            block_count,
            content_hash,
            doc_has,
        ) = match &edit.doc {
            Some(p) => (
                p.doc_json.clone(),
                p.doc_version as i64,
                p.title.clone(),
                p.plain_text.clone(),
                p.summary.clone(),
                p.char_count as i64,
                p.block_count as i64,
                p.content_hash.as_str().to_string(),
                p.doc_has_attachment,
            ),
            None => (
                cur.doc_json.clone(),
                cur.note.doc_format as i64,
                cur.note.title.clone(),
                cur.note.plain_text.clone(),
                cur.note.summary.clone(),
                cur.note.char_count as i64,
                cur.note.block_count as i64,
                cur.note.content_hash.clone(),
                cur.note.has_attachment,
            ),
        };
        let pinned = edit.pinned.unwrap_or(cur.note.pinned);
        let color = edit.color.clone().unwrap_or_else(|| cur.note.color.clone());
        let deleted_at = edit
            .deleted_at
            .clone()
            .unwrap_or_else(|| cur.note.deleted_at.clone());
        // 引用计数由 doc 派生：这一步缺席时链接表低报，GC 会把还在渲染的字节当无主物收走。
        // 只登记、不 prune —— 多算引用只是少收一点磁盘，少算引用是丢数据，方向不能反。
        if let Some(p) = &edit.doc {
            self.register_doc_attachments(tx, &cur.note.id, &p.attachments, now)?;
        }
        let has_attachment = doc_has || rows::has_linked_attachment(tx, &cur.note.id)?;
        let (sync_rev, sync_hash) = if edit.confirm_sync {
            (new_rev, Some(content_hash.clone()))
        } else {
            (cur.note.sync_rev, cur.note.sync_hash.clone())
        };
        let remote_rev = edit.remote_rev_to.unwrap_or(cur.note.remote_rev);

        // FTS：external content 无触发器 → 先按**旧值**删，再按新值插（同事务）。
        let text_changed = title != cur.note.title || plain_text != cur.note.plain_text;
        if text_changed {
            rows::fts_delete(tx, cur.rowid, &cur.note.title, &cur.note.plain_text)?;
        }
        tx.execute(
            "UPDATE notes SET
                folder_id = ?2, doc = ?3, doc_format = ?4, pinned = ?5, color = ?6,
                title = ?7, plain_text = ?8, summary = ?9, char_count = ?10, block_count = ?11,
                has_attachment = ?12, rev = ?13, sync_rev = ?14, sync_hash = ?15,
                content_hash = ?16, updated_at = ?17, updated_device = ?18, deleted_at = ?19,
                remote_rev = ?20
             WHERE id = ?1",
            params![
                cur.note.id.as_str(),
                folder_id.as_str(),
                doc_json,
                doc_format,
                pinned as i64,
                color,
                title,
                plain_text,
                summary,
                char_count,
                block_count,
                has_attachment as i64,
                new_rev.get() as i64,
                sync_rev.get() as i64,
                sync_hash,
                content_hash,
                now,
                self.device.to_string(),
                deleted_at,
                remote_rev.get() as i64
            ],
        )?;
        if text_changed {
            rows::fts_insert(tx, cur.rowid, &title, &plain_text)?;
        }
        rows::insert_revision(
            tx,
            &cur.note.id,
            new_rev,
            &doc_json,
            &content_hash,
            edit.origin,
            &self.device.to_string(),
            now,
        )?;
        if edit.enqueue {
            rows::enqueue(
                tx,
                now,
                EntityKind::Note,
                &cur.note.id,
                OpKind::Upsert,
                Some(new_rev),
                Some(&content_hash),
            )?;
        }
        rows::read_note(tx, &cur.note.id)?
            .ok_or_else(|| StoreError::not_found(EntityKind::Note, cur.note.id.clone()))
    }

    /// 文件夹的写入出口：内容变化 → `next_rev` + outbox（无派生列、无 revision 历史表）。
    pub(crate) fn commit_folder_edit(
        &self,
        tx: &Connection,
        cur: &Folder,
        name: Option<String>,
        parent: Option<Option<EntityId>>,
        now: &str,
    ) -> Result<Folder, StoreError> {
        let new_name = name.unwrap_or_else(|| cur.name.clone());
        let new_parent = parent.unwrap_or_else(|| cur.parent_id.clone());
        let rev = next_rev(cur.rev, cur.remote_rev);
        let hash = folder_hash(
            &new_name,
            &new_parent,
            &cur.color,
            cur.sort_order,
            &cur.system_kind,
        );
        tx.execute(
            "UPDATE folders SET name = ?2, parent_id = ?3, rev = ?4, content_hash = ?5, updated_at = ?6, updated_device = ?7
              WHERE id = ?1",
            params![
                cur.id.as_str(), new_name, new_parent.as_ref().map(|p| p.as_str().to_string()),
                rev.get() as i64, hash, now, self.device.to_string()
            ],
        )?;
        rows::enqueue(
            tx,
            now,
            EntityKind::Folder,
            &cur.id,
            OpKind::Upsert,
            Some(rev),
            Some(&hash),
        )?;
        rows::read_folder(tx, &cur.id)?
            .ok_or_else(|| StoreError::not_found(EntityKind::Folder, cur.id.clone()))
    }

    // 形参就是 `tombstones` 的列：包一层结构体不会少一个字段，只会多一处搬运。
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn write_tombstone(
        &self,
        tx: &Connection,
        kind: EntityKind,
        id: &EntityId,
        rev: Rev,
        purged: bool,
        content_hash: Option<String>,
        title_snap: Option<String>,
        now: &str,
    ) -> Result<(), StoreError> {
        tx.execute(
            "INSERT INTO tombstones (entity_type, entity_id, rev, deleted_at, purged, content_hash, device_id, title_snap, created_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?4)
             ON CONFLICT(entity_type, entity_id) DO UPDATE SET
               rev = MAX(tombstones.rev, excluded.rev),
               purged = MAX(tombstones.purged, excluded.purged),
               content_hash = COALESCE(excluded.content_hash, tombstones.content_hash),
               title_snap = COALESCE(excluded.title_snap, tombstones.title_snap)",
            params![
                rows::kind_tag(kind), id.as_str(), rev.get() as i64, now, purged as i64,
                content_hash, self.device.to_string(), title_snap
            ],
        )?;
        Ok(())
    }

    pub(crate) fn tombstone_of(
        conn: &Connection,
        kind: EntityKind,
        id: &EntityId,
    ) -> Result<Option<TombstoneRow>, StoreError> {
        let mut stmt = conn.prepare(
            "SELECT entity_type, entity_id, rev, deleted_at, purged, content_hash, device_id, title_snap, created_at
               FROM tombstones WHERE entity_type = ?1 AND entity_id = ?2",
        )?;
        let out = stmt
            .query_map(params![rows::kind_tag(kind), id.as_str()], |r| {
                Ok(TombstoneRow {
                    kind: rows::kind_from_tag(&r.get::<_, String>(0)?).map_err(rows::into_sql)?,
                    id: rows::parse_id(&r.get::<_, String>(1)?).map_err(rows::into_sql)?,
                    rev: Rev(r.get::<_, i64>(2)?.max(0) as u64),
                    deleted_at: r.get(3)?,
                    purged: r.get::<_, i64>(4)? != 0,
                    content_hash: r.get(5)?,
                    device_id: r.get(6)?,
                    title_snap: r.get(7)?,
                    created_at: r.get(8)?,
                })
            })?
            .next()
            .transpose()?;
        Ok(out)
    }

    /// 内容归属范围：选中项 + 全部后代，**不含祖先**。
    ///
    /// 与 `folder_closure` 的分工：祖先只是外键骨架（文件夹行要跟着走），但祖先自己
    /// 的笔记不属于这一棵子树。两者若共用一个集合，勾一个子层就会把上层（乃至默认本）
    /// 的笔记与附件字节一起带走 —— 用户以为导的是"这一棵"，拿到的却掺了别处的东西。
    pub fn folder_subtree(
        &self,
        ids: &[EntityId],
    ) -> Result<std::collections::BTreeSet<String>, StoreError> {
        let conn = self.read()?;
        let mut out = std::collections::BTreeSet::new();
        for id in ids {
            let exists: i64 = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM folders WHERE id = ?1)",
                [id.as_str()],
                |r| r.get(0),
            )?;
            if exists == 0 {
                return Err(StoreError::Rejected(format!(
                    "文件夹不存在，无法按其范围导出：{id}"
                )));
            }
            out.insert(id.to_string());
            for d in Self::descendant_ids(&conn, id)? {
                out.insert(d.to_string());
            }
        }
        Ok(out)
    }

    /// 文件夹导出范围：选中项 + 全部后代 + 祖先链（外键闭包）。
    ///
    /// 为什么要祖先：包里那条子文件夹的 `parent_id` 指向祖先，导入端若没有祖先行就
    /// 是外键失败 —— "导出一个文件夹，导不回去"比不导出更糟。
    /// 为什么不存在的 id 要直接拒：静默跳过一个文件夹，用户看到的却是一份"成功了"的
    /// 导出报告，那是把缺内容当成完整备份。
    pub fn folder_closure(
        &self,
        ids: &[EntityId],
    ) -> Result<std::collections::BTreeSet<String>, StoreError> {
        let conn = self.read()?;
        let mut out = std::collections::BTreeSet::new();
        for id in ids {
            let exists: i64 = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM folders WHERE id = ?1)",
                [id.as_str()],
                |r| r.get(0),
            )?;
            if exists == 0 {
                return Err(StoreError::Rejected(format!(
                    "文件夹不存在，无法按其范围导出：{id}"
                )));
            }
            out.insert(id.to_string());
            for d in Self::descendant_ids(&conn, id)? {
                out.insert(d.to_string());
            }
            // 往上走。`folder_cycle` 保证正常情况下不会有环，但这里宁可自己停在 visited：
            // 一次导出范围计算不该能把进程卡在库里的一条坏数据上。
            let mut cursor = id.clone();
            let mut hops = 0u32;
            while hops < 64 {
                let parent: Option<Option<String>> = conn
                    .query_row(
                        "SELECT parent_id FROM folders WHERE id = ?1",
                        [cursor.as_str()],
                        |r| r.get::<_, Option<String>>(0),
                    )
                    .optional()?;
                let Some(Some(p)) = parent else { break };
                let Ok(pid) = rows::parse_id(&p) else { break };
                if !out.insert(pid.to_string()) {
                    break; // 已经在集合里 = 走到已覆盖的分支或成环，停
                }
                cursor = pid;
                hops += 1;
            }
        }
        Ok(out)
    }

    /// 某文件夹的全部后代（递归 CTE）。成环检测的唯一依据。
    ///
    /// 用 `UNION`（而不是 `UNION ALL`）是**有意的**：它按 id 去重，因此即使盘上已经有一个环
    /// （老版本留下的 —— 远端那一支过去不查成环），这条查询也会停下来而不是永不返回。
    /// 换句话说：写入侧现在拒绝制造环，而读取侧要保证**万一**库里已经有环，用户至少不会卡死。
    pub(crate) fn descendant_ids(
        conn: &Connection,
        id: &EntityId,
    ) -> Result<Vec<EntityId>, StoreError> {
        let mut stmt = conn.prepare(
            "WITH RECURSIVE sub(id) AS (
               SELECT id FROM folders WHERE parent_id = ?1
               UNION
               SELECT f.id FROM folders f JOIN sub s ON f.parent_id = s.id
             )
             SELECT id FROM sub",
        )?;
        collect_ids(&mut stmt, [id.as_str()])
    }

    fn note_ids_in(conn: &Connection, folder: &EntityId) -> Result<Vec<EntityId>, StoreError> {
        let mut stmt = conn.prepare("SELECT id FROM notes WHERE folder_id = ?1 ORDER BY id")?;
        collect_ids(&mut stmt, [folder.as_str()])
    }

    fn child_folder_ids(conn: &Connection, parent: &EntityId) -> Result<Vec<EntityId>, StoreError> {
        let mut stmt = conn.prepare("SELECT id FROM folders WHERE parent_id = ?1 ORDER BY id")?;
        collect_ids(&mut stmt, [parent.as_str()])
    }

    pub(crate) fn default_folder_id(conn: &Connection) -> Result<EntityId, StoreError> {
        if let Some(cached) = rows::meta_get(conn, META_CACHED_ROOT)? {
            if let Ok(id) = EntityId::parse(&cached) {
                if rows::read_folder(conn, &id)?.is_some() {
                    return Ok(id);
                }
            }
        }
        let id: Option<String> = conn
            .query_row(
                "SELECT id FROM folders WHERE system_kind = 'default' ORDER BY created_at LIMIT 1",
                [],
                |r| r.get(0),
            )
            .optional()?;
        id.map(|s| EntityId::parse(&s))
            .transpose()?
            .ok_or_else(|| StoreError::Migration {
                from: 0,
                to: 0,
                detail: "库内缺少 system_kind='default' 的默认本".into(),
            })
    }

    // --------------------------------------------------------- 读（投影） ---

    pub fn get_note(&self, id: &EntityId) -> Result<Option<Note>, StoreError> {
        let id = id.clone();
        self.with_read(|c| rows::read_note(c, &id))
    }

    pub fn get_folder(&self, id: &EntityId) -> Result<Option<Folder>, StoreError> {
        let id = id.clone();
        self.with_read(|c| rows::read_folder(c, &id))
    }

    pub fn get_tombstone(
        &self,
        kind: EntityKind,
        id: &EntityId,
    ) -> Result<Option<TombstoneRow>, StoreError> {
        let id = id.clone();
        self.with_read(|c| Self::tombstone_of(c, kind, &id))
    }

    /// 列表投影：SQL 里**不出现 `doc`**（DATA-MODEL §13 性能约束）。
    pub fn list_notes(&self, q: &NoteQuery) -> Result<Vec<NoteListRow>, StoreError> {
        let conn = self.read()?;
        let limit = if q.limit == 0 { 500i64 } else { q.limit as i64 };
        let sql = format!(
            "SELECT {} FROM notes n JOIN folders f ON f.id = n.folder_id
              WHERE (CASE WHEN ?1 THEN n.deleted_at IS NOT NULL ELSE n.deleted_at IS NULL END)
                AND (?4 IS NULL OR n.folder_id = ?4)
              ORDER BY n.pinned DESC, n.updated_at DESC, n.id
              LIMIT ?2 OFFSET ?3",
            rows::LIST_COLS
        );
        // 绑定值必须是活到 `query_map` 之后的具名局部量（临时值会让 stmt 借用到悬垂引用）。
        let trash = q.trash as i64;
        let offset = q.offset as i64;
        let folder_bind = q.folder.as_ref().map(|f| f.as_str().to_string());
        let folder_param: Option<&str> = folder_bind.as_deref();
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = stmt.query_map(
            params![trash, limit, offset, folder_param],
            rows::list_from_row,
        )?;
        let out = rows.by_ref().collect::<Result<Vec<_>, _>>()?;
        Ok(out)
    }

    pub fn list_folders(&self) -> Result<Vec<Folder>, StoreError> {
        let conn = self.read()?;
        let sql = format!(
            "SELECT {} FROM folders ORDER BY created_at, id",
            rows::FOLDER_COLS
        );
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = stmt.query_map([], rows::folder_from_row)?;
        let out = rows.by_ref().collect::<Result<Vec<_>, _>>()?;
        Ok(out)
    }

    pub fn search(&self, q: &SearchQuery) -> Result<Vec<SearchHit>, StoreError> {
        let conn = self.read()?;
        search::run(&conn, q)
    }

    /// 三方合并取 base：`note_revisions(note_id, rev = sync_rev)`（DATA-MODEL §4.3）。
    pub fn revision_doc(
        &self,
        note: &EntityId,
        rev: Rev,
    ) -> Result<Option<serde_json::Value>, StoreError> {
        let note = note.clone();
        self.with_read(|c| rows::revision_doc(c, &note, rev))
    }

    /// 该笔记的 base（= `sync_rev`），供三方合并直接取用。
    pub fn base_doc(&self, note: &EntityId) -> Result<Option<serde_json::Value>, StoreError> {
        let base = self.get_note(note)?.map(|n| n.sync_rev);
        match base {
            Some(rev) => self.revision_doc(note, rev),
            None => Ok(None),
        }
    }

    pub fn stats(&self) -> Result<StoreStats, StoreError> {
        let conn = self.read()?;
        let one = |sql: &str| -> Result<i64, StoreError> {
            Ok(conn.query_row(sql, [], |r| r.get::<_, i64>(0))?)
        };
        let db_bytes = std::fs::metadata(&self.paths.db)
            .map(|m| m.len())
            .unwrap_or(0);
        Ok(StoreStats {
            notes: one("SELECT COUNT(*) FROM notes WHERE deleted_at IS NULL")? as u32,
            notes_trash: one("SELECT COUNT(*) FROM notes WHERE deleted_at IS NOT NULL")? as u32,
            folders: one("SELECT COUNT(*) FROM folders WHERE deleted_at IS NULL")? as u32,
            attachments: one("SELECT COUNT(*) FROM attachments")? as u32,
            attachment_bytes: one("SELECT COALESCE(SUM(size),0) FROM attachments")?.max(0) as u64,
            revisions: one("SELECT COUNT(*) FROM note_revisions")? as u32,
            tombstones: one("SELECT COUNT(*) FROM tombstones")? as u32,
            tombstones_purged: one("SELECT COUNT(*) FROM tombstones WHERE purged = 1")? as u32,
            dirty_notes: one("SELECT COUNT(*) FROM notes WHERE rev <> sync_rev")? as u32,
            // 「待发操作」只统计**会真往服务器发**的那些账户（启用中的）。本地哨兵账户
            // （enabled=0）是"提交即入 outbox"的留痕账（I8：不依赖网络可达），引擎永远不消费它，
            // 把它算进来这个计数就永远归不了零 —— 用户看到的是"同步卡住了"。
            // 本地有未上传改动这件事由 dirty_notes 表达，两者不混。
            outbox_pending: one(
                "SELECT COUNT(*) FROM sync_operations o
                  WHERE o.state IN ('pending','inflight','failed')
                    AND EXISTS(SELECT 1 FROM sync_accounts a WHERE a.id = o.account_id AND a.enabled = 1)",
            )? as u32,
            conflicts_open: one("SELECT COUNT(*) FROM sync_conflicts WHERE state = 'open'")? as u32,
            fts_rows: one("SELECT COUNT(*) FROM notes_fts")? as u32,
            user_version: migrate::current_version(&conn)?,
            search_generation: rows::meta_i64(&conn, META_SEARCH_GEN, 0)?,
            db_bytes,
        })
    }

    // ---------------------------------------------------- 偏好（settings） ---

    /// 写一条 UI 偏好到 `settings(key, scope='ui', account_id=NULL)`（DATA-MODEL §4.1）。
    ///
    /// 作用域按 §6 归类：UI 态属"永不上传"，因此这里**刻意不碰 outbox**——改个主题色
    /// 不得触发一轮同步。`value` 以 JSON 文本落列，天然满足 `json_valid` CHECK。
    pub fn set_pref(&self, key: &str, value: &serde_json::Value) -> Result<(), StoreError> {
        let key = key.trim();
        if key.is_empty() {
            return Err(StoreError::Constraint("settings.key 不能为空".into()));
        }
        let value = serde_json::to_string(value)
            .map_err(|e| StoreError::Constraint(format!("偏好值无法序列化为 JSON: {e}")))?;
        let key = key.to_string();
        self.write_tx(|tx, now| {
            tx.execute(
                "INSERT INTO settings (key, scope, account_id, value, updated_at)
                 VALUES (?1,'ui',NULL,?2,?3)
                 ON CONFLICT(key, scope, COALESCE(account_id,'')) DO UPDATE SET
                   value = excluded.value, updated_at = excluded.updated_at",
                params![key, value, now],
            )?;
            Ok(())
        })
    }

    /// 读本机可见的偏好：`scope ∈ {global,ui,device}` 且未绑定账户。
    ///
    /// 同键的优先级 device > ui > global（每设备覆盖项赢），与 §6"设备作用域不跨设备"一致。
    /// `scope='account'` 需要账户上下文，故不在此返回。
    pub fn get_prefs(&self) -> Result<serde_json::Value, StoreError> {
        let conn = self.read()?;
        let mut stmt = conn.prepare(
            "SELECT key, value FROM settings
              WHERE scope IN ('global','ui','device') AND account_id IS NULL
              ORDER BY CASE scope WHEN 'global' THEN 0 WHEN 'ui' THEN 1 ELSE 2 END, key",
        )?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        let mut map = serde_json::Map::new();
        for (k, raw) in rows {
            let v: serde_json::Value = serde_json::from_str(&raw).map_err(|e| {
                StoreError::Constraint(format!(
                    "settings[{k}] 存的不是合法 JSON（库被外部改写？）: {e}"
                ))
            })?;
            map.insert(k, v);
        }
        Ok(serde_json::Value::Object(map))
    }

    // ------------------------------------------------------- 搜索索引维护 ---

    /// 全量重建 FTS 索引并推进 `meta.search_generation`（DATA-MODEL §7.3）。
    pub fn rebuild_search(&self) -> Result<(), StoreError> {
        self.write_tx(|tx, _now| {
            tx.execute("INSERT INTO notes_fts(notes_fts) VALUES('rebuild')", [])?;
            let gen = rows::meta_i64(tx, META_SEARCH_GEN, 0)?;
            rows::meta_set(tx, META_SEARCH_GEN, &(gen + 1).to_string())?;
            Ok(())
        })
    }

    /// 抽样比对 `MATCH` 与 `LIKE` 的命中集合（DATA-MODEL §7.3 的 verify_search）。
    ///
    /// 这是唯一可靠的索引脱钩探针：external-content FTS5 的 `count(*)` 走内容表，
    /// 索引为空时数字依然"正确"（实测），行数比对会永久漏判。
    pub fn verify_search(&self) -> Vec<InvariantViolation> {
        let conn = match self.read() {
            Ok(c) => c,
            Err(e) => return vec![violation("I5", format!("读连接不可用: {e}"))],
        };
        let samples = match sample_trigrams(&conn, 16) {
            Ok(v) => v,
            Err(e) => return vec![violation("I5", format!("抽样失败: {e}"))],
        };
        let mut out = Vec::new();
        for tri in samples {
            let like = conn.query_row(
                "SELECT COUNT(*) FROM notes
                  WHERE deleted_at IS NULL AND plain_text COLLATE NOCASE LIKE ?1 ESCAPE '!'",
                [search::like_pattern(&tri)],
                |r| r.get::<_, i64>(0),
            );
            let fts = conn.query_row(
                "SELECT COUNT(*) FROM notes
                  WHERE deleted_at IS NULL
                    AND rowid IN (SELECT rowid FROM notes_fts WHERE notes_fts MATCH ?1)",
                [search::fts_phrase(&tri)],
                |r| r.get::<_, i64>(0),
            );
            match (like, fts) {
                (Ok(l), Ok(f)) if l != f => out.push(violation(
                    "I5",
                    format!(
                        "FTS 与派生列脱钩：样本 {tri:?} LIKE={l} MATCH={f}，需 rebuild_search()"
                    ),
                )),
                (Err(e), _) | (_, Err(e)) => {
                    out.push(violation("I5", format!("抽样查询失败: {e}")))
                }
                _ => {}
            }
        }
        out
    }

    // --------------------------------------------------------------- 自检 ---

    /// 启动自检 + 供测试断言（返回空 = 全部不变式成立）。
    /// 只读、不修库；修复路径是 `rebuild_search()` 与 host 的用户提示。
    pub fn verify(&self) -> Vec<InvariantViolation> {
        let mut v: Vec<InvariantViolation> = Vec::new();
        let conn = match self.read() {
            Ok(c) => c,
            Err(e) => {
                v.push(violation("§12", format!("读连接不可用: {e}")));
                return v;
            }
        };
        let bad =
            |v: &mut Vec<InvariantViolation>, id: &'static str, label: &str, sql: &str| match conn
                .prepare(sql)
            {
                Ok(mut stmt) => match stmt.query_map([], |r| r.get::<_, String>(0)) {
                    Ok(rows) => {
                        for row in rows.flatten().take(5) {
                            v.push(violation(id, format!("{label}: {row}")));
                        }
                    }
                    Err(e) => v.push(violation(id, format!("{label}: SQL 失败 {e}"))),
                },
                Err(e) => v.push(violation(id, format!("{label}: SQL 准备失败 {e}"))),
            };

        // §12：PRAGMA 必须真的生效（foreign_keys 是连接级，每个连接都要设）
        match crate::pool::foreign_keys_on(&conn) {
            Ok(true) => {}
            Ok(false) => v.push(violation("§12", "foreign_keys 未开启")),
            Err(e) => v.push(violation("§12", format!("foreign_keys 不可读: {e}"))),
        }
        match crate::pool::journal_mode(&conn) {
            Ok(m) if m.eq_ignore_ascii_case("wal") => {}
            Ok(m) => v.push(violation("§12", format!("journal_mode 应为 wal，实际 {m}"))),
            Err(e) => v.push(violation("§12", format!("journal_mode 不可读: {e}"))),
        }
        match crate::pool::synchronous_value(&conn) {
            Ok(2) | Ok(3) => {} // FULL=2 / EXTRA=3
            Ok(x) => v.push(violation(
                "§12",
                format!("synchronous 必须是 FULL，实际 {x}"),
            )),
            Err(e) => v.push(violation("§12", format!("synchronous 不可读: {e}"))),
        }

        // I1：ID 形态；已 purge 的实体不得复活（删除后永不复用）
        bad(
            &mut v,
            "I1",
            "notes.id 形态非法",
            "SELECT id FROM notes WHERE length(id) <> 36",
        );
        bad(
            &mut v,
            "I1",
            "folders.id 形态非法",
            "SELECT id FROM folders WHERE length(id) <> 36",
        );
        bad(&mut v, "I1", "purged 墓碑对应的笔记仍存在（复活）",
            "SELECT n.id FROM notes n JOIN tombstones t ON t.entity_type='note' AND t.entity_id=n.id WHERE t.purged=1");
        bad(&mut v, "I1", "purged 墓碑对应的文件夹仍存在（复活）",
            "SELECT f.id FROM folders f JOIN tombstones t ON t.entity_type='folder' AND t.entity_id=f.id WHERE t.purged=1");

        // I2：sync_rev 是双端确认过的一致点，不可能高于头部
        bad(
            &mut v,
            "I2",
            "notes.sync_rev > rev",
            "SELECT id FROM notes WHERE sync_rev > rev AND purged_at IS NULL",
        );
        bad(
            &mut v,
            "I2",
            "folders.sync_rev > rev",
            "SELECT id FROM folders WHERE sync_rev > rev",
        );

        // I5 + §4.4：当前 rev 与 base(sync_rev) 的 revision 必须在，否则三方合并找不回 base
        bad(&mut v, "I5", "缺当前 rev 的 revision",
            "SELECT n.id FROM notes n WHERE NOT EXISTS(SELECT 1 FROM note_revisions r WHERE r.note_id=n.id AND r.rev=n.rev)");
        bad(&mut v, "I5", "缺 base(sync_rev) 的 revision —— 冲突时找不回 base",
            "SELECT n.id FROM notes n WHERE n.sync_rev > 0 AND NOT EXISTS(SELECT 1 FROM note_revisions r WHERE r.note_id=n.id AND r.rev=n.sync_rev)");

        // I3：删除事实必须比数据活得更久，且不自动 GC
        bad(&mut v, "I3", "purged_at 却没有墓碑",
            "SELECT n.id FROM notes n WHERE n.purged_at IS NOT NULL AND NOT EXISTS(SELECT 1 FROM tombstones t WHERE t.entity_type='note' AND t.entity_id=n.id AND t.purged=1)");
        // 硬性要求 6：有 purged 墓碑的记录绝不能再被当作新增上传
        bad(&mut v, "I3", "purged 墓碑仍有 pending upsert/upload（会被重新上传）",
            "SELECT t.entity_id FROM tombstones t WHERE t.purged=1 AND EXISTS(SELECT 1 FROM sync_operations o
                WHERE o.entity_type=t.entity_type AND o.entity_id=t.entity_id
                  AND o.state IN ('pending','inflight','failed') AND o.op IN ('upsert','upload'))");

        // I6：哈希形态
        bad(
            &mut v,
            "I6",
            "content_hash 形态非法",
            "SELECT id FROM notes WHERE content_hash NOT GLOB 'sha256:[0-9a-f]*'",
        );

        // §8：local_state='available' 的 blob 必须真的在盘上
        let available: Vec<String> = match conn
            .prepare("SELECT sha256 FROM attachments WHERE local_state='available'")
            .and_then(|mut s| {
                s.query_map([], |r| r.get::<_, String>(0))
                    .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
            }) {
            Ok(list) => list,
            Err(e) => {
                v.push(violation("§8", format!("附件枚举失败: {e}")));
                Vec::new()
            }
        };
        for sha in available {
            if !blob_path(&self.paths.attachments, &sha).exists() {
                v.push(violation(
                    "§8",
                    format!("blob 缺失但 local_state='available': {sha}"),
                ));
            }
        }

        // I5：派生列 / content_hash 与 doc 一致；FTS 行数一致
        v.extend(verify_derived(&conn));
        // FTS 完整性只能靠 MATCH/LIKE 抽样比较（external content 的 count(*) 不可信）
        v.extend(self.verify_search());
        v
    }
}

// ------------------------------------------------------------------ 辅助 ---

fn violation(id: &'static str, detail: impl Into<String>) -> InvariantViolation {
    InvariantViolation {
        id,
        detail: detail.into(),
    }
}

fn collect_ids<P: rusqlite::Params>(
    stmt: &mut rusqlite::Statement<'_>,
    binds: P,
) -> Result<Vec<EntityId>, StoreError> {
    let rows = stmt.query_map(binds, |r| r.get::<_, String>(0))?;
    rows.map(|r| r.map_err(StoreError::Sql).and_then(|s| rows::parse_id(&s)))
        .collect()
}

fn attachment_for(
    conn: &Connection,
    note_id: &EntityId,
    sha: &str,
) -> Result<Attachment, StoreError> {
    let sql = format!(
        "SELECT {}, na.note_id, na.block_id, na.role, na.position
           FROM attachments a JOIN note_attachments na ON na.sha256 = a.sha256
          WHERE a.sha256 = ?1 AND na.note_id = ?2",
        rows::attachment_cols("a")
    );
    conn.query_row(
        &sql,
        params![sha, note_id.as_str()],
        rows::attachment_from_row,
    )
    .optional()?
    .ok_or_else(|| StoreError::not_found(EntityKind::Attachment, note_id.clone()))
}

/// 逐条复核派生列与 `content_hash`（I5/I6）。规模保护：最多 2000 条。
fn verify_derived(conn: &Connection) -> Vec<InvariantViolation> {
    let mut out = Vec::new();
    let mut stmt = match conn.prepare(
        "SELECT n.id, n.doc, n.title, n.plain_text, n.summary, n.char_count, n.block_count,
                n.has_attachment, n.content_hash,
                (SELECT EXISTS(SELECT 1 FROM note_attachments na WHERE na.note_id = n.id))
           FROM notes n ORDER BY n.updated_at DESC LIMIT 2000",
    ) {
        Ok(s) => s,
        Err(e) => return vec![violation("I5", format!("派生列复核失败: {e}"))],
    };
    let mapped = match stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, i64>(5)?,
            r.get::<_, i64>(6)?,
            r.get::<_, i64>(7)?,
            r.get::<_, String>(8)?,
            r.get::<_, i64>(9)?,
        ))
    }) {
        Ok(m) => m,
        Err(e) => return vec![violation("I5", format!("派生列复核语句失败: {e}"))],
    };
    for row in mapped {
        let (id, doc, title, plain, summary, chars, blocks, has, hash, linked) = match row {
            Ok(v) => v,
            Err(e) => {
                out.push(violation("I5", format!("派生列读取失败: {e}")));
                continue;
            }
        };
        match derive::prepare_text(&doc) {
            Ok(p) => {
                if p.title != title
                    || p.plain_text != plain
                    || p.summary != summary
                    || p.char_count as i64 != chars
                    || p.block_count as i64 != blocks
                {
                    out.push(violation("I5", format!("派生列与 doc 脱钩: {id}")));
                }
                if (p.doc_has_attachment || linked != 0) != (has != 0) {
                    out.push(violation("I5", format!("has_attachment 脱钩: {id}")));
                }
                if p.content_hash.as_str() != hash {
                    out.push(violation(
                        "I6",
                        format!("content_hash 与 canonical(doc) 不符: {id}"),
                    ));
                }
            }
            Err(e) => out.push(violation("I6", format!("{id} 的 doc 无法复核: {e}"))),
        }
    }
    out
}

/// 从 `plain_text` 取若干个 3 字符样本（trigram 阈值之上，两条路径可直接对比）。
fn sample_trigrams(conn: &Connection, n: usize) -> Result<Vec<String>, StoreError> {
    let mut stmt = conn.prepare("SELECT plain_text FROM notes WHERE length(plain_text) >= 3 ORDER BY updated_at DESC LIMIT 64")?;
    let mut out: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for row in stmt.query_map([], |r| r.get::<_, String>(0))? {
        let text = row?;
        let chars: Vec<char> = text.chars().collect();
        if chars.len() < 3 {
            continue;
        }
        for idx in [0usize, chars.len() / 4, chars.len() / 2, chars.len() - 3] {
            if idx + 3 > chars.len() {
                continue;
            }
            let tri: String = chars[idx..idx + 3].iter().collect();
            if tri.trim().is_empty() {
                continue;
            }
            if seen.insert(tri.clone()) {
                out.push(tri);
            }
            if out.len() >= n {
                return Ok(out);
            }
        }
    }
    Ok(out)
}

// ------------------------------------------------------------------ 引导 ---

fn bootstrap(conn: &mut Connection, device_id: &DeviceId) -> Result<DeviceId, StoreError> {
    let now = Timestamp::new(chrono::Utc::now());
    let device = device_id.to_string();
    let tx = conn.transaction()?;
    if rows::meta_get(&tx, META_CREATED_AT)?.is_none() {
        rows::meta_set(&tx, META_CREATED_AT, now.as_str())?;
    }
    if rows::meta_get(&tx, META_INSTALL_ID)?.is_none() {
        rows::meta_set(&tx, META_INSTALL_ID, &EntityId::new().to_string())?;
    }
    // meta.device_id 记录本机安装身份；每次 open 以调用方给出的为准（device_id 由 host 拥有）。
    rows::meta_set(&tx, META_DEVICE_ID, &device)?;
    if rows::meta_get(&tx, META_SEARCH_GEN)?.is_none() {
        rows::meta_set(&tx, META_SEARCH_GEN, "0")?;
    }
    // 哨兵账户：outbox.account_id 是 NOT NULL FK，而本地写入必须留痕（I8：不依赖网络可达）。
    // enabled=0 —— 未配置真实远端时，待办只落库不外发。
    tx.execute(
        "INSERT INTO sync_accounts (id, label, base_url, root_prefix, auth_kind, tls_policy, device_id, enabled, created_at)
         VALUES (?1,'本地（未配置远端）','none://local','/.notes','basic','strict',?2,0,?3)
         ON CONFLICT(id) DO NOTHING",
        params![LOCAL_ACCOUNT_ID, device, now.as_str()],
    )?;
    tx.execute(
        "INSERT INTO sync_state (account_id, phase) VALUES (?1,'unconfigured') ON CONFLICT(account_id) DO NOTHING",
        [LOCAL_ACCOUNT_ID],
    )?;
    // 默认本（DATA-MODEL §9：文件夹删除不级联，笔记移入默认本）。
    // 判据必须是"库里有没有 system_kind='default' 的行"，而不是只看 meta 缓存：
    // 升级上来的旧库 / 被外部改过 meta 的库都可能没有 cached_root_id，只看 meta 会造出第二个默认本。
    let cached_ok = match rows::meta_get(&tx, META_CACHED_ROOT)? {
        Some(cached) => match EntityId::parse(&cached) {
            Ok(id) => rows::read_folder(&tx, &id)?.is_some(),
            Err(_) => false,
        },
        None => false,
    };
    if cached_ok {
        // 已有且可用：什么都不做（幂等）
    } else if let Some(existing) = existing_default_folder(&tx)? {
        rows::meta_set(&tx, META_CACHED_ROOT, &existing.to_string())?;
    } else {
        // 固定 id：默认本是角色实体，不是"本机第一次开机时随手造的一个文件夹"。
        // 用随机 id 的话两台设备各公告一条，远端清单里就有两条 system_kind='default'
        //（实测第二台入伙后本地变 2 个文件夹、清单 1002 条），详见 types.rs 上的注释。
        let id = EntityId::parse(DEFAULT_FOLDER_ID).expect("默认本 id 是写死的合法 uuid");
        let rev = next_rev(Rev::ZERO, Rev::ZERO);
        let hash = folder_hash(
            DEFAULT_FOLDER_NAME,
            &None,
            &None,
            0,
            &Some("default".to_string()),
        );
        tx.execute(
            "INSERT INTO folders
               (id, parent_id, name, color, system_kind, sort_order, rev, sync_rev, sync_hash,
                remote_rev, content_hash, created_at, updated_at, deleted_at, purged_at, created_device, updated_device)
             VALUES (?1,NULL,?2,NULL,'default',0,?3,0,NULL,0,?4,?5,?5,NULL,NULL,?6,?6)",
            params![id.as_str(), DEFAULT_FOLDER_NAME, rev.get() as i64, hash, now.as_str(), device],
        )?;
        rows::meta_set(&tx, META_CACHED_ROOT, &id.to_string())?;
        rows::enqueue(
            &tx,
            now.as_str(),
            EntityKind::Folder,
            &id,
            OpKind::Upsert,
            Some(rev),
            Some(&hash),
        )?;
    }
    tx.commit()?;
    Ok(device_id.clone())
}

/// 库里已有的默认本（按创建序取第一个未被删掉的）。
fn existing_default_folder(conn: &Connection) -> Result<Option<EntityId>, StoreError> {
    let id: Option<String> = conn
        .query_row(
            "SELECT id FROM folders WHERE system_kind = 'default' ORDER BY deleted_at NULLS FIRST, created_at LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()?;
    id.map(|s| EntityId::parse(&s))
        .transpose()
        .map_err(Into::into)
}

/// 文件夹的同步载荷哈希：只取参与同步的字段（DATA-MODEL §6）。
pub(crate) fn folder_hash(
    name: &str,
    parent: &Option<EntityId>,
    color: &Option<String>,
    sort_order: i64,
    system_kind: &Option<String>,
) -> String {
    let payload = serde_json::json!({
        "name": name,
        "parent_id": parent.as_ref().map(|p| p.to_string()),
        "color": color,
        "sort_order": sort_order,
        "system_kind": system_kind,
    });
    hash_json(&payload).as_str().to_string()
}

pub(crate) fn validate_name(name: &str) -> Result<String, StoreError> {
    let t = name.trim();
    if t.is_empty() {
        return Err(StoreError::Constraint("名称不能为空".into()));
    }
    if t.chars().count() > 200 {
        return Err(StoreError::Constraint("名称超过 200 字符".into()));
    }
    Ok(t.to_string())
}

/// `<attachments>/<2hex>/<sha>`（DATA-MODEL §5.1 注释）。
pub(crate) fn blob_path(dir: &Path, sha256: &str) -> PathBuf {
    let prefix = &sha256[..sha256.len().min(2)];
    dir.join(prefix).join(sha256)
}

/// 先写 `.part` 再 rename：读者永远看不到半个 blob。
pub(crate) fn write_atomic(target: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let name = target
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "blob".into());
    let tmp = target.with_file_name(format!("{name}.{}.part", EntityId::new()));
    {
        let mut f = std::fs::File::create(&tmp)?;
        std::io::Write::write_all(&mut f, bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, target)?;
    Ok(())
}
