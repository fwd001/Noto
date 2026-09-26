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

use commands::{AccountDraftCmd, AccountDto, CmdError, ConflictDto, ExportCmd, FolderDto, ImportCmd, ListNotesCmd, NoteDto, NoteListDto, SearchCmd, SearchHitDto, SyncStatusDto};
use notera_config::{AccountConfig, AppConfig, ConfigRepository, ProxyMode, ProxyProfile, TlsPolicyKind};
use notera_core::{Clock, DeviceId, EntityId, EntityKind, Rev, SystemClock};
use notera_store::{ApplyOp as StoreApplyOp, ConflictRow, Folder, Note, NoteListRow, NoteQuery, SearchPath, Store, StoreError};
use notera_sync::{ApplyOp, EngineConfig, LocalError, LocalPort, Phase, RoundStats, SyncEvent};
use notera_sync::plan::{Decision, LocalView, RemoteView};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

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
    Conflict { conflict_id: i64, note_title: String },
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
    pub fn for_current_target() -> Self {
        if cfg!(target_os = "windows") {
            Self {
                tray: true,
                global_shortcuts: true,
                native_menu: true,
                notifications: true,
                share_sheet: false,
                background_task: "desktop_timer".into(),
                keychain: "credential_manager".into(),
                file_picker: "native".into(),
                safe_area: false,
            }
        } else if cfg!(target_os = "macos") {
            Self {
                tray: false,
                global_shortcuts: true,
                native_menu: true,
                notifications: true,
                share_sheet: true,
                background_task: "desktop_timer".into(),
                keychain: "keychain".into(),
                file_picker: "native".into(),
                safe_area: false,
            }
        } else if cfg!(target_os = "android") {
            Self {
                tray: false,
                global_shortcuts: false,
                native_menu: false,
                notifications: true,
                share_sheet: true,
                background_task: "workmanager".into(),
                keychain: "keystore".into(),
                file_picker: "saf".into(),
                safe_area: true,
            }
        } else if cfg!(target_os = "ios") {
            Self {
                tray: false,
                global_shortcuts: false,
                native_menu: false,
                notifications: true,
                share_sheet: true,
                background_task: "bgapprefresh".into(),
                keychain: "keychain".into(),
                file_picker: "document_picker".into(),
                safe_area: true,
            }
        } else {
            Self {
                tray: false,
                global_shortcuts: true,
                native_menu: true,
                notifications: false,
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
    caps: PlatformCaps,
    bus: Mutex<Vec<BusEvent>>,
    subs: Mutex<Vec<std::sync::mpsc::Sender<BusEvent>>>,
    sync_view: Mutex<SyncView>,
    cached_manifest: Mutex<Option<Vec<u8>>>,
    /// 上一轮提交后服务器给的清单 etag —— 下一轮带它才可能拿到 304（§6.3 空轮 0 字节）
    manifest_etag: Mutex<Option<String>>,
    cached_remote: Mutex<Vec<RemoteView>>,
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
            repo.save(&config).map_err(|e| BootError::Config(e.to_string()))?;
        }
        let store = Store::open(data_dir, device_id).map_err(BootError::Store)?;
        let violations = store.startup_violations().to_vec();
        let app = App {
            inner: Arc::new(Inner {
                store,
                data_dir: data_dir.to_path_buf(),
                config_repo: repo,
                config: Mutex::new(config),
                caps: PlatformCaps::for_current_target(),
                bus: Mutex::new(Vec::new()),
                subs: Mutex::new(Vec::new()),
                sync_view: Mutex::new(SyncView::initial()),
                cached_manifest: Mutex::new(None),
                manifest_etag: Mutex::new(None),
                cached_remote: Mutex::new(Vec::new()),
                seq_applied: AtomicU64::new(0),
                syncing: AtomicBool::new(false),
                dirty_ticks: AtomicU64::new(0),
            }),
        };
        // 启动自检结论必须可见：不静默修，也不静默忽略
        for v in &violations {
            app.emit(BusEvent::Toast { message_key: format!("verify.{}", v.id), level: "warn".into() });
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
        self.inner.caps.clone()
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
        self.emit(BusEvent::Sync { badge, progress: None, error_code: key });
    }

    // -------------------------------------------------------------- 用例 ---

    pub fn create_note(&self, folder_id: &EntityId, doc: serde_json::Value) -> Result<NoteDto, CmdError> {
        let n = self.inner.store.create_note(folder_id, doc)?;
        self.note_saved(&n);
        Ok(self.to_dto(n)?)
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

    pub fn edit_note(&self, id: &EntityId, doc: serde_json::Value, expected: Rev) -> Result<NoteDto, CmdError> {
        let n = self.inner.store.edit_note(id, doc, expected)?;
        self.note_saved(&n);
        Ok(self.to_dto(n)?)
    }

    /// 本地写入成功 = 同步被"需要跑一轮"标记。**这里绝不碰网络**（I8/P1）。
    fn note_saved(&self, n: &Note) {
        self.inner.dirty_ticks.fetch_add(1, Ordering::SeqCst);
        self.emit(BusEvent::NotesChanged { ids: vec![n.id.to_string()] });
        self.set_sync(|v| {
            if v.badge == Badge::Synced {
                v.in_flight = false;
            }
        });
    }

    pub fn get_note(&self, id: &EntityId) -> Result<Option<NoteDto>, CmdError> {
        Ok(self.inner.store.get_note(id)?.map(|n| self.to_dto(n)).transpose()?)
    }

    pub fn list_notes(&self, c: ListNotesCmd) -> Result<Vec<serde_json::Value>, CmdError> {
        let q = NoteQuery {
            folder: c.folder_id.as_deref().map(EntityId::parse).transpose().map_err(|_| CmdError::of("bad_id", false))?,
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
                .list_notes(&NoteQuery { folder: Some(f.id.clone()), trash: false, limit: 0, offset: 0 })?
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
            .list_notes(&NoteQuery { folder: Some(f.id.clone()), trash: false, limit: 0, offset: 0 })?
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
        let hits = self.inner.store.search(&notera_store::SearchQuery { text: c.text, limit: c.limit })?;
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
            out.push(SearchHitDto { note_id: h.note_id.to_string(), score: h.score, snippet_html: h.snippet_html, title });
        }
        Ok(out)
    }

    pub fn attach(&self, c: commands::AttachCmd) -> Result<serde_json::Value, CmdError> {
        let bytes = std::fs::read(&c.local_path).map_err(|e| CmdError::of("read_failed", false).with(serde_json::json!({ "why": e.to_string() })))?;
        let note = EntityId::parse(&c.note_id).map_err(|_| CmdError::of("bad_id", false))?;
        let a = self
            .inner
            .store
            .attach_blob(&note, &bytes, &c.media_type, c.filename.as_deref(), &c.block_id)
            .map_err(CmdError::from)?;
        self.inner.dirty_ticks.fetch_add(1, Ordering::SeqCst);
        Ok(serde_json::json!({ "sha256": a.sha256, "size": a.size, "mediaType": a.media_type }))
    }

    pub fn stats(&self) -> Result<serde_json::Value, CmdError> {
        let s = self.inner.store.stats()?;
        Ok(serde_json::to_value(s).map_err(|_| CmdError::of("serialize", false))?)
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
        Ok(ConflictDto {
            id: r.conflict_id,
            note_id: r.id.to_string(),
            note_title: title.unwrap_or_else(|| "（无标题）".into()),
            base_rev: r.base_rev.get(),
            local_rev: r.local_rev.get(),
            remote_rev: r.remote_rev.get(),
            copy_note_id: r.copy_note_id.as_ref().map(|i| i.to_string()),
            created_at: r.created_at,
        })
    }

    pub fn resolve_conflict(&self, c: commands::ResolveConflictCmd) -> Result<(), CmdError> {
        // UI 侧的动词（apps/desktop/src/api/types.ts 的 ConflictAction）与存储侧的
        // resolution 词表不是一套；翻译只能发生在这里 —— 存储与引擎都不该认识 UI 词汇。
        let resolution = match c.action.as_str() {
            "dismiss" => return self.inner.store.dismiss_conflict(c.id).map_err(CmdError::from),
            "keepBoth" | "keep_both" | "kept_both" => "kept_both",
            "replaceWithLocal" | "use_local" | "local" => "local",
            "replaceWithRemote" | "use_remote" | "remote" => "remote",
            "manualMerge" | "manual" => "manual",
            "merged" => "merged",
            other => return Err(CmdError::of("bad_action", false).with(serde_json::json!({ "action": other }))),
        };
        self.inner.store.resolve_conflict(c.id, resolution)?;
        self.emit(BusEvent::NotesChanged { ids: vec![] });
        Ok(())
    }

    pub fn set_pref(&self, key: &str, value: serde_json::Value) -> Result<(), CmdError> {
        self.inner.store.set_pref(key, &value).map_err(CmdError::from)
    }
    pub fn get_prefs(&self) -> Result<serde_json::Value, CmdError> {
        self.inner.store.get_prefs().map_err(CmdError::from)
    }

    /// 导出整库为自描述 ZIP（DATA-MODEL §15）。
    ///
    /// 记录由 `Store::all_records()` 用**与上传同一套**信封构造函数产出，
    /// 所以导入侧可以直接走 `apply_remote`，不必为导出另写一条写入路径。
    pub fn export_data(&self, c: ExportCmd) -> Result<serde_json::Value, CmdError> {
        if !c.folder_ids.is_empty() {
            // 只导出部分文件夹需要连带祖先与外键闭包。没做之前宁可明确拒绝，
            // 也不能"用户勾了两个文件夹、结果把整库交出去"。
            return Err(CmdError::of("bad_args", false).with(serde_json::json!({ "detail": "folder_scope_unsupported" })));
        }
        let records = self.inner.store.all_records().map_err(CmdError::from)?;
        let purged = |r: &serde_json::Value| r.get("purged").and_then(|p| p.as_bool()).unwrap_or(false);
        let bundle = notera_importer::Bundle {
            manifest: Some(notera_importer::Manifest {
                format: notera_importer::BUNDLE_FORMAT,
                protocol: 1,
                exported_at: self.inner.store.now(),
                app_version: env!("CARGO_PKG_VERSION").to_string(),
                root_id: None,
                counts: std::collections::BTreeMap::new(),
            }),
            folders: records.iter().filter(|r| r.get("kind").and_then(|k| k.as_str()) == Some("folder") && !purged(r)).cloned().collect(),
            notes: records.iter().filter(|r| r.get("kind").and_then(|k| k.as_str()) == Some("note") && !purged(r)).cloned().collect(),
            tombstones: records.iter().filter(|r| purged(r)).cloned().collect(),
            attachments: Vec::new(),
        };
        let mut attachments = Vec::new();
        if c.include_attachments {
            for sha in list_blob_names(&self.inner.store.attachments_dir())? {
                match std::fs::read(self.inner.store.blob_path(&sha)) {
                    Ok(bytes) => attachments.push((sha, bytes)),
                    // 读不到就少传一个附件：报告里如实记数，不静默当成功
                    Err(e) => tracing::warn!(%sha, error = %e, "附件读不到，本次导出不含它"),
                }
            }
        }
        let bundle = notera_importer::Bundle { attachments, ..bundle };
        let path = match c.path.as_deref() {
            Some(p) => std::path::PathBuf::from(p),
            None => {
                let dir = self.inner.data_dir.join("exports");
                std::fs::create_dir_all(&dir).map_err(|e| CmdError::of("storage", false).with(serde_json::json!({ "detail": e.to_string() })))?;
                // 时间戳里的 `:`/`-` 在 Windows 文件名里不安全，只留字母数字
                dir.join(format!("notera-{}.zip", self.inner.store.now().replace(|c: char| !c.is_ascii_alphanumeric(), "")))
            }
        };
        if path.exists() {
            // 绝不覆盖已有文件：用户指哪儿就写哪儿，指到一份备份上就是毁掉那次备份。
            return Err(CmdError::of("save_failed", false).with(serde_json::json!({
                "detail": format!("导出目标已存在，未覆盖：{}", path.display()),
            })));
        }
        notera_importer::write_bundle(&path, &bundle).map_err(|e| CmdError::of("save_failed", false).with(serde_json::json!({ "detail": e.to_string() })))?;
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
        }))
    }

    /// 导入一个 bundle。`intoEmpty` 只在库真的为空时放行；`merge` 走同步的
    /// 三方判定，绝不静默覆盖（§15 / I3）。
    pub fn import_data(&self, c: ImportCmd) -> Result<serde_json::Value, CmdError> {
        let path = std::path::PathBuf::from(c.path.ok_or_else(|| CmdError::of("bad_args", false))?);
        let bundle = notera_importer::read_bundle(&path)
            .map_err(|e| CmdError::of("save_failed", false).with(serde_json::json!({ "detail": e.to_string() })))?;
        let stats = self.inner.store.stats().map_err(CmdError::from)?;
        if c.mode.as_deref() == Some("intoEmpty") && (stats.notes > 0 || stats.folders > 1) {
            return Err(CmdError::of("save_failed", false).with(serde_json::json!({
                "detail": format!("目标库非空（笔记 {} / 文件夹 {}），intoEmpty 拒绝写入", stats.notes, stats.folders),
            })));
        }
        for (sha, bytes) in &bundle.attachments {
            self.inner.store.ingest_blob(sha, bytes).map_err(CmdError::from)?;
        }
        let mut ops = Vec::new();
        for env in bundle.records_in_apply_order() {
            let kind = env.get("kind").and_then(|k| k.as_str()).unwrap_or_default();
            let id = env.get("id").and_then(|i| i.as_str()).ok_or_else(|| CmdError::of("bad_args", false))?;
            let entity = EntityId::parse(id).map_err(|_| CmdError::of("bad_id", false))?;
            ops.push(if env.get("purged").and_then(|p| p.as_bool()).unwrap_or(false) {
                notera_store::ApplyOp::Purge {
                    kind: if kind == "folder" { notera_core::EntityKind::Folder } else { notera_core::EntityKind::Note },
                    id: entity,
                }
            } else if kind == "folder" {
                notera_store::ApplyOp::UpsertFolder { env }
            } else {
                notera_store::ApplyOp::UpsertNote { env }
            });
        }
        let report = self.inner.store.apply_remote(&ops).map_err(CmdError::from)?;
        let conflicts = self.inner.store.open_conflicts().map(|v| v.len()).unwrap_or(0);
        Ok(serde_json::json!({
            "path": path.to_string_lossy(),
            "merged": report.applied,
            "skipped": report.skipped,
            "conflicts": conflicts,
            "restoredAttachments": bundle.attachments.len(),
        }))
    }

    /// 一致性快照（DATA-MODEL §15）。产物自带 sha256 与 user_version，恢复闸门靠它们。
    pub fn backup_db(&self, dest_dir: Option<&std::path::Path>) -> Result<notera_store::BackupInfo, CmdError> {
        self.inner.store.create_backup(dest_dir).map_err(CmdError::from)
    }

    pub fn list_backups(&self) -> Result<Vec<notera_store::BackupInfo>, CmdError> {
        self.inner.store.list_backups().map_err(CmdError::from)
    }

    /// 恢复只"排期"，不在进程内换库：真正落地发生在下次启动 `Store::open` 之前。
    /// 返回 `restart_required=true` 是这条命令的正常结果，不是失败。
    pub fn stage_restore(&self, path: &std::path::Path) -> Result<serde_json::Value, CmdError> {
        let info = self.inner.store.stage_restore(path).map_err(CmdError::from)?;
        Ok(serde_json::json!({
            "restartRequired": true,
            "sha256": info.sha256,
            "userVersion": info.user_version,
            "path": info.path.to_string_lossy(),
        }))
    }

    // ------------------------------------------------------------ 配置面 ---

    pub fn current_account(&self) -> Result<Option<AccountDto>, CmdError> {
        Ok(ConfigRepository::active(&self.config()).map(account_dto))
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
        let acct = AccountConfig {
            id: draft.id.clone(),
            label: draft.label,
            base_url: draft.base_url,
            root_prefix: draft.root_prefix.unwrap_or_else(|| "/.notes".into()),
            auth_kind: match draft.auth_kind.as_deref() {
                Some("token") => notera_config::AuthKind::Token,
                _ => notera_config::AuthKind::Basic,
            },
            username: draft.username.clone(),
            credential_ref: if draft.password.is_some() || draft.username.is_some() {
                format!("keychain:{}", draft.id)
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
                username_ref: draft.proxy_username.map(|_| format!("keychain:proxy-user:{}", draft.id)),
                password_ref: draft.proxy_password.map(|_| format!("keychain:proxy-pass:{}", draft.id)),
                bypass: draft.bypass.unwrap_or_default(),
                resolve_remote_dns: true,
            },
            enabled: true,
        };
        // 校验/落盘的规则全在 notera-config 里；host 只折叠错误码（原子写、拒绝覆盖损坏配置）
        notera_config::validate_account(&acct)
            .map_err(|e| CmdError::of("invalid_account", false).with(serde_json::json!({ "why": e.to_string() })))?;
        self.inner
            .config_repo
            .upsert_account(&mut cfg, acct.clone())
            .map_err(|e| CmdError::of("invalid_account", false).with(serde_json::json!({ "why": e.to_string() })))?;
        self.inner.config_repo.save(&cfg).map_err(|e| {
            CmdError::of("save_failed", false).with(serde_json::json!({ "why": e.to_string() }))
        })?;
        *self.inner.config.lock().unwrap() = cfg.clone();
        // 配置里的账户必须在存储层落一行：outbox 是按 `sync_accounts` 扇出的，
        // 没这一行 = 本地写入永远不会排队给这台服务器（静默不同步）。
        self.inner
            .store
            .register_account(&acct.id, &acct.label, &acct.base_url)
            .map_err(|e| CmdError::of("invalid_account", false).with(serde_json::json!({ "why": e.to_string() })))?;
        self.set_sync(|v| v.phase = Phase::Provisioning);
        ConfigRepository::active(&cfg).map(account_dto).ok_or_else(|| CmdError::of("no_account", false))
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
        let device = DeviceId::parse(&cfg.device_id)
            .map_err(|e| CmdError::of("bad_device", false).with(serde_json::json!({ "why": e.to_string() })))?;
        let creds = self
            .secret_for(&acct)
            .map(|(user, secret)| {
                notera_webdav::Credentials::new(user, secret)
                    .map_err(|e| CmdError::of("invalid_account", false).with(serde_json::json!({ "why": e.to_string() })))
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
        Self::build_remote(&acct, device, credentials).map(Some)
    }

    /// 装配一个真适配器：地址/前缀/凭据/代理/TLS 全来自配置，出口只有 `notera-net`。
    /// 拆出来是为了让"能不能装起来"这条判定可以脱离钥匙串被测试到。
    fn build_remote(
        acct: &AccountConfig,
        device: DeviceId,
        credentials: notera_webdav::Credentials,
    ) -> Result<Arc<notera_webdav::WebDavRemote>, CmdError> {
        let proxy = net_proxy(&acct.proxy)?;
        let http = Arc::new(
            notera_net::HttpClient::build(&proxy, &net_tls(acct), notera_net::Timeouts::default())
                .map_err(|e| CmdError::of("net_config", true).with(serde_json::json!({ "why": e.to_string() })))?,
        );
        let remote = notera_webdav::WebDavRemote::new(
            notera_webdav::WebDavConfig::new(acct.base_url.clone())
                .with_root_prefix(acct.root_prefix.clone())
                .with_credentials(credentials)
                .with_device(device),
            http,
        )
        .map_err(|e| CmdError::of("invalid_account", false).with(serde_json::json!({ "why": e.to_string() })))?;
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
    pub async fn negotiate(&self, remote: &notera_webdav::WebDavRemote) -> Result<(), &'static str> {
        let cfg = self.config();
        let acct = ConfigRepository::active(&cfg).ok_or("sync.no_account")?;
        let mut state = self
            .inner
            .store
            .sync_state(&acct.id)
            .map_err(|_| "sync.storage_unavailable")?
            .ok_or("sync.state_missing")?;
        let observed = remote.fetch_protocol().await.map_err(|_| "sync.protocol_unreadable")?;
        let mine = self.default_folder_id().map_err(|_| "sync.no_default_folder")?;
        let ours = notera_sync::SYNC_PROTOCOL_VERSION as u64;
        match observed {
            Some(doc) => {
                let server = doc.get("protocol").and_then(|v| v.as_u64()).unwrap_or(0);
                let server_min = doc.get("min_protocol").and_then(|v| v.as_u64()).unwrap_or(server);
                if server_min > ours || server < ours {
                    self.set_sync(|v| {
                        v.phase = Phase::ReadOnly;
                        v.badge = Badge::Offline;
                        v.message_key = Some("sync.protocol_mismatch".into());
                    });
                    return Err("sync.protocol_mismatch");
                }
                let remote_root = doc.get("root_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
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
                if remote.root_looks_used().await.map_err(|_| "sync.protocol_unreadable")? {
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
                let created = remote.provision_protocol(&doc).await.map_err(|_| "sync.protocol_unreadable")?;
                state.root_id = Some(if created {
                    mine.as_str().to_string()
                } else {
                    // 抢输的一方必须接受对方那一份，而不是继续按自己的 root_id 写
                    let other = remote.fetch_protocol().await.map_err(|_| "sync.protocol_unreadable")?.ok_or("sync.protocol_mismatch")?;
                    other.get("root_id").and_then(|v| v.as_str()).ok_or("sync.protocol_mismatch")?.to_string()
                });
            }
        }
        state.phase = "online".into();
        self.inner.store.set_sync_state(&state).map_err(|_| "sync.storage_unavailable")?;
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
    pub async fn run_attachment_round(&self, remote: &notera_webdav::WebDavRemote) -> (usize, usize, usize) {
        const FILES: usize = 4;
        const BYTES: i64 = 64 * 1024 * 1024;
        let (mut up, mut down, mut failed) = (0usize, 0usize, 0usize);
        let mut budget = BYTES;

        for (i, job) in self.inner.store.attachment_uploads(FILES).unwrap_or_default().into_iter().enumerate() {
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
                    let _ = self.inner.store.set_attachment_states(&job.sha256, Some("error"), None);
                    let _ = self.inner.store.finish_attachment_ops(&job.sha256, false);
                    failed += 1;
                    continue;
                }
            };
            match remote.put_attachment(&job.sha256, &bytes).await {
                Ok(()) => {
                    let _ = self.inner.store.set_attachment_states(&job.sha256, None, Some("present"));
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
        for (i, job) in self.inner.store.attachment_downloads(FILES).unwrap_or_default().into_iter().enumerate() {
            if i > 0 && job.size > budget {
                break;
            }
            match remote.fetch_attachment(&job.sha256).await {
                Ok(Some(bytes)) => {
                    budget -= (bytes.len() as i64).min(budget);
                    if self.inner.store.ingest_blob(&job.sha256, &bytes).is_ok() {
                        let _ = self.inner.store.finish_attachment_ops(&job.sha256, true);
                        down += 1;
                    } else {
                        let _ = self.inner.store.finish_attachment_ops(&job.sha256, false);
                        failed += 1;
                    }
                }
                // 远端 404：只改远端态。§10 —— 任何情况下都不因远端缺失删本地
                Ok(None) => {
                    let _ = self.inner.store.set_attachment_states(&job.sha256, None, Some("absent"));
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
        (up, down, failed)
    }

    /// 常驻附件循环（壳 spawn 一次）。与文本调度器互不等待、互不阻塞。
    pub async fn run_attachments(self, remote: Arc<notera_webdav::WebDavRemote>, stop: Arc<AtomicBool>) {
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
            .map_err(|e| CmdError::of("unknown_account", false).with(serde_json::json!({ "why": e.to_string() })))?;
        self.inner.config_repo.save(&cfg).map_err(|e| CmdError::of("save_failed", false).with(serde_json::json!({ "why": e.to_string() })))?;
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
            .and_then(|a| self.inner.store.outbox_len(a, &[notera_store::OpState::Pending, notera_store::OpState::Inflight, notera_store::OpState::Failed]).ok())
            .unwrap_or(0);
        let conflicts = self.inner.store.open_conflicts().map(|v| v.len() as u32).unwrap_or(0);
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
        Scheduler { app: self, remote, stop: Arc::new(AtomicBool::new(false)) }
    }

    pub(crate) fn local_port(&self) -> HostLocalPort {
        HostLocalPort(self.clone())
    }

    pub(crate) fn apply_round_stats(&self, st: &RoundStats) {
        // 墙上时间只用于展示（I4/R3）；由 notera-core 的 Clock 统一供给，host 不引 chrono。
        let now = SystemClock.now().to_string();
        self.set_sync(|v| {
            v.in_flight = false;
            match st.outcome {
                notera_sync::RoundOutcome::Failed => {
                    v.badge = Badge::Failed;
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
                    progress: Some(Progress { done, total, bytes: 0 }),
                    error_code: None,
                });
            }
            SyncEvent::NeedsConflictAttention => {
                self.emit(BusEvent::Toast { message_key: "sync.conflict_attention".into(), level: "warn".into() })
            }
            SyncEvent::Completed(_) => {}
            SyncEvent::Failed { retryable, message_key } => {
                let key = message_key.to_string();
                self.set_sync(move |v| {
                    v.badge = if key.ends_with("offline") { Badge::Offline } else { Badge::Failed };
                    v.message_key = Some(key);
                    v.retryable = retryable;
                });
            }
        }
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
            EntityKind::Folder => store.get_folder(id).map_err(store_err)?.map(|f| f.deleted_at),
            EntityKind::Attachment => None,
        };
        match row {
            // 行还在：以行的 deleted_at 为准（None = 已恢复，绝不能回落到旧墓碑）
            Some(deleted) => Ok(deleted),
            None => store.get_tombstone(kind, id).map_err(store_err).map(|t| t.map(|t| t.deleted_at)),
        }
    }
}

/// 附件目录里的 blob 文件名（就是 sha256）。目录不存在 = 还没有任何附件。
fn list_blob_names(dir: &std::path::Path) -> Result<Vec<String>, CmdError> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(CmdError::of("storage", false).with(serde_json::json!({ "detail": e.to_string() }))),
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.len() == 64 && name.chars().all(|c| c.is_ascii_hexdigit()) {
            out.push(name);
        }
    }
    out.sort();
    Ok(out)
}

fn store_err(e: StoreError) -> LocalError {    match e {
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
    match acct.tls_policy.clone() {
        TlsPolicyKind::Strict => notera_net::TlsPolicy::Strict,
        // 选了 CaBundle 却没给 PEM：退回严格校验，而不是"什么都不校验"。
        TlsPolicyKind::CaBundle => acct
            .ca_pem
            .clone()
            .filter(|s| !s.trim().is_empty())
            .map(notera_net::TlsPolicy::CaBundle)
            .unwrap_or(notera_net::TlsPolicy::Strict),
        TlsPolicyKind::Pin => notera_net::TlsPolicy::Pin(acct.pinned_sha256.clone().unwrap_or_default()),
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
        let rows = self.0.store().dirty_entities(&acct).map_err(|e| LocalError::Storage(e.to_string()))?;
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
    fn cached_remote(&self) -> Result<Vec<RemoteView>, LocalError> {
        Ok(self.0.inner.cached_remote.lock().unwrap().clone())
    }
    fn cached_segment_hashes(&self) -> BTreeMap<String, String> {
        BTreeMap::new()
    }
    fn seq_applied(&self) -> u64 {
        self.0.inner.seq_applied.load(Ordering::SeqCst)
    }
    fn revision_json(&self, id: &str, rev: u64) -> Result<Option<serde_json::Value>, LocalError> {
        let eid = EntityId::parse(id).map_err(|_| LocalError::Storage("bad id".into()))?;
        self.0.store().revision_doc(&eid, Rev(rev)).map_err(|e| LocalError::Storage(e.to_string()))
    }
    fn envelope_wire(&self, kind: &str, id: &str) -> Result<Option<Vec<u8>>, LocalError> {
        let eid = EntityId::parse(id).map_err(|_| LocalError::Storage(format!("outbox 里的 id 不是 UUID: {id}")))?;
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
                ApplyOp::SetRemote { kind, id, rev, hash12 } => {
                    if let (Some(k), Ok(i)) = (EntityKind::from_tag(&kind), EntityId::parse(&id)) {
                        mapped.push(StoreApplyOp::SetRemote { kind: k, id: i, rev: Rev(rev), hash12 });
                    }
                }
                ApplyOp::MarkSynced { kind, id, rev } => {
                    if let (Some(k), Ok(i)) = (EntityKind::from_tag(&kind), EntityId::parse(&id)) {
                        let _ = self.0.store().mark_synced(k, &i, Rev(rev), "");
                    }
                }
                ApplyOp::Delete { kind, id, rev } | ApplyOp::Tombstone { kind, id, rev, .. } => {
                    if let (Some(k), Ok(i)) = (EntityKind::from_tag(&kind), EntityId::parse(&id)) {
                        mapped.push(StoreApplyOp::Tombstone { kind: k, id: i, rev: Rev(rev) });
                    }
                }
                ApplyOp::Purge { kind, id } => {
                    if let (Some(k), Ok(i)) = (EntityKind::from_tag(&kind), EntityId::parse(&id)) {
                        mapped.push(StoreApplyOp::Purge { kind: k, id: i });
                    }
                }
                ApplyOp::StoreManifest { wire, etag, seq } => {
                    self.0.set_manifest_cache(wire);
                    self.0.set_manifest_etag(etag);
                    // 本轮提交后的清单序号 = 本地已看到的远端头部（窗口覆盖判定用它，§6.2）
                    self.0.set_seq_applied(seq);
                }
            }
        }
        let rep = self.0.store().apply_remote(&mapped).map_err(|e| LocalError::Storage(e.to_string()))?;
        Ok(notera_sync::ApplyReport { applied: rep.applied, rejected: rep.skipped })
    }
    /// 引擎已经判完"这是冲突"，这里只做**记账字段**的翻译（不含任何判定）。
    ///
    /// `base_rev` 取 `sync_rev`：DATA-MODEL §4.3 定义共同祖先就是"最后确认一致的那一版"。
    fn record_conflict(&self, d: &Decision, l: &LocalView, r: &RemoteView) -> Result<(), LocalError> {
        let kind = EntityKind::from_tag(&l.kind).ok_or_else(|| LocalError::Storage(format!("未知 kind: {}", l.kind)))?;
        let id = EntityId::parse(&l.id).map_err(|e| LocalError::Storage(e.to_string()))?;
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
        self.0.emit(crate::BusEvent::Conflict { conflict_id, note_title: title });
        // 判定行号（P8/P10/P15…）只进日志：它是排障线索，不是 UI 词汇
        tracing::debug!(rule = d.rule, action = ?d.action, id = %l.id, conflict_id, "冲突已记账");
        Ok(())
    }
    fn outbox_take(&self, limit: usize) -> Result<Vec<notera_sync::OutboxItem>, LocalError> {
        let acct = self.account_id();
        let rows = self.0.store().outbox_take(&acct, limit).map_err(store_err)?;
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
    /// outbox 状态回写目前**接不上**：引擎传回来的是 `dedupe_key`（甚至不是键，见下），
    /// 而 `Store::outbox_state` 要的是行号 `id`。这里绝不用"看起来能编"的键去猜行号 ——
    /// 猜错就是把别的实体的待办标成 Done（丢上传）。缺的是 store 侧按键定位的能力。
    fn outbox_state(&self, key: &str, st: notera_sync::OutboxState, retry_at: Option<&str>) -> Result<(), LocalError> {
        tracing::debug!(key, state = ?st, ?retry_at, "outbox 状态回写未接线（Store 需要按 dedupe_key 定位行）");
        Ok(())
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
// ------------------------------------------------------------- 调度器 ---

/// 把 `Arc<R>` 借给"这一轮"的引擎用。
///
/// 为什么需要这层纯转发：`SyncEngine::new` 按值收 port，而 `R` 是壳层选的适配器类型，
/// host 无权给它加 `Clone` 约束 —— 加了就等于改 `App::start_sync::<R>(Arc<R>)` 的签名。
struct RemoteBorrow<'a, R>(&'a R);

#[async_trait::async_trait]
impl<R: notera_sync::RemotePort> notera_sync::RemotePort for RemoteBorrow<'_, R> {
    async fn fetch_manifest(&self, etag: Option<&str>) -> Result<Option<(Vec<u8>, Option<String>)>, notera_sync::RemoteError> {
        self.0.fetch_manifest(etag).await
    }
    async fn fetch_segment(&self, name: &str) -> Result<Vec<notera_sync::manifest::EntryRef>, notera_sync::RemoteError> {
        self.0.fetch_segment(name).await
    }
    async fn fetch_record(&self, kind: &str, id: &str) -> Result<Option<Vec<u8>>, notera_sync::RemoteError> {
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
    async fn commit_manifest(&self, wire: &[u8], cas_etag: Option<&str>) -> Result<Option<String>, notera_sync::RemoteError> {
        self.0.commit_manifest(wire, cas_etag).await
    }
    async fn put_segment(&self, name: &str, wire: &[u8]) -> Result<(), notera_sync::RemoteError> {
        self.0.put_segment(name, wire).await
    }
    async fn probe_record_etag(&self, kind: &str, id: &str) -> Result<Option<String>, notera_sync::RemoteError> {
        self.0.probe_record_etag(kind, id).await
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
            if app.inner.syncing.swap(true, Ordering::SeqCst) {
                continue; // 单轮并发上限 = 1
            }
            app.set_sync_public(Badge::Syncing);
            // §6.3：带上轮的 etag 才可能拿到 304（空轮 1 请求 0 字节正文）
            let etag = app.manifest_etag();
            let engine = notera_sync::SyncEngine::new(app.local_port(), RemoteBorrow(remote.as_ref()), EngineConfig::default());
            let (st, evs) = engine.run_round(etag.as_deref()).await;
            for e in evs {
                app.apply_sync_event(e);
            }
            app.apply_round_stats(&st);
            app.inner.syncing.store(false, Ordering::SeqCst);
        }
    }
}

impl App {
    fn set_sync_public(&self, b: Badge) {
        self.set_sync(|v| v.badge = b);
    }
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
        assert_eq!((s.phase.as_str(), s.badge.as_str()), ("unconfigured", "offline"));
        assert!(!s.retryable);
        assert_eq!(s.open_conflicts, 0);
        assert!(app.current_account().unwrap().is_none(), "没配过账户就没有活动账户");
    }

    /// DTO 的字段名就是前端契约（`apps/desktop/src/api/types.ts`），漂一个字母 UI 就静默显示 undefined。
    #[test]
    fn note_and_list_dto_field_names_match_the_frontend_contract() {
        let app = boot("dto");
        let folder = app.list_folders().unwrap().remove(0);
        let created = app.create_note(&EntityId::parse(&folder.id).unwrap(), doc("契约检查")).unwrap();
        let detail = commands::j(&created).unwrap();
        assert_eq!(
            sorted(keys(&detail)),
            sorted(
                [
                    "id", "folderId", "doc", "docFormat", "title", "summary", "charCount", "blockCount",
                    "hasAttachment", "pinned", "color", "rev", "contentHash", "createdAt", "updatedAt", "deletedAt"
                ]
                .map(str::to_string)
                .to_vec()
            )
        );
        assert_eq!(detail["title"], "契约检查", "title 由 doc 派生（DATA-MODEL §7.1）");

        let rows = app.list_notes(ListNotesCmd { folder_id: None, trash: false, limit: 10, offset: 0 }).unwrap();
        let row = rows.first().expect("列表必须有一行");
        assert_eq!(
            sorted(keys(row)),
            sorted(
                ["id", "folderId", "folderName", "title", "summary", "charCount", "hasAttachment", "pinned", "updatedAt", "deletedAt", "dirty"]
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
            (StoreError::NotFound { kind: EntityKind::Note, id: EntityId::new() }, "not_found"),
            (
                notera_core::error::StaleEdit { entity: EntityId::new(), expected: Rev(3), actual: Rev(5) }.into(),
                "stale_edit",
            ),
            (StoreError::ReadOnly { db: 9, supported: 3 }, "db_too_new"),
            (StoreError::Migration { from: 2, to: 3, detail: "x".into() }, "db_migration"),
            (StoreError::DocTooNew { doc: 9, supported: 1 }, "doc_too_new"),
            (StoreError::Constraint("名字为空".into()), "constraint"),
            (StoreError::InvalidDoc("坏文档".into()), "invalid_doc"),
            (StoreError::Rejected("远端拒绝".into()), "rejected"),
            (StoreError::Rich("富文本层拒绝".into()), "richtext"),
            (StoreError::Io(std::io::Error::other("磁盘满了")), "io"),
            (StoreError::Identity(notera_core::IdentityError::Malformed("x".into())), "bad_id"),
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
        let note = app.create_note(&EntityId::parse(&folder.id).unwrap(), doc("要吵架的笔记")).unwrap();

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
        let d = Decision { key: l.key(), action: Action::Conflict(ConflictKind::UpdateUpdate), rule: "P10" };
        app.local_port().record_conflict(&d, &l, &r).unwrap();

        let open = app.open_conflicts().unwrap();
        assert_eq!(open.len(), 1, "冲突必须进收件箱");
        assert_eq!(open[0].note_id, note.id);
        assert_eq!(open[0].note_title, "要吵架的笔记");
        assert_eq!(open[0].local_rev, bump(note.rev));
        assert_eq!(open[0].remote_rev, bump(bump(note.rev)));
        assert_eq!(open[0].base_rev, note.rev, "base = sync_rev = 共同祖先（DATA-MODEL §4.3）");
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
                BusEvent::Conflict { conflict_id, note_title } => {
                    assert_eq!(conflict_id, open[0].id);
                    assert_eq!(note_title, "要吵架的笔记");
                    break;
                }
                other => skipped.push(other),
            }
        }
        // 裁决走存储层，host 只做动词翻译；UI 的驼峰写法必须能用
        app.resolve_conflict(commands::ResolveConflictCmd { id: open[0].id, action: "keepBoth".into() }).unwrap();
        assert!(app.open_conflicts().unwrap().is_empty(), "裁决后不再是 open");
        let rows = app.list_notes(ListNotesCmd {
            folder_id: None,
            trash: false,
            limit: 50,
            offset: 0,
        }).unwrap();
        let copies: Vec<_> = rows.iter().filter(|r| r["title"].as_str().is_some_and(|t| t.ends_with("（本地副本）"))).collect();
        assert_eq!(copies.len(), 1, "§6：进收件箱的同一刻必须留下本地副本，用户之后选哪边都不丢字");
        let open2 = app.open_conflicts().unwrap();
        assert_eq!(open2.len(), 0);
    }

    /// UI 的四个动词（types.ts 的 ConflictAction）逐个都要翻得动。
    #[test]
    fn every_ui_conflict_action_maps_to_a_store_resolution() {
        for action in ["keepBoth", "replaceWithLocal", "replaceWithRemote", "manualMerge", "dismiss"] {
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
            app.resolve_conflict(commands::ResolveConflictCmd { id: cid, action: action.into() })
                .unwrap_or_else(|e| panic!("{action} 必须被接受，实际 {e:?}"));
        }
        let app = boot("resolve-bad");
        let e = app
            .resolve_conflict(commands::ResolveConflictCmd { id: 1, action: "yolo".into() })
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
        app.store().mark_synced(EntityKind::Note, &nid, Rev(note.rev), &note.content_hash).unwrap();
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
        assert_eq!(open[0].note_title, "对端已永久删除", "笔记行已不存在时要用墓碑快照，卡片不许空白");
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

        let wire = p.envelope_wire("n", &note.id).unwrap().expect("脏笔记必须有待 PUT 的记录");
        let env: serde_json::Value = serde_json::from_slice(&wire).unwrap();
        assert_eq!(env["kind"], "note");
        assert_eq!(env["hash"], note.content_hash);
        assert_eq!(env["payload"]["folder_id"], folder.id);
        assert!(p.envelope_wire("zz", &note.id).is_err(), "认不出的类型标记绝不能被当成笔记上传");

        app.store().delete_note(&nid).unwrap();
        let views = p.local_views().unwrap();
        let v = views.iter().find(|x| x.id == note.id).expect("软删的笔记仍在脏集里");
        assert!(v.deleted_at.is_some(), "P8/P11 的判据就是 deleted_at，填 None 等于把删除藏起来");
        assert!(v.purged_at.is_none());

        app.store().purge_note(&nid).unwrap();
        let ann = p.envelope_wire("n", &note.id).unwrap().expect("永久删除必须可传播");
        let ann: serde_json::Value = serde_json::from_slice(&ann).unwrap();
        assert_eq!(ann["purged"], true);
        assert!(ann["payload"].is_null());
        assert!(p.envelope_wire("a", "00000000-0000-0000-0000-000000000000").unwrap().is_none(), "附件走独立队列");
    }

    #[test]
    fn dispatch_reports_unknown_command_and_bad_args() {
        let app = boot("dispatch");
        let e = commands::dispatch(&app, "nope", json!({})).unwrap_err();
        assert_eq!((e.code.as_str(), e.message_key.as_str()), ("unknown_command", "cmd.unknown_command"));
        let e = commands::dispatch(&app, "get_note", json!({ "id": "不是 uuid" })).unwrap_err();
        assert_eq!(e.code, "bad_id");
        let e = commands::dispatch(&app, "get_note", json!({ "id": EntityId::new().to_string() })).unwrap();
        assert_eq!(e, Value::Null, "读不到的笔记返回 null，不是错误");
    }

    /// 没选文件夹也不能拒绝创建：落点是默认本，由核心决定（UI 只发 `folderId: null`）。
    #[test]
    fn create_note_without_folder_lands_in_the_default_one() {
        let app = boot("new-note");
        let default = app.default_folder_id().unwrap();
        let r = commands::dispatch(&app, "create_note", json!({ "folderId": Value::Null, "doc": doc("新建按钮") })).unwrap();
        assert_eq!(r["folderId"].as_str().unwrap(), default.as_str());
        let r = commands::dispatch(&app, "create_note", json!({ "doc": doc("连 folderId 都不给") })).unwrap();
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
        let a = app.configure_account(draft("a", "https://dav.home.example/dav", Some("u"))).unwrap();
        let e = app.configure_account(draft("b", "https://dav.work.example/dav", Some("u"))).unwrap_err();
        assert_eq!(e.code, "multi_account_unsupported");
        let now = app.current_account().unwrap().expect("a 仍在");
        assert_eq!((now.id.as_str(), now.base_url.as_str()), (a.id.as_str(), a.base_url.as_str()));
        // 停用它之后才允许换另一台
        app.remove_account("a").unwrap();
        app.configure_account(draft("b", "https://dav.work.example/dav", Some("u"))).unwrap();
    }

    /// 没凭据就绝不装适配器，而且**本地写入照常**（不变式 I8）。
    #[test]
    fn sync_remote_without_credentials_leaves_local_writes_working() {
        let app = boot("nocreds");
        assert!(app.sync_remote().unwrap().is_none(), "未配置账户时不启动引擎");
        app.configure_account(draft("a", "https://dav.home.example/dav", None)).unwrap();
        assert!(app.sync_remote().unwrap().is_none(), "没有用户名的账户拿不到凭据");
        let st = app.sync_status().unwrap();
        assert!(st.phase.contains("credential"), "状态必须可见，不能停在\"已同步\"：{st:?}");
        assert_eq!(st.badge, "offline");
        assert_eq!(st.message_key.as_deref(), Some("sync.needsCredentials"));
        let folder = app.list_folders().unwrap().remove(0);
        let n = app.create_note(&EntityId::parse(&folder.id).unwrap(), doc("离线也能写")).unwrap();
        assert_eq!(n.rev, 1, "拿不到凭据绝不能挡住本地写入（I8）");
    }

    /// 装配路径本身要能被测到（钥匙串还没接入，不能等它才有测试）。
    #[test]
    fn build_remote_makes_a_real_adapter_from_config() {
        let app = boot("remote");
        let acct = app.configure_account(draft("a", "https://dav.home.example/dav", Some("notera"))).unwrap();
        let cfg = app.config();
        let acct = cfg.accounts.iter().find(|a| a.id == acct.id).unwrap().clone();
        let device = DeviceId::parse(&cfg.device_id).unwrap();
        let creds = notera_webdav::Credentials::new("notera", "hunter2").unwrap();
        let remote = App::build_remote(&acct, device, creds).unwrap();
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
            app.configure_account(draft("srv", &url, Some("u"))).unwrap();
        }
        let remote_for = |app: &App| {
            let cfg = app.config();
            let acct = ConfigRepository::active(&cfg).unwrap().clone();
            let device = DeviceId::parse(&cfg.device_id).unwrap();
            std::sync::Arc::new(App::build_remote(&acct, device, Credentials::new("u", "p").unwrap()).unwrap())
        };
        let (ra, rb) = (remote_for(&a), remote_for(&b));
        a.negotiate(&ra).await.unwrap();
        b.negotiate(&rb).await.unwrap();

        let folder = Eid::parse(&a.list_folders().unwrap().remove(0).id).unwrap();
        let note = a.create_note(&folder, doc("带一张图")).unwrap();
        let blob = format!("PNG-ish bytes 图片 {}", note.id.as_str()).into_bytes();
        let sha = a
            .store()
            .attach_blob(&Eid::parse(&note.id).unwrap(), &blob, "image/png", Some("pic.png"), "blk0000001")
            .unwrap()
            .sha256;

        assert_eq!(a.run_attachment_round(&ra).await, (1, 0, 0), "A 应当把这一个附件传上去");
        assert!(ra.has_attachment(&sha).await.unwrap(), "服务器上必须真的存在这个 blob");
        assert_eq!(a.run_attachment_round(&ra).await.0, 0, "已 present 的不得重传");

        // B 只知道"清单说远端有这个 sha"
        b.store().register_remote_attachment(&sha, blob.len() as i64, "image/png").unwrap();
        assert_eq!(b.run_attachment_round(&rb).await, (0, 1, 0), "B 应当把它下载下来");
        assert_eq!(std::fs::read(b.store().blob_path(&sha)).unwrap(), blob, "字节必须逐字节相同");
        assert_eq!(
            b.store().attachment_for_state(&sha),
            ("available".into(), "present".into()),
            "落盘成功后两个状态位都要推进"
        );

        // 远端 404 时：只改远端态，绝不删本地（§10）
        let ghost = notera_core::ContentHash::of(b"ghost").as_str().replace("sha256:", "");
        b.store().register_remote_attachment(&ghost, 10, "image/png").unwrap();
        b.run_attachment_round(&rb).await;
        assert_eq!(b.store().attachment_for_state(&ghost).1, "absent");
        assert_eq!(b.store().attachment_for_state(&sha).0, "available", "别人的缺失不能牵连已存在的附件");
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
            app.configure_account(draft("srv", &url, Some("u"))).unwrap();
        }
        let remote_for = |app: &App| {
            let cfg = app.config();
            let acct = ConfigRepository::active(&cfg).unwrap().clone();
            let device = DeviceId::parse(&cfg.device_id).unwrap();
            App::build_remote(&acct, device, Credentials::new("u", "p").unwrap()).unwrap()
        };

        let ra = remote_for(&a);
        a.negotiate(&ra).await.expect("第一个库应当建好 protocol.json");
        let root_a = a.store().sync_state("srv").unwrap().unwrap().root_id.clone();
        assert_eq!(root_a.as_deref(), Some(a.default_folder_id().unwrap().as_str()));
        assert_eq!(a.sync_status().unwrap().phase, "online", "协商通过才允许进入同步态");

        // 情形一：本机已有自己的内容 → 必须停手，而不是接受别人的 root_id
        let bf = b.list_folders().unwrap().remove(0);
        b.create_note(&EntityId::parse(&bf.id).unwrap(), doc("我这台机器自己的笔记")).unwrap();
        let rb = remote_for(&b);
        assert_eq!(b.negotiate(&rb).await, Err("sync.root_mismatch"), "两个有内容的库不能并成一个");
        // 拒绝之后服务器上的 protocol.json 必须仍是 A 的那一份（没有被"顺手改写"）
        let still = rb.fetch_protocol().await.unwrap().unwrap();
        assert_eq!(still["root_id"].as_str(), root_a.as_deref());
        assert_eq!(b.sync_status().unwrap().phase, "error", "停手必须是可见状态，不是悄悄不干活");

        // 情形二：空库允许加入既有库（§9 的新设备场景）
        let c = boot("nego-c");
        c.configure_account(draft("srv", &url, Some("u"))).unwrap();
        let rc = remote_for(&c);
        c.negotiate(&rc).await.expect("空库应当接受服务器上已有的 root_id");
        assert_eq!(c.store().sync_state("srv").unwrap().unwrap().root_id.as_deref(), root_a.as_deref());
    }
}
