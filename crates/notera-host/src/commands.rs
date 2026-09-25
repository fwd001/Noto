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
    pub bypass: Vec<String>,
    pub enabled: bool,
    /// 永不下发明文，只告诉 UI 有没有存过
    pub has_credential: bool,
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
}

// ---------------------------------------------------------------- DTO 入参 ---

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateNoteCmd {
    pub folder_id: String,
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
    /// 已落到应用数据目录内的相对路径（由文件选择器复制进来）
    pub local_path: String,
    pub media_type: String,
    #[serde(default)]
    pub filename: Option<String>,
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
    fn of(code: &str, retryable: bool) -> Self {
        Self { code: code.into(), message_key: format!("cmd.{code}"), retryable, detail: None }
    }
    fn with(mut self, d: serde_json::Value) -> Self {
        self.detail = Some(d);
        self
    }
}

impl From<notera_store::StoreError> for CmdError {
    fn from(e: notera_store::StoreError) -> Self {
        use notera_store::StoreError as E;
        match e {
            E::NotFound => Self::of("not_found", false),
            E::StaleEdit(s) => Self::of("stale_edit", false).with(serde_json::json!({
                "expected": s.expected.get(),
                "actual": s.actual.get(),
            })),
            E::ReadOnly | E::Migration(_) => Self::of("db_too_new", false),
            E::Constraint(c) => Self::of("constraint", false).with(serde_json::json!({ "why": c })),
            other => Self::of("storage", false).with(serde_json::json!({ "why": other.to_string() })),
        }
    }
}

type R<T> = Result<T, CmdError>;

fn id(s: &str) -> R<EntityId> {
    EntityId::parse(s).map_err(|_| CmdError::of("bad_id", false))
}

// ---------------------------------------------------------------- 分发 ---

/// 命令名 → 处理器。Tauri 侧一个 `invoke` 转发到这里，dev 侧 HTTP 也走这里。
pub fn dispatch(app: &App, name: &str, args: serde_json::Value) -> R<serde_json::Value> {
    let j = |v: impl Serialize| serde_json::to_value(v).map_err(|e| CmdError::of("serialize", false).with(serde_json::json!({ "why": e.to_string() })));

    match name {
        "create_note" => {
            let c: CreateNoteCmd = serde_json::from_value(args).map_err(|_| CmdError::of("bad_args", false))?;
            j(app.create_note(&id(&c.folder_id)?, c.doc)?)
        }
        "edit_note" => {
            let c: EditNoteCmd = serde_json::from_value(args).map_err(|_| CmdError::of("bad_args", false))?;
            j(app.edit_note(&id(&c.id)?, c.doc, Rev(c.expected_rev))?)
        }
        "get_note" => {
            let c: IdCmd = serde_json::from_value(args).map_err(|_| CmdError::of("bad_args", false))?;
            j(app.get_note(&id(&c.id)?)?)
        }
        "list_notes" => {
            let c: ListNotesCmd = serde_json::from_value(args).map_err(|_| CmdError::of("bad_args", false))?;
            j(app.list_notes(c)?)
        }
        "delete_note" => {
            let c: IdCmd = serde_json::from_value(args).map_err(|_| CmdError::of("bad_args", false))?;
            app.store().delete_note(&id(&c.id)?)?;
            j(serde_json::Value::Null)
        }
        "restore_note" => {
            let c: IdCmd = serde_json::from_value(args).map_err(|_| CmdError::of("bad_args", false))?;
            app.store().restore_note(&id(&c.id)?)?;
            j(serde_json::Value::Null)
        }
        "purge_note" => {
            let c: IdCmd = serde_json::from_value(args).map_err(|_| CmdError::of("bad_args", false))?;
            app.store().purge_note(&id(&c.id)?)?;
            j(serde_json::Value::Null)
        }
        "set_note_folder" => {
            let c: MoveNoteCmd = serde_json::from_value(args).map_err(|_| CmdError::of("bad_args", false))?;
            j(app.to_dto(app.store().set_note_folder(&id(&c.id)?, &id(&c.folder_id)?)?))
        }
        "set_note_pinned" => {
            let c: PinCmd = serde_json::from_value(args).map_err(|_| CmdError::of("bad_args", false))?;
            j(app.to_dto(app.store().set_note_pinned(&id(&c.id)?, c.pinned)?))
        }
        "create_folder" => {
            let c: CreateFolderCmd = serde_json::from_value(args).map_err(|_| CmdError::of("bad_args", false))?;
            j(app.to_folder_dto(app.store().create_folder(c.parent_id.as_deref().map(id).transpose()?.as_ref(), &c.name)?)?)
        }
        "rename_folder" => {
            let c: RenameFolderCmd = serde_json::from_value(args).map_err(|_| CmdError::of("bad_args", false))?;
            j(app.to_folder_dto(app.store().rename_folder(&id(&c.id)?, &c.name)?)?)
        }
        "move_folder" => {
            let c: MoveFolderCmd = serde_json::from_value(args).map_err(|_| CmdError::of("bad_args", false))?;
            j(app.to_folder_dto(
                app.store().move_folder(&id(&c.id)?, c.parent_id.as_deref().map(id).transpose()?.as_ref())?,
            ))
        }
        "delete_folder" => {
            let c: IdCmd = serde_json::from_value(args).map_err(|_| CmdError::of("bad_args", false))?;
            app.store().delete_folder(&id(&c.id)?)?;
            j(serde_json::Value::Null)
        }
        "list_folders" => j(app.list_folders()?),
        "search" => {
            let c: SearchCmd = serde_json::from_value(args).map_err(|_| CmdError::of("bad_args", false))?;
            j(app.search(c)?)
        }
        "attach_file" => {
            let c: AttachCmd = serde_json::from_value(args).map_err(|_| CmdError::of("bad_args", false))?;
            j(app.attach(c)?)
        }
        "stats" => j(app.stats()?),
        "sync_now" => {
            app.request_sync();
            j(serde_json::Value::Null)
        }
        "sync_status" => j(app.sync_status()?),
        "account" => j(app.current_account()?),
        "configure_account" => {
            let c: AccountDraftCmd = serde_json::from_value(args).map_err(|_| CmdError::of("bad_args", false))?;
            j(app.configure_account(c)?)
        }
        "remove_account" => {
            let c: IdCmd = serde_json::from_value(args).map_err(|_| CmdError::of("bad_args", false))?;
            app.remove_account(&c.id)?;
            j(serde_json::Value::Null)
        }
        "open_conflicts" => j(app.open_conflicts()?),
        "resolve_conflict" => {
            let c: ResolveConflictCmd = serde_json::from_value(args).map_err(|_| CmdError::of("bad_args", false))?;
            app.resolve_conflict(c)?;
            j(serde_json::Value::Null)
        }
        "set_pref" => {
            let c: PrefsCmd = serde_json::from_value(args).map_err(|_| CmdError::of("bad_args", false))?;
            app.set_pref(&c.key, c.value)?;
            j(serde_json::Value::Null)
        }
        "get_prefs" => j(app.get_prefs()?),
        "platform_caps" => j(app.platform_caps()),
        other => Err(CmdError::of("unknown_command", false).with(serde_json::json!({ "name": other }))),
    }
}

// 让编译器盯住这些类型确实被用到（也作为"契约字段没漂"的哨兵）
const _: fn() -> (NoteDto, NoteListDto, FolderDto, SearchHitDto, ConflictDto, AccountDto, SyncStatusDto, StoreStats, NoteQuery, SearchQuery, ConflictRow) =
    || unreachable!();
