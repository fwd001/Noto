//! 命令面：UI 与核心之间唯一的调用契约。
//!
//! 三条规矩（docs/ARCHITECTURE-MAP.md §5）：
//! 1. 这里只做 DTO ↔ 用例转换与错误折叠，**不写业务规则**；
//! 2. 处理器内**不得 await 网络**：需要网络的一律交给同步引擎，靠事件回流；
//! 3. 错误一律折成 `CmdError{code}`，UI 按 code 查文案，拿不到内部术语。
//!
//! 同一份 `dispatch` 同时服务 Tauri `invoke` 与本地 dev HTTP 桥，
//! 因此"浏览器里验证过的行为"与"桌面里的行为"是同一份代码，不存在两套逻辑。

use crate::App;
use notera_core::{EntityId, Rev};
use notera_store::{ConflictRow, NoteQuery, SearchQuery, StoreStats};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------- DTO 出参 ---

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteDto {
    pub id: String,
    pub folder_id: String,
    pub doc: serde_json::Value,
    pub doc_format: u16,
    pub title: String,
    pub summary: String,
    pub char_count: u32,
    pub block_count: u32,
    pub has_attachment: bool,
    pub pinned: bool,
    pub color: Option<String>,
    pub rev: u64,
    pub content_hash: String,
    pub created_at: String,
    pub updated_at: String,
    pub deleted_at: Option<String>,
}

/// 「今天这一篇日记」的出参。`created` 明写这一按是**新建**还是**回到已有那一篇** ——
/// 前端不许靠"标题是不是今天的日期"自己猜（那是第二份判定，与核心的判据迟早分叉）。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyNoteDto {
    pub note: NoteDto,
    pub day: String,
    pub created: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteListDto {
    pub id: String,
    pub folder_id: String,
    pub folder_name: String,
    pub title: String,
    pub summary: String,
    pub char_count: u32,
    pub has_attachment: bool,
    pub pinned: bool,
    pub updated_at: String,
    pub deleted_at: Option<String>,
    pub dirty: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderDto {
    pub id: String,
    pub parent_id: Option<String>,
    pub name: String,
    pub color: Option<String>,
    pub system_kind: Option<String>,
    pub note_count: u64,
    pub children: Vec<FolderDto>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHitDto {
    pub note_id: String,
    pub score: f64,
    pub snippet_html: String,
    pub title: String,
    /// §3.3 的分档：`true` = 每个词段都连着出现（精准），`false` = 只有三字串档命中（模糊）。
    ///
    /// 存储层一直算好了（`SearchHit::match_kind`），丢在这一刀。以前界面上"精准 N · 模糊 N"
    /// 根本画不出来，就是因为线上没这一格。
    pub exact: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictDto {
    pub id: i64,
    pub note_id: String,
    pub note_title: String,
    pub base_rev: u64,
    pub local_rev: u64,
    pub remote_rev: u64,
    pub copy_note_id: Option<String>,
    /// 副本笔记当前的 rev。面板左栏要按 `(copyNoteId, copyRev)` 取预览 ——
    /// 采纳远端正文之后，`noteId` 的那个 rev 已经是**服务器那一版**了，
    /// 拿它当"本地那一版"会把左右两栏显示成同一份内容。
    pub copy_rev: Option<u64>,
    /// **服务器那一版**的正文预览（`None` = 这一轮没把那一版取回来）。
    ///
    /// 为什么由卡片带着走而不是让面板去查 `preview_text(noteId, remoteRev)`：
    /// `rev` 是**各设备自己的编号**，本机历史上同一个号往往是另一份内容 —— 那样
    /// 左右两栏会显示同一段文字，用户据此做的决定就没有依据（前端
    /// `stores/conflicts.ts` 的注释早就写明这点）。载荷来自迁移 0008 的 `remote_wire`。
    pub remote_preview: Option<String>,
    pub created_at: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountDto {
    pub id: String,
    pub label: String,
    pub base_url: String,
    pub root_prefix: String,
    pub auth_kind: String,
    pub tls_policy: String,
    pub proxy_mode: String,
    pub proxy_host: Option<String>,
    pub proxy_port: Option<u16>,
    /// 代理用户名存在凭据项里（与它的口令同一条），配置只有引用 ⇒ 回传不了本体。
    /// 与 `has_credential` / `has_ca_pem` 同一套口径：告诉界面"存过没有"，
    /// 那一格才能显示"已设置（留空则不改）"，而不是每次重开都被洗成空。
    pub proxy_has_username: bool,
    pub bypass: Vec<String>,
    pub enabled: bool,
    /// 永不下发明文，只告诉 UI 有没有存过
    pub has_credential: bool,
    /// 这一轮**真拿得到**口令吗（系统凭据库里有，或本次会话的内存表里有）。
    /// 引用挂着但这里是 false = 重启过了 / 换机器了 —— 界面要说的不是"已保存"而是"请重填"。
    pub credential_live: bool,
    /// 拿到的那一份是不是在**系统**凭据库里（= 退出后还在）。缺口 G38 选 B 之后，
    /// 没有系统凭据库的平台上口令只活在这次进程里，所以这一位是 false，
    /// 设置页照它显示"退出后需要重填"。
    pub credential_persistent: bool,
    /// 同上，针对 `ca_bundle` 那档的根证书 PEM：界面要能区分"没配"与"配了但不回显"，
    /// 否则重开表单看着像空着，用户会以为自己的设置被吞了（口令那一格早有同样的先例）。
    /// PEM 本体可以有几 KB，不该每次回填都端过去，所以这里只给布尔。
    pub has_ca_pem: bool,
    /// 而指纹**不是**秘密（就是服务器公钥证书的 sha256），原样回填才编辑得了 ——
    /// 缺了它，选了 `pin` 档的账户每次保存都得重敲一遍 64 位十六进制。
    pub pinned_sha256: Vec<String>,
    /// 用户名不是秘密，而且是表单回填的唯一依据 —— 没有它，用户改一次设置就得重填用户名，
    /// 少填一个字段还会让配置静默退回"需要凭据"。口令与 `credential_ref` 一律不下发。
    pub username: Option<String>,
    /// §5 的探测结果。`None` = **还没探过**（不是"探过了但不支持"）——
    /// 界面必须能把这两件事分开说，否则用户会以为自己的服务器不行。
    pub cap_mask: Option<u32>,
    /// 由位图选出的写入策略：`S1`/`S2`/`S3`（`S3` = 盲写复验，无并发保护）。
    pub write_strategy: Option<String>,
    pub caps_probed_at: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatusDto {
    pub phase: String,
    pub badge: String,
    pub last_success_at: Option<String>,
    pub pending_ops: u32,
    pub open_conflicts: u32,
    pub message_key: Option<String>,
    pub retryable: bool,
    /// §4.3 / G87：这一轮被"远端少了一大截"那道闸门停下的那笔账；没停就是 `null`。
    pub divergence_held: Option<DivergenceHeldDto>,
}

/// 停下时那两个数直接发给界面：原因要说得出"少了多少"，而不是一句"同步已暂停"。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DivergenceHeldDto {
    pub cached_records: usize,
    pub received_records: usize,
}

/// 附件在这台设备账上的那对状态，给界面的占位/按钮用。
///
/// 走 DTO 而不是 `serde_json::json!` 手写字面：这条边上的键名就是契约（`stats` 那次
/// snake_case 直发让设置页三行恒为 `—`，而前端测试因为喂的是 camelCase 假数据，
/// 全程绿灯）。字段名交给 `rename_all` 派生，写错编译不过。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentStateDto {
    pub sha256: String,
    /// `missing` / `partial` / `available` / `error`（DATA-MODEL §8）。
    pub local_state: String,
    /// `unknown` / `absent` / `present` / `error`。
    pub remote_state: String,
}

/// §6「附件管理器」那一格的一行：一份对象在这台设备与服务器上的账。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentRowDto {
    pub sha256: String,
    pub bytes: i64,
    pub local_state: String,
    pub remote_state: String,
    /// 有几条笔记引用它 —— **含回收站里的那些**（链接还在就还不能算没人要，与 GC 同一口径）。
    pub refs: i64,
    /// 在隔离区里的那一份什么时候到期可释放；不在隔离区、或时间源读不出来时是 `null`。
    pub quarantined_until: Option<String>,
    /// 这一行的可读名字（账上的 `filename`）。没名字 ⇒ `null`，界面按"这张图 / 这份文件"说，
    /// **不许**拿 sha 的前几位顶上（§5：可读名字；§4.5：不许用哈希代替内容）。
    pub name: Option<String>,
    /// 是不是图片。这里替界面把它折成一个 bool 而不是把 `media_type` 发过去：
    /// 后者是 `image/png` 这种协议词汇，一旦过桥就迟早会有人把它直接画上屏（§8 第一问）。
    pub is_image: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentTotalsDto {
    pub count: i64,
    pub bytes: i64,
    /// 字节**不在这台设备上**的份数与字节（`local_state != 'available'`）。
    pub unavailable_count: i64,
    pub unavailable_bytes: i64,
    /// 隔离区里的份数与字节：过了宽限期之后**可以**释放多少（没说已释放）。
    pub quarantined_count: i64,
    pub quarantined_bytes: i64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentInventoryDto {
    pub rows: Vec<AttachmentRowDto>,
    pub totals: AttachmentTotalsDto,
}

/// §6「版本历史」列表里的一行（正文不在这里 —— 见 `note_revision`）。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RevisionRowDto {
    pub rev: i64,
    /// `local` / `remote` / `merged` / `conflict_copy` / `restored`。
    /// **`restored` 说的是"从回收站回来"**，不是"回到旧版本" —— 界面那句话别说反。
    pub origin: String,
    pub device_id: String,
    pub created_at: String,
    /// 这一版的正文与当前那一版逐字节相同。由核心判 —— `content_hash` 一个字都不过桥
    /// （§4.5：绝不能用一串哈希代替内容）。
    pub same_as_now: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteRevisionsDto {
    pub rows: Vec<RevisionRowDto>,
    /// 本机认为的最新一版（`notes.rev`）。
    pub current_rev: i64,
    /// 双方都确认过的那一版（`notes.sync_rev`）。两者不等 = 本机还有个头部没公告。
    pub sync_rev: i64,
    /// 列表被上限截断：还有更早的版本没列出来，界面要说出这一句。
    pub truncated: bool,
}

/// 某一版的正文（点开列表里那一行时才读，一次一发）。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RevisionDocDto {
    pub rev: u64,
    pub doc: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteRevisionCmd {
    pub id: String,
    pub rev: u64,
}

/// 库统计的对外视图。
///
/// 这条边以前是 `serde_json::to_value(StoreStats)` 直发 —— 存储层的字段名（`notes_trash`、
/// `fts_rows`、`outbox_pending`）就这么漏到了 UI，而契约图和前端读的是
/// `notesInTrash / ftsEntries / inflightOps`。TypeScript 的类型是断言不是校验，
/// 于是设置页的"回收站 / 占用空间 / 待同步"三行恒为 `—`、侧栏回收站恒为 0，
/// 而单元测试喂的是 camelCase 假数据，正好把这个洞盖住。
/// 现在按命令面的规矩走 DTO：字段名是契约，存储层内部叫什么不算。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatsDto {
    pub notes: u32,
    pub notes_in_trash: u32,
    pub folders: u32,
    pub attachments: u32,
    pub fts_entries: u32,
    pub db_bytes: u64,
    pub search_generation: i64,
    /// 待发的服务端操作数。口径见 `StoreStats::outbox_pending`：只算启用中的账户，
    /// 本地哨兵账户的留痕行不计入，与 `SyncStatusDto::pending_ops` 同一个意思。
    pub inflight_ops: u32,
    /// 这本库比本程序新（ADR-0012 只读闸门）。界面据此挂"请升级以编辑"的全局横幅。
    pub library_read_only: bool,
    /// 这台设备的身份（`meta.device_id`，安装时生成一次、跨同步不变）。
    ///
    /// §6 那条「每条记录带 device_id，界面上从没出现过"是哪台设备改的"」的入口：
    /// 先让"这一台是谁"上屏，冲突/版本那一侧的"对面是哪一台"才有可比的东西。
    /// 取的是**存储层那一份**（记录信封上的 `device` 就是它），不是配置里的字符串 ——
    /// 两者应当相等，这条不变量在 `lib.rs` 的 stats 契约测试里钉着。
    pub device_id: String,
}

impl From<StoreStats> for StatsDto {
    fn from(s: StoreStats) -> Self {
        Self {
            notes: s.notes,
            notes_in_trash: s.notes_trash,
            folders: s.folders,
            attachments: s.attachments,
            fts_entries: s.fts_rows,
            db_bytes: s.db_bytes,
            search_generation: s.search_generation,
            inflight_ops: s.outbox_pending,
            library_read_only: s.library_read_only,
            // 设备的身份不在存储层的统计数字里，由 `App::stats()` 那一层填（同源是 `meta.device_id`）。
            device_id: String::new(),
        }
    }
}

// ---------------------------------------------------------------- DTO 入参 ---

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateNoteCmd {
    /// `None` = 用户还没选过文件夹。落点由核心决定（默认本），
    /// 不让 UI 自己猜"该放哪"—— 那是业务判定。
    #[serde(default)]
    pub folder_id: Option<String>,
    pub doc: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EditNoteCmd {
    pub id: String,
    pub doc: serde_json::Value,
    pub expected_rev: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdCmd {
    pub id: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveNoteCmd {
    pub id: String,
    pub folder_id: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PinCmd {
    pub id: String,
    pub pinned: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateFolderCmd {
    #[serde(default)]
    pub parent_id: Option<String>,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameFolderCmd {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveFolderCmd {
    pub id: String,
    #[serde(default)]
    pub parent_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListNotesCmd {
    #[serde(default)]
    pub folder_id: Option<String>,
    #[serde(default)]
    pub trash: bool,
    #[serde(default = "default_limit")]
    pub limit: u32,
    #[serde(default)]
    pub offset: u32,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchCmd {
    pub text: String,
    #[serde(default = "default_limit")]
    pub limit: u32,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachCmd {
    pub note_id: String,
    pub block_id: String,
    pub role: String,
    /// 字节的来源之一：壳里选好的文件路径（绝对或相对应用数据目录）。
    /// 与 `bytesBase64` **恰好二选一** —— 两个都给或都不给是形状错误，
    /// 宁可拒绝也不能猜，猜错就是把一份附件当成另一份。
    #[serde(default)]
    pub local_path: Option<String>,
    /// 字节的来源之二：前端 `<input type=file>` 读到的字节（标准 base64）。
    /// 走这条时字节仍然只经核心算 sha256、落盘、写库 —— 前端不做任何"存"的决定。
    #[serde(default)]
    pub bytes_base64: Option<String>,
    /// 空串 = 按 `application/octet-stream` 落库（浏览器对未知扩展就是给不出 type）。
    #[serde(default)]
    pub media_type: String,
    #[serde(default)]
    pub filename: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentDataCmd {
    /// 内容寻址键。必须是 64 位小写 hex —— 它会被拼进 blob 路径，
    /// 不校验就等于把"任意相对路径"交给文件系统（`../../` 那类）。
    pub sha256: String,
}

/// 「重试取回」/「重新上传本机这份」这两条用户动作的参数（同形，同一个校验）。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentShaCmd {
    pub sha256: String,
}

/// `attachment_states` 的参数：一篇笔记引用到的那批内容键。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentShasCmd {
    pub shas: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewTextCmd {
    pub id: String,
    pub rev: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountDraftCmd {
    #[serde(default)]
    pub id: String,
    pub label: String,
    pub base_url: String,
    #[serde(default)]
    pub root_prefix: Option<String>,
    #[serde(default)]
    pub auth_kind: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub tls_policy: Option<String>,
    #[serde(default)]
    pub ca_pem: Option<String>,
    /// `pin` 档的指纹表。此前这个字段**根本不存在**，而保存路径把 `pinned_sha256` 硬编成
    /// `None` —— 于是 §6 那一档在界面上永远配不出来（选它就只会在保存时撞到"至少需要一个指纹"）。
    #[serde(default)]
    pub pinned_sha256: Option<Vec<String>>,
    #[serde(default)]
    pub proxy_mode: Option<String>,
    #[serde(default)]
    pub proxy_host: Option<String>,
    #[serde(default)]
    pub proxy_port: Option<u16>,
    #[serde(default)]
    pub proxy_username: Option<String>,
    #[serde(default)]
    pub proxy_password: Option<String>,
    #[serde(default)]
    pub bypass: Option<Vec<String>>,
    /// 「启用同步」那一格。此前这个字段**根本不在结构体里**：前端一直在发（`toWire`），
    /// serde 静默丢掉，`configure_account` 又硬编 `enabled: true` —— 于是那个勾选框
    /// 是个没有任何作用的控件（用户关掉了同步，它照跑）。
    /// `Option`：缺省 = **不改**（编辑账户时保留原值），与口令/PEM 同一套语义。
    #[serde(default)]
    pub enabled: Option<bool>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveConflictCmd {
    pub id: i64,
    /// keep_both | use_local | use_remote | dismiss
    pub action: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrefsCmd {
    pub key: String,
    pub value: serde_json::Value,
}

/// 备份/恢复用：路径可省略，省略时由本地核心决定位置。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PathCmd {
    #[serde(default)]
    pub path: Option<String>,
}

/// 不可撤销操作的确认闸门。
///
/// 刻意用 `deny_unknown_fields`：`多传一个字段`（比如手滑传了 `path`）就让整条命令
/// 按 `bad_args` 拒掉，而不是"因为只读 confirmed 所以别的字段随它去" ——
/// 那会让将来加参数时静默失效。
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfirmCmd {
    /// 必须显式为真。缺失时 serde 会因 `bool` 无默认值而报 `bad_args` ——
    /// 也就是"没确认"不是默认值。
    pub confirmed: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportCmd {
    #[serde(default)]
    pub folder_ids: Vec<String>,
    #[serde(default)]
    pub include_attachments: bool,
    #[serde(default)]
    pub include_trash: bool,
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportCmd {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub mode: Option<String>,
}

fn default_limit() -> u32 {
    200
}

// ---------------------------------------------------------------- 错误 ---

/// UI 只认识 code 与可选细节；协议术语在这里被挡死。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CmdError {
    pub code: String,
    pub message_key: String,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<serde_json::Value>,
}

impl CmdError {
    /// `pub(crate)`：整个 host（含 cli 走不到的内部路径）共用同一套错误码，
    /// 但**不外泄**给壳层 —— 壳层只看见 `CmdError` 这个值类型。
    pub(crate) fn of(code: &str, retryable: bool) -> Self {
        Self {
            code: code.into(),
            message_key: format!("cmd.{code}"),
            retryable,
            detail: None,
        }
    }
    pub(crate) fn with(mut self, d: serde_json::Value) -> Self {
        self.detail = Some(d);
        self
    }
}

/// 出参序列化：所有命令走同一个出口，序列化失败也折成错误码而不是 panic。
pub(crate) fn j<T: Serialize>(v: T) -> R<serde_json::Value> {
    serde_json::to_value(v).map_err(|e| {
        CmdError::of("serialize", false).with(serde_json::json!({ "why": e.to_string() }))
    })
}

impl From<notera_store::StoreError> for CmdError {
    fn from(e: notera_store::StoreError) -> Self {
        use notera_store::StoreError as E;
        // 每个变体一个码：UI 要能区分"我改晚了"和"库被新版程序写过"。
        match e {
            E::NotFound { kind, id } => Self::of("not_found", false).with(serde_json::json!({
                "kind": format!("{kind:?}"),
                "id": id.to_string(),
            })),
            E::StaleEdit(s) => Self::of("stale_edit", false).with(serde_json::json!({
                "expected": s.expected.get(),
                "actual": s.actual.get(),
            })),
            // ADR-0012：库版本过新 → 只读，绝不降级写回
            E::ReadOnly { db, supported } => Self::of("db_too_new", false)
                .with(serde_json::json!({ "db": db, "supported": supported })),
            // 迁移失败不是"版本过新"，混在一起会让人去查错方向
            E::Migration { from, to, detail } => Self::of("db_migration", false)
                .with(serde_json::json!({ "from": from, "to": to, "detail": detail })),
            // 文档格式超前 → 该笔记只读（I7），与整库只读是两回事
            E::DocTooNew { doc, supported } => Self::of("doc_too_new", false)
                .with(serde_json::json!({ "doc": doc, "supported": supported })),
            E::Constraint(c) => Self::of("constraint", false).with(serde_json::json!({ "why": c })),
            // I6 闸门拒绝：本地写不进去 = 我们生成的内容不合法，不是存储坏了
            E::InvalidDoc(m) => {
                Self::of("invalid_doc", false).with(serde_json::json!({ "why": m }))
            }
            E::Rejected(m) => Self::of("rejected", false).with(serde_json::json!({ "why": m })),
            E::Rich(m) => Self::of("richtext", false).with(serde_json::json!({ "why": m })),
            E::Io(e) => Self::of("io", true).with(serde_json::json!({ "why": e.to_string() })),
            E::Identity(e) => {
                Self::of("bad_id", false).with(serde_json::json!({ "why": e.to_string() }))
            }
            E::Sql(e) => {
                Self::of("storage", false).with(serde_json::json!({ "why": e.to_string() }))
            }
        }
    }
}

type R<T> = Result<T, CmdError>;

fn id(s: &str) -> R<EntityId> {
    EntityId::parse(s).map_err(|_| CmdError::of("bad_id", false))
}

// ---------------------------------------------------------------- 分发 ---

/// 命令名 → 处理器。Tauri 侧一个 `invoke` 转发到这里，dev 侧 HTTP 也走这里。
/// ADR-0012 的只读闸门下必须先挡的 command。
///
/// 这里**只列不经过 `Store::write_tx` 的那些**：改数据目录里的文件（附件字节、导入、
/// 恢复、清除、备份）以及账户配置 —— 它们的 config.json 一半落了、库那一半才会失败，
/// 所以不能留给库层闸门去拒。库写不用列：`write_tx` 是全库唯一写入口，闸门就在那一处。
const READ_ONLY_REFUSED: [&str; 10] = [
    "attach_file",
    "import_data",
    "import_files",
    "restore_db",
    "erase_all_data",
    "backup_db",
    "sync_now",
    "configure_account",
    "remove_account",
    // 确认一份可疑清单是一次**写动作**（放行之后就要应用远端），只读闸门下不放行。
    "accept_divergence",
];

pub fn dispatch(app: &App, name: &str, args: serde_json::Value) -> R<serde_json::Value> {
    if READ_ONLY_REFUSED.contains(&name) {
        if let Some((db, supported)) = app.store().library_read_only() {
            return Err(CmdError::of("db_too_new", false)
                .with(serde_json::json!({ "db": db, "supported": supported })));
        }
    }
    match name {
        "create_note" => {
            let c: CreateNoteCmd =
                serde_json::from_value(args).map_err(|_| CmdError::of("bad_args", false))?;
            let fid = match c.folder_id.as_deref() {
                Some(s) => id(s)?,
                None => app.default_folder_id()?,
            };
            j(app.create_note(&fid, c.doc)?)
        }
        "daily_note" => j(app.daily_note()?),
        "edit_note" => {
            let c: EditNoteCmd =
                serde_json::from_value(args).map_err(|_| CmdError::of("bad_args", false))?;
            j(app.edit_note(&id(&c.id)?, c.doc, Rev(c.expected_rev))?)
        }
        "get_note" => {
            let c: IdCmd =
                serde_json::from_value(args).map_err(|_| CmdError::of("bad_args", false))?;
            j(app.get_note(&id(&c.id)?)?)
        }
        "list_notes" => {
            let c: ListNotesCmd =
                serde_json::from_value(args).map_err(|_| CmdError::of("bad_args", false))?;
            j(app.list_notes(c)?)
        }
        "delete_note" => {
            let c: IdCmd =
                serde_json::from_value(args).map_err(|_| CmdError::of("bad_args", false))?;
            app.store().delete_note(&id(&c.id)?)?;
            j(serde_json::Value::Null)
        }
        "restore_note" => {
            let c: IdCmd =
                serde_json::from_value(args).map_err(|_| CmdError::of("bad_args", false))?;
            app.store().restore_note(&id(&c.id)?)?;
            j(serde_json::Value::Null)
        }
        "purge_note" => {
            let c: IdCmd =
                serde_json::from_value(args).map_err(|_| CmdError::of("bad_args", false))?;
            app.store().purge_note(&id(&c.id)?)?;
            j(serde_json::Value::Null)
        }
        "set_note_folder" => {
            let c: MoveNoteCmd =
                serde_json::from_value(args).map_err(|_| CmdError::of("bad_args", false))?;
            j(app.to_dto(
                app.store()
                    .set_note_folder(&id(&c.id)?, &id(&c.folder_id)?)?,
            )?)
        }
        "set_note_pinned" => {
            let c: PinCmd =
                serde_json::from_value(args).map_err(|_| CmdError::of("bad_args", false))?;
            j(app.to_dto(app.store().set_note_pinned(&id(&c.id)?, c.pinned)?)?)
        }
        "create_folder" => {
            let c: CreateFolderCmd =
                serde_json::from_value(args).map_err(|_| CmdError::of("bad_args", false))?;
            // 口径落定（2026-10-09 拍板）：文件夹**只允许一层**，而且要在核心强制 ——
            // 以前"只有一层"只是文档里的一句话，底层照收嵌套，于是侧栏 IA 与 §6-10 那颗
            // 「移动到文件夹」按钮都没法画（画了就等于替产品默认了另一套结构）。
            // 只拒**用户这一侧的动作**：对面同步回来的嵌套走 `apply_remote`，不在这里，
            // 因为把远程那侧一起收紧等于偷偷改同步协议能接受的输入（那是另一件事、另一次决定）。
            if c.parent_id.is_some() {
                return Err(CmdError::of("folder_nested", false));
            }
            j(app.to_folder_dto(app.store().create_folder(
                c.parent_id.as_deref().map(id).transpose()?.as_ref(),
                &c.name,
            )?)?)
        }
        "rename_folder" => {
            let c: RenameFolderCmd =
                serde_json::from_value(args).map_err(|_| CmdError::of("bad_args", false))?;
            j(app.to_folder_dto(app.store().rename_folder(&id(&c.id)?, &c.name)?)?)
        }
        "move_folder" => {
            let c: MoveFolderCmd =
                serde_json::from_value(args).map_err(|_| CmdError::of("bad_args", false))?;
            // 同一个口径的第二道门：移动到**别的文件夹里**就是造出第二层，一并拒。
            // `parent_id: null`（搬到最外层）仍然允许 —— 那是收拢，不是加深。
            if c.parent_id.is_some() {
                return Err(CmdError::of("folder_nested", false));
            }
            j(app.to_folder_dto(app.store().move_folder(
                &id(&c.id)?,
                c.parent_id.as_deref().map(id).transpose()?.as_ref(),
            )?)?)
        }
        "delete_folder" => {
            let c: IdCmd =
                serde_json::from_value(args).map_err(|_| CmdError::of("bad_args", false))?;
            app.store().delete_folder(&id(&c.id)?)?;
            j(serde_json::Value::Null)
        }
        "list_folders" => j(app.list_folders()?),
        "search" => {
            let c: SearchCmd =
                serde_json::from_value(args).map_err(|_| CmdError::of("bad_args", false))?;
            j(app.search(c)?)
        }
        "attach_file" => {
            let c: AttachCmd =
                serde_json::from_value(args).map_err(|_| CmdError::of("bad_args", false))?;
            j(app.attach(c)?)
        }
        "attachment_data" => {
            let c: AttachmentDataCmd =
                serde_json::from_value(args).map_err(|_| CmdError::of("bad_args", false))?;
            j(app.attachment_data(&c.sha256)?)
        }
        // 编辑器读侧的"这台设备有没有这份字节"。批量、只读、不下载任何字节 ——
        // 一颗芯片的显示判据不该触发一次 32 MiB 的读盘。
        "attachment_states" => {
            let c: AttachmentShasCmd =
                serde_json::from_value(args).map_err(|_| CmdError::of("bad_args", false))?;
            j(app.attachment_states(&c.shas)?)
        }
        "attachment_inventory" => j(app.attachment_inventory()?),
        // §6「版本历史浏览」：列表与单版正文都是只读 —— 覆盖动作走已有的 `edit_note`
        // （CAS + 唯一写出口），不在这里另开一条写路径。
        "note_revisions" => {
            let c: IdCmd =
                serde_json::from_value(args).map_err(|_| CmdError::of("bad_args", false))?;
            j(app.note_revisions(&c.id)?)
        }
        "note_revision" => {
            let c: NoteRevisionCmd =
                serde_json::from_value(args).map_err(|_| CmdError::of("bad_args", false))?;
            j(app.note_revision(&c.id, c.rev)?)
        }
        // 用户在坏图占位上点「重试取回」。为什么是一条命令而不是后台自己再试一次：
        // 后台对 `absent`/`error` 收手是**刻意的**（§27/§28 那两条保证句要的就是不每 20 s 空转），
        // 而收手的代价是那一格永远不会自愈。重开它的凭据只能是用户的一次意图。
        "attachment_retry" => {
            let c: AttachmentShaCmd =
                serde_json::from_value(args).map_err(|_| CmdError::of("bad_args", false))?;
            j(app.retry_attachment(&c.sha256)?)
        }
        // 同一格的动作面另一半：本机有好字节、服务器那份被证明坏了 → 授权一次覆盖式重传。
        // 注意它**不是**在这里做网络（dispatch 的处理器不许 await 网络，见本文件开头）：
        // 它只把意图落到账上（remote_state='error'），由常驻附件轮去做那次覆盖。
        "attachment_reupload" => {
            let c: AttachmentShaCmd =
                serde_json::from_value(args).map_err(|_| CmdError::of("bad_args", false))?;
            j(app.reupload_attachment(&c.sha256)?)
        }
        // 冲突面板的并排预览：前端一直在调这条，而核心没有 —— 于是它每次都是
        // unknown_command，面板安静地退回卡片摘要（用户以为看到的就是那一版）。
        "preview_text" => {
            let c: PreviewTextCmd =
                serde_json::from_value(args).map_err(|_| CmdError::of("bad_args", false))?;
            j(app.preview_text(&c.id, c.rev)?)
        }
        "stats" => j(app.stats()?),
        "sync_now" => {
            app.request_sync();
            j(serde_json::Value::Null)
        }
        "sync_status" => j(app.sync_status()?),
        // §4.3 的"人工确认"就是这一发：把**这一版**清单记成本机已接受，然后要走一轮就走。
        // 没有停下过的东西没什么可确认的 —— 那时它是空操作，不是一门假开关。
        "accept_divergence" => {
            app.accept_divergence()?;
            j(serde_json::Value::Null)
        }
        "account" => j(app.current_account()?),
        "configure_account" => {
            let c: AccountDraftCmd =
                serde_json::from_value(args).map_err(|_| CmdError::of("bad_args", false))?;
            j(app.configure_account(c)?)
        }
        "remove_account" => {
            let c: IdCmd =
                serde_json::from_value(args).map_err(|_| CmdError::of("bad_args", false))?;
            app.remove_account(&c.id)?;
            j(serde_json::Value::Null)
        }
        "open_conflicts" => j(app.open_conflicts()?),
        "resolve_conflict" => {
            let c: ResolveConflictCmd =
                serde_json::from_value(args).map_err(|_| CmdError::of("bad_args", false))?;
            app.resolve_conflict(c)?;
            j(serde_json::Value::Null)
        }
        "set_pref" => {
            let c: PrefsCmd =
                serde_json::from_value(args).map_err(|_| CmdError::of("bad_args", false))?;
            app.set_pref(&c.key, c.value)?;
            j(serde_json::Value::Null)
        }
        "get_prefs" => j(app.get_prefs()?),
        "platform_caps" => j(app.platform_caps()),
        // 前端把整份请求包在 `req` 里（沿用旧契约），这里剥一层再反序列化。
        "export_data" => {
            let body = args.get("req").cloned().unwrap_or(args);
            let c: ExportCmd =
                serde_json::from_value(body).map_err(|_| CmdError::of("bad_args", false))?;
            j(app.export_data(c)?)
        }
        "import_files" => {
            let body = args.get("req").cloned().unwrap_or(args);
            let paths: Vec<String> = serde_json::from_value(
                body.get("paths")
                    .cloned()
                    .unwrap_or_else(|| serde_json::Value::Array(Vec::new())),
            )
            .map_err(|_| CmdError::of("bad_args", false))?;
            j(app.import_files(&paths)?)
        }
        "import_data" => {
            let body = args.get("req").cloned().unwrap_or(args);
            let c: ImportCmd =
                serde_json::from_value(body).map_err(|_| CmdError::of("bad_args", false))?;
            j(app.import_data(c)?)
        }
        "backup_db" => {
            let c: PathCmd =
                serde_json::from_value(args).map_err(|_| CmdError::of("bad_args", false))?;
            j(app.backup_db(c.path.as_ref().map(std::path::Path::new))?)
        }
        "list_backups" => j(app.list_backups()?),
        "erase_all_data" => {
            let c: ConfirmCmd =
                serde_json::from_value(args).map_err(|_| CmdError::of("bad_args", false))?;
            j(app.erase_all_data(c.confirmed)?)
        }
        "restore_db" => {
            let c: PathCmd =
                serde_json::from_value(args).map_err(|_| CmdError::of("bad_args", false))?;
            let path = c.path.ok_or_else(|| CmdError::of("bad_args", false))?;
            j(app.stage_restore(std::path::Path::new(&path))?)
        }
        other => {
            Err(CmdError::of("unknown_command", false).with(serde_json::json!({ "name": other })))
        }
    }
}

// 让编译器盯住这些类型确实被用到（也作为"契约字段没漂"的哨兵）
// 元组里的类型再"复杂"也是被测对象本身：抽成别名就等于用被检查的写法去检查它。
#[allow(clippy::type_complexity)]
const _: fn() -> (
    NoteDto,
    NoteListDto,
    FolderDto,
    SearchHitDto,
    ConflictDto,
    AccountDto,
    SyncStatusDto,
    StoreStats,
    NoteQuery,
    SearchQuery,
    ConflictRow,
) = || unreachable!();
