//! 公共 DTO：契约里给定的结构体/枚举全部住在这里（字段与契约一致，未删不改名）。
//!
//! 说明：`NoteListRow` 是"Note 去掉 `doc`/`plain_text` 的列表投影"（DATA-MODEL §13），
//! 另加 `folder_name` 与 `dirty` 两个纯派生展示位；`doc` 一律不出现在列表 SQL 里。

use notera_core::{EntityId, EntityKind, Rev};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 本程序支持的 schema 版本 = 迁移文件数（`migrations/0001..00NN`）。
pub const SUPPORTED_SCHEMA_VERSION: u32 = crate::migrate::MIGRATIONS.len() as u32;

/// 未配置任何远端账户时的哨兵账户：outbox 的 `account_id` 是 NOT NULL FK，
/// 本地写入也必须留下持久化待办记录（I8：不依赖网络可达）。
pub const LOCAL_ACCOUNT_ID: &str = "local";

/// 内置默认本名字（DATA-MODEL §9：文件夹删除不级联，笔记移入默认本）。
pub const DEFAULT_FOLDER_NAME: &str = "默认本";

/// 内置默认本的**固定** id。
///
/// 这个 id 不能每台设备新生成：默认本是"每个账户有且只有一个"的角色实体，
/// 而清单按 `(kind, id)` 认条目。以前每台设备 `EntityId::new()` 各造一个，两台设备
/// 入伙之后远端清单就有两条 `folder/default`，各自都叫"默认本"，并互相把对方的那一份
/// 拉回来 —— 实测 1000 条笔记的库上，第二台设备本地变成 2 个文件夹、清单公告 1002 条
/// （见 `notera-host/tests/big_library.rs`）。固定 id 让"同一个角色"在协议里真的只是
/// 同一个条目，内容哈希也自然相同。
pub const DEFAULT_FOLDER_ID: &str = "00000000-0000-7000-8000-6e6f74657261";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorePaths {
    pub db: PathBuf,
    pub attachments: PathBuf,
}

/// 一条笔记的权威记录 + 派生列 + revision 头部（契约字段，逐一对应 `notes` 表）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Note {
    pub id: EntityId,
    pub folder_id: EntityId,
    pub doc: serde_json::Value,
    pub doc_format: u16,
    pub title: String,
    pub plain_text: String,
    pub summary: String,
    pub char_count: u32,
    pub block_count: u32,
    pub has_attachment: bool,
    pub pinned: bool,
    pub color: Option<String>,
    pub rev: Rev,
    pub sync_rev: Rev,
    pub sync_hash: Option<String>,
    pub remote_rev: Rev,
    pub content_hash: String,
    pub created_at: String,
    pub updated_at: String,
    pub deleted_at: Option<String>,
    pub purged_at: Option<String>,
    /// 最后改这一行的是哪台设备（`meta.device_id` 那一族的字符串）。
    ///
    /// 表里一直有这一列（每次写都落），但**从没读到 DTO 上** —— 界面那句"是哪台设备改的"
    /// （规范 §6 第 12 格）就卡在"没有来源"上。2026-10-10 把它接到详情 DTO。
    pub updated_device: String,
}

/// 列表投影（不含 `doc`/`plain_text`）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoteListRow {
    pub id: EntityId,
    pub folder_id: EntityId,
    pub folder_name: String,
    pub title: String,
    pub summary: String,
    pub char_count: u32,
    pub block_count: u32,
    pub has_attachment: bool,
    pub pinned: bool,
    pub color: Option<String>,
    pub rev: Rev,
    pub sync_rev: Rev,
    pub remote_rev: Rev,
    pub content_hash: String,
    pub created_at: String,
    pub updated_at: String,
    pub deleted_at: Option<String>,
    pub purged_at: Option<String>,
    /// `rev != sync_rev`（侧栏"待上传"徽标的唯一来源，不依赖时间戳，I4）。
    pub dirty: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Folder {
    pub id: EntityId,
    pub parent_id: Option<EntityId>,
    pub name: String,
    pub color: Option<String>,
    /// `Some("default")` = 内置默认本，不可删/移/改名（`folders` 表 CHECK）。
    pub system_kind: Option<String>,
    pub sort_order: i64,
    pub rev: Rev,
    pub sync_rev: Rev,
    pub sync_hash: Option<String>,
    pub remote_rev: Rev,
    pub content_hash: String,
    pub created_at: String,
    pub updated_at: String,
    pub deleted_at: Option<String>,
    pub purged_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    /// 完整 64 hex（内容寻址键，不含 `sha256:` 前缀）。
    pub sha256: String,
    pub size: i64,
    pub media_type: String,
    pub filename: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub duration_ms: Option<i64>,
    pub local_state: String,
    pub remote_state: String,
    pub created_at: String,
    pub verified_at: Option<String>,
    pub deleted_at: Option<String>,
    /// 本次挂载所在的笔记与块（`note_attachments` 主键）。
    pub note_id: EntityId,
    pub block_id: String,
    pub role: String,
    pub position: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoteQuery {
    pub folder: Option<EntityId>,
    pub trash: bool,
    pub limit: u32,
    pub offset: u32,
}

impl NoteQuery {
    pub fn all() -> Self {
        Self {
            folder: None,
            trash: false,
            limit: 0,
            offset: 0,
        }
    }
    pub fn in_folder(folder: &EntityId) -> Self {
        Self {
            folder: Some(folder.clone()),
            trash: false,
            limit: 0,
            offset: 0,
        }
    }
    pub fn trash() -> Self {
        Self {
            folder: None,
            trash: true,
            limit: 0,
            offset: 0,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchQuery {
    pub text: String,
    pub limit: u32,
}

impl SearchQuery {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            limit: 0,
        }
    }
}

/// 实际走过的检索路径。**断言用**：trigram 对 <3 字符查询静默返回 0 行（ADR-0015）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SearchPath {
    FtsTrigram,
    LikeFallback,
}

/// 命中来自哪一档。**两档同时跑**（用户口径「精准匹配和模糊匹配共同的去搜索」）：
/// `Exact` = 每个词段都连着出现；`Fuzzy` = 每个词段的三字串都在同一条笔记里，但允许中间隔话。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MatchKind {
    Exact,
    Fuzzy,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchHit {
    pub note_id: EntityId,
    pub score: f64,
    /// 已 HTML 转义并插入 `<mark>` 的片段，UI 可直接渲染。
    pub snippet_html: String,
    pub path_used: SearchPath,
    pub match_kind: MatchKind,
}

/// 写入口使用的 revision 来源标记（`note_revisions.origin`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RevOrigin {
    Local,
    Remote,
    Merged,
    ConflictCopy,
    Restored,
}

impl RevOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            RevOrigin::Local => "local",
            RevOrigin::Remote => "remote",
            RevOrigin::Merged => "merged",
            RevOrigin::ConflictCopy => "conflict_copy",
            RevOrigin::Restored => "restored",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "local" => RevOrigin::Local,
            "remote" => RevOrigin::Remote,
            "merged" => RevOrigin::Merged,
            "conflict_copy" => RevOrigin::ConflictCopy,
            "restored" => RevOrigin::Restored,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreStats {
    pub notes: u32,
    pub notes_trash: u32,
    pub folders: u32,
    pub attachments: u32,
    pub attachment_bytes: u64,
    pub revisions: u32,
    pub tombstones: u32,
    pub tombstones_purged: u32,
    pub dirty_notes: u32,
    /// 待发操作数：只统计**启用中账户**的 pending/inflight/failed。
    /// 本地哨兵账户（`local`）的留痕行不计入，否则这个数永远归不了零。
    pub outbox_pending: u32,
    pub conflicts_open: u32,
    pub fts_rows: u32,
    pub user_version: u32,
    /// ADR-0012 的只读闸门开着 = 这本库比本程序新，界面要据此说"请升级以编辑"。
    /// 由 `Store` 自己填：它已经是这个事实的唯一持有者，不许第二处再推一遍。
    pub library_read_only: bool,
    pub search_generation: i64,
    pub db_bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DirtyWhy {
    /// `sync_rev = 0`：从未与远端确认过一致。
    NeverPushed,
    Edited,
    Deleted,
    Purged,
    /// 本机这一行是**干净**的，而持久远端视图已经越过本机确认点（`Store::remote_moved_entities`）。
    /// 它不是"脏"，但没有它本轮计划就看不见对面的删除（见那条查询的注释）。
    RemoteMoved,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirtyEntity {
    pub kind: EntityKind,
    pub id: EntityId,
    pub rev: Rev,
    pub content_hash: String,
    pub sync_rev: Rev,
    pub why: DirtyWhy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpKind {
    Upsert,
    Delete,
    Purge,
    Upload,
    Download,
}

impl OpKind {
    pub fn as_str(self) -> &'static str {
        match self {
            OpKind::Upsert => "upsert",
            OpKind::Delete => "delete",
            OpKind::Purge => "purge",
            OpKind::Upload => "upload",
            OpKind::Download => "download",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "upsert" => OpKind::Upsert,
            "delete" => OpKind::Delete,
            "purge" => OpKind::Purge,
            "upload" => OpKind::Upload,
            "download" => OpKind::Download,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpState {
    Pending,
    Inflight,
    Done,
    Failed,
    Superseded,
    Blocked,
}

impl OpState {
    pub fn as_str(self) -> &'static str {
        match self {
            OpState::Pending => "pending",
            OpState::Inflight => "inflight",
            OpState::Done => "done",
            OpState::Failed => "failed",
            OpState::Superseded => "superseded",
            OpState::Blocked => "blocked",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "pending" => OpState::Pending,
            "inflight" => OpState::Inflight,
            "done" => OpState::Done,
            "failed" => OpState::Failed,
            "superseded" => OpState::Superseded,
            "blocked" => OpState::Blocked,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncOperation {
    pub id: i64,
    pub dedupe_key: String,
    pub kind: EntityKind,
    /// UUID 键实体（note/folder）就是它自己；**附件以 sha256 寻址**，此时该字段是
    /// nil UUID（见 [`SyncOperation::entity_key`]）。
    pub id_: EntityId,
    pub op: OpKind,
    pub payload_rev: Option<Rev>,
    pub sha256: Option<String>,
    pub state: OpState,
    pub attempts: u32,
    /// `sync_operations.entity_id` 的原始值（附件上传时就是 64hex 的 sha，权威）。
    pub entity_key: String,
    pub account_id: String,
}

/// 同步引擎交给存储层落地的远端动作。整批在**单事务**内执行（I5/I6）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ApplyOp {
    /// `env` = 记录信封（DATA-MODEL §11 / SYNC-PROTOCOL §3）。
    /// 存储层自行校验 `hash == sha256(canonical(payload))`，不符即整批回滚。
    UpsertNote {
        env: serde_json::Value,
    },
    UpsertFolder {
        env: serde_json::Value,
    },
    /// 冲突解决的一半：**采纳服务器那一版作为正文**（CONFLICT-RESOLUTION §6.1）。
    ///
    /// 与 `UpsertNote` 的唯一区别是允许 `rev` 相等而内容不同 —— 普通 upsert 必须拒掉那种
    /// 情况（"服务器侧异常"），但两侧各自从同一个确认点推到同一个 rev，正是分布式写作的
    /// 正常结果。前提由调用方保证：**本机那一份已经在副本笔记里**，否则这条就是把用户
    /// 输入扔掉。`rev` 依旧绝不许倒退（I2）。
    AdoptConflict {
        env: serde_json::Value,
    },
    SetRemote {
        kind: EntityKind,
        id: EntityId,
        rev: Rev,
        hash12: String,
    },
    Tombstone {
        kind: EntityKind,
        id: EntityId,
        rev: Rev,
    },
    Purge {
        kind: EntityKind,
        id: EntityId,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplyReport {
    pub applied: usize,
    pub skipped: usize,
    pub notes_written: usize,
    pub folders_written: usize,
    pub tombstones_written: usize,
    pub rows_removed: usize,
}

/// `sync_remote_index` 的一行（清单缓存条目，`hash12` 是快速路径提示）。
///
/// `kind = Attachment` 时实体身份是 **sha256**（内容寻址，`EntityId` 装不下），
/// 因此 `sha256` 必填且作为索引键；`id` 用 nil UUID 占位。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteIndexEntry {
    pub kind: EntityKind,
    pub id: EntityId,
    pub rev: Rev,
    pub hash12: Option<String>,
    pub size: Option<i64>,
    pub deleted: bool,
    pub purged: bool,
    pub seg: Option<String>,
    /// 附件专用：`attachments.sha256`（64hex）。note/folder 一律 `None`。
    pub sha256: Option<String>,
    /// 远端声明的删除时间（`None` = 未删）。引擎的 `RemoteView` 要它才能还原
    /// "删除先后 / 删后又改"那类判据；少了它，把视图缓存进 `sync_remote_index`
    /// 反而是有损的（见 migrations/0007）。
    pub deleted_at: Option<String>,
}

/// 写 `sync_conflicts` 的输入（收件箱一条候选）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictRecord {
    pub account_id: String,
    pub kind: EntityKind,
    pub id: EntityId,
    pub base_rev: Rev,
    pub local_rev: Rev,
    pub remote_rev: Rev,
    pub local_hash: String,
    pub remote_hash: String,
    pub auto_merged: bool,
    pub copy_note_id: Option<EntityId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConflictState {
    Open,
    Resolved,
    Dismissed,
}

impl ConflictState {
    pub fn as_str(self) -> &'static str {
        match self {
            ConflictState::Open => "open",
            ConflictState::Resolved => "resolved",
            ConflictState::Dismissed => "dismissed",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "open" => ConflictState::Open,
            "resolved" => ConflictState::Resolved,
            "dismissed" => ConflictState::Dismissed,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictRow {
    /// `sync_conflicts.id`（自增行号，非实体 ID）。
    pub conflict_id: i64,
    pub account_id: String,
    pub kind: EntityKind,
    pub id: EntityId,
    pub base_rev: Rev,
    pub local_rev: Rev,
    pub remote_rev: Rev,
    pub local_hash: String,
    pub remote_hash: String,
    pub auto_merged: bool,
    pub copy_note_id: Option<EntityId>,
    pub state: ConflictState,
    pub resolution: Option<String>,
    pub created_at: String,
    pub resolved_at: Option<String>,
    /// 服务器那一版的**原始记录信封字节**（`None` = 这一轮没取回来）。
    /// 界面读它来决定右栏显示"对面那一版"还是"没取到 + 原因"，见迁移 0008。
    pub remote_wire: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TombstoneRow {
    pub kind: EntityKind,
    pub id: EntityId,
    pub rev: Rev,
    pub deleted_at: String,
    pub purged: bool,
    pub content_hash: Option<String>,
    pub device_id: String,
    pub title_snap: Option<String>,
    pub created_at: String,
}

/// 一条历史版本在界面上的**那一行**（§6「版本历史浏览」）。
///
/// 刻意**不带 doc**：一篇笔记的保留窗口是 200 行（DATA-MODEL §4.4），把 200 份正文一次发给界面
/// 是这一格最容易写出来的性能事故 —— 列表只要"有哪一版、谁在什么时候改的、和现在是不是同一份"，
/// 要看正文是点进去之后的另一次读。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RevisionMeta {
    pub rev: i64,
    /// `local` / `remote` / `merged` / `conflict_copy` / `restored`（迁移里的 CHECK）。
    /// 注意 `restored` 说的是"从回收站回来"，**不是**"回到旧版本" —— 界面上那句话别说反。
    pub origin: String,
    pub device_id: String,
    pub created_at: String,
    /// 这一版的正文哈希。界面拿它与当前那一版比，才知道"这一版其实就是现在"。
    pub content_hash: String,
}

/// 一次附件传输的最小信息（队列按体积排序，预算按字节数截断）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttachmentJob {
    pub sha256: String,
    pub size: i64,
    pub media_type: String,
    /// 账上此刻的远端态。上传队列带它出来，是因为**"这次搬运要不要覆盖服务器上那份"
    /// 是一个关于账的判断，不是关于 HTTP 的判断**：`remote_state='error'` 的含义是
    /// "这一份被内容比对否定过"（下载复验不符，或用户看着坏图点了「重新上传本机这份」），
    /// 于是"服务器上有同名对象所以跳过"那个省流量的例外对它不成立 —— 见 SYNC-PROTOCOL §13。
    pub remote_state: String,
}

/// 设置页「附件管理器」那一格的一行：这台设备上**一份对象**的账（全表，一次读）。
///
/// 为什么是命名字段而不是元组：这些数各自来自一列，界面上三句话分别读其中三个
/// （"不在这台设备上"看 `local_state`、"在隔离区"看 `quarantined_at`、"几篇在用"看 `refs`）。
/// 写成七个位置的元组之后，哪一位是哪个意思
/// 只能靠数位置 —— 数错一位就是一句语法正确、数字错误的实话。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttachmentInventoryRow {
    pub sha256: String,
    /// 账上登记的字节数（`size` 列）。只有账没有字节的那些行也带着它 —— 那是对面同步
    /// 过来的一张图"应该有多大"，界面上"这几份一共 N MB"读的就是这一列。
    pub size: i64,
    pub local_state: String,
    pub remote_state: String,
    /// 进了 GC 隔离区的那一行才有；值是**隔离发生**的时刻，不是到期时刻。
    /// 宽限期（几天后可释放）是 host 的策略，存储层不复制一份 30 天。
    pub quarantined_at: Option<String>,
    /// `COUNT(DISTINCT note_id)`，且**含回收站里的笔记**（笔记行没真删，它的链接就还在）。
    pub refs: i64,
    /// 账上那个名字（`filename` 列）。可能是 `None`：从剪贴板粘进来的一张图本来就没名字。
    /// 界面上这一行要有可读的名字，没名字时要按"这张图 / 这份文件"说 ——
    /// 拿 sha 的前几位当名字就是把内容寻址的键露给用户（§5：可读名字）。
    pub filename: Option<String>,
    /// MIME 类型，存储层原样给（`media_type` 列，NOT NULL）。界面只用它分辨"图"还是"文件"
    /// 这一个词，**不许**把它本身画上屏（host 侧折成 `isImage`）。
    pub media_type: String,
}

/// 附件「已回收」账的合计（`attachment_reclaims` 全表的两个 `SUM`）。
///
/// 与 [`AttachmentInventoryRow`] **不是一族**：那些行是"还活着的对象"，这本账记的是
/// 已经销毁的 —— 它数不进 `attachment_inventory` 的行里，界面也不能把它与
/// "这台设备上有几份"合成一句（一本是资产、一本是流水，合成就会出现总份数比实际大）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReclaimedTotals {
    /// 已销毁的份数（每一份按销毁那一刻账上的行数一份）。
    pub files: i64,
    /// 已销毁的字节数（按销毁那一刻 `attachments.size` 落账；不是磁盘上文件的实际大小 ——
    /// 账与字节从来是两本，与 `unavailableBytes` 同一口径）。
    pub bytes: i64,
}
