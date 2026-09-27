//! notera-host —— 组装根、用例编排、调度与事件总线。
//!
//! 启动铁律（docs/PLATFORM.md §3，规范而非建议）：
//! ```text
//! 开库 → 迁移闸门 → 读最近列表 → 首帧可输入 → 后台才启同步引擎
//! ```
//! 首帧路径上不存在任何网络等待。本 crate 因此把 `start_sync()` 设计成
//! 必须在 `App::boot()` 返回**之后**才调用的独立步骤，类型上就分不开。

pub mod commands;

pub mod devserver;
/// 平台原生物的计划表（菜单项 / 通知判定）—— 纯逻辑，壳只负责摆上去。
pub mod platform;

use commands::{
    AccountDraftCmd, AccountDto, CmdError, ConflictDto, ExportCmd, FolderDto, ImportCmd,
    ListNotesCmd, NoteDto, NoteListDto, SearchCmd, SearchHitDto, StatsDto, SyncStatusDto,
};
use notera_config::{
    AccountConfig, AppConfig, ConfigRepository, ProxyMode, ProxyProfile, TlsPolicyKind,
};
use notera_core::{Clock, DeviceId, EntityId, EntityKind, Rev, SystemClock, Timestamp};
use notera_store::{
    ApplyOp as StoreApplyOp, ConflictRow, Folder, Note, NoteListRow, NoteQuery, SearchPath, Store,
    StoreError,
};
use notera_sync::plan::{Decision, LocalView, RemoteView};
use notera_sync::{ApplyOp, EngineConfig, LocalError, LocalPort, Phase, RoundStats, SyncEvent};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// §11.4 租约的有效期。桌面轮次间隔 25s（§14），所以正常在线时一直在续；
/// 设备崩溃后这份租约最多挡住别人 60s，不需要任何清理进程。
const LEASE_TTL_MS: i64 = 60_000;

/// 单个附件的上限。锚在 SYNC-PROTOCOL §13 的"一轮 ≤ 4 个文件 / ≤ 64 MiB"：
/// 一个文件就超过整轮预算的话，放进去只会反复上传失败，用户看到的是"永远同步不完"，
/// 所以在入口就挡并说清楚，而不是让它进来慢慢坏。
const MAX_ATTACHMENT_BYTES: u64 = 32 * 1024 * 1024;

/// UI 能看到的同步语义，一共四态。任何协议细节都必须先折进这四个值。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Badge {
    Synced,
    Syncing,
    Offline,
    Failed,
}

/// 事件总线上的一条。前端只订阅这个枚举。
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum BusEvent {
    #[serde(rename_all = "camelCase")]
    Sync {
        badge: Badge,
        progress: Option<Progress>,
        error_code: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    NotesChanged { ids: Vec<String> },
    #[serde(rename_all = "camelCase")]
    Conflict {
        conflict_id: i64,
        note_title: String,
    },
    #[serde(rename_all = "camelCase")]
    Toast { message_key: String, level: String },
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub done: u32,
    pub total: u32,
    pub bytes: u64,
}

/// 只有这两项是"构建期不确定、运行期才知道成没成"的：托盘要系统接受图标，
/// 全局快捷键要和别的应用抢键位（可能被占）。其余能力在编译期就定死了。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeCap {
    Tray,
    GlobalShortcuts,
}

/// 平台能力声明。UI 按能力渲染，**禁止**按机型分支（ADR-0011）。
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformCaps {
    pub tray: bool,
    pub global_shortcuts: bool,
    pub native_menu: bool,
    pub notifications: bool,
    pub share_sheet: bool,
    pub background_task: String,
    pub keychain: String,
    pub file_picker: String,
    pub safe_area: bool,
}

impl PlatformCaps {
    /// **按已实现的能力**回答，不是按"这个平台原则上能做到什么"。
    ///
    /// PLATFORM.md §3 那张表是目标态；本结构体是 as-built，UI 拿它决定要不要把开关
    /// 摆出来。之前这里对 Windows 报 `tray/global_shortcuts/native_menu = true`，
    /// 于是设置页摆出"关闭窗口时留在系统托盘"，而壳里一行托盘代码都没有 ——
    /// 用户勾完得到一个存了却没人读的偏好，比看不到这个选项更糟。
    ///
    /// as-built 现状（2026-09-27）：**原生菜单、系统通知、托盘、全局快捷键都已真的接上**
    /// （`src-tauri/src/lib.rs` 的 `attach_menu` / `attach_tray` /
    /// `register_global_shortcuts` + 事件泵里的 `notice_for`），但**后面两项不在这里报
    /// true**：托盘与全局快捷键是运行时才知道成没成的（系统不让挂、键位被别的应用占了），
    /// 由壳在注册成功之后经 `report_native_cap` 写回。这里若按平台直接报 true，就又是
    /// 那次"设置页摆出一组按了没反应的组合键"的假声明。
    /// 仍然报 false 的两类各欠一件事：钥匙串要 `credential_ref` 落地（Phase 5）；
    /// 移动端后台任务要各平台的后台执行权限。这两项都会动依赖图，按 §9 走评审。
    pub fn for_current_target() -> Self {
        if cfg!(target_os = "windows") {
            Self {
                tray: false,
                global_shortcuts: false,
                native_menu: true,
                notifications: true,
                share_sheet: false,
                background_task: "desktop_timer".into(),
                keychain: "none".into(),
                file_picker: "native".into(),
                safe_area: false,
            }
        } else if cfg!(target_os = "macos") {
            Self {
                tray: false,
                global_shortcuts: false,
                native_menu: true,
                notifications: true,
                share_sheet: true,
                background_task: "desktop_timer".into(),
                keychain: "none".into(),
                file_picker: "native".into(),
                safe_area: false,
            }
        } else if cfg!(target_os = "android") {
            Self {
                tray: false,
                global_shortcuts: false,
                native_menu: false,
                notifications: false,
                share_sheet: true,
                background_task: "workmanager".into(),
                keychain: "none".into(),
                file_picker: "saf".into(),
                safe_area: true,
            }
        } else if cfg!(target_os = "ios") {
            Self {
                tray: false,
                global_shortcuts: false,
                native_menu: false,
                notifications: false,
                share_sheet: true,
                background_task: "bgapprefresh".into(),
                keychain: "none".into(),
                file_picker: "document_picker".into(),
                safe_area: true,
            }
        } else {
            Self {
                tray: false,
                global_shortcuts: false,
                native_menu: true,
                notifications: true,
                share_sheet: false,
                background_task: "desktop_timer".into(),
                keychain: "none".into(),
                file_picker: "web".into(),
                safe_area: false,
            }
        }
    }
}

/// 同步引擎可见状态（host 侧维护，供 `sync_status` 查询）。
///
/// 刻意**不** derive `Default`：`Phase`/`Badge` 的"零值"必须是明说的
/// `Unconfigured`/`Offline`，不能让 `Default` 悄悄给出 `Synced` 这类谎话。
#[derive(Clone, Debug)]
struct SyncView {
    phase: Phase,
    badge: Badge,
    last_success_at: Option<String>,
    message_key: Option<String>,
    retryable: bool,
    in_flight: bool,
}

impl SyncView {
    /// 首帧的诚实起点：没配过账户、也没联网。
    fn initial() -> Self {
        SyncView {
            phase: Phase::Unconfigured,
            badge: Badge::Offline,
            last_success_at: None,
            message_key: None,
            retryable: false,
            in_flight: false,
        }
    }
}

#[derive(Clone)]
pub struct App {
    inner: Arc<Inner>,
}

struct Inner {
    store: Store,
    data_dir: PathBuf,
    config_repo: ConfigRepository,
    config: Mutex<AppConfig>,
    caps: Mutex<PlatformCaps>,
    bus: Mutex<Vec<BusEvent>>,
    subs: Mutex<Vec<std::sync::mpsc::Sender<BusEvent>>>,
    sync_view: Mutex<SyncView>,
    cached_manifest: Mutex<Option<Vec<u8>>>,
    /// 上一轮提交后服务器给的清单 etag —— 下一轮带它才可能拿到 304（§6.3 空轮 0 字节）
    manifest_etag: Mutex<Option<String>>,
    seq_applied: AtomicU64,
    syncing: AtomicBool,
    dirty_ticks: AtomicU64,
}

impl App {
    /// 步骤 1–4：开库、迁移、读最近数据、准备出首帧。**不做任何网络动作。**
    pub fn boot(data_dir: &Path) -> Result<App, BootError> {
        std::fs::create_dir_all(data_dir).map_err(|e| BootError::Io(e.to_string()))?;
        let repo = ConfigRepository::new(data_dir);
        let mut config = repo.load().map_err(|e| BootError::Config(e.to_string()))?;
        // 新生成的 device_id 必须同时进**内存里的**配置：只写文件不写内存的话，
        // 本次会话里 `config().device_id` 是空串，而它是要进记录信封 `device` 字段的
        // （SYNC-PROTOCOL §3 要求每条记录都带设备 UUID）——空值会让另一台设备无法判定来源。
        let device_id = if config.device_id.is_empty() {
            DeviceId::default()
        } else {
            DeviceId::parse(&config.device_id).map_err(|e| BootError::Config(e.to_string()))?
        };
        if config.device_id.is_empty() {
            config.device_id = device_id.to_string();
            repo.save(&config)
                .map_err(|e| BootError::Config(e.to_string()))?;
        }
        let store = Store::open(data_dir, device_id).map_err(BootError::Store)?;
        let violations = store.startup_violations().to_vec();
        let app = App {
            inner: Arc::new(Inner {
                store,
                data_dir: data_dir.to_path_buf(),
                config_repo: repo,
                config: Mutex::new(config),
                caps: Mutex::new(PlatformCaps::for_current_target()),
                bus: Mutex::new(Vec::new()),
                subs: Mutex::new(Vec::new()),
                sync_view: Mutex::new(SyncView::initial()),
                cached_manifest: Mutex::new(None),
                manifest_etag: Mutex::new(None),
                seq_applied: AtomicU64::new(0),
                syncing: AtomicBool::new(false),
                dirty_ticks: AtomicU64::new(0),
            }),
        };
        // 启动自检结论必须可见：不静默修，也不静默忽略
        for v in &violations {
            app.emit(BusEvent::Toast {
                message_key: format!("verify.{}", v.id),
                level: "warn".into(),
            });
        }
        Ok(app)
    }

    pub fn store(&self) -> &Store {
        &self.inner.store
    }
    pub fn data_dir(&self) -> &Path {
        &self.inner.data_dir
    }
    pub fn platform_caps(&self) -> PlatformCaps {
        self.inner.caps.lock().unwrap().clone()
    }

    /// 壳**真的**把某项原生能力挂上了，才允许这里改口。
    ///
    /// 为什么不直接让 `for_current_target()` 报 true：那正是此前两次假声明的形状 ——
    /// 平台原则上能做到 ≠ 这个构建做到了（macOS/Linux 曾报 `global_shortcuts: true`
    /// 而壳里一个注册都没有，设置页于是摆出一组按了没反应的键）。注册成功与否是
    /// 运行时事实，就由运行时写；失败时留在 false，界面自动退回"这台设备不支持"。
    pub fn report_native_cap(&self, cap: NativeCap, ok: bool) {
        let mut c = self.inner.caps.lock().unwrap();
        match cap {
            NativeCap::Tray => c.tray = ok,
            NativeCap::GlobalShortcuts => c.global_shortcuts = ok,
        }
    }
    pub fn config(&self) -> AppConfig {
        self.inner.config.lock().unwrap().clone()
    }

    // ------------------------------------------------------------ 事件总线 ---

    pub fn subscribe(&self) -> std::sync::mpsc::Receiver<BusEvent> {
        let (tx, rx) = std::sync::mpsc::channel();
        self.inner.subs.lock().unwrap().push(tx);
        rx
    }

    pub fn emit(&self, e: BusEvent) {
        self.inner.bus.lock().unwrap().push(e.clone());
        let mut dead = Vec::new();
        for (i, tx) in self.inner.subs.lock().unwrap().iter().enumerate() {
            if tx.send(e.clone()).is_err() {
                dead.push(i);
            }
        }
        if !dead.is_empty() {
            let mut subs = self.inner.subs.lock().unwrap();
            for i in dead.into_iter().rev() {
                subs.remove(i);
            }
        }
    }

    fn set_sync(&self, f: impl FnOnce(&mut SyncView)) {
        {
            let mut v = self.inner.sync_view.lock().unwrap();
            f(&mut v);
        }
        let (badge, key) = {
            let v = self.inner.sync_view.lock().unwrap();
            (v.badge, v.message_key.clone())
        };
        self.emit(BusEvent::Sync {
            badge,
            progress: None,
            error_code: key,
        });
    }

    // -------------------------------------------------------------- 用例 ---

    pub fn create_note(
        &self,
        folder_id: &EntityId,
        doc: serde_json::Value,
    ) -> Result<NoteDto, CmdError> {
        let n = self.inner.store.create_note(folder_id, doc)?;
        self.note_saved(&n);
        self.to_dto(n)
    }

    /// 没选文件夹时的落点：默认本。规则放在核心，UI 不猜。
    pub fn default_folder_id(&self) -> Result<EntityId, CmdError> {
        self.inner
            .store
            .list_folders()?
            .into_iter()
            .find(|f| f.system_kind.as_deref() == Some("default"))
            .map(|f| f.id)
            .ok_or_else(|| CmdError::of("no_default_folder", false))
    }

    pub fn edit_note(
        &self,
        id: &EntityId,
        doc: serde_json::Value,
        expected: Rev,
    ) -> Result<NoteDto, CmdError> {
        let n = self.inner.store.edit_note(id, doc, expected)?;
        self.note_saved(&n);
        self.to_dto(n)
    }

    /// 本地写入成功 = 同步被"需要跑一轮"标记。**这里绝不碰网络**（I8/P1）。
    fn note_saved(&self, n: &Note) {
        self.inner.dirty_ticks.fetch_add(1, Ordering::SeqCst);
        self.emit(BusEvent::NotesChanged {
            ids: vec![n.id.to_string()],
        });
        self.set_sync(|v| {
            if v.badge == Badge::Synced {
                v.in_flight = false;
            }
        });
    }

    pub fn get_note(&self, id: &EntityId) -> Result<Option<NoteDto>, CmdError> {
        self.inner
            .store
            .get_note(id)?
            .map(|n| self.to_dto(n))
            .transpose()
    }

    pub fn list_notes(&self, c: ListNotesCmd) -> Result<Vec<serde_json::Value>, CmdError> {
        let q = NoteQuery {
            folder: c
                .folder_id
                .as_deref()
                .map(EntityId::parse)
                .transpose()
                .map_err(|_| CmdError::of("bad_id", false))?,
            trash: c.trash,
            limit: c.limit,
            offset: c.offset,
        };
        let rows = self.inner.store.list_notes(&q)?;
        rows.iter().map(|r| self.to_list_dto(r)).collect()
    }

    pub fn list_folders(&self) -> Result<Vec<FolderDto>, CmdError> {
        let all = self.inner.store.list_folders()?;
        let mut counts: BTreeMap<String, u64> = BTreeMap::new();
        for f in &all {
            let n = self
                .inner
                .store
                .list_notes(&NoteQuery {
                    folder: Some(f.id.clone()),
                    trash: false,
                    limit: 0,
                    offset: 0,
                })?
                .len();
            *counts.entry(f.id.to_string()).or_insert(0) = n as u64;
        }
        let flat: Vec<FolderDto> = all
            .iter()
            .map(|f| FolderDto {
                id: f.id.to_string(),
                parent_id: f.parent_id.as_ref().map(|p| p.to_string()),
                name: f.name.clone(),
                color: f.color.clone(),
                system_kind: f.system_kind.clone(),
                note_count: counts.get(&f.id.to_string()).copied().unwrap_or(0),
                children: Vec::new(),
            })
            .collect();
        // 组装树
        fn build(flat: &[FolderDto], parent: Option<&str>) -> Vec<FolderDto> {
            flat.iter()
                .filter(|f| f.parent_id.as_deref() == parent)
                .cloned()
                .map(|mut f| {
                    f.children = build(flat, Some(f.id.as_str()));
                    f
                })
                .collect()
        }
        Ok(build(&flat, None))
    }

    pub fn to_folder_dto(&self, f: Folder) -> Result<FolderDto, CmdError> {
        let count = self
            .inner
            .store
            .list_notes(&NoteQuery {
                folder: Some(f.id.clone()),
                trash: false,
                limit: 0,
                offset: 0,
            })?
            .len();
        Ok(FolderDto {
            id: f.id.to_string(),
            parent_id: f.parent_id.as_ref().map(|p| p.to_string()),
            name: f.name,
            color: f.color,
            system_kind: f.system_kind,
            note_count: count as u64,
            children: Vec::new(),
        })
    }

    /// 单条笔记 → `NoteDto`。字段名以 `apps/desktop/src/api/types.ts` 为准（契约）。
    ///
    /// `plainText` 故意不外发：详情里 UI 用 `doc` 渲染，正文摘要走 `summary`。
    pub fn to_dto(&self, n: Note) -> Result<NoteDto, CmdError> {
        Ok(NoteDto {
            id: n.id.to_string(),
            folder_id: n.folder_id.to_string(),
            doc: n.doc,
            doc_format: n.doc_format,
            title: n.title,
            summary: n.summary,
            char_count: n.char_count,
            block_count: n.block_count,
            has_attachment: n.has_attachment,
            pinned: n.pinned,
            color: n.color,
            rev: n.rev.get(),
            content_hash: n.content_hash,
            created_at: n.created_at,
            updated_at: n.updated_at,
            deleted_at: n.deleted_at,
        })
    }

    /// 列表投影 → JSON（列表路径**不读 `doc`**，DATA-MODEL §13）。
    ///
    /// `folder_name` 直接取存储层 JOIN 出来的那一列，不再为每一行回查文件夹表。
    pub fn to_list_dto(&self, r: &NoteListRow) -> Result<serde_json::Value, CmdError> {
        commands::j(NoteListDto {
            id: r.id.to_string(),
            folder_id: r.folder_id.to_string(),
            folder_name: r.folder_name.clone(),
            title: r.title.clone(),
            summary: r.summary.clone(),
            char_count: r.char_count,
            has_attachment: r.has_attachment,
            pinned: r.pinned,
            updated_at: r.updated_at.clone(),
            deleted_at: r.deleted_at.clone(),
            dirty: r.dirty,
        })
    }

    pub fn search(&self, c: SearchCmd) -> Result<Vec<SearchHitDto>, CmdError> {
        let hits = self.inner.store.search(&notera_store::SearchQuery {
            text: c.text,
            limit: c.limit,
        })?;
        let mut out = Vec::with_capacity(hits.len());
        for h in hits {
            // 短查询走 LIKE 兜底是**实现细节**，但它是排障关键，因此记进日志而不进 UI。
            if matches!(h.path_used, SearchPath::LikeFallback) {
                tracing::debug!("search fallback LIKE for short query: {}", h.note_id);
            }
            let title = self
                .inner
                .store
                .get_note(&h.note_id)?
                .map(|n| n.title)
                .unwrap_or_default();
            out.push(SearchHitDto {
                note_id: h.note_id.to_string(),
                score: h.score,
                snippet_html: h.snippet_html,
                title,
            });
        }
        Ok(out)
    }

    /// 挂一个附件。字节可以来自壳里选的文件，也可以来自前端 `<input type=file>` 读到的
    /// base64 —— 但**两条路都只有核心**算 sha256、落盘、写 `attachments`/`note_attachments`
    /// 并入上传队列。前端只负责"把用户选的东西变成字节"，不负责"存到哪、叫什么、有没有存成"。
    pub fn attach(&self, c: commands::AttachCmd) -> Result<serde_json::Value, CmdError> {
        let bytes = match (c.local_path.as_deref(), c.bytes_base64.as_deref()) {
            (Some(_), Some(_)) | (None, None) => {
                return Err(CmdError::of("bad_args", false).with(serde_json::json!({
                    "detail": "localPath 与 bytesBase64 必须恰好给一个",
                })));
            }
            (Some(p), None) => std::fs::read(p).map_err(|e| {
                CmdError::of("read_failed", false).with(serde_json::json!({ "why": e.to_string() }))
            })?,
            (None, Some(b)) => notera_crypto::b64::decode(b).map_err(|e| {
                CmdError::of("bad_args", false).with(serde_json::json!({ "detail": e.to_string() }))
            })?,
        };
        if bytes.len() as u64 > MAX_ATTACHMENT_BYTES {
            // 单个附件超过同步一轮的预算上限时，先在这里挡住并给出能看懂的话；
            // 让它进去只会在上传阶段反复失败，用户看到的是"永远同步不完"。
            return Err(CmdError::of("too_large", false).with(serde_json::json!({
                "bytes": bytes.len(),
                "limit": MAX_ATTACHMENT_BYTES,
            })));
        }
        let note = EntityId::parse(&c.note_id).map_err(|_| CmdError::of("bad_id", false))?;
        let media = if c.media_type.trim().is_empty() {
            "application/octet-stream".to_string()
        } else {
            c.media_type.clone()
        };
        let a = self
            .inner
            .store
            .attach_blob(&note, &bytes, &media, c.filename.as_deref(), &c.block_id)
            .map_err(CmdError::from)?;
        self.inner.dirty_ticks.fetch_add(1, Ordering::SeqCst);
        // 回上笔记的**新 rev**：`attach_blob` 会改笔记的派生列与引用表，因而把 rev 推进一格。
        // 编辑器若不接住这个数，它随后那次自动保存就带着旧 rev 出发，被核心判成
        // `stale_edit` —— 用户插一张图，却看到"这条笔记在别处被改动了"（端到端实测踩过）。
        let rev = self
            .inner
            .store
            .get_note(&note)
            .map_err(CmdError::from)?
            .map(|n| n.rev.get())
            .unwrap_or(0);
        Ok(
            serde_json::json!({ "sha256": a.sha256, "size": a.size, "mediaType": a.media_type, "rev": rev }),
        )
    }

    /// 读回一个附件的字节（编辑器显示图片用）。
    ///
    /// `sha256` 会被拼进 blob 路径（`<attachments>/<2hex>/<sha>`），所以先校验形态：
    /// 不校验就等于允许 `../../xxx` 这类相对路径去碰文件系统。
    pub fn attachment_data(&self, sha256: &str) -> Result<serde_json::Value, CmdError> {
        // 只认**小写** 64hex：`sha256_hex` 出来的就是这个形态，收大写等于允许同一个
        // blob 有两种拼法、两条路径（Windows 上还不报错）。更关键的是它挡住 `../` 那类
        // 字符串走进 `blob_path` —— 这个参数会被拼进文件路径，不能靠"调用方总不会乱传"。
        if sha256.len() != 64
            || !sha256
                .chars()
                .all(|ch| ch.is_ascii_digit() || ('a'..='f').contains(&ch))
        {
            return Err(CmdError::of("bad_args", false)
                .with(serde_json::json!({ "detail": "sha256 必须是 64 位 hex" })));
        }
        let path = self.inner.store.blob_path(sha256);
        let bytes = std::fs::read(&path).map_err(|_| {
            CmdError::of("attachment_missing", false).with(serde_json::json!({ "sha256": sha256 }))
        })?;
        if bytes.len() as u64 > MAX_ATTACHMENT_BYTES {
            return Err(CmdError::of("too_large", false)
                .with(serde_json::json!({ "bytes": bytes.len(), "limit": MAX_ATTACHMENT_BYTES })));
        }
        let media_type = self
            .inner
            .store
            .attachment_media_type(sha256)
            .map_err(CmdError::from)?
            .unwrap_or_else(|| "application/octet-stream".to_string());
        Ok(serde_json::json!({
            "sha256": sha256,
            "mediaType": media_type,
            "size": bytes.len(),
            "bytesBase64": notera_crypto::b64::encode(&bytes),
        }))
    }

    pub fn stats(&self) -> Result<StatsDto, CmdError> {
        Ok(self.inner.store.stats()?.into())
    }

    pub fn open_conflicts(&self) -> Result<Vec<ConflictDto>, CmdError> {
        let rows = self.inner.store.open_conflicts()?;
        rows.into_iter().map(|r| self.to_conflict_dto(r)).collect()
    }

    /// 冲突卡片要能显示"是哪条笔记"。已被永久删除的实体从 `tombstones.title_snap` 取，
    /// 否则用户看到的是一条说不出在跟谁打架的空白卡片。
    fn to_conflict_dto(&self, r: ConflictRow) -> Result<ConflictDto, CmdError> {
        let title = match r.kind {
            EntityKind::Note => self
                .inner
                .store
                .get_note(&r.id)?
                .map(|n| n.title)
                .or_else(|| {
                    self.inner
                        .store
                        .get_tombstone(EntityKind::Note, &r.id)
                        .ok()
                        .flatten()
                        .and_then(|t| t.title_snap)
                })
                .filter(|t| !t.trim().is_empty()),
            EntityKind::Folder => self
                .inner
                .store
                .get_folder(&r.id)?
                .map(|f| f.name)
                .or_else(|| {
                    self.inner
                        .store
                        .get_tombstone(EntityKind::Folder, &r.id)
                        .ok()
                        .flatten()
                        .and_then(|t| t.title_snap)
                })
                .filter(|t| !t.trim().is_empty()),
            // 附件按 sha256 寻址，不是 UUID 实体：卡片只能用哈希当前缀。
            EntityKind::Attachment => Some(r.local_hash.chars().take(12).collect()),
        };
        // 副本还在才报得出它的 rev；副本已被删则给 None，面板那栏退回卡片自带的兜底预览。
        let copy_rev = match &r.copy_note_id {
            Some(cid) => self.inner.store.get_note(cid)?.map(|n| n.rev.get()),
            None => None,
        };
        Ok(ConflictDto {
            id: r.conflict_id,
            note_id: r.id.to_string(),
            note_title: title.unwrap_or_else(|| "（无标题）".into()),
            base_rev: r.base_rev.get(),
            local_rev: r.local_rev.get(),
            remote_rev: r.remote_rev.get(),
            copy_note_id: r.copy_note_id.as_ref().map(|i| i.to_string()),
            copy_rev,
            // 服务器那一版：从冲突行上挂着的原始信封里取正文（P11 引擎只登记不采纳，
            // 所以这份内容在本机修订历史里根本没有）。取不到就是 None，面板退回说明。
            remote_preview: r.remote_wire.as_deref().and_then(|w| {
                let env: serde_json::Value = serde_json::from_str(w).ok()?;
                let payload = env.get("payload").or_else(|| env.get("doc"))?.clone();
                let doc = notera_richtext::parse_from_value(&payload).ok()?;
                Some(notera_richtext::extract(&doc).plain_text)
            }),
            created_at: r.created_at,
        })
    }

    pub fn resolve_conflict(&self, c: commands::ResolveConflictCmd) -> Result<(), CmdError> {
        // UI 侧的动词（apps/desktop/src/api/types.ts 的 ConflictAction）与存储侧的
        // resolution 词表不是一套；翻译只能发生在这里 —— 存储与引擎都不该认识 UI 词汇。
        let resolution = match c.action.as_str() {
            "dismiss" => {
                return self
                    .inner
                    .store
                    .dismiss_conflict(c.id)
                    .map_err(CmdError::from)
            }
            "keepBoth" | "keep_both" | "kept_both" => "kept_both",
            "replaceWithLocal" | "use_local" | "local" => "local",
            "replaceWithRemote" | "use_remote" | "remote" => "remote",
            "manualMerge" | "manual" => "manual",
            "merged" => "merged",
            other => {
                return Err(
                    CmdError::of("bad_action", false).with(serde_json::json!({ "action": other }))
                )
            }
        };
        // "用我这一版"不是记账就完事：§6.1 采纳之后正文是对面那一版，用户点这颗按钮
        // 要的就是把两版互换（互换后两版各有一处存放，谁都没被吃掉）。
        // 前提不成立时（正文本来就是我要的那一版）`swap_conflict_sides` 返回 false，
        // 那就只关卡片 —— 用户要的结果已经在了，不去动任何内容。
        if resolution == "local"
            && self
                .inner
                .store
                .swap_conflict_sides(c.id)
                .map_err(CmdError::from)?
        {
            self.emit(BusEvent::NotesChanged { ids: vec![] });
            return Ok(());
        }
        self.inner.store.resolve_conflict(c.id, resolution)?;
        self.emit(BusEvent::NotesChanged { ids: vec![] });
        Ok(())
    }

    pub fn set_pref(&self, key: &str, value: serde_json::Value) -> Result<(), CmdError> {
        self.inner
            .store
            .set_pref(key, &value)
            .map_err(CmdError::from)
    }
    pub fn get_prefs(&self) -> Result<serde_json::Value, CmdError> {
        self.inner.store.get_prefs().map_err(CmdError::from)
    }

    /// 导出为自描述 ZIP（DATA-MODEL §15）。整库或按文件夹子树。
    ///
    /// 记录由 `Store::all_records()` 用**与上传同一套**信封构造函数产出，
    /// 所以导入侧可以直接走 `apply_remote`，不必为导出另写一条写入路径。
    pub fn export_data(&self, c: ExportCmd) -> Result<serde_json::Value, CmdError> {
        let ids: Vec<EntityId> = c
            .folder_ids
            .iter()
            .map(|s| EntityId::parse(s).map_err(|_| CmdError::of("bad_id", false)))
            .collect::<Result<_, _>>()?;
        // 两个范围、两种用途：文件夹行要按外键闭包带上祖先骨架，
        // 笔记与附件只认这一棵子树 —— 否则祖先（默认本）里的东西会被顺手带走。
        let skeleton: Option<std::collections::BTreeSet<String>> = if ids.is_empty() {
            None
        } else {
            Some(
                self.inner
                    .store
                    .folder_closure(&ids)
                    .map_err(CmdError::from)?,
            )
        };
        let content: Option<std::collections::BTreeSet<String>> = if ids.is_empty() {
            None
        } else {
            Some(
                self.inner
                    .store
                    .folder_subtree(&ids)
                    .map_err(CmdError::from)?,
            )
        };
        let in_skeleton = |id: &str| skeleton.as_ref().is_none_or(|s| s.contains(id));
        let in_content = |id: &str| content.as_ref().is_none_or(|s| s.contains(id));

        let records = self.inner.store.all_records().map_err(CmdError::from)?;
        let kind_of = |r: &serde_json::Value| {
            r.get("kind")
                .and_then(|k| k.as_str())
                .unwrap_or_default()
                .to_string()
        };
        let id_of = |r: &serde_json::Value| {
            r.get("id")
                .and_then(|i| i.as_str())
                .unwrap_or_default()
                .to_string()
        };
        let folder_of = |r: &serde_json::Value| {
            r.get("payload")
                .and_then(|p| p.get("folder_id"))
                .and_then(|f| f.as_str())
                .unwrap_or_default()
                .to_string()
        };
        let purged =
            |r: &serde_json::Value| r.get("purged").and_then(|p| p.as_bool()).unwrap_or(false);
        // 按文件夹导时：
        //   文件夹 → 子树 + 祖先链（`folder_closure` 的键集就是它：祖先只是外键骨架）
        //   笔记   → 父本在**子树**里的（`folder_subtree`，祖先自己的笔记不算这一棵的内容）
        //            软删的跟着走：删除事实是内容的一部分
        //   墓碑   → 只带**文件夹**的永久删除公告。`tombstones` 表不记父本，笔记的
        //            永久删除公告无法归属到某个文件夹 —— 宁可标 `partial` 让导入端拒绝
        //            "导进空库"，也不能猜：猜错就是把别人库里的一条笔记删掉。
        let bundle = notera_importer::Bundle {
            manifest: Some(notera_importer::Manifest {
                format: notera_importer::BUNDLE_FORMAT,
                protocol: 1,
                exported_at: self.inner.store.now(),
                app_version: env!("CARGO_PKG_VERSION").to_string(),
                root_id: None,
                counts: std::collections::BTreeMap::new(),
                partial: skeleton.is_some(),
            }),
            folders: records
                .iter()
                .filter(|r| kind_of(r) == "folder" && !purged(r) && in_skeleton(&id_of(r)))
                .cloned()
                .collect(),
            notes: records
                .iter()
                .filter(|r| kind_of(r) == "note" && !purged(r) && in_content(&folder_of(r)))
                .cloned()
                .collect(),
            tombstones: records
                .iter()
                .filter(|r| {
                    purged(r)
                        && (skeleton.is_none()
                            || (kind_of(r) == "folder" && in_skeleton(&id_of(r))))
                })
                .cloned()
                .collect(),
            attachments: Vec::new(),
        };
        let shas: Vec<String> = match &content {
            None => self
                .inner
                .store
                .local_attachment_shas()
                .map_err(CmdError::from)?,
            Some(set) => {
                let folders: Vec<String> = set.iter().cloned().collect();
                self.inner
                    .store
                    .attachment_shas_in_folders(&folders)
                    .map_err(CmdError::from)?
            }
        };
        let mut attachments = Vec::new();
        if c.include_attachments {
            for sha in shas {
                match std::fs::read(self.inner.store.blob_path(&sha)) {
                    Ok(bytes) => attachments.push((sha, bytes)),
                    // 读不到就少传一个附件：报告里如实记数，不静默当成功
                    Err(e) => tracing::warn!(%sha, error = %e, "附件读不到，本次导出不含它"),
                }
            }
        }
        let bundle = notera_importer::Bundle {
            attachments,
            ..bundle
        };
        let scope_size = skeleton.map(|s| s.len()).unwrap_or(0);
        let path = match c.path.as_deref() {
            Some(p) => std::path::PathBuf::from(p),
            None => {
                let dir = self.inner.data_dir.join("exports");
                std::fs::create_dir_all(&dir).map_err(|e| {
                    CmdError::of("storage", false)
                        .with(serde_json::json!({ "detail": e.to_string() }))
                })?;
                // 时间戳里的 `:`/`-` 在 Windows 文件名里不安全，只留字母数字
                dir.join(format!(
                    "notera-{}.zip",
                    self.inner
                        .store
                        .now()
                        .replace(|c: char| !c.is_ascii_alphanumeric(), "")
                ))
            }
        };
        if path.exists() {
            // 绝不覆盖已有文件：用户指哪儿就写哪儿，指到一份备份上就是毁掉那次备份。
            return Err(CmdError::of("save_failed", false).with(serde_json::json!({
                "detail": format!("导出目标已存在，未覆盖：{}", path.display()),
            })));
        }
        notera_importer::write_bundle(&path, &bundle).map_err(|e| {
            CmdError::of("save_failed", false).with(serde_json::json!({ "detail": e.to_string() }))
        })?;
        let counts = serde_json::json!({
            "notes": bundle.notes.len(),
            "folders": bundle.folders.len(),
            "tombstones": bundle.tombstones.len(),
            "attachments": bundle.attachments.len(),
        });
        Ok(serde_json::json!({
            "path": path.to_string_lossy(),
            "created": bundle.notes.len(),
            "counts": counts,
            // 报告必须自己说清这是整库还是子树：拿一份子树包当"整库备份"是最危险的误用。
            "scope": if scope_size == 0 { "full" } else { "folders" },
            "scopeFolders": scope_size,
        }))
    }

    /// 导入一个 bundle。`intoEmpty` 只在库真的为空时放行；`merge` 走同步的
    /// 三方判定，绝不静默覆盖（§15 / I3）。
    /// 导入**散文件**（Evernote 的 `.enex`、Markdown、纯文本）到默认本。
    ///
    /// 与 `import_data` 的区别不是"格式不同"而是"语义不同"：那个是把自己导出的 ZIP
    /// 整库还原（含删除公告，所以有空库闸门），这里只是往里加内容，一条删除事实都
    /// 不携带 —— 所以它不碰 intoEmpty 那套判定，也绝不该被拿去"恢复备份"。
    ///
    /// 幂等靠导入器的内容哈希（同一份文件导两次，第二次一条都不新建），去重范围是
    /// 目标文件夹。`notices` 是"没坏但用户该知道"的部分：`.enex` 里没落地的字段、
    /// 按字面保留的结构、没解出来的附件 —— §39 不许静默降级，所以它们一路带到界面。
    pub fn import_files(&self, paths: &[String]) -> Result<serde_json::Value, CmdError> {
        use notera_importer::{apply, plan, FolderTarget, ImportSource};
        if paths.is_empty() {
            return Err(CmdError::of("bad_args", false));
        }
        let default_folder = self
            .inner
            .store
            .list_folders()
            .map_err(|e| {
                CmdError::of("save_failed", false)
                    .with(serde_json::json!({ "detail": e.to_string() }))
            })?
            .into_iter()
            .find(|f| f.system_kind.as_deref() == Some("default"))
            .ok_or_else(|| CmdError::of("folder_missing", false))?;
        let mut sources = Vec::new();
        let mut unreadable: Vec<serde_json::Value> = Vec::new();
        for p in paths {
            match ImportSource::read(std::path::Path::new(p)) {
                Ok(src) => sources.push(src),
                Err(e) => unreadable.push(serde_json::json!({
                    "path": p,
                    "why": e.to_string(),
                })),
            }
        }
        let plan = plan(&sources);
        let report = apply(
            &self.inner.store,
            &FolderTarget::Existing(default_folder.id.clone()),
            &plan,
        )
        .map_err(|e| {
            CmdError::of("save_failed", false).with(serde_json::json!({ "detail": e.to_string() }))
        })?;
        let mut notices: Vec<String> = report.notices.clone();
        for f in &plan.failures {
            notices.push(format!("{}：{}", f.label, f.error));
        }
        for u in &unreadable {
            notices.push(format!("读不了 {}：{}", u["path"], u["why"]));
        }
        Ok(serde_json::json!({
            "folderName": report.folder_name,
            "created": report.created.iter().map(|c| serde_json::json!({
                "label": c.label, "title": c.stored_title, "id": c.note_id.as_str(),
            })).collect::<Vec<_>>(),
            "duplicates": report.duplicates.len(),
            "failed": report.failed.iter().map(|f| serde_json::json!({
                "label": f.label, "why": f.error.to_string(),
            })).collect::<Vec<_>>(),
            "notices": notices,
        }))
    }

    pub fn import_data(&self, c: ImportCmd) -> Result<serde_json::Value, CmdError> {
        let path = std::path::PathBuf::from(c.path.ok_or_else(|| CmdError::of("bad_args", false))?);
        let bundle = notera_importer::read_bundle(&path).map_err(|e| {
            CmdError::of("save_failed", false).with(serde_json::json!({ "detail": e.to_string() }))
        })?;
        let stats = self.inner.store.stats().map_err(CmdError::from)?;
        // 子树包缺两样东西：不在范围内的其它内容，以及（无法归属的）**笔记永久删除公告**。
        // 把它当"整库还原"导进一个空库，之后一旦与服务器同步，那些本该保持删除的笔记
        // 就会被别的设备的副本带回来 —— 这正是 §8 硬性要求 6 禁的事。所以 intoEmpty 响亮拒绝；
        // 合并模式不受影响：那只是往里加内容，少带几篇不会删掉任何人的东西。
        if bundle.manifest.as_ref().is_some_and(|m| m.partial)
            && c.mode.as_deref() == Some("intoEmpty")
        {
            return Err(CmdError::of("save_failed", false).with(serde_json::json!({
                "detail": "这是一个按文件夹导出的部分包，不能用于「仅在空库时导入」的整库还原：它缺库内其它内容，也缺笔记的永久删除公告。请导整库，或改用合并模式。",
            })));
        }
        if c.mode.as_deref() == Some("intoEmpty") && (stats.notes > 0 || stats.folders > 1) {
            return Err(CmdError::of("save_failed", false).with(serde_json::json!({
                "detail": format!("目标库非空（笔记 {} / 文件夹 {}），intoEmpty 拒绝写入", stats.notes, stats.folders),
            })));
        }
        for (sha, bytes) in &bundle.attachments {
            // 还原走 `restore_blob` 而不是同步的 `ingest_blob`：包里的字节没经过服务器，
            // 远端态必须是 unknown（否则永远不补传），而且目标库很可能没有这一行。
            self.inner
                .store
                .restore_blob(sha, bytes)
                .map_err(CmdError::from)?;
        }
        let mut ops = Vec::new();
        for env in bundle.records_in_apply_order() {
            let kind = env.get("kind").and_then(|k| k.as_str()).unwrap_or_default();
            let id = env
                .get("id")
                .and_then(|i| i.as_str())
                .ok_or_else(|| CmdError::of("bad_args", false))?;
            let entity = EntityId::parse(id).map_err(|_| CmdError::of("bad_id", false))?;
            ops.push(
                if env.get("purged").and_then(|p| p.as_bool()).unwrap_or(false) {
                    notera_store::ApplyOp::Purge {
                        kind: if kind == "folder" {
                            notera_core::EntityKind::Folder
                        } else {
                            notera_core::EntityKind::Note
                        },
                        id: entity,
                    }
                } else if kind == "folder" {
                    notera_store::ApplyOp::UpsertFolder { env }
                } else {
                    notera_store::ApplyOp::UpsertNote { env }
                },
            );
        }
        let report = self
            .inner
            .store
            .apply_remote(&ops)
            .map_err(CmdError::from)?;
        let conflicts = self
            .inner
            .store
            .open_conflicts()
            .map(|v| v.len())
            .unwrap_or(0);
        Ok(serde_json::json!({
            "path": path.to_string_lossy(),
            "merged": report.applied,
            "skipped": report.skipped,
            "conflicts": conflicts,
            "restoredAttachments": bundle.attachments.len(),
        }))
    }

    /// 冲突并排预览：某条笔记在某个 rev 上的纯文本。
    ///
    /// 解析与抽取用**和写路径同一套** `notera_richtext`（`§10.3` 那条顺序），
    /// 预览因此不会和列表/正文用的是"另一种算法"—— 两套算法迟早会给出不同的文本。
    pub fn preview_text(&self, id: &str, rev: u64) -> Result<String, CmdError> {
        let id = EntityId::parse(id).map_err(|_| CmdError::of("bad_id", false))?;
        let doc = match self.inner.store.revision_doc(&id, Rev(rev))? {
            Some(d) => d,
            // 注意：这里**不**退回冲突行上的服务器信封。`rev` 是各设备自己的编号，
            // 本机历史上同一个号往往是另一份内容 —— 第一版这么退过，两台真设备的
            // 测试立刻抓到：右栏显示的是本机那一版，正是 §6.1 要避免的那种假象。
            // 对面那一版走卡片的 `remote_preview`（载荷来自迁移 0008）。
            None => {
                return Err(CmdError::of("not_found", false).with(serde_json::json!(
                    { "kind": "note", "id": id.to_string(), "rev": rev }
                )))
            }
        };
        let parsed = notera_richtext::parse_from_value(&doc).map_err(|e| {
            CmdError::of("corrupt_record", false).with(serde_json::json!({ "why": e.to_string() }))
        })?;
        Ok(notera_richtext::extract(&parsed).plain_text)
    }

    /// 一致性快照（DATA-MODEL §15）。产物自带 sha256 与 user_version，恢复闸门靠它们。
    pub fn backup_db(
        &self,
        dest_dir: Option<&std::path::Path>,
    ) -> Result<notera_store::BackupInfo, CmdError> {
        self.inner
            .store
            .create_backup(dest_dir)
            .map_err(CmdError::from)
    }

    pub fn list_backups(&self) -> Result<Vec<notera_store::BackupInfo>, CmdError> {
        self.inner.store.list_backups().map_err(CmdError::from)
    }

    /// 恢复只"排期"，不在进程内换库：真正落地发生在下次启动 `Store::open` 之前。
    /// 返回 `restart_required=true` 是这条命令的正常结果，不是失败。
    pub fn stage_restore(&self, path: &std::path::Path) -> Result<serde_json::Value, CmdError> {
        let info = self
            .inner
            .store
            .stage_restore(path)
            .map_err(CmdError::from)?;
        Ok(serde_json::json!({
            "restartRequired": true,
            "sha256": info.sha256,
            "userVersion": info.user_version,
            "path": info.path.to_string_lossy(),
        }))
    }

    // ------------------------------------------------------------ 配置面 ---

    pub fn current_account(&self) -> Result<Option<AccountDto>, CmdError> {
        Ok(ConfigRepository::active(&self.config())
            .map(account_dto)
            .map(|d| self.attach_caps(d)))
    }

    pub fn configure_account(&self, draft: AccountDraftCmd) -> Result<AccountDto, CmdError> {
        let mut cfg = self.inner.config.lock().unwrap().clone();
        // ADR-0018：确认点（notes.sync_rev）目前是全局的，同时启用两台服务器会让第二台
        // **静默半同步**。在按账户确认点落地之前，这里显式拒绝，而不是留着错。
        let others_enabled = cfg
            .accounts
            .iter()
            .filter(|a| a.enabled && a.id != draft.id)
            .count();
        if others_enabled > 0 {
            return Err(CmdError::of("multi_account_unsupported", false));
        }
        // 账户 id 必须在这里就定下来。`upsert_account` 遇到空 id 也会自己生成一个，
        // 但那是**它那一份**：host 若继续拿空串去 `register_account`，配置里的账户在
        // `sync_accounts` 里就没有对应行 —— 本地写入永远不会排队给这台服务器，
        // 用户看到的却是"已配置、已同步"。（端到端测试 sync_once 抓到的静默不同步）
        let id = if draft.id.is_empty() {
            EntityId::new().to_string()
        } else {
            draft.id.clone()
        };
        let acct = AccountConfig {
            id: id.clone(),
            label: draft.label,
            base_url: draft.base_url,
            root_prefix: draft.root_prefix.unwrap_or_else(|| "/.notes".into()),
            auth_kind: match draft.auth_kind.as_deref() {
                Some("token") => notera_config::AuthKind::Token,
                _ => notera_config::AuthKind::Basic,
            },
            username: draft.username.clone(),
            credential_ref: if draft.password.is_some() || draft.username.is_some() {
                format!("keychain:{id}")
            } else {
                String::new()
            },
            tls_policy: match draft.tls_policy.as_deref() {
                Some("ca_bundle") => TlsPolicyKind::CaBundle,
                Some("pin") => TlsPolicyKind::Pin,
                Some("insecure_local") => TlsPolicyKind::InsecureLocal,
                _ => TlsPolicyKind::Strict,
            },
            ca_pem: draft.ca_pem,
            pinned_sha256: None,
            proxy: ProxyProfile {
                mode: match draft.proxy_mode.as_deref() {
                    Some("system") => ProxyMode::System,
                    Some("http") => ProxyMode::Http,
                    Some("https") => ProxyMode::Https,
                    Some("socks5") => ProxyMode::Socks5,
                    _ => ProxyMode::Direct,
                },
                host: draft.proxy_host,
                port: draft.proxy_port,
                username_ref: draft
                    .proxy_username
                    .map(|_| format!("keychain:proxy-user:{id}")),
                password_ref: draft
                    .proxy_password
                    .map(|_| format!("keychain:proxy-pass:{id}")),
                bypass: draft.bypass.unwrap_or_default(),
                resolve_remote_dns: true,
            },
            enabled: true,
        };
        // 校验/落盘的规则全在 notera-config 里；host 只折叠错误码（原子写、拒绝覆盖损坏配置）
        notera_config::validate_account(&acct).map_err(|e| {
            CmdError::of("invalid_account", false).with(serde_json::json!({ "why": e.to_string() }))
        })?;
        self.inner
            .config_repo
            .upsert_account(&mut cfg, acct.clone())
            .map_err(|e| {
                CmdError::of("invalid_account", false)
                    .with(serde_json::json!({ "why": e.to_string() }))
            })?;
        self.inner.config_repo.save(&cfg).map_err(|e| {
            CmdError::of("save_failed", false).with(serde_json::json!({ "why": e.to_string() }))
        })?;
        *self.inner.config.lock().unwrap() = cfg.clone();
        // 配置里的账户必须在存储层落一行：outbox 是按 `sync_accounts` 扇出的，
        // 没这一行 = 本地写入永远不会排队给这台服务器（静默不同步）。
        self.inner
            .store
            .register_account(&acct.id, &acct.label, &acct.base_url)
            .map_err(|e| {
                CmdError::of("invalid_account", false)
                    .with(serde_json::json!({ "why": e.to_string() }))
            })?;
        self.set_sync(|v| v.phase = Phase::Provisioning);
        // 带上新账户的探测状态（刚配上 = 还没探过），界面才不会把"没探过"说成"不支持"
        ConfigRepository::active(&cfg)
            .map(account_dto)
            .map(|d| self.attach_caps(d))
            .ok_or_else(|| CmdError::of("no_account", false))
    }

    // ---------------------------------------------------- 远端适配器 ---

    /// 活跃账户 → 可直接交给 [`App::start_sync`] 的远端适配器。
    ///
    /// `Ok(None)` 是**正常状态**而不是错误：没配服务器，或凭据还取不到
    /// （OS 钥匙串是 PLATFORM.md 的 Phase 5 任务）。此时引擎根本不启动，
    /// 本地照常写（不变式 I8），徽标停在"需要凭据"，绝不拿空凭据去写远端。
    pub fn sync_remote(&self) -> Result<Option<Arc<notera_webdav::WebDavRemote>>, CmdError> {
        let cfg = self.config();
        let Some(acct) = ConfigRepository::active(&cfg) else {
            self.set_sync(|v| v.phase = Phase::Unconfigured);
            return Ok(None);
        };
        let device = DeviceId::parse(&cfg.device_id).map_err(|e| {
            CmdError::of("bad_device", false).with(serde_json::json!({ "why": e.to_string() }))
        })?;
        let creds = self
            .secret_for(acct)
            .map(|(user, secret)| {
                notera_webdav::Credentials::new(user, secret).map_err(|e| {
                    CmdError::of("invalid_account", false)
                        .with(serde_json::json!({ "why": e.to_string() }))
                })
            })
            .transpose()?;
        let Some(credentials) = creds else {
            self.set_sync(|v| {
                v.phase = Phase::NeedsCredentials;
                v.badge = Badge::Offline;
                v.message_key = Some("sync.needsCredentials".into());
                v.retryable = false;
            });
            return Ok(None);
        };
        // §5：探测过一次就把结果用到底 —— 不这么做，一台支持条件写的服务器会被
        // 永久按保守默认对待，S1/S2 的好处一辈子拿不到。
        let caps = self.stored_caps(&acct.id);
        Self::build_remote(acct, device, credentials, caps).map(Some)
    }

    /// 已探测过就用实测位图；没探测过退回保守默认（`conventional`）。
    /// 读失败也退回默认 —— 探测结果只是优化写路径，不该因为它读不到就阻塞同步。
    fn stored_caps(&self, account_id: &str) -> notera_webdav::Caps {
        match self.inner.store.account_caps(account_id) {
            Ok(Some(mask)) => notera_webdav::Caps::from_mask(mask),
            _ => notera_webdav::Caps::conventional(),
        }
    }

    /// 把 §5 的探测结果挂到账户视图上（SYNC-PROTOCOL §5 要求 S3 账户在界面上明说
    /// "这台服务器不提供并发保护"）。读不到就当没探过 —— 界面上的这一行是**说明**，
    /// 不是判定依据，所以宁可少说，也不能为它把一次本地存储错误变成配置失败。
    fn attach_caps(&self, mut dto: AccountDto) -> AccountDto {
        let Ok(Some(mask)) = self.inner.store.account_caps(&dto.id) else {
            return dto;
        };
        dto.cap_mask = Some(mask);
        dto.write_strategy = Some(format!("{:?}", self.stored_caps(&dto.id).write_strategy()));
        dto.caps_probed_at = self
            .inner
            .store
            .account_caps_probed_at(&dto.id)
            .unwrap_or(None);
        dto
    }

    /// §5：跑一次能力探测并写回 `sync_accounts.cap_mask`。
    /// 探测失败必须冒出去 —— 把"连不上"当成"什么都不支持"会把服务器降级到 S3 盲写。
    pub async fn probe_and_store_caps(&self) -> Result<serde_json::Value, CmdError> {
        let remote = self
            .sync_remote()?
            .ok_or_else(|| CmdError::of("no_account", false))?;
        let report = remote.probe_caps().await.map_err(|e| {
            CmdError::of("net_config", true).with(serde_json::json!({ "why": e.to_string() }))
        })?;
        let cfg = self.config();
        let acct =
            ConfigRepository::active(&cfg).ok_or_else(|| CmdError::of("no_account", false))?;
        let mask = report.to_caps().mask();
        self.inner
            .store
            .set_account_caps(&acct.id, mask)
            .map_err(CmdError::from)?;
        Ok(serde_json::json!({
            "capMask": mask,
            "writeStrategy": format!("{:?}", report.to_caps().write_strategy()),
            "strongEtag": report.strong_etag,
            "conditionalPut": report.conditional_put,
            "overwriteFMove": report.overwrite_f_move,
            "depthInfinity": report.depth_infinity,
            "range": report.range,
            "describe": report.describe(),
        }))
    }

    /// §5「首次连接与每日一次探测」：该不该现在探测。
    /// 时间戳读不懂也算到期 —— 宁可多探一次，也不要把一台好服务器永久锁在 S3。
    fn caps_probe_due(&self) -> Result<bool, CmdError> {
        let cfg = self.config();
        let Some(acct) = ConfigRepository::active(&cfg) else {
            return Ok(false);
        };
        let prev = self
            .inner
            .store
            .account_caps_probed_at(&acct.id)
            .map_err(CmdError::from)?;
        let Some(prev) = prev else {
            return Ok(true);
        };
        let Some(prev_ms) = Timestamp::parse(&prev).and_then(|t| t.as_millis()) else {
            return Ok(true);
        };
        let now_ms = Timestamp::parse(&self.inner.store.now())
            .and_then(|t| t.as_millis())
            .unwrap_or_default();
        Ok(now_ms - prev_ms >= Self::CAPS_PROBE_TTL_MS)
    }

    /// 启动/手动同步的统一入口：**先按需探测能力，再装适配器**。
    ///
    /// 顺序不能反 —— 先装后用，探测到的"这台服务器支持条件写"要等下次启动才生效，
    /// 本次会话仍然走保守盲写（§5 的优化白做）。
    /// 探测失败**不是**错误：记一条提示并以保守默认继续（拿不到结论 ≠ 连不上）。
    pub async fn remote_for_sync(
        &self,
    ) -> Result<Option<Arc<notera_webdav::WebDavRemote>>, CmdError> {
        if self.caps_probe_due()? {
            if let Err(e) = self.probe_and_store_caps().await {
                tracing::warn!(code = %e.code, "§5 能力探测未完成，本轮按保守默认走");
                self.emit(BusEvent::Toast {
                    message_key: "sync.probeDeferred".into(),
                    level: "warn".into(),
                });
            }
        }
        self.sync_remote()
    }

    /// 跑**一轮**同步（§6）。调度器与 `notera-cli sync-once` 共用这一条路径 ——
    /// 否则 CLI 测出来的绿灯和产品行为根本不是同一个绿灯。
    /// `None` = 已有一轮在跑（§6.4 单轮并发上限 = 1）。
    pub(crate) async fn run_round<R: notera_sync::RemotePort + 'static>(
        &self,
        remote: &Arc<R>,
    ) -> Option<RoundStats> {
        if self.inner.syncing.swap(true, Ordering::SeqCst) {
            return None;
        }
        self.set_sync_public(Badge::Syncing);
        // §6.3：带上轮的 etag 才可能拿到 304（空轮 1 请求 0 字节正文）
        let etag = self.manifest_etag();
        let engine = notera_sync::SyncEngine::new(
            self.local_port(),
            RemoteBorrow(remote.as_ref()),
            self.engine_config(),
        );
        let (st, evs) = engine.run_round(etag.as_deref()).await;
        let yielded = evs.iter().any(|e| matches!(e, SyncEvent::Deferred { .. }));
        for e in evs {
            self.apply_sync_event(e);
        }
        self.apply_round_stats(&st, yielded);
        self.inner.syncing.store(false, Ordering::SeqCst);
        Some(st)
    }

    /// §11.4 的开关：**只在别的保护缺位时**才花这两次请求。
    /// * 写入策略 S3（条件写与不覆盖式 MOVE 都没有）→ 记录写全靠复验，让路有意义；
    /// * 探不到强 ETag → 清单 CAS 形同虚设，公告可能互相覆盖，同样要让路。
    ///   两者都不成立时（S1/S2 + 强 ETag）服务器自己就拦并发写，开租约只是白多两个请求。
    fn engine_config(&self) -> EngineConfig {
        EngineConfig {
            lease: self.lease_policy(),
            ..EngineConfig::default()
        }
    }

    /// 从 §5 的探测结果推出租约策略。没探过 → 用保守默认（含强 ETag 且有条件写）→ 关。
    pub fn lease_policy(&self) -> notera_sync::LeasePolicy {
        let Some(acct) = ConfigRepository::active(&self.config()).cloned() else {
            return notera_sync::LeasePolicy::Off;
        };
        let caps = self.stored_caps(&acct.id);
        let weak_announcement_cas = !caps.has(notera_webdav::Caps::STRONG_ETAG);
        let blind_writes = caps.write_strategy() == notera_webdav::WriteStrategy::S3;
        if weak_announcement_cas || blind_writes {
            notera_sync::LeasePolicy::On {
                ttl_ms: LEASE_TTL_MS,
            }
        } else {
            notera_sync::LeasePolicy::Off
        }
    }

    /// 按需探测 → 协商 → 跑一轮，返回这一轮的统计（CLI `sync-once` 的落点）。
    /// 没配账户/凭据 → `Err`：诊断工具宁可不报，也不许拿"零请求"冒充同步成功。
    pub async fn sync_once(&self) -> Result<RoundStats, CmdError> {
        let Some(remote) = self.remote_for_sync().await? else {
            return Err(CmdError::of("no_account", false));
        };
        if let Err(key) = self.negotiate(&remote).await {
            return Err(
                CmdError::of("sync_refused", false).with(serde_json::json!({ "reason": key }))
            );
        }
        self.run_round(&remote)
            .await
            .ok_or_else(|| CmdError::of("sync_busy", true))
    }

    /// §5 的"每日一次"。
    const CAPS_PROBE_TTL_MS: i64 = 24 * 60 * 60 * 1000;

    /// PROXY.md §9 证据链③：按**当前配置**判定这个 URL 会走哪条出口。
    /// 纯判定，一个包都不发（`HttpClient::probe` 的性质），因此离线可跑。
    /// 没配账户时按"直连 + 严格 TLS"判定 —— 那正是产品此时真正的出口。
    pub fn route_for(&self, url: &str) -> Result<serde_json::Value, CmdError> {
        let cfg = self.config();
        let (proxy, tls, from) = match ConfigRepository::active(&cfg) {
            Some(acct) => (
                net_proxy(&acct.proxy)?,
                net_tls(acct),
                format!("account:{}", acct.id),
            ),
            None => (
                notera_net::ProxyProfile::direct(),
                notera_net::TlsPolicy::Strict,
                "default-direct".to_string(),
            ),
        };
        let http = notera_net::HttpClient::build(&proxy, &tls, notera_net::Timeouts::default())
            .map_err(|e| {
                CmdError::of("net_config", true).with(serde_json::json!({ "why": e.to_string() }))
            })?;
        let proof = http.probe(url).map_err(|e| {
            CmdError::of("net_config", false).with(serde_json::json!({ "why": e.to_string() }))
        })?;
        Ok(serde_json::json!({
            "url": url,
            "configFrom": from,
            "oneLine": proof.one_line(),
            "endpoint": proof.endpoint,
            "bypassed": proof.bypassed,
        }))
    }

    /// 装配一个真适配器：地址/前缀/凭据/代理/TLS 全来自配置，出口只有 `notera-net`。
    /// 拆出来是为了让"能不能装起来"这条判定可以脱离钥匙串被测试到。
    fn build_remote(
        acct: &AccountConfig,
        device: DeviceId,
        credentials: notera_webdav::Credentials,
        caps: notera_webdav::Caps,
    ) -> Result<Arc<notera_webdav::WebDavRemote>, CmdError> {
        let proxy = net_proxy(&acct.proxy)?;
        let http = Arc::new(
            notera_net::HttpClient::build(&proxy, &net_tls(acct), notera_net::Timeouts::default())
                .map_err(|e| {
                    CmdError::of("net_config", true)
                        .with(serde_json::json!({ "why": e.to_string() }))
                })?,
        );
        let remote = notera_webdav::WebDavRemote::new(
            notera_webdav::WebDavConfig::new(acct.base_url.clone())
                .with_root_prefix(acct.root_prefix.clone())
                .with_credentials(credentials)
                .with_device(device)
                .with_caps(caps),
            http,
        )
        .map_err(|e| {
            CmdError::of("invalid_account", false).with(serde_json::json!({ "why": e.to_string() }))
        })?;
        Ok(Arc::new(remote))
    }

    /// 凭据解析点。`credential_ref` 指向 OS 钥匙串，那套接入在 Phase 5；
    /// 在此之前**只有 debug 构建**能从开发用环境变量拿到口令 —— 发布版宁可
    /// 显示"需要凭据"，也不把明文写进配置文件。
    fn secret_for(&self, acct: &AccountConfig) -> Option<(String, String)> {
        let user = acct.username.clone()?;
        if !cfg!(debug_assertions) {
            return None;
        }
        let secret = std::env::var("NOTERA_DEV_WEBDAV_SECRET").ok()?;
        (!secret.is_empty()).then_some((user, secret))
    }

    /// SYNC-PROTOCOL §2 的启动期协商。返回 `Err(文案键)` 意思是**不许开始同步**，
    /// 调用方必须把它显示出来 —— "静默地不同步"和"静默地同步错"一样不可接受。
    ///
    /// 判据顺序按规范：读 `protocol.json` → 版本区间 → `root_id` 是否同一个库。
    /// 缺 `protocol.json` 时先看根是不是已被用过（有清单）：用过就拒，
    /// 绝不当成"空库就地初始化"（那等于清空别人现有的库）。
    pub async fn negotiate(
        &self,
        remote: &notera_webdav::WebDavRemote,
    ) -> Result<(), &'static str> {
        let cfg = self.config();
        let acct = ConfigRepository::active(&cfg).ok_or("sync.no_account")?;
        let mut state = self
            .inner
            .store
            .sync_state(&acct.id)
            .map_err(|_| "sync.storage_unavailable")?
            .ok_or("sync.state_missing")?;
        let observed = remote
            .fetch_protocol()
            .await
            .map_err(|_| "sync.protocol_unreadable")?;
        let mine = self
            .default_folder_id()
            .map_err(|_| "sync.no_default_folder")?;
        let ours = notera_sync::SYNC_PROTOCOL_VERSION as u64;
        match observed {
            Some(doc) => {
                let server = doc.get("protocol").and_then(|v| v.as_u64()).unwrap_or(0);
                let server_min = doc
                    .get("min_protocol")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(server);
                if server_min > ours || server < ours {
                    self.set_sync(|v| {
                        v.phase = Phase::ReadOnly;
                        v.badge = Badge::Offline;
                        v.message_key = Some("sync.protocol_mismatch".into());
                    });
                    return Err("sync.protocol_mismatch");
                }
                let remote_root = doc
                    .get("root_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if remote_root.is_empty() {
                    return Err("sync.protocol_mismatch");
                }
                match state.root_id.as_deref() {
                    None if self.local_library_is_empty() => state.root_id = Some(remote_root),
                    None => {
                        // 本机已经是一个有内容的库，却要接受另一个库的 root_id ——
                        // 这就是 §2 说的"两库并一库"。停手，让用户改路径或改配置。
                        self.set_sync(|v| {
                            v.phase = Phase::Error;
                            v.badge = Badge::Failed;
                            v.message_key = Some("sync.root_mismatch".into());
                        });
                        return Err("sync.root_mismatch");
                    }
                    Some(local) if local != remote_root => {
                        // 两个库指向了同一个目录：停，绝不合并（§2）
                        self.set_sync(|v| {
                            v.phase = Phase::Error;
                            v.badge = Badge::Failed;
                            v.message_key = Some("sync.root_mismatch".into());
                        });
                        return Err("sync.root_mismatch");
                    }
                    Some(_) => {}
                }
            }
            None => {
                if remote
                    .root_looks_used()
                    .await
                    .map_err(|_| "sync.protocol_unreadable")?
                {
                    self.set_sync(|v| {
                        v.phase = Phase::Error;
                        v.badge = Badge::Failed;
                        v.message_key = Some("sync.foreign_root".into());
                    });
                    return Err("sync.foreign_root");
                }
                let doc = serde_json::json!({
                    "protocol": ours,
                    "min_protocol": ours,
                    "layout": "v1",
                    "root_id": mine.as_str(),
                    "created_at": SystemClock.now().to_string(),
                    "created_by": cfg.device_id,
                    "software": format!("notera {}", env!("CARGO_PKG_VERSION")),
                    "segment_target_entries": 2000,
                    "window_max_entries": 200,
                    "capabilities_hint": { "conditional_put": null },
                });
                let created = remote
                    .provision_protocol(&doc)
                    .await
                    .map_err(|_| "sync.protocol_unreadable")?;
                state.root_id = Some(if created {
                    mine.as_str().to_string()
                } else {
                    // 抢输的一方必须接受对方那一份，而不是继续按自己的 root_id 写
                    let other = remote
                        .fetch_protocol()
                        .await
                        .map_err(|_| "sync.protocol_unreadable")?
                        .ok_or("sync.protocol_mismatch")?;
                    other
                        .get("root_id")
                        .and_then(|v| v.as_str())
                        .ok_or("sync.protocol_mismatch")?
                        .to_string()
                });
            }
        }
        state.phase = "online".into();
        self.inner
            .store
            .set_sync_state(&state)
            .map_err(|_| "sync.storage_unavailable")?;
        // 协商通过才把可见状态推进同步态；否则徽标会停在"未配置"，用户不知道为什么没动。
        self.set_sync(|v| {
            v.phase = Phase::Online;
            v.message_key = None;
        });
        Ok(())
    }

    /// "本机还是个空库"= 除了开库自带的默认本，没有任何笔记/文件夹/回收站条目。
    /// 只有空库才允许接受别人已经建好的 `root_id`（§9 的新设备加入场景）。
    fn local_library_is_empty(&self) -> bool {
        match self.inner.store.stats() {
            Ok(s) => s.notes == 0 && s.notes_trash == 0 && s.folders <= 1,
            Err(_) => false,
        }
    }

    /// §13 附件轮：与文本轮次**解耦**的独立传输。单轮预算 ≤4 个文件 / ≤64 MiB，
    /// 超出的留下一轮 —— 移动网络不该被一个大文件长期占住。
    ///
    /// 返回 `(上传, 下载, 失败)`。任何失败都不向上抛：附件全失败时，
    /// 文本同步必须照常完成（TEST-PLAN 的"只拔附件端点"用例）。
    pub async fn run_attachment_round(
        &self,
        remote: &notera_webdav::WebDavRemote,
    ) -> (usize, usize, usize) {
        const FILES: usize = 4;
        const BYTES: i64 = 64 * 1024 * 1024;
        // §8 的下载窗口：一轮一个窗口，进度落在 `.part` 上。4 MiB 是"内存上界 vs
        // 往返次数"的折中 —— 一次整块拉完会让一张大图吃掉等量内存，且中途断网全丢。
        const ATTACH_WINDOW: u64 = 4 * 1024 * 1024;
        use std::io::Write;
        let (mut up, mut down, mut failed) = (0usize, 0usize, 0usize);
        let mut budget = BYTES;

        for (i, job) in self
            .inner
            .store
            .attachment_uploads(FILES)
            .unwrap_or_default()
            .into_iter()
            .enumerate()
        {
            // 第一个文件不设预算，否则一个 100 MiB 的附件永远轮不到
            if i > 0 && job.size > budget {
                break;
            }
            budget -= job.size.min(budget);
            let bytes = match std::fs::read(self.inner.store.blob_path(&job.sha256)) {
                Ok(b) => b,
                // 账上"本地有"而盘上没有：标 error，绝不把它当成已上传
                Err(e) => {
                    tracing::warn!(?e, sha = %job.sha256, "本地 blob 缺失");
                    let _ =
                        self.inner
                            .store
                            .set_attachment_states(&job.sha256, Some("error"), None);
                    let _ = self.inner.store.finish_attachment_ops(&job.sha256, false);
                    failed += 1;
                    continue;
                }
            };
            notera_core::crash_point("before_attachment_upload");
            match remote.put_attachment(&job.sha256, &bytes).await {
                Ok(()) => {
                    // 字节已经在服务器上、账上还没标 present：崩在这里必须既不重复传、
                    // 也不留下"以为没传"的悬账（内容寻址 + 复验就是为这一刻准备的）。
                    notera_core::crash_point("after_attachment_upload");
                    let _ =
                        self.inner
                            .store
                            .set_attachment_states(&job.sha256, None, Some("present"));
                    let _ = self.inner.store.finish_attachment_ops(&job.sha256, true);
                    up += 1;
                }
                Err(e) => {
                    tracing::warn!(%e, sha = %job.sha256, "附件上传失败，留下一轮");
                    let _ = self.inner.store.finish_attachment_ops(&job.sha256, false);
                    failed += 1;
                }
            }
        }

        budget = BYTES;
        for (i, job) in self
            .inner
            .store
            .attachment_downloads(FILES)
            .unwrap_or_default()
            .into_iter()
            .enumerate()
        {
            if i > 0 && job.size > budget {
                break;
            }
            // §8 的 resume：一次一个窗口，已拿到的部分写在 `.part` 上。
            // 于是"进程被杀 / 网断 / 这一轮预算用完了"都不会把进度清零 ——
            // 下一轮从 `have` 处接着要，而不是从头再拉一遍大文件。
            let sha = job.sha256.as_str();
            let part = self.inner.store.blob_part_path(sha);
            let mut have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
            if job.size >= 0 && have > job.size as u64 {
                // 比对象还长的 part 只能是坏数据（换过内容、或上一次写歪了）：
                // 留着它续传只会拼出一个永远校验不过的文件，不如重来。
                let _ = std::fs::remove_file(&part);
                have = 0;
            }
            match remote
                .fetch_attachment_window(sha, have, ATTACH_WINDOW)
                .await
            {
                Ok(Some(w)) => {
                    budget -= (w.bytes.len() as i64).min(budget);
                    // blob 目录是 `<attachments>/<2hex>/<sha>` 两层：`create(true)` 不会把父
                    // 目录顺手建出来，本机从没有过这个附件时就会写失败（实测踩过）。
                    if let Some(dir) = part.parent() {
                        let _ = std::fs::create_dir_all(dir);
                    }
                    // `w.from == 0` 意味着这一份是**整份对象**（服务器不支持 Range，或支持
                    // 探测却没理我们那个头）：这时必须覆盖，追加会拼出一份双份内容的文件。
                    let opened = if w.from == 0 {
                        std::fs::OpenOptions::new()
                            .create(true)
                            .write(true)
                            .truncate(true)
                            .open(&part)
                            .and_then(|mut f| f.write_all(&w.bytes))
                    } else {
                        std::fs::OpenOptions::new()
                            .create(true)
                            .append(true)
                            .open(&part)
                            .and_then(|mut f| f.write_all(&w.bytes))
                    };
                    if let Err(e) = opened {
                        tracing::warn!(%e, %sha, "半截附件写不进去，本轮放弃这个附件");
                        failed += 1;
                        continue;
                    }
                    let got = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
                    if got < w.total {
                        // 有进展但没完：不动 outbox（还挂着），下一轮接着要
                        tracing::debug!(%sha, got, total = w.total, "附件窗口推进中，未完成");
                        continue;
                    }
                    let bytes = match std::fs::read(&part) {
                        Ok(b) => b,
                        Err(e) => {
                            tracing::warn!(%e, %sha, "半截附件读不出来");
                            failed += 1;
                            continue;
                        }
                    };
                    // 拼完先自己核对一次哈希：服务器给错东西、或者中途混进了别的
                    // 窗口的字节，都必须在这里被拦下，而不是当成一份"完整的附件"落盘。
                    if notera_crypto::sha256_hex(&bytes) != sha {
                        tracing::warn!(%sha, "续传拼出来的附件哈希不符，丢弃半截文件");
                        let _ = std::fs::remove_file(&part);
                        let _ = self.inner.store.finish_attachment_ops(sha, false);
                        failed += 1;
                        continue;
                    }
                    if self.inner.store.ingest_blob(sha, &bytes).is_ok() {
                        let _ = std::fs::remove_file(&part);
                        let _ = self.inner.store.finish_attachment_ops(sha, true);
                        down += 1;
                    } else {
                        let _ = self.inner.store.finish_attachment_ops(sha, false);
                        failed += 1;
                    }
                }
                // 远端 404：只改远端态。§10 —— 任何情况下都不因远端缺失删本地
                Ok(None) => {
                    let _ = std::fs::remove_file(&part);
                    let _ =
                        self.inner
                            .store
                            .set_attachment_states(&job.sha256, None, Some("absent"));
                }
                Err(e) => {
                    tracing::warn!(%e, sha = %job.sha256, "附件下载失败");
                    failed += 1;
                }
            }
        }
        if up + down > 0 {
            // 附件到位 = 列表里的缩略图/占位要重画，走同一条事件回流
            self.emit(BusEvent::NotesChanged { ids: vec![] });
        }
        // 状态已满足却还挂着的附件单要关掉：同一份字节被反复引用时会重复入队，
        // 而队列按状态挑活，那些行永远不会被取出，也就永远没人结它 ——
        // 表现是"待同步"计数永久不掉（§18 要求这个数诚实）。
        match self.inner.store.settle_satisfied_attachment_ops() {
            Ok(n) if n > 0 => tracing::debug!(n, "附件待办已按状态结清"),
            Ok(_) => {}
            Err(e) => tracing::warn!(%e, "附件待办结清失败，计数可能虚高"),
        }
        (up, down, failed)
    }

    /// 常驻附件循环（壳 spawn 一次）。与文本调度器互不等待、互不阻塞。
    pub async fn run_attachments(
        self,
        remote: Arc<notera_webdav::WebDavRemote>,
        stop: Arc<AtomicBool>,
    ) {
        let mut ticker = tokio::time::interval(Duration::from_secs(20));
        loop {
            ticker.tick().await;
            if stop.load(Ordering::SeqCst) {
                return;
            }
            self.run_attachment_round(remote.as_ref()).await;
        }
    }

    pub fn remove_account(&self, id: &str) -> Result<(), CmdError> {
        let mut cfg = self.inner.config.lock().unwrap().clone();
        self.inner
            .config_repo
            .remove_account(&mut cfg, id)
            .map_err(|e| {
                CmdError::of("unknown_account", false)
                    .with(serde_json::json!({ "why": e.to_string() }))
            })?;
        self.inner.config_repo.save(&cfg).map_err(|e| {
            CmdError::of("save_failed", false).with(serde_json::json!({ "why": e.to_string() }))
        })?;
        *self.inner.config.lock().unwrap() = cfg;
        Ok(())
    }

    // ------------------------------------------------------------ 同步面 ---

    pub fn request_sync(&self) {
        self.inner.dirty_ticks.fetch_add(1, Ordering::SeqCst);
    }

    pub fn sync_status(&self) -> Result<SyncStatusDto, CmdError> {
        let v = self.inner.sync_view.lock().unwrap();
        let cfg = self.config();
        let pending = cfg
            .active_account
            .as_deref()
            .and_then(|a| {
                self.inner
                    .store
                    .outbox_len(
                        a,
                        &[
                            notera_store::OpState::Pending,
                            notera_store::OpState::Inflight,
                            notera_store::OpState::Failed,
                        ],
                    )
                    .ok()
            })
            .unwrap_or(0);
        let conflicts = self
            .inner
            .store
            .open_conflicts()
            .map(|v| v.len() as u32)
            .unwrap_or(0);
        Ok(SyncStatusDto {
            phase: format!("{:?}", v.phase).to_lowercase(),
            badge: match v.badge {
                Badge::Synced => "synced",
                Badge::Syncing => "syncing",
                Badge::Offline => "offline",
                Badge::Failed => "failed",
            }
            .into(),
            last_success_at: v.last_success_at.clone(),
            pending_ops: pending,
            open_conflicts: conflicts,
            message_key: v.message_key.clone(),
            retryable: v.retryable,
        })
    }

    /// 步骤 6：**首帧之后**才调用。返回一个可后台运行的调度句柄。
    pub fn start_sync<R: notera_sync::RemotePort + 'static>(self, remote: Arc<R>) -> Scheduler<R> {
        Scheduler {
            app: self,
            remote,
            stop: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(crate) fn local_port(&self) -> HostLocalPort {
        HostLocalPort(self.clone())
    }

    pub(crate) fn apply_round_stats(&self, st: &RoundStats, yielded: bool) {
        // 墙上时间只用于展示（I4/R3）；由 notera-core 的 Clock 统一供给，host 不引 chrono。
        let now = SystemClock.now().to_string();
        self.set_sync(|v| {
            v.in_flight = false;
            match st.outcome {
                notera_sync::RoundOutcome::Failed => {
                    v.badge = Badge::Failed;
                }
                notera_sync::RoundOutcome::Partial => {
                    // 本轮被请求预算截断 = **活还没干完**。以前它顺着 `_` 落进"已同步"，
                    // 于是一台正在追大库的设备（实测 5000 条要 26 轮）连着十几分钟显示
                    // "✓已同步"，而库里还差几千条 —— 队列侧的"待同步"计数帮不上忙：
                    // 纯拉的那一侧本来就没有待推的东西。徽标必须说实话。
                    v.badge = Badge::Syncing;
                    v.in_flight = true;
                }
                _ if yielded => {
                    // §11.4：本轮让路了。徽标留在"离线/待重试"，别报成已同步 ——
                    // 那会让用户以为改动已经公告出去，而它其实还在队列里。
                    v.badge = Badge::Offline;
                }
                _ => {
                    v.badge = Badge::Synced;
                    v.phase = Phase::Online;
                    v.message_key = None;
                    v.retryable = false;
                    v.last_success_at = Some(now.clone());
                }
            }
        });
        tracing::info!(
            outcome = ?st.outcome,
            requests = st.requests,
            bytes_up = st.bytes_up,
            bytes_down = st.bytes_down,
            pushed = st.pushed,
            pulled = st.pulled,
            conflicts = st.conflicts,
            cas_retries = st.cas_retries,
            "sync round"
        );
    }

    pub(crate) fn apply_sync_event(&self, e: SyncEvent) {
        match e {
            SyncEvent::Phase(p) => self.set_sync(|v| v.phase = p),
            SyncEvent::Progress { done, total } => {
                self.set_sync(|v| v.badge = Badge::Syncing);
                self.emit(BusEvent::Sync {
                    badge: Badge::Syncing,
                    progress: Some(Progress {
                        done,
                        total,
                        bytes: 0,
                    }),
                    error_code: None,
                });
            }
            SyncEvent::NeedsConflictAttention => self.emit(BusEvent::Toast {
                message_key: "sync.conflict_attention".into(),
                level: "warn".into(),
            }),
            // §11.4：让路不是失败，但必须看得见 —— "安静地不下公告"就是静默不同步。
            SyncEvent::Deferred { device } => {
                self.set_sync(|v| {
                    v.badge = Badge::Offline;
                    v.message_key = Some("sync.leaseHeld".into());
                    v.retryable = true;
                });
                tracing::info!(%device, "本轮让路给另一台正在写的设备（§11.4）");
            }
            SyncEvent::Completed(_) => {}
            SyncEvent::Failed {
                retryable,
                message_key,
            } => {
                let key = message_key.to_string();
                self.set_sync(move |v| {
                    v.badge = if key.ends_with("offline") {
                        Badge::Offline
                    } else {
                        Badge::Failed
                    };
                    v.message_key = Some(key);
                    v.retryable = retryable;
                });
            }
        }
    }

    /// 把本轮贴上的租约写进 `sync_state`（那两列自 0002 起就在，此前没人写它们）。
    /// 失败只记日志：它是诊断线索，不是正确性依赖。
    pub(crate) fn store_lease(&self, token: &str, expires_at: &str) -> Result<(), LocalError> {
        let Some(acct) = ConfigRepository::active(&self.config()).cloned() else {
            return Ok(());
        };
        let Ok(Some(mut st)) = self.inner.store.sync_state(&acct.id) else {
            return Ok(());
        };
        st.lease_token = Some(token.to_string());
        st.lease_expires_at = Some(expires_at.to_string());
        self.inner
            .store
            .set_sync_state(&st)
            .map_err(|e| LocalError::Storage(format!("租约状态写不进本地库: {e}")))?;
        Ok(())
    }

    pub(crate) fn set_manifest_cache(&self, wire: Vec<u8>) {
        *self.inner.cached_manifest.lock().unwrap() = Some(wire);
    }
    pub(crate) fn set_manifest_etag(&self, etag: Option<String>) {
        *self.inner.manifest_etag.lock().unwrap() = etag;
    }
    pub(crate) fn manifest_etag(&self) -> Option<String> {
        self.inner.manifest_etag.lock().unwrap().clone()
    }
    pub(crate) fn set_seq_applied(&self, seq: u64) {
        self.inner.seq_applied.store(seq, Ordering::SeqCst);
    }

    /// 实体的删除时刻：软删看 `notes/folders.deleted_at`，永久删除（行已不在）看墓碑。
    ///
    /// 为什么要给引擎这个值：`LocalView.deleted_at/purged_at` 是 P8/P11/P13 的判据，
    /// 全填 `None` 会让"本地已删 + 远端已改"被误判成普通编辑。
    fn delete_time(&self, kind: EntityKind, id: &EntityId) -> Result<Option<String>, LocalError> {
        let store = self.store();
        let row = match kind {
            EntityKind::Note => store.get_note(id).map_err(store_err)?.map(|n| n.deleted_at),
            EntityKind::Folder => store
                .get_folder(id)
                .map_err(store_err)?
                .map(|f| f.deleted_at),
            EntityKind::Attachment => None,
        };
        match row {
            // 行还在：以行的 deleted_at 为准（None = 已恢复，绝不能回落到旧墓碑）
            Some(deleted) => Ok(deleted),
            None => store
                .get_tombstone(kind, id)
                .map_err(store_err)
                .map(|t| t.map(|t| t.deleted_at)),
        }
    }
}

fn store_err(e: StoreError) -> LocalError {
    match e {
        StoreError::ReadOnly { .. } => LocalError::ReadOnly,
        other => LocalError::Storage(other.to_string()),
    }
}

/// 账户 → 下发给 UI 的视图（host 侧映射；字符串取值与 `api/types.ts` 的联合类型同名）。
///
/// `has_credential` 只看"有没有凭据引用"——配置里从来没有明文口令（DATA-MODEL §6 凭据行），
/// 所以这里不可能漏出口令，也不存在"下发明文"这条路。
fn account_dto(a: &AccountConfig) -> AccountDto {
    AccountDto {
        id: a.id.clone(),
        label: a.label.clone(),
        base_url: a.base_url.clone(),
        root_prefix: a.root_prefix.clone(),
        auth_kind: match a.auth_kind {
            notera_config::AuthKind::Basic => "basic",
            notera_config::AuthKind::Token => "token",
        }
        .into(),
        tls_policy: match a.tls_policy {
            TlsPolicyKind::Strict => "strict",
            TlsPolicyKind::CaBundle => "caBundle",
            TlsPolicyKind::Pin => "pin",
            TlsPolicyKind::InsecureLocal => "insecureLocal",
        }
        .into(),
        proxy_mode: match a.proxy.mode {
            ProxyMode::Direct => "direct",
            ProxyMode::System => "system",
            ProxyMode::Http => "http",
            ProxyMode::Https => "https",
            ProxyMode::Socks5 => "socks5",
        }
        .into(),
        proxy_host: a.proxy.host.clone(),
        proxy_port: a.proxy.port,
        bypass: a.proxy.bypass.clone(),
        enabled: a.enabled,
        has_credential: !a.credential_ref.is_empty(),
        username: a.username.clone(),
        cap_mask: None,
        write_strategy: None,
        caps_probed_at: None,
    }
}

/// 配置里的代理 → 出口层真正用的代理。
///
/// 两层的类型不同是有意的（配置只管持久化，传输决策归 `notera-net`），
/// 所以映射只能发生在组装根这里。引用了代理凭据而拿不到时**如实报错**：
/// 静默按无凭据连，用户看到的是"配了代理却 407"，比一条明确提示难查得多。
fn net_proxy(p: &ProxyProfile) -> Result<notera_net::ProxyProfile, CmdError> {
    if p.username_ref.is_some() || p.password_ref.is_some() {
        return Err(CmdError::of("proxy_credentials_pending", true));
    }
    let mode = match p.mode {
        ProxyMode::Direct => notera_net::ProxyMode::Direct,
        ProxyMode::System => notera_net::ProxyMode::System,
        ProxyMode::Http => notera_net::ProxyMode::Http,
        ProxyMode::Https => notera_net::ProxyMode::Https,
        ProxyMode::Socks5 => notera_net::ProxyMode::Socks5,
    };
    Ok(notera_net::ProxyProfile {
        mode,
        host: p.host.clone(),
        port: p.port,
        username: None,
        password: None,
        bypass: p.bypass.clone(),
        resolve_remote_dns: p.resolve_remote_dns,
    })
}

fn net_tls(acct: &AccountConfig) -> notera_net::TlsPolicy {
    match acct.tls_policy {
        TlsPolicyKind::Strict => notera_net::TlsPolicy::Strict,
        // 选了 CaBundle 却没给 PEM：退回严格校验，而不是"什么都不校验"。
        TlsPolicyKind::CaBundle => acct
            .ca_pem
            .clone()
            .filter(|s| !s.trim().is_empty())
            .map(notera_net::TlsPolicy::CaBundle)
            .unwrap_or(notera_net::TlsPolicy::Strict),
        TlsPolicyKind::Pin => {
            notera_net::TlsPolicy::Pin(acct.pinned_sha256.clone().unwrap_or_default())
        }
        TlsPolicyKind::InsecureLocal => notera_net::TlsPolicy::InsecureLocal,
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BootError {
    #[error("数据目录不可用: {0}")]
    Io(String),
    #[error("配置不可用: {0}")]
    Config(String),
    #[error("本地库不可用: {0}")]
    Store(#[source] StoreError),
}

// ------------------------------------------------------ Store → LocalPort ---

/// 把 Store 适配成同步引擎的 `LocalPort`。
///
/// 单独一个类型的好处：引擎与存储层的接口漂移会**只**在这里爆炸，
/// 而不是散落到同步逻辑各处。
pub struct HostLocalPort(App);

impl LocalPort for HostLocalPort {
    fn account_id(&self) -> String {
        self.0.config().active_account.unwrap_or_default()
    }
    fn device_id(&self) -> String {
        self.0.config().device_id
    }
    fn now(&self) -> String {
        SystemClock.now().to_string()
    }
    fn local_views(&self) -> Result<Vec<LocalView>, LocalError> {
        let acct = self.account_id();
        let rows = self
            .0
            .store()
            .dirty_entities(&acct)
            .map_err(|e| LocalError::Storage(e.to_string()))?;
        rows.into_iter()
            .map(|d| {
                // `DirtyEntity` 只带"为什么脏"，不带时间戳；删除/永久删除必须让引擎看得见
                // 是哪一种（P8/P11/P13 的判据），所以这里回查一次实体行/墓碑。
                let (deleted_at, purged_at) = match d.why {
                    notera_store::DirtyWhy::Deleted => (self.0.delete_time(d.kind, &d.id)?, None),
                    notera_store::DirtyWhy::Purged => {
                        let t = self.0.delete_time(d.kind, &d.id)?;
                        (t.clone(), t)
                    }
                    _ => (None, None),
                };
                Ok(LocalView {
                    kind: kind_tag(d.kind).into(),
                    id: d.id.to_string(),
                    rev: d.rev.get(),
                    sync_rev: d.sync_rev.get(),
                    sync_hash: None,
                    content_hash: d.content_hash,
                    deleted_at,
                    purged_at,
                    edited_after_delete: false,
                })
            })
            .collect()
    }
    /// 远端视图读的是**库里那张表**，不是进程内存：重启之后的第一轮照样要面对整份基线，
    /// 只活在内存里等于每次开机都重下一遍（也让 `cached_segment_hashes` 的跳过判据失去
    /// 前提 —— 跳过下载得有个地方把条目读回来）。
    fn cached_remote(&self) -> Result<Vec<RemoteView>, LocalError> {
        let acct = self.account_id();
        let rows = self
            .0
            .store()
            .remote_index_list(&acct)
            .map_err(|e| LocalError::Storage(e.to_string()))?;
        Ok(rows
            .into_iter()
            .map(|r| RemoteView {
                kind: kind_tag(r.kind).to_string(),
                // 附件的身份是 sha256（内容寻址），表里就存在 sha256 列
                id: r.sha256.clone().unwrap_or_else(|| r.id.to_string()),
                rev: r.rev.get(),
                hash: r.hash12.clone(),
                deleted_at: r.deleted_at.clone(),
                purged: r.purged,
            })
            .collect())
    }

    fn set_cached_remote(&self, entries: Vec<RemoteView>) {
        let acct = self.account_id();
        let nil = match EntityId::parse("00000000-0000-0000-0000-000000000000") {
            Ok(id) => id,
            Err(_) => return,
        };
        let rows: Vec<notera_store::RemoteIndexEntry> = entries
            .into_iter()
            .filter_map(|v| {
                let (kind, id, sha256) = match tag_kind(v.kind.as_str()) {
                    Some(EntityKind::Note) => {
                        (EntityKind::Note, EntityId::parse(&v.id).ok()?, None)
                    }
                    Some(EntityKind::Folder) => {
                        (EntityKind::Folder, EntityId::parse(&v.id).ok()?, None)
                    }
                    Some(EntityKind::Attachment) => {
                        (EntityKind::Attachment, nil.clone(), Some(v.id.clone()))
                    }
                    None => {
                        // 认不出的类型整条跳过：宁可下一轮重下，也不能把它写成别的实体的行
                        tracing::warn!(kind = v.kind, "远端视图里有未知类型，本条不缓存");
                        return None;
                    }
                };
                Some(notera_store::RemoteIndexEntry {
                    kind,
                    id,
                    rev: Rev(v.rev),
                    hash12: v.hash.clone(),
                    size: None,
                    deleted: v.deleted_at.is_some(),
                    purged: v.purged,
                    seg: None,
                    sha256,
                    deleted_at: v.deleted_at.clone(),
                })
            })
            .collect();
        // 写失败只是"下一轮多下一次"，不该把这一轮顶死（内容已经在本机落库了）
        if let Err(e) = self.0.store().remote_index_replace(&acct, &rows) {
            tracing::warn!(error = %e, "远端视图缓存写入失败：下一轮会重读，不影响正确性");
        }
    }

    /// 分段缓存读的是**库里存的那一份**，不是进程内存：调度器每 25 秒一轮，而重启之后
    /// 第一次追平照样要面对整份基线 —— 只活在内存里等于每次开机都重下一遍。
    fn cached_segment_hashes(&self) -> BTreeMap<String, String> {
        let acct = self.account_id();
        self.0.store().segment_hashes(&acct).unwrap_or_default()
    }
    fn set_cached_segment_hashes(&self, map: BTreeMap<String, String>) {
        let acct = self.account_id();
        // 写失败只让下一轮多下一次分段，不该把这一轮顶死（引擎已经拿到内容并落库了）
        if let Err(e) = self.0.store().set_segment_hashes(&acct, &map) {
            tracing::warn!(error = %e, "分段哈希缓存写入失败：下一轮会重读，不影响正确性");
        }
    }
    /// 干净行也要给：只报脏行的话，一台追平了的设备每轮都会把整份清单重下一遍
    /// （见 `Store::synced_heads` 的注释里那条实测 196/260 的卡死）。
    fn synced_heads(&self) -> BTreeMap<(String, String), (u64, String)> {
        self.0
            .store()
            .synced_heads()
            .unwrap_or_default()
            .into_iter()
            .map(|(kind, id, rev, hash)| {
                ((kind_tag(kind).into(), id.to_string()), (rev.get(), hash))
            })
            .collect()
    }
    fn seq_applied(&self) -> u64 {
        self.0.inner.seq_applied.load(Ordering::SeqCst)
    }
    fn revision_json(&self, id: &str, rev: u64) -> Result<Option<serde_json::Value>, LocalError> {
        let eid = EntityId::parse(id).map_err(|_| LocalError::Storage("bad id".into()))?;
        self.0
            .store()
            .revision_doc(&eid, Rev(rev))
            .map_err(|e| LocalError::Storage(e.to_string()))
    }
    fn envelope_wire(&self, kind: &str, id: &str) -> Result<Option<Vec<u8>>, LocalError> {
        let eid = EntityId::parse(id)
            .map_err(|_| LocalError::Storage(format!("outbox 里的 id 不是 UUID: {id}")))?;
        let wire = match EntityKind::from_tag(kind) {
            // 附件按 sha256 寻址、走独立队列（SYNC-PROTOCOL §13），不是这里的 UUID 实体
            Some(EntityKind::Note) => self.0.store().note_envelope_wire(&eid),
            Some(EntityKind::Folder) => self.0.store().folder_envelope_wire(&eid),
            Some(EntityKind::Attachment) => Ok(None),
            // 认不出的类型标记：绝不当成笔记上传（那会把别的东西写进 note/ 目录）
            None => return Err(LocalError::Storage(format!("未知的实体类型标记: {kind}"))),
        }
        .map_err(store_err)?;
        Ok(wire)
    }
    fn apply(&self, ops: Vec<ApplyOp>) -> Result<notera_sync::ApplyReport, LocalError> {
        let mut mapped = Vec::new();
        for o in ops {
            match o {
                ApplyOp::Upsert { kind, id, wire } => {
                    let env: serde_json::Value = serde_json::from_slice(&wire).map_err(|e| {
                        LocalError::Storage(format!("远端记录 {kind}/{id} 不是合法 JSON: {e}"))
                    })?;
                    match kind.as_str() {
                        "f" => mapped.push(StoreApplyOp::UpsertFolder { env }),
                        _ => mapped.push(StoreApplyOp::UpsertNote { env }),
                    }
                }
                ApplyOp::ConflictPayload { kind, id, wire } => {
                    // 只把服务器那一版的原始字节登记到冲突行上，**不碰任何笔记**：
                    // P11 保留哪一边由用户决定（CONFLICT-RESOLUTION §5.1.1）。
                    // 命中 0 行是合法的（冲突刚被裁决掉），这里不报错也不补建行。
                    let entity = if kind == "f" {
                        notera_core::EntityKind::Folder
                    } else {
                        notera_core::EntityKind::Note
                    };
                    let ok = match notera_core::EntityId::parse(&id) {
                        Ok(eid) => self
                            .0
                            .store()
                            .conflict_attach_payload(&self.account_id(), entity, &eid, &wire)
                            .map_err(|e| LocalError::Storage(e.to_string()))?,
                        Err(_) => {
                            return Err(LocalError::Storage(format!(
                                "冲突载荷里的 id 不合法：{id}"
                            )))
                        }
                    };
                    if ok == 0 {
                        tracing::debug!("冲突载荷没挂上（{kind}/{id} 已无未裁决冲突）");
                    }
                }
                ApplyOp::AdoptConflict { kind, id, wire } => {
                    // 只有笔记有"正文"可采纳；文件夹的冲突留给用户（引擎也只会对 n 发这条）
                    let env: serde_json::Value = serde_json::from_slice(&wire).map_err(|e| {
                        LocalError::Storage(format!("冲突记录 {kind}/{id} 不是合法 JSON: {e}"))
                    })?;
                    if kind == "n" {
                        mapped.push(StoreApplyOp::AdoptConflict { env });
                    }
                }
                ApplyOp::SetRemote {
                    kind,
                    id,
                    rev,
                    hash12,
                } => {
                    // 引擎的"这一版清单我已经 reconcile 到这儿了"标记。它是进程内的账，
                    // 304 快路径靠它把"远端没变"和"本机已追平"分开 —— 搞混过一次就会
                    // 出现"账上干净、库里少 63 条、徽标说已同步"那种谎报。
                    // 重启后归零是**偏保守**的方向：宁可多算一轮全量计划，不可少算。
                    if kind == "seq" && id == "applied" {
                        self.0.set_seq_applied(rev);
                    }
                    if let (Some(k), Ok(i)) = (EntityKind::from_tag(&kind), EntityId::parse(&id)) {
                        mapped.push(StoreApplyOp::SetRemote {
                            kind: k,
                            id: i,
                            rev: Rev(rev),
                            hash12,
                        });
                    }
                }
                ApplyOp::MarkSynced { kind, id, rev } => {
                    if let (Some(k), Ok(i)) = (EntityKind::from_tag(&kind), EntityId::parse(&id)) {
                        let _ = self.0.store().mark_synced(k, &i, Rev(rev), "");
                    }
                }
                ApplyOp::Delete { kind, id, rev } | ApplyOp::Tombstone { kind, id, rev, .. } => {
                    if let (Some(k), Ok(i)) = (EntityKind::from_tag(&kind), EntityId::parse(&id)) {
                        mapped.push(StoreApplyOp::Tombstone {
                            kind: k,
                            id: i,
                            rev: Rev(rev),
                        });
                    }
                }
                ApplyOp::Purge { kind, id } => {
                    if let (Some(k), Ok(i)) = (EntityKind::from_tag(&kind), EntityId::parse(&id)) {
                        mapped.push(StoreApplyOp::Purge { kind: k, id: i });
                    }
                }
                ApplyOp::StoreManifest { wire, etag, seq: _ } => {
                    self.0.set_manifest_cache(wire);
                    self.0.set_manifest_etag(etag);
                    // 这里**不**记 `seq_applied`。那个标记的语义是"本机已经把这个清单
                    //  reconcile 完了"，而只有引擎知道本轮有没有被请求预算截断在下载中途。
                    // 以前在这里顺手写掉，代价是：260 条的库里第二台设备拉了 197 条就被截断，
                    // seq 却已落账 → 下一轮拿到 304 判"无事可做" → 剩下 63 条永远不来。
                    // 判据移到 `notera-sync` 的收尾（见 late_device 测试）。
                }
            }
        }
        let rep = self
            .0
            .store()
            .apply_remote(&mapped)
            .map_err(|e| LocalError::Storage(e.to_string()))?;
        Ok(notera_sync::ApplyReport {
            applied: rep.applied,
            rejected: rep.skipped,
        })
    }
    /// 引擎已经判完"这是冲突"，这里只做**记账字段**的翻译（不含任何判定）。
    ///
    /// `base_rev` 取 `sync_rev`：DATA-MODEL §4.3 定义共同祖先就是"最后确认一致的那一版"。
    fn record_conflict(
        &self,
        d: &Decision,
        l: &LocalView,
        r: &RemoteView,
    ) -> Result<(), LocalError> {
        let kind = EntityKind::from_tag(&l.kind)
            .ok_or_else(|| LocalError::Storage(format!("未知 kind: {}", l.kind)))?;
        let id = EntityId::parse(&l.id).map_err(|e| LocalError::Storage(e.to_string()))?;
        // 同一对哈希已经有张未处理的卡片 → 这一轮什么都不做。不这么做就会每轮多一张卡片、
        // 每轮多造一篇"本地副本"（P11 这类引擎故意不收敛的判定实测真出现过两篇同名副本）。
        if self
            .0
            .store()
            .open_conflict_exists(
                kind,
                &id,
                &l.content_hash,
                r.hash.as_deref().unwrap_or_default(),
            )
            .map_err(store_err)?
        {
            tracing::debug!(rule = d.rule, id = %l.id, "这条冲突已有未处理卡片，跳过重复登记");
            return Ok(());
        }
        // CONFLICT-RESOLUTION §6：进收件箱的**同一刻**先把本地未合并版本存一份副本。
        // 用户之后无论选哪一边，这一份都不会丢 —— 副本不是提醒，是保底。
        let mut copy_note_id = None;
        if kind == EntityKind::Note {
            if let Some(n) = self.0.store().get_note(&id).map_err(store_err)? {
                let mut doc = n.doc.clone();
                let head = serde_json::json!({
                    "id": format!("copy-{}", &n.id.as_str()[..8]),
                    "type": "heading",
                    "attrs": { "level": 1 },
                    "content": [{ "text": format!("{}（本地副本）", n.title) }],
                });
                if let Some(blocks) = doc.get_mut("content").and_then(|b| b.as_array_mut()) {
                    blocks.insert(0, head);
                }
                match self.0.store().create_note(&n.folder_id.clone(), doc) {
                    Ok(copy) => copy_note_id = Some(copy.id),
                    // 副本失败绝不升级为"丢掉冲突记录"：冲突仍然进箱，用户仍能看到双方 rev。
                    Err(e) => tracing::warn!(?e, "冲突副本创建失败，冲突记录仍然入账"),
                }
            }
        }
        let rec = notera_store::ConflictRecord {
            account_id: self.account_id(),
            kind,
            id: id.clone(),
            base_rev: Rev(l.sync_rev),
            local_rev: Rev(l.rev),
            remote_rev: Rev(r.rev),
            local_hash: l.content_hash.clone(),
            remote_hash: r.hash.clone().unwrap_or_default(),
            auto_merged: false,
            copy_note_id,
        };
        let conflict_id = self.0.store().record_conflict(&rec).map_err(store_err)?;
        let title = self
            .0
            .store()
            .get_note(&id)
            .map_err(store_err)?
            .map(|n| n.title)
            .unwrap_or_else(|| "（无标题）".into());
        self.0.emit(crate::BusEvent::Conflict {
            conflict_id,
            note_title: title,
        });
        // 判定行号（P8/P10/P15…）只进日志：它是排障线索，不是 UI 词汇
        tracing::debug!(rule = d.rule, action = ?d.action, id = %l.id, conflict_id, "冲突已记账");
        Ok(())
    }
    fn outbox_take(&self, limit: usize) -> Result<Vec<notera_sync::OutboxItem>, LocalError> {
        let acct = self.account_id();
        let rows = self
            .0
            .store()
            .outbox_take(&acct, limit)
            .map_err(store_err)?;
        Ok(rows
            .into_iter()
            .map(|o| notera_sync::OutboxItem {
                dedupe_key: o.dedupe_key,
                kind: kind_tag(o.kind).into(),
                // `entity_key` 是 `sync_operations.entity_id` 原值：note/folder 是 UUID，
                // 附件是 64hex（`SyncOperation::id_` 那种情况下是 nil UUID，不能用）。
                id: o.entity_key,
                rev: o.payload_rev.map(|r| r.get()).unwrap_or(0),
                op: o.op.as_str().to_string(),
            })
            .collect())
    }
    /// 结清某个实体某 rev 的待办。以前这里是**空实现**（注释说"Store 缺按键定位的能力"），
    /// 后果是每一行都永远停在 `inflight`：设置页的"待同步"计数不会掉，`sync_operations`
    /// 只增不清，而崩溃恢复说的"重放"也无从判断从哪重放。
    /// 现在按 `(账户, kind, id, rev)` 精确结清 —— 匹配不到就记一条 warn，
    /// 绝不"猜一行"来标完成。
    ///
    /// 引擎给的 `kind` 是**线上短标记**（n/f/a，用于远端路径），而 `sync_operations.entity_type`
    /// 存的是长标记。这里就是这两种词汇唯一的翻译点（`local_views`/`outbox_take` 是反向那一条边）；
    /// 认不出的标记直接放弃并留痕，不去猜数据库里叫什么。
    fn outbox_settle(
        &self,
        kind: &str,
        id: &str,
        rev: u64,
        st: notera_sync::OutboxState,
    ) -> Result<(), LocalError> {
        let Some(entity_kind) = EntityKind::from_tag(kind) else {
            tracing::warn!(kind, id, rev, state = ?st, "待办结清收到未知的 kind 标记，已放弃（不猜词汇）");
            return Ok(());
        };
        let Some(acct) = ConfigRepository::active(&self.0.config()).cloned() else {
            return Ok(());
        };
        let mapped = match st {
            notera_sync::OutboxState::Pending => notera_store::OpState::Pending,
            notera_sync::OutboxState::Inflight => notera_store::OpState::Inflight,
            notera_sync::OutboxState::Done => notera_store::OpState::Done,
            notera_sync::OutboxState::Failed => notera_store::OpState::Failed,
            notera_sync::OutboxState::Superseded => notera_store::OpState::Superseded,
            notera_sync::OutboxState::Blocked => notera_store::OpState::Blocked,
        };
        let found = self
            .0
            .store()
            .outbox_settle(&acct.id, entity_kind, id, rev as i64, mapped)
            .map_err(store_err)?;
        if !found {
            tracing::warn!(account = %acct.id, kind, id, rev, state = ?st, "待办结清没找到匹配行（outbox 里这一条的状态不会变）");
        }
        Ok(())
    }
    /// §11.4：把本轮贴上的租约记进 sync_state 的两列（诊断用）。
    fn record_lease(&self, token: &str, expires_at: &str) -> Result<(), LocalError> {
        self.0.store_lease(token, expires_at)
    }
    fn cached_manifest(&self) -> Option<Vec<u8>> {
        self.0.inner.cached_manifest.lock().unwrap().clone()
    }
}

fn kind_tag(k: EntityKind) -> &'static str {
    match k {
        EntityKind::Note => "n",
        EntityKind::Folder => "f",
        EntityKind::Attachment => "a",
    }
}

/// [`kind_tag`] 的逆函数。远端视图的读写**两侧都必须走这一对**：
/// 写侧当初按 `"note"`/`"folder"` 匹配，而引擎给的是线上标签 `"n"`/`"f"`/`"a"`，
/// 于是整表被"未知类型"静默跳过 —— 表永远是空的，而分段哈希缓存已经说了"这段我有了"，
/// 结果第二台设备停在 198/1000（`big_library` 当场抓到）。词汇漂移过一次，就别再给第二次机会。
fn tag_kind(tag: &str) -> Option<EntityKind> {
    match tag {
        "n" => Some(EntityKind::Note),
        "f" => Some(EntityKind::Folder),
        "a" => Some(EntityKind::Attachment),
        _ => None,
    }
}
// ------------------------------------------------------------- 调度器 ---

/// 把 `Arc<R>` 借给"这一轮"的引擎用。
///
/// 为什么需要这层纯转发：`SyncEngine::new` 按值收 port，而 `R` 是壳层选的适配器类型，
/// host 无权给它加 `Clone` 约束 —— 加了就等于改 `App::start_sync::<R>(Arc<R>)` 的签名。
struct RemoteBorrow<'a, R>(&'a R);

#[async_trait::async_trait]
impl<R: notera_sync::RemotePort> notera_sync::RemotePort for RemoteBorrow<'_, R> {
    async fn fetch_manifest(
        &self,
        etag: Option<&str>,
    ) -> Result<Option<(Vec<u8>, Option<String>)>, notera_sync::RemoteError> {
        self.0.fetch_manifest(etag).await
    }
    async fn fetch_segment(
        &self,
        name: &str,
    ) -> Result<Vec<notera_sync::manifest::EntryRef>, notera_sync::RemoteError> {
        self.0.fetch_segment(name).await
    }
    async fn fetch_record(
        &self,
        kind: &str,
        id: &str,
    ) -> Result<Option<Vec<u8>>, notera_sync::RemoteError> {
        self.0.fetch_record(kind, id).await
    }
    async fn put_record(
        &self,
        kind: &str,
        id: &str,
        wire: &[u8],
        if_match: Option<&str>,
    ) -> Result<notera_sync::Commit, notera_sync::RemoteError> {
        self.0.put_record(kind, id, wire, if_match).await
    }
    async fn commit_manifest(
        &self,
        wire: &[u8],
        cas_etag: Option<&str>,
    ) -> Result<Option<String>, notera_sync::RemoteError> {
        self.0.commit_manifest(wire, cas_etag).await
    }
    async fn put_segment(&self, name: &str, wire: &[u8]) -> Result<(), notera_sync::RemoteError> {
        self.0.put_segment(name, wire).await
    }
    async fn probe_record_etag(
        &self,
        kind: &str,
        id: &str,
    ) -> Result<Option<String>, notera_sync::RemoteError> {
        self.0.probe_record_etag(kind, id).await
    }
    // 这两条必须**显式转发**。漏一条 = 租约静默空转，而"空转的并发保护"比没有更糟：
    // 界面会以为自己有让路能力。
    async fn lease_publish(
        &self,
        token: &str,
        expires_at: &str,
        seq: u64,
    ) -> Result<(), notera_sync::RemoteError> {
        self.0.lease_publish(token, expires_at, seq).await
    }
    async fn lease_holders(
        &self,
        known: &[String],
    ) -> Result<Vec<notera_sync::PeerLease>, notera_sync::RemoteError> {
        self.0.lease_holders(known).await
    }
}

/// 桌面 debounce 2.5s + 周期 25s + 事件触发（docs/SYNC-PROTOCOL.md §14）。
pub struct Scheduler<R: notera_sync::RemotePort + 'static> {
    app: App,
    remote: Arc<R>,
    stop: Arc<AtomicBool>,
}

impl<R: notera_sync::RemotePort + 'static> Scheduler<R> {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    /// 在后台任务里跑。空轮 = 1 请求 0 字节，因此 25s 轮询不构成负担。
    ///
    /// 但 `Partial`（本轮被请求预算截断，活没干完）**不能**跟着 25 秒的节拍走：
    /// 实测 5000 条库要 26 轮才追平，25 秒一轮就是 11 分钟的"看起来已同步、其实还在下"。
    /// 所以截断就立刻续跑，只留 1 秒喘息（避免错误型 Partial 在此热转圈）。
    pub async fn run(self) {
        let app = self.app.clone();
        let remote = self.remote.clone();
        let stop = self.stop.clone();
        let mut ticker = tokio::time::interval(Duration::from_secs(25));
        loop {
            tokio::select! {
                _ = ticker.tick() => {}
                _ = wait_dirty(&app) => {}
                _ = stop_signal(&stop) => break,
            }
            if stop.load(Ordering::SeqCst) {
                break;
            }
            let mut more = should_drain(app.run_round(&remote).await.as_ref());
            while more {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(1)) => {}
                    _ = stop_signal(&stop) => return,
                }
                if stop.load(Ordering::SeqCst) {
                    return;
                }
                more = should_drain(app.run_round(&remote).await.as_ref());
            }
        }
    }
}

impl App {
    fn set_sync_public(&self, b: Badge) {
        self.set_sync(|v| v.badge = b);
    }
}

/// 被预算截断的一轮要不要**立刻**续跑，而不是等 25 秒下一拍。
///
/// 判据是"这一轮有没有真的推进"，不是"结果是不是 `Partial`"：`Partial` 也会由
/// 清单条目 404、单次请求出错这类情况产生，那种时候 1 秒一轮就是无界热转圈 ——
/// 手机上就是耗电耗流量。所以有进展就连跑（实测 5000 条库的追平因此从 ~11 分钟
/// 降到 ~96 秒），没进展就退回常规节拍，交给退避与徽标去说话。
fn should_drain(stats: Option<&notera_sync::RoundStats>) -> bool {
    let Some(st) = stats else { return false };
    st.outcome == notera_sync::RoundOutcome::Partial && st.pushed + st.pulled > 0
}

async fn wait_dirty(app: &App) {
    // 每次 `select!` 都新建这个 future，所以 `last` 就是"进入等待时"的刻度；
    // 看到变化后再 debounce 2.5s（§14：停止输入才推）。
    let last = app.inner.dirty_ticks.load(Ordering::SeqCst);
    loop {
        tokio::time::sleep(Duration::from_millis(250)).await;
        if app.inner.dirty_ticks.load(Ordering::SeqCst) != last {
            tokio::time::sleep(Duration::from_millis(2500)).await; // debounce
            return;
        }
    }
}

async fn stop_signal(stop: &Arc<AtomicBool>) {
    while !stop.load(Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

// ---------------------------------------------------------------- 测试 ---

#[cfg(test)]
mod tests {
    #[test]
    fn drain_only_follows_real_progress() {
        let st = |outcome, pushed, pulled| notera_sync::RoundStats {
            requests: 200,
            bytes_up: 4096,
            bytes_down: 4096,
            pushed,
            pulled,
            conflicts: 0,
            cas_retries: 0,
            outcome,
        };
        use notera_sync::RoundOutcome as O;
        // 被截断但确有推进：立刻续跑（5000 条库的追平靠的就是这个）
        assert!(
            should_drain(Some(&st(O::Partial, 0, 197))),
            "追平中的截断轮次被当成了不用续跑"
        );
        assert!(
            should_drain(Some(&st(O::Partial, 100, 0))),
            "推送中的截断轮次被当成了不用续跑"
        );
        // 没推进的 Partial（404、请求出错）：退回常规节拍，不许 1 秒一轮热转圈
        assert!(
            !should_drain(Some(&st(O::Partial, 0, 0))),
            "没有进展的 Partial 会引发无界续跑"
        );
        assert!(
            !should_drain(Some(&st(O::Converged, 3, 3))),
            "干完的一轮不该再续"
        );
        assert!(!should_drain(Some(&st(O::NoOp, 0, 0))), "空轮不该再续");
        assert!(
            !should_drain(Some(&st(O::Failed, 0, 0))),
            "失败的一轮交给退避，不该立刻续"
        );
        // 并发里另一条轮次占着时 run_round 返回 None：同样不许续
        assert!(!should_drain(None), "本轮没跑成不该续跑");
    }

    use super::*;
    use notera_core::next_rev;
    use notera_sync::plan::{Action, ConflictKind};
    use serde_json::{json, Value};

    fn tmpdir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("notera-host-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn boot(tag: &str) -> App {
        App::boot(&tmpdir(tag)).expect("boot")
    }

    fn doc(text: &str) -> serde_json::Value {
        json!({ "v": 1, "content": [{ "id": "b1", "type": "paragraph", "content": [{ "text": text }] }] })
    }

    fn keys(v: &serde_json::Value) -> Vec<String> {
        v.as_object().expect("对象").keys().cloned().collect()
    }

    fn sorted(mut v: Vec<String>) -> Vec<String> {
        v.sort();
        v
    }

    /// 首帧的诚实起点：没配置过账户就绝不是"已同步"，也绝不联网（PLATFORM §3）。
    #[test]
    fn boot_starts_unconfigured_and_offline() {
        let app = boot("boot");
        let s = app.sync_status().unwrap();
        assert_eq!(
            (s.phase.as_str(), s.badge.as_str()),
            ("unconfigured", "offline")
        );
        assert!(!s.retryable);
        assert_eq!(s.open_conflicts, 0);
        assert!(
            app.current_account().unwrap().is_none(),
            "没配过账户就没有活动账户"
        );
    }

    /// DTO 的字段名就是前端契约（`apps/desktop/src/api/types.ts`），漂一个字母 UI 就静默显示 undefined。
    #[test]
    fn note_and_list_dto_field_names_match_the_frontend_contract() {
        let app = boot("dto");
        let folder = app.list_folders().unwrap().remove(0);
        let created = app
            .create_note(&EntityId::parse(&folder.id).unwrap(), doc("契约检查"))
            .unwrap();
        let detail = commands::j(&created).unwrap();
        assert_eq!(
            sorted(keys(&detail)),
            sorted(
                [
                    "id",
                    "folderId",
                    "doc",
                    "docFormat",
                    "title",
                    "summary",
                    "charCount",
                    "blockCount",
                    "hasAttachment",
                    "pinned",
                    "color",
                    "rev",
                    "contentHash",
                    "createdAt",
                    "updatedAt",
                    "deletedAt"
                ]
                .map(str::to_string)
                .to_vec()
            )
        );
        assert_eq!(
            detail["title"], "契约检查",
            "title 由 doc 派生（DATA-MODEL §7.1）"
        );

        let rows = app
            .list_notes(ListNotesCmd {
                folder_id: None,
                trash: false,
                limit: 10,
                offset: 0,
            })
            .unwrap();
        let row = rows.first().expect("列表必须有一行");
        assert_eq!(
            sorted(keys(row)),
            sorted(
                [
                    "id",
                    "folderId",
                    "folderName",
                    "title",
                    "summary",
                    "charCount",
                    "hasAttachment",
                    "pinned",
                    "updatedAt",
                    "deletedAt",
                    "dirty"
                ]
                .map(str::to_string)
                .to_vec()
            )
        );
        assert_eq!(row["folderName"], folder.name);
        assert_eq!(row["dirty"], true, "新笔记未确认过 = 待上传");
        assert!(row.get("doc").is_none(), "列表绝不带正文（DATA-MODEL §13）");
    }

    /// 一个 StoreError 一个码：塌成 catch-all 就等于告诉用户"存储坏了"。
    #[test]
    fn every_store_error_keeps_its_own_code() {
        let cases: Vec<(StoreError, &str)> = vec![
            (
                StoreError::NotFound {
                    kind: EntityKind::Note,
                    id: EntityId::new(),
                },
                "not_found",
            ),
            (
                notera_core::error::StaleEdit {
                    entity: EntityId::new(),
                    expected: Rev(3),
                    actual: Rev(5),
                }
                .into(),
                "stale_edit",
            ),
            (
                StoreError::ReadOnly {
                    db: 9,
                    supported: 3,
                },
                "db_too_new",
            ),
            (
                StoreError::Migration {
                    from: 2,
                    to: 3,
                    detail: "x".into(),
                },
                "db_migration",
            ),
            (
                StoreError::DocTooNew {
                    doc: 9,
                    supported: 1,
                },
                "doc_too_new",
            ),
            (StoreError::Constraint("名字为空".into()), "constraint"),
            (StoreError::InvalidDoc("坏文档".into()), "invalid_doc"),
            (StoreError::Rejected("远端拒绝".into()), "rejected"),
            (StoreError::Rich("富文本层拒绝".into()), "richtext"),
            (StoreError::Io(std::io::Error::other("磁盘满了")), "io"),
            (
                StoreError::Identity(notera_core::IdentityError::Malformed("x".into())),
                "bad_id",
            ),
        ];
        let mut seen = std::collections::BTreeSet::new();
        for (e, want) in cases {
            let code = CmdError::from(e).code;
            assert_eq!(code, want, "{code} 不是预期的码");
            assert!(seen.insert(code.clone()), "错误码 {code} 重复了");
        }
        // retryable 只许给"重试可能成功"的那一类
        let io = CmdError::from(StoreError::Io(std::io::Error::other("临时故障")));
        assert!(io.retryable, "IO 故障必须标成可重试");
        let constraint = CmdError::from(StoreError::Constraint("x".into()));
        assert!(!constraint.retryable, "约束失败重试也是失败");
    }

    #[test]
    fn prefs_roundtrip_through_the_command_surface() {
        let app = boot("prefs");
        commands::dispatch(&app, "set_pref", json!({ "key": "theme", "value": "dark" })).unwrap();
        commands::dispatch(&app, "set_pref", json!({ "key": "fontSize", "value": 15 })).unwrap();
        let got = commands::dispatch(&app, "get_prefs", json!({})).unwrap();
        assert_eq!(got["theme"], "dark");
        assert_eq!(got["fontSize"], 15);
        // 偏好不是内容：不该产生任何待上传的东西
        assert_eq!(app.store().stats().unwrap().dirty_notes, 0);
    }

    #[test]
    fn export_honors_the_requested_path_and_the_attachment_flag_in_both_envelope_shapes() {
        // 这条边的两条要求都关乎数据：
        // 1) 用户指哪儿就写哪儿（写错位置就是"备份不存在"）；
        // 2) `includeAttachments` 必须真的决定 ZIP 里有没有附件 —— 勾了却没带，
        //    用户以为手里有一份完整备份，实际缺全部附件。
        // 封套形状：命令面在这两条上同时接受平铺与 `{req:…}`（dispatch 里剥一层），
        // 这里把两种都钉住，免得日后单改一边把另一种悄悄变成"全部退回默认值"。
        let app = boot("export-scope");
        let dir = tmpdir("export-target");
        let folder = app.default_folder_id().unwrap();
        let note = app.create_note(&folder, doc("带一张图的笔记")).unwrap();
        let blob: Vec<u8> = b"PNG-ish bytes".to_vec();
        let sha = app
            .store()
            .attach_blob(
                &EntityId::parse(&note.id).unwrap(),
                &blob,
                "image/png",
                Some("pic.png"),
                "b1",
            )
            .unwrap()
            .sha256;
        // 附件按 `<attachments>/<2hex>/<sha>` 两层分片落盘，所以这里既是在证明
        // "真有一条 blob"，也是在证明导出没有靠"扫一层目录名"去猜它在哪。
        assert!(
            app.store().blob_path(&sha).exists(),
            "blob 就该在分片目录里"
        );

        for (label, name, body) in [
            (
                "平铺",
                "平铺.zip",
                json!({ "path": dir.join("平铺.zip").to_string_lossy(), "includeAttachments": true }),
            ),
            (
                "req 包一层",
                "包一层.zip",
                json!({ "req": { "path": dir.join("包一层.zip").to_string_lossy(), "includeAttachments": true } }),
            ),
        ] {
            let got = commands::dispatch(&app, "export_data", body)
                .unwrap_or_else(|e| panic!("{label} 形态导出失败：{e:?}"));
            let want = dir.join(name).to_string_lossy().replace('\\', "/");
            assert_eq!(
                got["path"].as_str().unwrap_or_default().replace('\\', "/"),
                want,
                "{label}：输出路径必须按用户指的写"
            );
            let out = Path::new(got["path"].as_str().unwrap());
            assert!(out.exists(), "{label}：报告说有文件，盘上就得有");
            assert_eq!(
                got["counts"]["attachments"].as_u64(),
                Some(1),
                "{label}：勾了包含附件就必须真带上"
            );
            // 报告里的数还得和包里的字节对得上
            let back = notera_importer::read_bundle(out)
                .unwrap_or_else(|e| panic!("{label}：包读回来就失败：{e:?}"));
            assert_eq!(
                back.attachments.len(),
                1,
                "{label}：ZIP 里必须真有那一个附件"
            );
            assert_eq!(back.attachments[0].0, sha, "{label}：附件键就是 sha256");
            assert_eq!(
                back.attachments[0].1, blob,
                "{label}：附件字节必须逐字节相同"
            );
        }
        // 没勾就不该带上（体积与"完整备份"的语义要能区分）
        let bare = commands::dispatch(
            &app,
            "export_data",
            json!({ "path": dir.join("不含附件.zip").to_string_lossy() }),
        )
        .unwrap();
        assert_eq!(
            bare["counts"]["attachments"].as_u64(),
            Some(0),
            "默认不含附件"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn import_honors_the_requested_mode_in_both_envelope_shapes() {
        // 导入模式是会动用户数据的决定：`merge` 与 `intoEmpty` 走的是两条不同的
        // 判定，静默换成默认值等于替用户做了决定。这里用一个不存在的包让两种形态
        // 都在"读文件"这一步失败 —— 失败信息里必须带着用户要的那个 mode。
        let app = boot("import-scope");
        for body in [
            json!({ "path": "不存在.zip", "mode": "merge" }),
            json!({ "req": { "path": "不存在.zip", "mode": "merge" } }),
        ] {
            let r = commands::dispatch(&app, "import_data", body);
            let e = r.expect_err("包不存在必须失败，不能静默当成空导入");
            assert_eq!(e.code, "save_failed", "读不到包是 IO 类失败：{e:?}");
        }
    }

    #[test]
    fn importing_ones_own_export_advances_nothing_and_invents_no_conflict() {
        // 端到端里偶发过一次 `edit_note → 400 stale_edit（expected 7，actual 8）`，
        // 而那次正好发生在"导出→把同一个包导回同一个库"之后。若自导入会把本地
        // 笔记的 rev 顶高一格，编辑器手里的 rev 就变成旧的，用户看到的就是
        // "这条笔记在别处被改动了" —— 而那个"别处"是我们自己的导入。
        let app = boot("self-import");
        let folder = app.default_folder_id().unwrap();
        let note = app.create_note(&folder, doc("自导入不该制造冲突")).unwrap();
        let before = app
            .store()
            .get_note(&EntityId::parse(&note.id).unwrap())
            .unwrap()
            .unwrap();
        let out = tmpdir("self-import").join("库.zip");
        commands::dispatch(
            &app,
            "export_data",
            json!({ "path": out.to_string_lossy(), "includeAttachments": true }),
        )
        .unwrap();
        let report = commands::dispatch(
            &app,
            "import_data",
            json!({ "path": out.to_string_lossy(), "mode": "merge" }),
        )
        .unwrap();
        let after = app
            .store()
            .get_note(&EntityId::parse(&note.id).unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(after.rev, before.rev, "把同一个包导回同一个库不许推进任何本地 rev（rev 一动，编辑器的 expectedRev 就过期 → 假冲突）");
        assert_eq!(after.content_hash, before.content_hash, "内容也不许变");
        assert_eq!(
            app.store().open_conflicts().unwrap().len(),
            0,
            "自导入不许造出冲突：{report:?}"
        );
        let _ = std::fs::remove_dir_all(out.parent().unwrap());
    }

    #[test]
    fn attachment_bytes_may_arrive_as_base64_and_come_back_identical() {
        // 编辑器选不到文件时，这条路是"前端读字节 → 核心算 sha 并落盘"。
        // 关键是**核心仍然是唯一的写入口**：sha 由核心算、盘由核心写、库由核心记，
        // 前端给的字节只是输入，不是决定。
        let app = boot("attach-b64");
        let folder = app.default_folder_id().unwrap();
        let note = app.create_note(&folder, doc("贴一张图")).unwrap();
        let blob: Vec<u8> = (0..600u16).map(|i| (i % 251) as u8).collect();
        let b64 = notera_crypto::b64::encode(&blob);
        let want_sha = notera_crypto::sha256_hex(&blob);
        let got = commands::dispatch(
            &app,
            "attach_file",
            json!({ "noteId": note.id, "blockId": "b1", "role": "inline", "bytesBase64": b64, "mediaType": "image/png", "filename": "图.png" }),
        )
        .unwrap();
        assert_eq!(got["sha256"], want_sha.as_str(), "sha 必须由核心算出来");
        assert_eq!(got["size"], blob.len());
        // 附件写入推进了笔记的 rev（改了派生列与引用表），因此**必须把新 rev 回给编辑器**：
        // 编辑器手里若还是旧 rev，它随后那次自动保存就会被判成 stale_edit —— 用户插一张图，
        // 得到的却是"这条笔记在别处被改动了"。端到端实测过：expected 7 / actual 8。
        let now = app
            .store()
            .get_note(&EntityId::parse(&note.id).unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(
            now.rev.get(),
            note.rev + 1,
            "attach 会把笔记 rev 推进一格（这是回 rev 的前提）"
        );
        assert_eq!(got["rev"], now.rev.get(), "响应里必须带推进后的新 rev");
        assert_eq!(
            std::fs::read(app.store().blob_path(&want_sha)).unwrap(),
            blob,
            "落盘的字节必须与输入相同"
        );
        assert_eq!(
            app.store()
                .attachment_media_type(&want_sha)
                .unwrap()
                .as_deref(),
            Some("image/png")
        );

        let back =
            commands::dispatch(&app, "attachment_data", json!({ "sha256": want_sha })).unwrap();
        assert_eq!(
            notera_crypto::b64::decode(back["bytesBase64"].as_str().unwrap()).unwrap(),
            blob,
            "读回来必须逐字节相同"
        );
        assert_eq!(back["mediaType"], "image/png");
    }

    #[test]
    fn attachment_entry_points_reject_bad_shapes_instead_of_guessing() {
        let app = boot("attach-shape");
        let folder = app.default_folder_id().unwrap();
        let note = app.create_note(&folder, doc("形状检查")).unwrap();
        let base = json!({ "noteId": note.id, "blockId": "b1", "role": "inline", "mediaType": "image/png" });
        let mut both = base.clone();
        both["bytesBase64"] = json!(notera_crypto::b64::encode(b"x"));
        both["localPath"] = json!("某处.png");
        let e = commands::dispatch(&app, "attach_file", both)
            .expect_err("两个来源都给 = 不知道听谁的，必须拒");
        assert_eq!(e.code, "bad_args");
        let e = commands::dispatch(&app, "attach_file", base).expect_err("两个来源都不给同样要拒");
        assert_eq!(e.code, "bad_args");
        // 坏 base64 不许被"尽量解一下"，那是把用户的文件改成另一份内容
        let e = commands::dispatch(
            &app,
            "attach_file",
            json!({ "noteId": note.id, "blockId": "b1", "role": "inline", "bytesBase64": "###" }),
        )
        .expect_err("坏 base64 必须报错");
        assert_eq!(e.code, "bad_args");
        // 空字节不允许挂载（store 的既有约束，别在这里绕过）
        let e = commands::dispatch(
            &app,
            "attach_file",
            json!({ "noteId": note.id, "blockId": "b1", "role": "inline", "bytesBase64": "" }),
        )
        .expect_err("空 blob 必须报错");
        assert!(
            matches!(e.code.as_str(), "bad_args" | "storage" | "constraint"),
            "实际 {}",
            e.code
        );
    }

    #[test]
    fn attachment_reads_reflect_the_size_cap_and_the_shape_of_a_content_key() {
        // sha 会被拼进 blob 路径（<attachments>/<2hex>/<sha>）。不校验形态就是允许
        // `../../…` 去碰任意文件 —— 读命令尤其不能靠"调用方总不会乱传"。
        let app = boot("attach-cap");
        let bad_keys: Vec<String> = vec![
            "../../etc/passwd".into(),
            "a".repeat(63),
            "a".repeat(65),
            "A".repeat(64),
            "zz".to_string() + &"0".repeat(62),
        ];
        for bad in bad_keys {
            let e = commands::dispatch(&app, "attachment_data", json!({ "sha256": bad }))
                .expect_err(&format!("非法 sha 必须被拒：{bad}"));
            assert_eq!(e.code, "bad_args", "实际 {e:?}");
        }
        // 形态合法但盘上没有：是"缺附件"，不是"参数错"，UI 要显示占位而不是报错堆栈
        let missing = "ab".to_string() + &"0".repeat(62);
        let e = commands::dispatch(&app, "attachment_data", json!({ "sha256": missing }))
            .expect_err("没有的附件不能返回空成功");
        assert_eq!(e.code, "attachment_missing");
    }

    /// 造一个"默认本 / 项目 / 项目·子夹"加上平级的"别的"，各放笔记与附件。
    /// 默认本自己也有一篇带附件的笔记 —— 只放子层的话，"祖先链"就看不出是
    /// 文件夹跟着走还是笔记也跟着走（实测踩过：勾一个子夹，祖先的笔记和字节一起被带走）。
    fn scoped_tree(app: &App) -> (EntityId, EntityId, EntityId, EntityId) {
        let root = app.default_folder_id().unwrap();
        let parent = app.store().create_folder(Some(&root), "项目").unwrap();
        let kid = app.store().create_folder(Some(&parent.id), "子夹").unwrap();
        let other = app.store().create_folder(Some(&root), "别的").unwrap();
        let in_kid =
            EntityId::parse(&app.create_note(&kid.id, doc("范围内的笔记")).unwrap().id).unwrap();
        app.create_note(&other.id, doc("范围外的笔记")).unwrap();
        let ancestor_note = app.create_note(&root, doc("祖先自己的笔记")).unwrap();
        let blob: Vec<u8> = b"in-scope attachment bytes".to_vec();
        app.store()
            .attach_blob(&in_kid, &blob, "image/png", Some("a.png"), "b1")
            .unwrap();
        app.store()
            .attach_blob(
                &EntityId::parse(&ancestor_note.id).unwrap(),
                b"ancestor attachment bytes",
                "image/png",
                Some("z.png"),
                "b9",
            )
            .unwrap();
        (root, kid.id, other.id, in_kid)
    }

    #[test]
    fn exporting_a_folder_yields_a_subtree_that_a_clean_library_can_actually_import() {
        let app = boot("scoped-a");
        let (root, kid, other, in_kid) = scoped_tree(&app);
        let out = tmpdir("scoped-export").join("子树.zip");
        let got = commands::dispatch(&app, "export_data", json!({ "path": out.to_string_lossy(), "folderIds": [kid.to_string()], "includeAttachments": true })).unwrap();
        assert_eq!(got["scope"], "folders", "报告必须自己说清这是子树包");
        assert_eq!(got["counts"]["notes"], 1, "只该带上范围内那一篇：{got:?}");
        // 祖先链：子夹的 parent 指向"项目"，"项目"指向默认本 —— 少了它们就是导不回去的包
        assert_eq!(got["counts"]["folders"], 3, "子夹 + 项目 + 默认本：{got:?}");
        assert_eq!(got["counts"]["attachments"], 1, "范围内的附件要跟着走");

        let b = notera_importer::read_bundle(&out).unwrap();
        assert!(b.manifest.as_ref().unwrap().partial, "包自己也得标 partial");
        assert!(b.notes.iter().any(|n| n["id"] == json!(in_kid.to_string())));
        assert!(
            !b.notes
                .iter()
                .any(|n| n["payload"]["folder_id"] == json!(other.to_string())),
            "平级文件夹的内容不许混进来"
        );
        assert!(
            !b.notes
                .iter()
                .any(|n| n["payload"]["folder_id"] == json!(root.to_string())),
            "祖先文件夹自己的笔记不许混进来"
        );
        assert!(
            b.folders.iter().any(|f| f["id"] == json!(root.to_string())),
            "祖先链的文件夹行得在，否则外键接不上"
        );

        // 真正的验收：干净库能把它导回来，且层级完整。
        let fresh = boot("scoped-b");
        let rep = commands::dispatch(
            &fresh,
            "import_data",
            json!({ "path": out.to_string_lossy(), "mode": "merge" }),
        )
        .expect("子树包必须能导进干净库（外键闭包不完整就会在这里失败）");
        assert_eq!(rep["merged"], 4, "1 篇笔记 + 3 个文件夹：{rep:?}");
        assert_eq!(
            rep["restoredAttachments"], 1,
            "包里的附件字节要真的落进新库"
        );
        let back = fresh
            .store()
            .get_note(&in_kid)
            .unwrap()
            .expect("笔记要能按原 id 读回");
        assert_eq!(back.folder_id, kid, "父本必须还是那个子夹");
        let chain: Vec<String> = fresh
            .store()
            .list_folders()
            .unwrap()
            .iter()
            .map(|f| f.name.clone())
            .collect();
        assert!(
            chain.contains(&"项目".to_string()) && chain.iter().any(|n| n == "子夹"),
            "祖先链要一起落地：{chain:?}"
        );
        // 附件不是"字节躺在盘上"就算还原好了：库里得有行、本地态是 available，
        // 而远端态必须是 unknown —— 这个包没经过服务器，谎报 present 就永远不会补传。
        let shas = fresh.store().local_attachment_shas().unwrap();
        assert_eq!(shas.len(), 1, "还原后账上必须认得这一个附件：{shas:?}");
        let data = commands::dispatch(&fresh, "attachment_data", json!({ "sha256": shas[0] }))
            .expect("还原出来的附件必须读得出来");
        // 源库里现在有两个附件，"随手取第一个 sha"就会比错对象（实测踩过）——
        // 直接和已知字节比，并且钉住祖先那份不许跟着子树包走。
        assert_eq!(
            notera_crypto::b64::decode(data["bytesBase64"].as_str().unwrap()).unwrap(),
            b"in-scope attachment bytes".to_vec(),
            "字节要一字不差"
        );
        let ancestor_sha = notera_crypto::sha256_hex(b"ancestor attachment bytes");
        assert!(
            !shas.contains(&ancestor_sha),
            "祖先文件夹的附件不该跟着子树包走：{shas:?}"
        );
        let (local, remote) = fresh.store().attachment_for_state(&shas[0]);
        assert_eq!(
            (local.as_str(), remote.as_str()),
            ("available", "unknown"),
            "还原的附件不得谎报服务器已有"
        );
        let _ = std::fs::remove_file(&out);
    }

    #[test]
    fn a_partial_bundle_is_refused_for_the_empty_library_restore_path() {
        // "仅在空库时导入"的语义是整库还原。喂它一个子树包 = 之后与服务器同步时，
        // 包里缺的那些"永久删除公告"会让已删的笔记被别人的副本带回来。
        let app = boot("scoped-partial");
        let (_root, kid, _other, _in_kid) = scoped_tree(&app);
        let out = tmpdir("scoped-partial").join("子树.zip");
        commands::dispatch(
            &app,
            "export_data",
            json!({ "path": out.to_string_lossy(), "folderIds": [kid.to_string()] }),
        )
        .unwrap();
        let fresh = boot("scoped-c");
        let e = commands::dispatch(
            &fresh,
            "import_data",
            json!({ "path": out.to_string_lossy(), "mode": "intoEmpty" }),
        )
        .expect_err("部分包不许走整库还原");
        assert_eq!(e.code, "save_failed");
        assert!(
            fresh
                .store()
                .list_notes(&notera_store::NoteQuery::all())
                .unwrap()
                .is_empty(),
            "被拒的导入不得留下半包状态"
        );
        // 整库包走同一条路则必须成功
        let full = tmpdir("scoped-partial").join("整库.zip");
        commands::dispatch(
            &app,
            "export_data",
            json!({ "path": full.to_string_lossy() }),
        )
        .unwrap();
        commands::dispatch(
            &fresh,
            "import_data",
            json!({ "path": full.to_string_lossy(), "mode": "intoEmpty" }),
        )
        .expect("整库包在空库上应当放行");
        let _ = std::fs::remove_dir_all(out.parent().unwrap());
    }

    #[test]
    fn an_unknown_folder_in_the_export_scope_is_refused_not_ignored() {
        let app = boot("scoped-bad");
        let e = commands::dispatch(&app, "export_data", json!({ "folderIds": ["not-a-uuid"] }));
        assert!(e.is_err(), "不存在的文件夹不能当成\"没勾\"");
        let ghost = EntityId::new().to_string();
        let e = commands::dispatch(&app, "export_data", json!({ "folderIds": [ghost] }))
            .expect_err("格式对但不存在的 id 也要拒");
        assert!(
            matches!(e.code.as_str(), "constraint" | "rejected" | "storage"),
            "实际 {}",
            e.code
        );
    }

    #[test]
    fn preview_text_shows_the_exact_revision_the_conflict_panel_is_comparing() {
        // 冲突面板并排看的是"我方那一版 / 对方那一版"。它调的 `preview_text` 一度在
        // 核心里根本不存在 → 每次 unknown_command，面板安静地退回卡片摘要，
        // 用户以为看到的就是那一版，其实看的是同一条派生摘要。
        let app = boot("preview");
        let folder = app.default_folder_id().unwrap();
        let note = app.create_note(&folder, doc("第一版的内容")).unwrap();
        let eid = EntityId::parse(&note.id).unwrap();
        app.edit_note(&eid, doc("第二版的内容"), Rev(note.rev))
            .unwrap();
        let at = |rev: u64| {
            commands::dispatch(&app, "preview_text", json!({ "id": note.id, "rev": rev }))
                .unwrap()
                .as_str()
                .unwrap_or_default()
                .to_string()
        };
        assert!(
            at(1).contains("第一版"),
            "rev 1 的预览必须是第一版：{}",
            at(1)
        );
        assert!(
            at(2).contains("第二版"),
            "rev 2 的预览必须是第二版：{}",
            at(2)
        );
        assert_ne!(at(1), at(2), "两版预览一模一样 = 面板在做样子");
        let e = commands::dispatch(&app, "preview_text", json!({ "id": note.id, "rev": 99 }))
            .expect_err("没有的 rev 不许给空成功");
        assert_eq!(e.code, "not_found");
    }

    #[test]
    fn stats_command_emits_exactly_the_contract_keys_the_shell_reads() {
        // 这条边的历史事故：`StoreStats` 被原样序列化，存储层字段名（notes_trash /
        // fts_rows / outbox_pending）直接漏到 UI，而契约图和前端读的是
        // notesInTrash / ftsEntries / inflightOps。TS 的类型是断言不是校验，
        // 于是设置页"回收站 / 占用空间 / 待同步"三行恒为「—」、侧栏回收站恒为 0，
        // 而单元测试喂的是 camelCase 假数据 —— 正好把洞盖住。键集合就是契约，逐项钉死。
        let app = boot("stats");
        let got = commands::dispatch(&app, "stats", json!({})).unwrap();
        let mut keys: Vec<&str> = got
            .as_object()
            .expect("stats 应是对象")
            .keys()
            .map(|k| k.as_str())
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "attachments",
                "dbBytes",
                "folders",
                "ftsEntries",
                "inflightOps",
                "notes",
                "notesInTrash",
                "searchGeneration"
            ]
        );
        // 值也得接得上：全新库里没有待发操作（本地哨兵账户的留痕行不算队列）
        assert_eq!(got["inflightOps"], 0, "没配置远端时待发队列必须是 0");
        assert_eq!(got["notes"], 0);
        assert_eq!(got["notesInTrash"], 0);
    }

    #[test]
    fn account_mapping_roundtrips_without_leaking_secrets() {
        let app = boot("account");
        let dto = commands::dispatch(
            &app,
            "configure_account",
            json!({ "id": "", "label": "家里", "baseUrl": "http://127.0.0.1:5005/.notes", "username": "u", "password": "pw" }),
        )
        .unwrap();
        assert_eq!(dto["label"], "家里");
        assert_eq!(dto["rootPrefix"], "/.notes");
        assert_eq!(dto["authKind"], "basic");
        assert_eq!(dto["tlsPolicy"], "strict");
        assert_eq!(dto["proxyMode"], "direct");
        assert_eq!(dto["hasCredential"], true);
        let text = dto.to_string();
        assert!(!text.contains("pw"), "下发给 UI 的账户视图里绝不能出现口令");
        assert!(!text.contains("credentialRef"), "凭据引用也不属于 UI");

        let same = commands::dispatch(&app, "account", json!({})).unwrap();
        assert_eq!(same["id"], dto["id"], "`account` 必须回同一个活动账户");
        let id = dto["id"].as_str().unwrap().to_string();
        commands::dispatch(&app, "remove_account", json!({ "id": id })).unwrap();
        assert!(app.current_account().unwrap().is_none());
    }

    /// 引擎判出的冲突要在收件箱里看得见，并且卡片说得出是哪条笔记。
    #[test]
    fn record_conflict_lands_in_the_inbox_and_names_the_note() {
        // Rev 只经 `next_rev` 推进（I2）——测试也不例外，否则等于教人写 `rev + 1`。
        let bump = |r: u64| next_rev(Rev(r), Rev(0)).get();
        let app = boot("conflict");
        // 先订阅再触发：总线只把"订阅之后"的事件投给这个接收端
        let rx = app.subscribe();
        let folder = app.list_folders().unwrap().remove(0);
        let note = app
            .create_note(&EntityId::parse(&folder.id).unwrap(), doc("要吵架的笔记"))
            .unwrap();

        let l = LocalView {
            kind: "n".into(),
            id: note.id.clone(),
            rev: bump(note.rev),
            sync_rev: note.rev,
            sync_hash: None,
            content_hash: "sha256:local".into(),
            deleted_at: None,
            purged_at: None,
            edited_after_delete: false,
        };
        let r = RemoteView {
            kind: "n".into(),
            id: note.id.clone(),
            rev: bump(bump(note.rev)),
            hash: Some("sha256:remote".into()),
            deleted_at: None,
            purged: false,
        };
        let d = Decision {
            key: l.key(),
            action: Action::Conflict(ConflictKind::UpdateUpdate),
            rule: "P10",
        };
        app.local_port().record_conflict(&d, &l, &r).unwrap();

        let open = app.open_conflicts().unwrap();
        assert_eq!(open.len(), 1, "冲突必须进收件箱");
        assert_eq!(open[0].note_id, note.id);
        assert_eq!(open[0].note_title, "要吵架的笔记");
        assert_eq!(open[0].local_rev, bump(note.rev));
        assert_eq!(open[0].remote_rev, bump(bump(note.rev)));
        assert_eq!(
            open[0].base_rev, note.rev,
            "base = sync_rev = 共同祖先（DATA-MODEL §4.3）"
        );
        // 订阅发生在 create_note 之前，所以总线上先出现的是那条 NotesChanged ——
        // 那是正当事件。这里要证的性质是"冲突记录一定会投出一条 Conflict"，
        // 于是跳过在先的其它事件，直到看见 Conflict 或超时。
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut skipped: Vec<BusEvent> = Vec::new();
        loop {
            let budget = deadline.saturating_duration_since(std::time::Instant::now());
            if budget.is_zero() {
                panic!("5s 内没有收到 Conflict 事件，只看到：{skipped:?}");
            }
            match rx.recv_timeout(budget).expect("超时前没有任何事件") {
                BusEvent::Conflict {
                    conflict_id,
                    note_title,
                } => {
                    assert_eq!(conflict_id, open[0].id);
                    assert_eq!(note_title, "要吵架的笔记");
                    break;
                }
                other => skipped.push(other),
            }
        }
        // 裁决走存储层，host 只做动词翻译；UI 的驼峰写法必须能用
        app.resolve_conflict(commands::ResolveConflictCmd {
            id: open[0].id,
            action: "keepBoth".into(),
        })
        .unwrap();
        assert!(
            app.open_conflicts().unwrap().is_empty(),
            "裁决后不再是 open"
        );
        let rows = app
            .list_notes(ListNotesCmd {
                folder_id: None,
                trash: false,
                limit: 50,
                offset: 0,
            })
            .unwrap();
        let copies: Vec<_> = rows
            .iter()
            .filter(|r| {
                r["title"]
                    .as_str()
                    .is_some_and(|t| t.ends_with("（本地副本）"))
            })
            .collect();
        assert_eq!(
            copies.len(),
            1,
            "§6：进收件箱的同一刻必须留下本地副本，用户之后选哪边都不丢字"
        );
        let open2 = app.open_conflicts().unwrap();
        assert_eq!(open2.len(), 0);
    }

    /// UI 的四个动词（types.ts 的 ConflictAction）逐个都要翻得动。
    #[test]
    fn every_ui_conflict_action_maps_to_a_store_resolution() {
        for action in [
            "keepBoth",
            "replaceWithLocal",
            "replaceWithRemote",
            "manualMerge",
            "dismiss",
        ] {
            let app = boot(&format!("resolve-{action}"));
            let folder = app.list_folders().unwrap().remove(0);
            let fid = EntityId::parse(&folder.id).unwrap();
            let note = app.create_note(&fid, doc("要裁决")).unwrap();
            let cid = app
                .store()
                .record_conflict(&notera_store::ConflictRecord {
                    account_id: notera_store::LOCAL_ACCOUNT_ID.into(),
                    kind: EntityKind::Note,
                    id: EntityId::parse(&note.id).unwrap(),
                    base_rev: Rev(1),
                    local_rev: Rev(2),
                    remote_rev: Rev(3),
                    local_hash: "sha256:l".into(),
                    remote_hash: "sha256:r".into(),
                    auto_merged: false,
                    copy_note_id: None,
                })
                .unwrap();
            app.resolve_conflict(commands::ResolveConflictCmd {
                id: cid,
                action: action.into(),
            })
            .unwrap_or_else(|e| panic!("{action} 必须被接受，实际 {e:?}"));
        }
        let app = boot("resolve-bad");
        let e = app
            .resolve_conflict(commands::ResolveConflictCmd {
                id: 1,
                action: "yolo".into(),
            })
            .unwrap_err();
        assert_eq!(e.code, "bad_action");
    }

    #[test]
    fn conflict_title_falls_back_to_the_tombstone_snapshot() {
        let app = boot("conflict-tomb");
        let folder = app.list_folders().unwrap().remove(0);
        let fid = EntityId::parse(&folder.id).unwrap();
        let note = app.create_note(&fid, doc("对端已永久删除")).unwrap();
        let nid = EntityId::parse(&note.id).unwrap();
        app.store()
            .mark_synced(EntityKind::Note, &nid, Rev(note.rev), &note.content_hash)
            .unwrap();
        app.store().purge_note(&nid).unwrap();
        app.store()
            .record_conflict(&notera_store::ConflictRecord {
                account_id: notera_store::LOCAL_ACCOUNT_ID.into(),
                kind: EntityKind::Note,
                id: nid.clone(),
                base_rev: Rev(note.rev),
                local_rev: Rev(note.rev),
                remote_rev: Rev(note.rev),
                local_hash: note.content_hash.clone(),
                remote_hash: note.content_hash,
                auto_merged: false,
                copy_note_id: None,
            })
            .unwrap();
        let open = app.open_conflicts().unwrap();
        assert_eq!(
            open[0].note_title, "对端已永久删除",
            "笔记行已不存在时要用墓碑快照，卡片不许空白"
        );
    }

    /// LocalPort 的 push 侧：wire 由存储层给，host 只搬运；删除态必须引擎看得见。
    #[test]
    fn local_port_exposes_wire_and_delete_state() {
        let app = boot("port");
        let p = app.local_port();
        let folder = app.list_folders().unwrap().remove(0);
        let fid = EntityId::parse(&folder.id).unwrap();
        let note = app.create_note(&fid, doc("待上传")).unwrap();
        let nid = EntityId::parse(&note.id).unwrap();

        let wire = p
            .envelope_wire("n", &note.id)
            .unwrap()
            .expect("脏笔记必须有待 PUT 的记录");
        let env: serde_json::Value = serde_json::from_slice(&wire).unwrap();
        assert_eq!(env["kind"], "note");
        assert_eq!(env["hash"], note.content_hash);
        assert_eq!(env["payload"]["folder_id"], folder.id);
        assert!(
            p.envelope_wire("zz", &note.id).is_err(),
            "认不出的类型标记绝不能被当成笔记上传"
        );

        app.store().delete_note(&nid).unwrap();
        let views = p.local_views().unwrap();
        let v = views
            .iter()
            .find(|x| x.id == note.id)
            .expect("软删的笔记仍在脏集里");
        assert!(
            v.deleted_at.is_some(),
            "P8/P11 的判据就是 deleted_at，填 None 等于把删除藏起来"
        );
        assert!(v.purged_at.is_none());

        app.store().purge_note(&nid).unwrap();
        let ann = p
            .envelope_wire("n", &note.id)
            .unwrap()
            .expect("永久删除必须可传播");
        let ann: serde_json::Value = serde_json::from_slice(&ann).unwrap();
        assert_eq!(ann["purged"], true);
        assert!(ann["payload"].is_null());
        assert!(
            p.envelope_wire("a", "00000000-0000-0000-0000-000000000000")
                .unwrap()
                .is_none(),
            "附件走独立队列"
        );
    }

    #[test]
    fn dispatch_reports_unknown_command_and_bad_args() {
        let app = boot("dispatch");
        let e = commands::dispatch(&app, "nope", json!({})).unwrap_err();
        assert_eq!(
            (e.code.as_str(), e.message_key.as_str()),
            ("unknown_command", "cmd.unknown_command")
        );
        let e = commands::dispatch(&app, "get_note", json!({ "id": "不是 uuid" })).unwrap_err();
        assert_eq!(e.code, "bad_id");
        let e = commands::dispatch(
            &app,
            "get_note",
            json!({ "id": EntityId::new().to_string() }),
        )
        .unwrap();
        assert_eq!(e, Value::Null, "读不到的笔记返回 null，不是错误");
    }

    /// 没选文件夹也不能拒绝创建：落点是默认本，由核心决定（UI 只发 `folderId: null`）。
    #[test]
    fn create_note_without_folder_lands_in_the_default_one() {
        let app = boot("new-note");
        let default = app.default_folder_id().unwrap();
        let r = commands::dispatch(
            &app,
            "create_note",
            json!({ "folderId": Value::Null, "doc": doc("新建按钮") }),
        )
        .unwrap();
        assert_eq!(r["folderId"].as_str().unwrap(), default.as_str());
        let r = commands::dispatch(
            &app,
            "create_note",
            json!({ "doc": doc("连 folderId 都不给") }),
        )
        .unwrap();
        assert_eq!(r["folderId"].as_str().unwrap(), default.as_str());
    }

    fn draft(id: &str, url: &str, user: Option<&str>) -> AccountDraftCmd {
        AccountDraftCmd {
            id: id.into(),
            label: "内网".into(),
            base_url: url.into(),
            root_prefix: Some("/.notes".into()),
            auth_kind: Some("basic".into()),
            username: user.map(Into::into),
            password: None,
            tls_policy: None,
            ca_pem: None,
            proxy_mode: None,
            proxy_host: None,
            proxy_port: None,
            proxy_username: None,
            proxy_password: None,
            bypass: None,
        }
    }

    /// ADR-0018：确认点还是全局的，第二台服务器会**静默半同步**，所以在配置入口就拒绝。
    #[test]
    fn second_enabled_account_is_refused_and_the_first_stays_untouched() {
        let app = boot("multi");
        let a = app
            .configure_account(draft("a", "https://dav.home.example/dav", Some("u")))
            .unwrap();
        let e = app
            .configure_account(draft("b", "https://dav.work.example/dav", Some("u")))
            .unwrap_err();
        assert_eq!(e.code, "multi_account_unsupported");
        let now = app.current_account().unwrap().expect("a 仍在");
        assert_eq!(
            (now.id.as_str(), now.base_url.as_str()),
            (a.id.as_str(), a.base_url.as_str())
        );
        // 停用它之后才允许换另一台
        app.remove_account("a").unwrap();
        app.configure_account(draft("b", "https://dav.work.example/dav", Some("u")))
            .unwrap();
    }

    /// 没凭据就绝不装适配器，而且**本地写入照常**（不变式 I8）。
    #[test]
    fn sync_remote_without_credentials_leaves_local_writes_working() {
        let app = boot("nocreds");
        assert!(
            app.sync_remote().unwrap().is_none(),
            "未配置账户时不启动引擎"
        );
        app.configure_account(draft("a", "https://dav.home.example/dav", None))
            .unwrap();
        assert!(
            app.sync_remote().unwrap().is_none(),
            "没有用户名的账户拿不到凭据"
        );
        let st = app.sync_status().unwrap();
        assert!(
            st.phase.contains("credential"),
            "状态必须可见，不能停在\"已同步\"：{st:?}"
        );
        assert_eq!(st.badge, "offline");
        assert_eq!(st.message_key.as_deref(), Some("sync.needsCredentials"));
        let folder = app.list_folders().unwrap().remove(0);
        let n = app
            .create_note(&EntityId::parse(&folder.id).unwrap(), doc("离线也能写"))
            .unwrap();
        assert_eq!(n.rev, 1, "拿不到凭据绝不能挡住本地写入（I8）");
    }

    /// 空 id 的草案必须在**两个地方**落在同一个 id 上。`upsert_account` 碰到空 id 会
    /// 自己生成一个，host 若继续拿空串去 `register_account`，`sync_accounts` 里就没有
    /// 这台服务器那一行 → 本地写入永远不排队给它 → 配了服务器却静默不同步。
    /// （端到端测试 `sync_once` 第一次跑就撞见了这个）
    #[test]
    fn a_draft_without_id_still_registers_the_account_row() {
        let app = boot("draft-id");
        let dto = app
            .configure_account(draft("", "https://dav.home.example/dav", Some("notera")))
            .unwrap();
        let active = app
            .current_account()
            .unwrap()
            .expect("活跃账户")
            .id
            .to_string();
        assert_eq!(
            active,
            dto.id.as_str(),
            "返回的 id 与配置里的 id 必须是一个"
        );
        assert!(!active.is_empty(), "id 不能是空串");
        assert!(
            app.store().account_exists(&active).unwrap(),
            "配置里的账户要在 sync_accounts 有对应行（否则不出站）"
        );
        assert!(
            !app.store().account_exists("").unwrap(),
            "不该留下空 id 的幽灵行"
        );
        // 探测结果也要写到这一行上，而不是写到空串那行
        app.store().set_account_caps(&active, 0b000101).unwrap();
        assert_eq!(app.store().account_caps(&active).unwrap(), Some(0b000101));
    }

    /// §5 的判定必须能到界面，而且要区分三件事：没探过 / 探到并发保护 / 探到没有。
    /// 用户看到的句子不一样，"建议多设备串行编辑"只在第三种情况下该出现。
    #[test]
    fn account_view_reports_the_probe_verdict_and_distinguishes_never_probed() {
        let app = boot("caps-view");
        let draft = draft("c1", "https://dav.home.example/dav", Some("notera"));
        app.configure_account(draft).unwrap();
        let fresh = app.current_account().unwrap().expect("账户在");
        assert_eq!(
            (fresh.cap_mask.as_ref(), fresh.write_strategy.as_deref()),
            (None, None),
            "刚配上时是『还没探』而不是『不支持』"
        );

        // S1：条件写在位图里
        app.store()
            .set_account_caps(&fresh.id, notera_webdav::Caps::conventional().mask())
            .unwrap();
        let s1 = app.current_account().unwrap().expect("账户在");
        assert_eq!(s1.write_strategy.as_deref(), Some("S1"));
        assert!(
            s1.caps_probed_at
                .as_deref()
                .unwrap_or_default()
                .ends_with('Z'),
            "要带上什么时候探的：{:?}",
            s1.caps_probed_at
        );

        // S3：什么都没探到（位图为 0，与 None 不同）
        app.store().set_account_caps(&fresh.id, 0).unwrap();
        let s3 = app.current_account().unwrap().expect("账户在");
        assert_eq!(s3.cap_mask, Some(0), "『探过了，全不支持』必须原样报出去");
        assert_eq!(s3.write_strategy.as_deref(), Some("S3"));

        // S2：只有 Overwrite:F MOVE
        app.store()
            .set_account_caps(&fresh.id, notera_webdav::Caps::OVERWRITE_F_MOVE)
            .unwrap();
        assert_eq!(
            app.current_account()
                .unwrap()
                .expect("账户在")
                .write_strategy
                .as_deref(),
            Some("S2")
        );
    }

    /// 装配路径本身要能被测到（钥匙串还没接入，不能等它才有测试）。
    #[test]
    fn build_remote_makes_a_real_adapter_from_config() {
        let app = boot("remote");
        let acct = app
            .configure_account(draft("a", "https://dav.home.example/dav", Some("notera")))
            .unwrap();
        let cfg = app.config();
        let acct = cfg
            .accounts
            .iter()
            .find(|a| a.id == acct.id)
            .unwrap()
            .clone();
        let device = DeviceId::parse(&cfg.device_id).unwrap();
        let creds = notera_webdav::Credentials::new("notera", "hunter2").unwrap();
        let remote =
            App::build_remote(&acct, device, creds, notera_webdav::Caps::conventional()).unwrap();
        // base_url 里的路径会被并进前缀（`https://h/dav` + `/.notes` → `.../dav/.notes`），
        // 所以这里断言的是合并后的 origin+path，不是原始输入字符串。
        assert_eq!(remote.paths().base_url(), "https://dav.home.example/dav");
        assert_eq!(remote.device_id(), cfg.device_id);
        assert_eq!(remote.paths().root_prefix(), "/.notes");
        // 适配器刻意不实现 Debug：里面藏着凭据，derive 出来就可能整条打印进日志。
    }

    /// §13 的完整回路：A 上传 → B 下载，中间只经过真 HTTP 服务器。
    #[tokio::test]
    async fn attachments_travel_between_two_devices_through_the_real_server() {
        use notera_core::EntityId as Eid;
        use notera_webdav::Credentials;
        let srv = notera_test_webdav::TestServer::start(notera_test_webdav::Backend::Mem).await;
        let url = srv.base_url.clone();
        let a = boot("att-a");
        let b = boot("att-b");
        for app in [&a, &b] {
            app.configure_account(draft("srv", &url, Some("u")))
                .unwrap();
        }
        let remote_for = |app: &App| {
            let cfg = app.config();
            let acct = ConfigRepository::active(&cfg).unwrap().clone();
            let device = DeviceId::parse(&cfg.device_id).unwrap();
            std::sync::Arc::new(
                App::build_remote(
                    &acct,
                    device,
                    Credentials::new("u", "p").unwrap(),
                    notera_webdav::Caps::conventional(),
                )
                .unwrap(),
            )
        };
        let (ra, rb) = (remote_for(&a), remote_for(&b));
        a.negotiate(&ra).await.unwrap();
        b.negotiate(&rb).await.unwrap();

        let folder = Eid::parse(&a.list_folders().unwrap().remove(0).id).unwrap();
        let note = a.create_note(&folder, doc("带一张图")).unwrap();
        let blob = format!("PNG-ish bytes 图片 {}", note.id.as_str()).into_bytes();
        let sha = a
            .store()
            .attach_blob(
                &Eid::parse(&note.id).unwrap(),
                &blob,
                "image/png",
                Some("pic.png"),
                "blk0000001",
            )
            .unwrap()
            .sha256;

        assert_eq!(
            a.run_attachment_round(&ra).await,
            (1, 0, 0),
            "A 应当把这一个附件传上去"
        );
        assert!(
            ra.has_attachment(&sha).await.unwrap(),
            "服务器上必须真的存在这个 blob"
        );
        assert_eq!(
            a.run_attachment_round(&ra).await.0,
            0,
            "已 present 的不得重传"
        );

        // B 只知道"清单说远端有这个 sha"
        b.store()
            .register_remote_attachment(&sha, blob.len() as i64, "image/png")
            .unwrap();
        assert_eq!(
            b.run_attachment_round(&rb).await,
            (0, 1, 0),
            "B 应当把它下载下来"
        );
        assert_eq!(
            std::fs::read(b.store().blob_path(&sha)).unwrap(),
            blob,
            "字节必须逐字节相同"
        );
        assert_eq!(
            b.store().attachment_for_state(&sha),
            ("available".into(), "present".into()),
            "落盘成功后两个状态位都要推进"
        );

        // 远端 404 时：只改远端态，绝不删本地（§10）
        let ghost = notera_core::ContentHash::of(b"ghost")
            .as_str()
            .replace("sha256:", "");
        b.store()
            .register_remote_attachment(&ghost, 10, "image/png")
            .unwrap();
        b.run_attachment_round(&rb).await;
        assert_eq!(b.store().attachment_for_state(&ghost).1, "absent");
        assert_eq!(
            b.store().attachment_for_state(&sha).0,
            "available",
            "别人的缺失不能牵连已存在的附件"
        );
    }

    /// SYNC-PROTOCOL §2：两个库指到同一个目录时必须**停手**，而不是把两库并成一库。
    #[tokio::test]
    async fn negotiate_refuses_to_share_a_directory_with_another_library() {
        use notera_webdav::Credentials;
        let srv = notera_test_webdav::TestServer::start(notera_test_webdav::Backend::Mem).await;
        let a = boot("nego-a");
        let b = boot("nego-b");
        let url = srv.base_url.clone();
        for app in [&a, &b] {
            app.configure_account(draft("srv", &url, Some("u")))
                .unwrap();
        }
        let remote_for = |app: &App| {
            let cfg = app.config();
            let acct = ConfigRepository::active(&cfg).unwrap().clone();
            let device = DeviceId::parse(&cfg.device_id).unwrap();
            App::build_remote(
                &acct,
                device,
                Credentials::new("u", "p").unwrap(),
                notera_webdav::Caps::conventional(),
            )
            .unwrap()
        };

        let ra = remote_for(&a);
        a.negotiate(&ra)
            .await
            .expect("第一个库应当建好 protocol.json");
        let root_a = a
            .store()
            .sync_state("srv")
            .unwrap()
            .unwrap()
            .root_id
            .clone();
        assert_eq!(
            root_a.as_deref(),
            Some(a.default_folder_id().unwrap().as_str())
        );
        assert_eq!(
            a.sync_status().unwrap().phase,
            "online",
            "协商通过才允许进入同步态"
        );

        // 情形一：本机已有自己的内容 → 必须停手，而不是接受别人的 root_id
        let bf = b.list_folders().unwrap().remove(0);
        b.create_note(
            &EntityId::parse(&bf.id).unwrap(),
            doc("我这台机器自己的笔记"),
        )
        .unwrap();
        let rb = remote_for(&b);
        assert_eq!(
            b.negotiate(&rb).await,
            Err("sync.root_mismatch"),
            "两个有内容的库不能并成一个"
        );
        // 拒绝之后服务器上的 protocol.json 必须仍是 A 的那一份（没有被"顺手改写"）
        let still = rb.fetch_protocol().await.unwrap().unwrap();
        assert_eq!(still["root_id"].as_str(), root_a.as_deref());
        assert_eq!(
            b.sync_status().unwrap().phase,
            "error",
            "停手必须是可见状态，不是悄悄不干活"
        );

        // 情形二：空库允许加入既有库（§9 的新设备场景）
        let c = boot("nego-c");
        c.configure_account(draft("srv", &url, Some("u"))).unwrap();
        let rc = remote_for(&c);
        c.negotiate(&rc)
            .await
            .expect("空库应当接受服务器上已有的 root_id");
        assert_eq!(
            c.store()
                .sync_state("srv")
                .unwrap()
                .unwrap()
                .root_id
                .as_deref(),
            root_a.as_deref()
        );
    }
}
