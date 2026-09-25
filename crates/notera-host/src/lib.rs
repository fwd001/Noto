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

use commands::{AccountDraftCmd, AccountDto, CmdError, ConflictDto, FolderDto, ListNotesCmd, NoteDto, SearchCmd, SearchHitDto, SyncStatusDto};
use notera_config::{AccountConfig, AppConfig, ConfigError, ConfigRepository, ProxyProfile, ProxyMode, TlsPolicyKind};
use notera_core::{DeviceId, EntityId, EntityKind, Rev, Timestamp};
use notera_store::{ApplyOp as StoreApplyOp, ConflictRow, Note, NoteListRow, NoteQuery, SearchPath, Store, StoreError};
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
#[derive(Clone, Debug, Default)]
struct SyncView {
    phase: Phase,
    badge: Badge,
    last_success_at: Option<String>,
    message_key: Option<String>,
    retryable: bool,
    in_flight: bool,
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
        let config = repo.load().map_err(|e| BootError::Config(e.to_string()))?;
        let device_id = match config.device_id.as_str() {
            "" => {
                let d = DeviceId::default();
                let mut c = config.clone();
                c.device_id = d.to_string();
                repo.save(&c).map_err(|e| BootError::Config(e.to_string()))?;
                d
            }
            s => DeviceId::parse(s).map_err(|e| BootError::Config(e.to_string()))?,
        };
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
                sync_view: Mutex::new(SyncView { phase: Phase::Unconfigured, badge: Badge::Offline, ..Default::default() }),
                cached_manifest: Mutex::new(None),
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
        let names: BTreeMap<String, String> = self
            .inner
            .store
            .list_folders()?
            .into_iter()
            .map(|f| (f.id.to_string(), f.name))
            .collect();
        let rows = self.inner.store.list_notes(&q)?;
        rows.iter().map(|r| self.to_list_dto(r, &names)).collect()
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

    pub fn to_folder_dto(&self, f: notera_store::Folder) -> Result<FolderDto, CmdError> {
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

    pub fn search(&self, c: SearchCmd) -> Result<Vec<SearchHitDto>, CmdError> {
        let hits = self.inner.store.search(&notera_store::SearchQuery { text: c.text, limit: c.limit })?;
        let mut out = Vec::with_capacity(hits.len());
        for h in hits {
            // 短查询走 LIKE 兜底是**实现细节**，但它是排障关键，因此记进日志而不进 UI。
            if matches!(h.path, SearchPath::LikeFallback) {
                log::debug!("search fallback LIKE for short query: {}", h.note_id);
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
        Ok(rows.into_iter().map(to_conflict_dto).collect())
    }

    pub fn resolve_conflict(&self, c: commands::ResolveConflictCmd) -> Result<(), CmdError> {
        match c.action.as_str() {
            "dismiss" => self.inner.store.dismiss_conflict(c.id)?,
            a @ ("keep_both" | "use_local" | "use_remote" | "merged") => self.inner.store.resolve_conflict(c.id, a)?,
            other => return Err(CmdError::of("bad_action", false).with(serde_json::json!({ "action": other }))),
        }
        self.emit(BusEvent::NotesChanged { ids: vec![] });
        Ok(())
    }

    pub fn set_pref(&self, key: &str, value: serde_json::Value) -> Result<(), CmdError> {
        self.inner.store.set_pref(key, &value).map_err(CmdError::from)
    }
    pub fn get_prefs(&self) -> Result<serde_json::Value, CmdError> {
        self.inner.store.get_prefs().map_err(CmdError::from)
    }

    // ------------------------------------------------------------ 配置面 ---

    pub fn current_account(&self) -> Result<Option<AccountDto>, CmdError> {
        let cfg = self.config();
        Ok(AccountConfig::from_cfg(notera_config::active(&cfg)))
    }

    pub fn configure_account(&self, draft: AccountDraftCmd) -> Result<AccountDto, CmdError> {
        let mut cfg = self.inner.config.lock().unwrap().clone();
        let acct = AccountConfig {
            id: draft.id.clone(),
            label: draft.label,
            base_url: draft.base_url,
            root_prefix: draft.root_prefix.unwrap_or_else(|| "/.notes".into()),
            auth_kind: match draft.auth_kind.as_deref() {
                Some("token") => notera_config::AuthKind::Token,
                _ => notera_config::AuthKind::Basic,
            },
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
                username_ref: draft.proxy_username.map(|u| format!("keychain:proxy-user:{}", draft.id)),
                password_ref: draft.proxy_password.map(|p| format!("keychain:proxy-pass:{}", draft.id)),
                bypass: draft.bypass.unwrap_or_default(),
                resolve_remote_dns: true,
            },
            enabled: true,
        };
        notera_config::validate_account(&acct).map_err(|e| CmdError::of("invalid_account", false).with(serde_json::json!({ "why": e.to_string() })))?;
        self.inner
            .config_repo
            .upsert_account(&mut cfg, acct)
            .map_err(|e| CmdError::of("invalid_account", false).with(serde_json::json!({ "why": e.to_string() })))?;
        self.inner.config_repo.save(&cfg).map_err(|e| CmdError::of("save_failed", false).with(serde_json::json!({ "why": e.to_string() })))?;
        *self.inner.config.lock().unwrap() = cfg.clone();
        self.set_sync(|v| v.phase = Phase::Provisioning);
        notera_config::active(&cfg).and_then(|a| AccountConfig::from_cfg(Some(a))).ok_or_else(|| CmdError::of("no_account", false))
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
        let now = Timestamp::new(chrono::Utc::now()).to_string();
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
        log::info!(
            "sync round {:?}: req={} up={}B down={}B push={} pull={} conflict={} cas={}",
            st.outcome, st.requests, st.bytes_up, st.bytes_down, st.pushed, st.pulled, st.conflicts, st.cas_retries
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
    pub(crate) fn set_seq_applied(&self, seq: u64) {
        self.inner.seq_applied.store(seq, Ordering::SeqCst);
    }
}

fn to_conflict_dto(r: ConflictRow) -> ConflictDto {
    ConflictDto {
        id: r.id,
        note_id: r.entity_id,
        note_title: r.note_title.unwrap_or_else(|| "（无标题）".into()),
        base_rev: r.base_rev,
        local_rev: r.local_rev,
        remote_rev: r.remote_rev,
        copy_note_id: r.copy_note_id,
        created_at: r.created_at,
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
        Timestamp::new(chrono::Utc::now()).to_string()
    }
    fn local_views(&self) -> Result<Vec<LocalView>, LocalError> {
        let acct = self.account_id();
        let rows = self.0.store().dirty_entities(&acct).map_err(|e| LocalError::Storage(e.to_string()))?;
        Ok(rows
            .into_iter()
            .map(|d| LocalView {
                kind: kind_tag(d.kind).into(),
                id: d.id.to_string(),
                rev: d.rev.get(),
                sync_rev: d.sync_rev.get(),
                sync_hash: None,
                content_hash: d.content_hash,
                deleted_at: None,
                purged_at: None,
                edited_after_delete: false,
            })
            .collect())
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
        let eid = EntityId::parse(id).map_err(|_| LocalError::Storage("bad id".into()))?;
        let k = kind_of(kind);
        let wire = match k {
            EntityKind::Note => self.0.store().note_envelope_wire(&eid),
            EntityKind::Folder => self.0.store().folder_envelope_wire(&eid),
            EntityKind::Attachment => Ok(None),
        }
        .map_err(|e| LocalError::Storage(e.to_string()))?;
        Ok(wire)
    }
    fn apply(&self, ops: Vec<ApplyOp>) -> Result<notera_sync::ApplyReport, LocalError> {
        let mut mapped = Vec::new();
        for o in ops {
            match o {
                ApplyOp::Upsert { kind, id, wire } => {
                    let env: serde_json::Value = serde_json::from_slice(&wire).unwrap_or_default();
                    match kind.as_str() {
                        "f" => mapped.push(StoreApplyOp::UpsertFolder { env }),
                        _ => mapped.push(StoreApplyOp::UpsertNote { env }),
                    }
                }
                ApplyOp::SetRemote { kind, id, rev, hash12 } => {
                    if let (Ok(k), Ok(i)) = (parse_kind(&kind), EntityId::parse(&id)) {
                        mapped.push(StoreApplyOp::SetRemote { kind: k, id: i, rev: Rev(rev), hash12 });
                    }
                }
                ApplyOp::MarkSynced { kind, id, rev } => {
                    if let (Ok(k), Ok(i)) = (parse_kind(&kind), EntityId::parse(&id)) {
                        let _ = self.0.store().mark_synced(k, &i, Rev(rev), "");
                    }
                }
                ApplyOp::Delete { kind, id, rev } | ApplyOp::Tombstone { kind, id, rev, .. } => {
                    if let (Ok(k), Ok(i)) = (parse_kind(&kind), EntityId::parse(&id)) {
                        mapped.push(StoreApplyOp::Tombstone { kind: k, id: i, rev: Rev(rev) });
                    }
                }
                ApplyOp::Purge { kind, id } => {
                    if let (Ok(k), Ok(i)) = (parse_kind(&kind), EntityId::parse(&id)) {
                        mapped.push(StoreApplyOp::Purge { kind: k, id: i });
                    }
                }
                ApplyOp::StoreManifest { wire, .. } => {
                    self.0.set_manifest_cache(wire);
                }
            }
        }
        let rep = self.0.store().apply_remote(&mapped).map_err(|e| LocalError::Storage(e.to_string()))?;
        Ok(notera_sync::ApplyReport { applied: rep.applied, rejected: rep.skipped })
    }
    fn record_conflict(&self, d: &Decision, l: &LocalView, r: &RemoteView) -> Result<(), LocalError> {
        let _ = (d, l, r);
        Ok(())
    }
    fn outbox_take(&self, limit: usize) -> Result<Vec<notera_sync::OutboxItem>, LocalError> {
        let acct = self.account_id();
        let rows = self.0.store().outbox_take(&acct, limit).map_err(|e| LocalError::Storage(e.to_string()))?;
        Ok(rows
            .into_iter()
            .map(|o| notera_sync::OutboxItem {
                dedupe_key: o.dedupe_key,
                kind: kind_tag(o.entity_kind).into(),
                id: o.entity_id.to_string(),
                rev: o.payload_rev.map(|r| r.get()).unwrap_or(0),
                op: format!("{:?}", o.op).to_lowercase(),
            })
            .collect())
    }
    fn outbox_state(&self, key: &str, st: notera_sync::OutboxState, retry_at: Option<&str>) -> Result<(), LocalError> {
        let s = match st {
            notera_sync::OutboxState::Pending => notera_store::OpState::Pending,
            notera_sync::OutboxState::Inflight => notera_store::OpState::Inflight,
            notera_sync::OutboxState::Done => notera_store::OpState::Done,
            notera_sync::OutboxState::Failed => notera_store::OpState::Failed,
            notera_sync::OutboxState::Superseded => notera_store::OpState::Superseded,
            notera_sync::OutboxState::Blocked => notera_store::OpState::Blocked,
        };
        let _ = retry_at;
        let _ = key;
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
fn kind_of(tag: &str) -> EntityKind {
    match tag {
        "f" => EntityKind::Folder,
        "a" => EntityKind::Attachment,
        _ => EntityKind::Note,
    }
}
fn parse_kind(tag: &str) -> Result<EntityKind, ()> {
    Ok(kind_of(tag))
}

// ------------------------------------------------------------- 调度器 ---

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
        let mut last_etag: Option<String> = None;
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
            let engine = notera_sync::SyncEngine::new(app.local_port(), (*remote).clone_for_engine(), EngineConfig::default());
            let (st, evs) = engine.run_round(last_etag.as_deref()).await;
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
    let mut last = app.inner.dirty_ticks.load(Ordering::SeqCst);
    loop {
        tokio::time::sleep(Duration::from_millis(250)).await;
        let now = app.inner.dirty_ticks.load(Ordering::SeqCst);
        if now != last {
            last = now;
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
