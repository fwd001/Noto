//! 引擎 + 真适配器 + 真服务器：device A push → device B pull。
//!
//! 这是本 crate 最有价值的一个文件：它证明 [`WebDavRemote`] 不是"能过自己的测试"的 mock，
//! 而是 `notera_sync::RemotePort` 的一个**可用实现** —— 引擎那句"清单是公告板"的契约
//! （§0 R1/R2）在这里是逐请求跑在 TCP 上验证的。本地侧只需要一个最小内存 `LocalPort`
//! （`crates/notera-sync/tests/engine.rs` 里的假端口是私有的，且它的远端是假的），
//! 远端一行都不打折。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use notera_core::{canonical_json, ContentHash, DeviceId, EntityId};
use notera_net::{HttpClient, ProxyProfile, Timeouts, TlsPolicy};
use notera_sync::plan::{LocalView, RemoteView};
use notera_sync::{
    ApplyOp, ApplyReport, EngineConfig, LocalError, LocalPort, OutboxItem, OutboxState, RemotePort,
    RoundOutcome, SyncEngine,
};
use notera_test_webdav::{Backend, Started, TestServer};
use notera_webdav::{Caps, Credentials, WebDavConfig, WebDavRemote};
use serde_json::{json, Value};

const AT: &str = "2026-09-25T12:00:00.000Z";

// ------------------------------------------------------------- 本地端口 ---

#[derive(Clone)]
struct Note {
    id: String,
    rev: u64,
    sync_rev: u64,
    hash: String,
    deleted_at: Option<String>,
    purged_at: Option<String>,
}

/// 一台设备的最小本地库：够引擎判 P1..P18，不多做一件事。
#[derive(Clone, Default)]
struct Device {
    device_id: Arc<Mutex<DeviceId>>,
    notes: Arc<Mutex<BTreeMap<String, Note>>>,
    envelopes: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
    remote: Arc<Mutex<BTreeMap<(String, String), RemoteView>>>,
    applied: Arc<Mutex<Vec<ApplyOp>>>,
    conflicts: Arc<Mutex<Vec<(String, String)>>>,
    outbox: Arc<Mutex<Vec<(String, OutboxState)>>>,
    manifest: Arc<Mutex<Option<Vec<u8>>>>,
    etag: Arc<Mutex<Option<String>>>,
    seq: Arc<AtomicU64>,
}

impl Device {
    fn new() -> Device {
        Device { device_id: Arc::new(Mutex::new(DeviceId::new())), ..Default::default() }
    }

    fn device_str(&self) -> String {
        self.device_id.lock().unwrap().to_string()
    }

    /// 本机新建/编辑一条笔记：rev 前进、信封重算、内容哈希随之变。
    fn write_note(&self, id: &EntityId, text: &str) {
        let rev = self.notes.lock().unwrap().get(id.as_str()).map(|n| n.rev).unwrap_or(0) + 1;
        let wire = note_envelope(id, rev, text, &self.device_str());
        let hash = envelope_hash(&wire);
        let key = format!("n/{id}");
        self.envelopes.lock().unwrap().insert(key, wire);
        let mut notes = self.notes.lock().unwrap();
        let entry = notes.entry(id.as_str().to_string()).or_insert(Note {
            id: id.as_str().to_string(),
            rev: 0,
            sync_rev: 0,
            hash: String::new(),
            deleted_at: None,
            purged_at: None,
        });
        entry.rev = rev;
        entry.hash = hash;
    }

    fn rev_of(&self, id: &EntityId) -> u64 {
        self.notes.lock().unwrap().get(id.as_str()).map(|n| n.rev).unwrap_or(0)
    }

    fn sync_rev_of(&self, id: &EntityId) -> u64 {
        self.notes.lock().unwrap().get(id.as_str()).map(|n| n.sync_rev).unwrap_or(0)
    }

    fn stored_wire(&self, id: &EntityId) -> Option<Vec<u8>> {
        self.envelopes.lock().unwrap().get(&format!("n/{id}")).cloned()
    }

    fn etag(&self) -> Option<String> {
        self.etag.lock().unwrap().clone()
    }

    fn applied(&self) -> Vec<ApplyOp> {
        self.applied.lock().unwrap().clone()
    }
}

impl LocalPort for Device {
    fn account_id(&self) -> String {
        "acct-webdav".into()
    }
    fn device_id(&self) -> String {
        self.device_str()
    }
    fn now(&self) -> String {
        AT.into()
    }
    fn local_views(&self) -> Result<Vec<LocalView>, LocalError> {
        Ok(self
            .notes
            .lock()
            .unwrap()
            .values()
            .map(|n| LocalView {
                kind: "n".into(),
                id: n.id.clone(),
                rev: n.rev,
                sync_rev: n.sync_rev,
                sync_hash: Some(n.hash.clone()),
                content_hash: n.hash.clone(),
                deleted_at: n.deleted_at.clone(),
                purged_at: n.purged_at.clone(),
                edited_after_delete: false,
            })
            .collect())
    }
    fn cached_remote(&self) -> Result<Vec<RemoteView>, LocalError> {
        Ok(self.remote.lock().unwrap().values().cloned().collect())
    }
    fn cached_segment_hashes(&self) -> BTreeMap<String, String> {
        BTreeMap::new()
    }
    fn seq_applied(&self) -> u64 {
        self.seq.load(Ordering::SeqCst)
    }
    fn revision_json(&self, _id: &str, _rev: u64) -> Result<Option<Value>, LocalError> {
        Ok(None)
    }
    fn envelope_wire(&self, kind: &str, id: &str) -> Result<Option<Vec<u8>>, LocalError> {
        Ok(self.envelopes.lock().unwrap().get(&format!("{kind}/{id}")).cloned())
    }
    fn apply(&self, ops: Vec<ApplyOp>) -> Result<ApplyReport, LocalError> {
        let n = ops.len();
        for op in ops.clone() {
            match op {
                ApplyOp::Upsert { kind, id, wire } => {
                    let v: Value = serde_json::from_slice(&wire).map_err(|e| LocalError::Storage(e.to_string()))?;
                    let rev = v["rev"].as_u64().unwrap_or(0);
                    let hash = v["hash"].as_str().unwrap_or_default().to_string();
                    let mut notes = self.notes.lock().unwrap();
                    let note = notes.entry(id.clone()).or_insert(Note {
                        id: id.clone(),
                        rev: 0,
                        sync_rev: 0,
                        hash: String::new(),
                        deleted_at: None,
                        purged_at: None,
                    });
                    // P2/P6：拉到即一致点，rev == sync_rev。
                    note.rev = rev;
                    note.sync_rev = rev;
                    note.hash = hash.clone();
                    drop(notes);
                    self.envelopes.lock().unwrap().insert(format!("{kind}/{id}"), wire);
                    self.remote.lock().unwrap().insert(
                        ("n".to_string(), id.clone()),
                        RemoteView { kind: "n".into(), id, rev, hash: Some(hash), deleted_at: None, purged: false },
                    );
                }
                ApplyOp::MarkSynced { id, rev, .. } => {
                    if let Some(note) = self.notes.lock().unwrap().get_mut(&id) {
                        note.sync_rev = rev;
                    }
                }
                ApplyOp::SetRemote { kind, id, rev, hash12 } => {
                    match kind.as_str() {
                        "seq" => {
                            self.seq.store(rev, Ordering::SeqCst);
                        }
                        "manifest" => {
                            let _ = id;
                            let _ = hash12;
                        }
                        k => {
                            let key = (k.to_string(), id.clone());
                            self.remote.lock().unwrap().insert(
                                key.clone(),
                                RemoteView { kind: key.0, id: key.1, rev, hash: Some(hash12), deleted_at: None, purged: false },
                            );
                        }
                    }
                }
                ApplyOp::StoreManifest { wire, etag, seq } => {
                    *self.manifest.lock().unwrap() = Some(wire);
                    *self.etag.lock().unwrap() = etag;
                    self.seq.store(seq, Ordering::SeqCst);
                }
                ApplyOp::Tombstone { id, rev, deleted_at, purged, .. } => {
                    let mut notes = self.notes.lock().unwrap();
                    if let Some(note) = notes.get_mut(&id) {
                        note.rev = rev;
                        note.sync_rev = rev;
                        note.deleted_at = deleted_at;
                        if purged {
                            note.purged_at = Some(AT.to_string());
                        }
                    }
                }
                ApplyOp::Delete { id, .. } => {
                    self.notes.lock().unwrap().remove(&id);
                }
                ApplyOp::Purge { id, .. } => {
                    self.notes.lock().unwrap().remove(&id);
                }
            }
        }
        self.applied.lock().unwrap().extend(ops);
        Ok(ApplyReport { applied: n, rejected: 0 })
    }
    fn record_conflict(&self, d: &notera_sync::plan::Decision, _l: &LocalView, _r: &RemoteView) -> Result<(), LocalError> {
        self.conflicts.lock().unwrap().push(d.key.clone());
        Ok(())
    }
    fn outbox_take(&self, _limit: usize) -> Result<Vec<OutboxItem>, LocalError> {
        Ok(Vec::new())
    }
    fn outbox_settle(&self, kind: &str, id: &str, rev: u64, st: OutboxState) -> Result<(), LocalError> {
        self.outbox.lock().unwrap().push((format!("{kind}:{id}:{rev}"), st));
        Ok(())
    }
    fn cached_manifest(&self) -> Option<Vec<u8>> {
        self.manifest.lock().unwrap().clone()
    }
}

// ----------------------------------------------------------------- 夹具 ---

/// 信封形态同 `crates/notera-store/tests/common/mod.rs::note_envelope`；
/// `hash` 按 §3 现算 `sha256(canonical(payload))`（本 crate 不依赖 notera-richtext）。
fn note_envelope(id: &EntityId, rev: u64, text: &str, device: &str) -> Vec<u8> {
    let payload = json!({
        "v": 1,
        "content": [{ "id": "blk000001", "type": "paragraph", "content": [{ "text": text }] }]
    });
    let env = json!({
        "protocol": 1, "kind": "note", "id": id.as_str(), "rev": rev, "sync_rev": rev - 1,
        "hash": ContentHash::of(canonical_json(&payload).as_bytes()).as_str(),
        "updated_at": AT, "device": device,
        "deleted_at": null, "purged": false,
        "enc": { "alg": "none", "hash_alg": "sha256" },
        "payload": payload, "ct": null
    });
    serde_json::to_vec(&env).expect("信封")
}

fn envelope_hash(wire: &[u8]) -> String {
    serde_json::from_slice::<Value>(wire)
        .ok()
        .and_then(|v| v["hash"].as_str().map(str::to_string))
        .unwrap_or_default()
}

fn http() -> Arc<HttpClient> {
    Arc::new(
        HttpClient::build(&ProxyProfile::direct(), &TlsPolicy::Strict, Timeouts::short(Duration::from_secs(15)))
            .expect("出口"),
    )
}

fn remote(srv: &Started, http: &Arc<HttpClient>, device: DeviceId) -> WebDavRemote {
    let cfg = WebDavConfig::new(srv.base_url.clone())
        .with_credentials(Credentials::new("engine", "engine-pass").expect("凭据"))
        .with_device(device)
        .with_caps(Caps::conventional());
    WebDavRemote::new(cfg, Arc::clone(http)).expect("远端")
}

// ----------------------------------------------------------------- 用例 ---

#[tokio::test]
async fn device_a_pushes_and_device_b_pulls_over_real_http() {
    let srv = TestServer::start(Backend::Mem).await;
    let http = http();
    let a = Device::new();
    let b = Device::new();
    let id = EntityId::new();

    let ea = SyncEngine::new(a.clone(), remote(&srv, &http, a.device_id.lock().unwrap().clone()), EngineConfig::default());
    let eb = SyncEngine::new(b.clone(), remote(&srv, &http, b.device_id.lock().unwrap().clone()), EngineConfig::default());

    // ---- A 的第一轮：空根 + 本地脏 → P3 补传（记录先落，清单最后公告，R2）
    a.write_note(&id, "设备 A 的第一条笔记");
    let (st_a1, ev_a1) = ea.run_round(None).await;
    assert_eq!(st_a1.pushed, 1, "A 的脏笔记必须上传，事件 {ev_a1:?}");
    assert_eq!(st_a1.outcome, RoundOutcome::Converged);
    assert_eq!(st_a1.cas_retries, 0);
    assert_eq!(a.rev_of(&id), 1);
    assert_eq!(a.sync_rev_of(&id), 1, "写成功即清脏（MarkSynced）");
    assert!(a.etag().is_some(), "A 必须记住清单 etag，否则下一轮不是空轮");

    // 记录真的在服务器上，且就是 A 发出去的那一坨字节。
    let wire_a1 = a.stored_wire(&id).expect("信封");
    assert_eq!(
        ea.remote().fetch_record("n", id.as_str()).await.unwrap().as_deref(),
        Some(wire_a1.as_slice()),
        "远端记录必须与本地信封逐字节相同"
    );
    let log = srv.request_log();
    assert!(
        log.iter().any(|r| r.method == "PUT" && r.path.ends_with(&format!("/records/note/{id}.json"))),
        "写入必须真的打到记录路径：{log:?}"
    );

    // ---- A 的第二轮：真 304 空轮（§6.3：1 请求 0 字节正文），且服务器确实回了 304
    let reqs_before = srv.request_log().len();
    let (st_a_empty, _) = ea.run_round(a.etag().as_deref()).await;
    assert_eq!(st_a_empty.outcome, RoundOutcome::NoOp, "无改动无新内容必须是空轮");
    assert_eq!(st_a_empty.requests, 1, "空轮必须恰好 1 请求");
    assert_eq!(st_a_empty.bytes_down, 0, "空轮必须 0 字节正文（If-None-Match 命中）");
    assert_eq!(srv.request_log().len() - reqs_before, 1, "服务器上只多了一条请求");
    assert_eq!(srv.request_log().last().map(|r| (r.method.as_str(), r.status)), Some(("GET", 304)), "304 是真报文，不是客户端自我声明");

    // ---- B 的第一轮：清单里的新条目 → P2 拉取并落库
    let (st_b1, _) = eb.run_round(None).await;
    assert_eq!(st_b1.pulled, 1, "B 必须把 A 公告的那条拉下来");
    assert_eq!(st_b1.outcome, RoundOutcome::Converged);
    assert_eq!(st_b1.pushed, 0, "纯拉取轮次不得重写清单");
    assert_eq!(b.rev_of(&id), 1);
    assert_eq!(b.stored_wire(&id).as_deref(), Some(wire_a1.as_slice()), "B 落库的就是 A 写的那一条");
    assert!(b.applied().iter().any(|o| matches!(o, ApplyOp::Upsert { id: i, .. } if i == id.as_str())));

    // ---- B 的第二轮：仍然只有公告，没有新内容 → NoOp。
    // 注意这一轮**不是** 0 字节：引擎在纯拉取轮次里不会缓存清单 etag（只有提交了才
    // StoreManifest），所以下一轮无 ETag 可发、只能整份重读。行为正确，流量偏贵 ——
    // 这是 notera-sync 侧的可改进点，本 crate 不改它。
    let (st_b2, _) = eb.run_round(b.etag().as_deref()).await;
    assert_eq!(st_b2.outcome, RoundOutcome::NoOp);
    assert_eq!(st_b2.requests, 1);
    assert_eq!(st_b2.pulled, 0);

    // ---- A 编辑 → 推第二轮（304 命中，但本地脏 → 计划来自本地 + 清单回落到缓存）
    a.write_note(&id, "设备 A 又改了一次");
    let (st_a2, _) = ea.run_round(a.etag().as_deref()).await;
    assert_eq!(st_a2.pushed, 1, "304 轮也必须公告本地变更（cached_manifest 回落路径）");
    assert_eq!(a.rev_of(&id), 2);
    let wire_a2 = a.stored_wire(&id).expect("信封");
    assert_ne!(wire_a2, wire_a1);

    // ---- B 拉第二轮
    let (st_b3, _) = eb.run_round(b.etag().as_deref()).await;
    assert_eq!(st_b3.pulled, 1);
    assert_eq!(b.rev_of(&id), 2, "B 必须前进到 A 的第二版");
    assert_eq!(b.stored_wire(&id).as_deref(), Some(wire_a2.as_slice()));

    // ---- 反向：B 编辑 → 推 → A 拉。两个方向都得通。
    b.write_note(&id, "设备 B 的补充");
    let (st_b4, _) = eb.run_round(b.etag().as_deref()).await;
    assert_eq!(st_b4.pushed, 1, "B 的编辑同样要能推上去");
    let (st_a3, _) = ea.run_round(a.etag().as_deref()).await;
    assert_eq!(st_a3.pulled, 1);
    assert_eq!(a.rev_of(&id), 3, "A 应看到 B 的第三版");
    assert_eq!(a.sync_rev_of(&id), 3, "拉到的即一致点，不产生假脏");
    let (st_b5, _) = eb.run_round(b.etag().as_deref()).await;
    assert_eq!(st_b5.outcome, RoundOutcome::NoOp);

    // ---- 服务端事实：清单 seq 每公告一次前进一次，prev 是上一版，没有暂存残留
    let (final_wire, _) = ea.remote().fetch_manifest(None).await.unwrap().expect("清单在");
    let final_seq = notera_sync::manifest::Manifest::parse(&final_wire).expect("清单自校验").seq;
    assert_eq!(
        final_seq, 4,
        "seq: initial 1 → A 首推 2 → A 二次推 3 → B 推 4；纯拉取轮不得重写清单（§6.3）"
    );
    let dump = srv.dump_prefix("/.notes/manifest");
    let paths: Vec<&str> = dump["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["path"].as_str().unwrap_or_default())
        .collect();
    assert!(paths.iter().any(|p| p.ends_with("manifest/index.json")), "{paths:?}");
    assert!(paths.iter().any(|p| p.ends_with("manifest/index.json.prev")), "上一版必须被轮转保留（§1）");
    assert!(!paths.iter().any(|p| p.contains(".tmp-")), "提交完成后不得留下暂存对象: {paths:?}");
    assert_eq!(srv.request_log().iter().filter(|r| r.status == 500).count(), 0, "全程无服务器侧异常");
}

#[tokio::test]
async fn pushed_data_survives_a_real_restart() {
    // Fs 后端 + restart()：状态从磁盘重建，etag 由内容 sha256 派生故天然稳定。
    let dir = scratch_dir("restart");
    let srv = TestServer::start(Backend::Fs(dir.clone())).await;
    let http = http();
    let a = Device::new();
    let b = Device::new();
    let id = EntityId::new();

    let ea = SyncEngine::new(a.clone(), remote(&srv, &http, a.device_id.lock().unwrap().clone()), EngineConfig::default());
    a.write_note(&id, "重启前写下的笔记");
    let (st, _) = ea.run_round(None).await;
    assert_eq!(st.pushed, 1);
    let wire = a.stored_wire(&id).expect("信封");
    let etag_before = a.etag().expect("etag");

    srv.restart().await;
    assert_eq!(srv.generation(), 2, "服务器确实换了一代（进程内重启）");

    // 换一个"新设备"B（也是新引擎实例），从磁盘重建的库里把 A 的笔记拉回来。
    let eb = SyncEngine::new(b.clone(), remote(&srv, &http, b.device_id.lock().unwrap().clone()), EngineConfig::default());
    let (st_b, _) = eb.run_round(None).await;
    assert_eq!(st_b.pulled, 1, "重启后必须还能读到同一条记录");
    assert_eq!(b.stored_wire(&id).as_deref(), Some(wire.as_slice()));

    // A 的 etag 跨重启仍然有效：304 空轮不是运气，是内容寻址 ETag 的事实。
    let (st_a, _) = ea.run_round(Some(etag_before.as_str())).await;
    assert_eq!(st_a.outcome, RoundOutcome::NoOp);
    assert_eq!(st_a.bytes_down, 0);
    assert_eq!(st_a.requests, 1);

    let disk = dir.join(".notes").join("records").join("note").join(format!("{id}.json"));
    assert_eq!(std::fs::read(&disk).expect("磁盘上真有这个文件").as_slice(), wire.as_slice());
    let _ = std::fs::remove_dir_all(&dir);
}

/// 唯一临时目录（不引入 tempfile：本 crate 的 dev-deps 里没有它）。
fn scratch_dir(tag: &str) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let id = N.fetch_add(1, Ordering::SeqCst);
    let mut p = std::env::temp_dir();
    p.push(format!("notera-webdav-it-{tag}-{}-{id}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).expect("临时目录");
    p
}
