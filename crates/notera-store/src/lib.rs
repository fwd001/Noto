//! notera-store —— SQLite 仓储层（权威内容 + 派生索引 + 同步协作面）。
//!
//! 铁律（docs/ARCHITECTURE-MAP.md §2/§3/§5）：
//! * **存储层不知道同步**：本 crate 不依赖 `notera-net` / `notera-webdav` / `notera-sync`，
//!   只提供同步引擎需要的持久化协作面（outbox / 远端清单缓存 / 墓碑 / apply 闸门）。
//! * **唯一写入口**：所有权威表写入都经 `Store` 的方法，且 `notes` + `note_revisions`
//!   + 派生列 + `notes_fts` + `sync_operations` 在**同一事务**内完成（I5）。
//!   共享实现是 [`store::Store::commit_edit`]。
//! * **rev 只能由 `notera_core::next_rev` 推进**（I2），禁止任何手写 `+1`。
//! * **校验失败的数据永不进入权威表**（I6）：写入前 `richtext::parse` + canonical 哈希核对。
//! * 列表查询**不读 `doc`**（DATA-MODEL §13 性能约束）。

pub mod error;
pub mod types;

mod apply;
mod derive;
mod migrate;
mod pool;
mod rows;
mod search;
mod store;
mod syncml;

pub use crate::error::StoreError;
pub use crate::migrate::MigrateReport;
pub use crate::syncml::SyncStateRow;
pub use crate::types::{
    ApplyOp, ApplyReport, Attachment, ConflictRecord, ConflictRow, ConflictState, DirtyEntity,
    DirtyWhy, Folder, Note, NoteListRow, NoteQuery, OpKind, OpState, RemoteIndexEntry, RevOrigin,
    SearchHit, SearchPath, SearchQuery, StorePaths, StoreStats, SyncOperation, TombstoneRow,
    DEFAULT_FOLDER_NAME, LOCAL_ACCOUNT_ID, SUPPORTED_SCHEMA_VERSION,
    AttachmentJob,};

pub use crate::store::Store;

/// 让 `notera-core` 的类型在本 crate 的公共签名里保持可达（sync/host 直接复用）。
pub use notera_core::{ContentHash, DeviceId, EntityId, EntityKind, Rev};
