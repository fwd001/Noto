//! 引擎行为测试：用内存假端口跑真实 `run_round`，断言 SYNC-PROTOCOL 的规范性质。
//!
//! 价值在于"引擎确实按协议走"，而不是"某个纯函数返回了预期值"。
//! 假端口内部状态走 Arc，因此既能把所有权交给引擎、又能在事后断言。

use async_trait::async_trait;
use notera_sync::manifest::{EntryRef, Manifest, SegmentRef, Window};
use notera_sync::plan::{Decision, LocalView, RemoteView};
use notera_sync::*;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

fn path(kind: &str, id: &str) -> String {
    format!("{kind}/{id}")
}

// ------------------------------------------------------------------ 远端 ---

#[derive(Clone, Default)]
struct FakeRemote {
    requests: Arc<AtomicU32>,
    manifest: Arc<Mutex<Option<Vec<u8>>>>,
    etag: Arc<Mutex<Option<String>>>,
    records: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
    segments: Arc<Mutex<BTreeMap<String, Vec<EntryRef>>>>,
    cas_failures: Arc<AtomicU32>,
    corrupt_manifest: Arc<AtomicBool>,
    /// 让所有调用返回 401，用于验证"认证失败必须停轮"
    deny_auth: Arc<AtomicBool>,
    /// §11.4：pretend 别人贴着的租约
    leases: Arc<Mutex<Vec<PeerLease>>>,
    /// 本机贴出去的每一租约（token, expires_at）
    publishes: Arc<Mutex<Vec<(String, String)>>>,
    fail_publish: Arc<AtomicBool>,
    /// 读别人的租约直接报错（模拟列目录能力坏掉 / 离线）
    fail_holders: Arc<AtomicBool>,
}

#[async_trait]
impl RemotePort for FakeRemote {
    async fn fetch_manifest(
        &self,
        etag: Option<&str>,
    ) -> Result<Option<(Vec<u8>, Option<String>)>, RemoteError> {
        self.tick()?;
        let body = self.manifest.lock().unwrap().clone();
        let cur = self.etag.lock().unwrap().clone();
        let Some(body) = body else { return Ok(None) };
        if etag.is_some() && etag == cur.as_deref() {
            return Ok(None); // 304：0 字节正文
        }
        let out = if self.corrupt_manifest.load(Ordering::SeqCst) {
            let mut bad = body.clone();
            let n = bad.len() / 2;
            bad[n] = b'X';
            bad
        } else {
            body
        };
        Ok(Some((out, cur)))
    }
    async fn fetch_segment(&self, name: &str) -> Result<Vec<EntryRef>, RemoteError> {
        self.tick()?;
        Ok(self
            .segments
            .lock()
            .unwrap()
            .get(name)
            .cloned()
            .unwrap_or_default())
    }
    async fn fetch_record(&self, kind: &str, id: &str) -> Result<Option<Vec<u8>>, RemoteError> {
        self.tick()?;
        Ok(self.records.lock().unwrap().get(&path(kind, id)).cloned())
    }
    async fn put_record(
        &self,
        kind: &str,
        id: &str,
        wire: &[u8],
        _if_match: Option<&str>,
    ) -> Result<Commit, RemoteError> {
        self.tick()?;
        self.records
            .lock()
            .unwrap()
            .insert(path(kind, id), wire.to_vec());
        Ok(Commit {
            etag: Some(format!("\"{id}-v1\"")),
            verified: true,
        })
    }
    async fn commit_manifest(
        &self,
        wire: &[u8],
        cas_etag: Option<&str>,
    ) -> Result<Option<String>, RemoteError> {
        self.tick()?;
        if self.cas_failures.load(Ordering::SeqCst) > 0 {
            self.cas_failures.fetch_sub(1, Ordering::SeqCst);
            return Err(RemoteError::Precondition);
        }
        let cur = self.etag.lock().unwrap().clone();
        if cur.is_some() && cas_etag != cur.as_deref() {
            return Err(RemoteError::Precondition);
        }
        *self.manifest.lock().unwrap() = Some(wire.to_vec());
        let fresh = format!("\"blob-{}\"", wire.len());
        *self.etag.lock().unwrap() = Some(fresh.clone());
        Ok(Some(fresh))
    }
    async fn put_segment(&self, name: &str, wire: &[u8]) -> Result<(), RemoteError> {
        self.tick()?;
        let entries: Vec<EntryRef> = serde_json::from_slice(wire).unwrap_or_default();
        self.segments.lock().unwrap().insert(name.into(), entries);
        Ok(())
    }
    async fn probe_record_etag(&self, kind: &str, id: &str) -> Result<Option<String>, RemoteError> {
        Ok(self
            .records
            .lock()
            .unwrap()
            .contains_key(&path(kind, id))
            .then(|| format!("\"{id}-v0\"")))
    }

    async fn lease_publish(
        &self,
        token: &str,
        expires_at: &str,
        _seq: u64,
    ) -> Result<(), RemoteError> {
        self.tick()?;
        if self.fail_publish.load(Ordering::SeqCst) {
            return Err(RemoteError::Server);
        }
        self.publishes
            .lock()
            .unwrap()
            .push((token.to_string(), expires_at.to_string()));
        Ok(())
    }

    async fn lease_holders(&self, _known: &[String]) -> Result<Vec<PeerLease>, RemoteError> {
        if self.fail_holders.load(Ordering::SeqCst) {
            return Err(RemoteError::Server);
        }
        Ok(self.leases.lock().unwrap().clone())
    }
}

impl FakeRemote {
    fn tick(&self) -> Result<(), RemoteError> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        if self.deny_auth.load(Ordering::SeqCst) {
            return Err(RemoteError::Auth);
        }
        Ok(())
    }
    fn seed(&self, m: Manifest) {
        *self.manifest.lock().unwrap() = Some(m.to_wire());
        *self.etag.lock().unwrap() = Some("\"m0\"".into());
    }
    fn etag(&self) -> Option<String> {
        self.etag.lock().unwrap().clone()
    }
    fn req(&self) -> u32 {
        self.requests.load(Ordering::SeqCst)
    }
    fn manifest_bytes(&self) -> Vec<u8> {
        self.manifest.lock().unwrap().clone().unwrap_or_default()
    }
}

// ------------------------------------------------------------------ 本地 ---

#[derive(Clone, Default)]
struct FakeLocal {
    locals: Arc<Mutex<Vec<LocalView>>>,
    cached: Arc<Mutex<Vec<RemoteView>>>,
    seg_hashes: Arc<Mutex<BTreeMap<String, String>>>,
    seq: Arc<AtomicU64>,
    applied: Arc<Mutex<Vec<ApplyOp>>>,
    conflicts: Arc<Mutex<Vec<(String, String)>>>,
    envelopes: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
    #[allow(dead_code)]
    outbox_states: Arc<Mutex<Vec<(String, OutboxState)>>>,
    cached_manifest: Arc<Mutex<Option<Vec<u8>>>>,
    cached_etag: Arc<Mutex<Option<String>>>,
    /// §11.4：引擎贴完租约后有没有把状态记到本地
    leases: Arc<Mutex<Vec<(String, String)>>>,
}

impl LocalPort for FakeLocal {
    fn account_id(&self) -> String {
        "acct".into()
    }
    fn device_id(&self) -> String {
        "dev-1".into()
    }
    fn now(&self) -> String {
        "2026-09-25T00:00:00.000Z".into()
    }
    fn local_views(&self) -> Result<Vec<LocalView>, LocalError> {
        Ok(self.locals.lock().unwrap().clone())
    }
    fn cached_remote(&self) -> Result<Vec<RemoteView>, LocalError> {
        Ok(self.cached.lock().unwrap().clone())
    }
    fn cached_segment_hashes(&self) -> BTreeMap<String, String> {
        self.seg_hashes.lock().unwrap().clone()
    }
    fn seq_applied(&self) -> u64 {
        self.seq.load(Ordering::SeqCst)
    }
    fn record_lease(&self, token: &str, expires_at: &str) -> Result<(), LocalError> {
        self.leases
            .lock()
            .unwrap()
            .push((token.to_string(), expires_at.to_string()));
        Ok(())
    }
    fn revision_json(&self, _id: &str, _rev: u64) -> Result<Option<serde_json::Value>, LocalError> {
        Ok(None)
    }
    fn envelope_wire(&self, kind: &str, id: &str) -> Result<Option<Vec<u8>>, LocalError> {
        Ok(self.envelopes.lock().unwrap().get(&path(kind, id)).cloned())
    }
    fn apply(&self, ops: Vec<ApplyOp>) -> Result<ApplyReport, LocalError> {
        let n = ops.len();
        for o in &ops {
            // 引擎提交成功后写回缓存清单，下一轮 304 才有的可回落
            if let ApplyOp::StoreManifest { wire, etag, .. } = o {
                *self.cached_manifest.lock().unwrap() = Some(wire.clone());
                *self.cached_etag.lock().unwrap() = etag.clone();
            }
        }
        self.applied.lock().unwrap().extend(ops);
        Ok(ApplyReport {
            applied: n,
            rejected: 0,
        })
    }
    fn record_conflict(
        &self,
        d: &Decision,
        _l: &LocalView,
        _r: &RemoteView,
    ) -> Result<(), LocalError> {
        self.conflicts.lock().unwrap().push(d.key.clone());
        Ok(())
    }
    fn outbox_take(&self, _limit: usize) -> Result<Vec<OutboxItem>, LocalError> {
        Ok(Vec::new())
    }
    fn outbox_settle(
        &self,
        kind: &str,
        id: &str,
        rev: u64,
        st: OutboxState,
    ) -> Result<(), LocalError> {
        self.outbox_states
            .lock()
            .unwrap()
            .push((format!("{kind}:{id}:{rev}"), st));
        Ok(())
    }
    fn cached_manifest(&self) -> Option<Vec<u8>> {
        self.cached_manifest.lock().unwrap().clone()
    }
}

// ----------------------------------------------------------------- 夹具 ---

fn local(id: &str, rev: u64, sync_rev: u64, hash: &str) -> LocalView {
    LocalView {
        kind: "n".into(),
        id: id.into(),
        rev,
        sync_rev,
        sync_hash: Some(format!("sha256:{hash}")),
        content_hash: format!("sha256:{hash}"),
        deleted_at: None,
        purged_at: None,
        edited_after_delete: false,
    }
}

fn ent(i: &str, r: u64, h: &str) -> EntryRef {
    EntryRef {
        i: i.into(),
        t: "n".into(),
        r,
        h: h.into(),
        s: 40,
        d: None,
        p: 0,
    }
}

fn base_manifest() -> Manifest {
    Manifest::initial("root", "dev-1", "2026-09-25T00:00:00.000Z", "notera test")
}

async fn run(l: FakeLocal, r: FakeRemote, etag: Option<&str>) -> (RoundStats, Vec<SyncEvent>) {
    SyncEngine::new(l, r, EngineConfig::default())
        .run_round(etag)
        .await
}
async fn run_with(
    l: FakeLocal,
    r: FakeRemote,
    etag: Option<&str>,
    cfg: EngineConfig,
) -> (RoundStats, Vec<SyncEvent>) {
    SyncEngine::new(l, r, cfg).run_round(etag).await
}

/// §11.4 的开关。FakeLocal 的 now() 固定在 2026-09-25T00:00:00Z，
/// 所以测试里的"新鲜/过期"都以它为基准，不碰墙上时间。
fn lease_cfg() -> EngineConfig {
    EngineConfig {
        lease: LeasePolicy::On { ttl_ms: 60_000 },
        ..EngineConfig::default()
    }
}

fn dirty_local(l: &FakeLocal, id: &str, rev: u64, hash: &str) {
    l.locals.lock().unwrap().push(local(id, rev, rev - 1, hash));
    l.envelopes
        .lock()
        .unwrap()
        .insert(path("n", id), format!("{{\"rev\":{rev}}}").into_bytes());
}

// ------------------------------------------------------------------ 用例 ---

#[tokio::test]
async fn empty_round_costs_exactly_one_request_and_zero_bytes() {
    // SYNC-PROTOCOL §6.3：空轮必须是 1 请求 0 字节正文。
    let (l, r) = (FakeLocal::default(), FakeRemote::default());
    r.seed(base_manifest());
    let etag = r.etag();
    let (st, ev) = run(l, r.clone(), etag.as_deref()).await;
    assert_eq!(st.requests, 1, "空轮应恰好 1 请求，实际 {}", st.requests);
    assert_eq!(st.bytes_down, 0, "304 轮不得下载正文");
    assert_eq!(st.bytes_up, 0);
    assert_eq!(st.outcome, RoundOutcome::NoOp);
    assert_eq!(r.req(), 1);
    // 或断言：空轮次允许两种等价形态 —— 要么不发事件，要么发一个 NoOp 完成事件，两种都算「什么都没发生」
    assert!(ev.is_empty() || ev.contains(&SyncEvent::Completed(RoundOutcome::NoOp)));
}

#[tokio::test]
async fn local_edit_is_pushed_then_announced_in_manifest() {
    let l = FakeLocal::default();
    l.locals.lock().unwrap().push(local("n1", 3, 2, "aaaa"));
    l.envelopes
        .lock()
        .unwrap()
        .insert(path("n", "n1"), b"{\"rev\":3}".to_vec());
    let r = FakeRemote::default();
    r.seed(base_manifest());
    let etag = r.etag();
    let (st, _) = run(l.clone(), r.clone(), etag.as_deref()).await;

    assert_eq!(st.pushed, 1, "本地脏实体必须上传");
    assert!(
        r.records.lock().unwrap().contains_key("n/n1"),
        "记录必须先于清单落远端（R2）"
    );
    let m = Manifest::parse(&r.manifest_bytes()).expect("提交后的清单必须自校验通过");
    assert!(
        m.window.entries.iter().any(|x| x.i == "n1" && x.r == 3),
        "窗口应公告该变更"
    );
    assert!(m.seq > 1, "seq 必须前进");
    assert!(
        l.applied
            .lock()
            .unwrap()
            .iter()
            .any(|o| matches!(o, ApplyOp::MarkSynced { rev: 3, .. })),
        "上传成功须记 synced"
    );
}

#[tokio::test]
async fn remote_new_note_is_pulled_and_applied() {
    let l = FakeLocal::default();
    let r = FakeRemote::default();
    let mut m = base_manifest();
    m.window = Window {
        since_seq: 1,
        complete: true,
        entries: vec![ent("remote1", 7, "bbbbbbbbbbbb")],
    };
    m.refresh_checksum();
    r.seed(m);
    r.records
        .lock()
        .unwrap()
        .insert(path("n", "remote1"), b"{\"rev\":7}".to_vec());
    // 首轮没有已知 etag：传当前 etag 会得到 304，那是正确行为但测不到 pull。
    let (st, _) = run(l.clone(), r, None).await;
    assert_eq!(st.pulled, 1, "远端新增必须被拉取");
    assert!(
        l.applied
            .lock()
            .unwrap()
            .iter()
            .any(|o| matches!(o, ApplyOp::Upsert { id, .. } if id == "remote1")),
        "拉到的记录必须交给 apply"
    );
}

#[tokio::test]
async fn corrupt_manifest_never_deletes_local_data() {
    // D1/D2 底线：清单坏了要停轮，绝不能当成"远端是空的"继续。
    let l = FakeLocal::default();
    l.locals.lock().unwrap().push(local("keepme", 1, 1, "aaaa"));
    let r = FakeRemote::default();
    r.seed(base_manifest());
    r.corrupt_manifest.store(true, Ordering::SeqCst);
    let (st, ev) = run(l.clone(), r, None).await;
    assert_eq!(st.outcome, RoundOutcome::Failed, "坏清单必须终止本轮");
    assert!(st.requests <= 1, "不得继续刷后续请求，实际 {}", st.requests);
    assert!(
        ev.iter().any(|x| matches!(
            x,
            SyncEvent::Failed {
                retryable: true,
                ..
            }
        )),
        "应给出可重试失败事件"
    );
    assert!(
        l.applied.lock().unwrap().is_empty(),
        "坏清单情形下不得对本地做任何写入或删除"
    );
}

#[tokio::test]
async fn cas_conflict_is_retried_then_converges() {
    let l = FakeLocal::default();
    l.locals.lock().unwrap().push(local("n1", 4, 2, "cccc"));
    l.envelopes
        .lock()
        .unwrap()
        .insert(path("n", "n1"), b"{\"rev\":4}".to_vec());
    let r = FakeRemote::default();
    r.seed(base_manifest());
    r.cas_failures.store(2, Ordering::SeqCst); // 前两次 CAS 被别人抢了
    let etag = r.etag();
    let (st, _) = run(l, r.clone(), etag.as_deref()).await;
    assert_eq!(st.cas_retries, 2, "应重试两次后成功");
    let m = Manifest::parse(&r.manifest_bytes()).unwrap();
    assert!(
        m.window.entries.iter().any(|x| x.i == "n1"),
        "最终清单必须包含本次变更"
    );
}

#[tokio::test]
async fn remote_delete_versus_local_edit_reaches_conflict_inbox() {
    let l = FakeLocal::default();
    l.locals
        .lock()
        .unwrap()
        .push(local("n1", 8, 5, "localhash"));
    let r = FakeRemote::default();
    let mut m = base_manifest();
    let mut del = ent("n1", 9, "remotehash12");
    del.d = Some("2026-09-25T00:00:00.000Z".into());
    m.window.entries = vec![del];
    m.refresh_checksum();
    r.seed(m);
    let (st, ev) = run(l.clone(), r, None).await;
    assert_eq!(st.conflicts, 1, "删除 vs 修改必须计入冲突");
    assert!(ev.contains(&SyncEvent::NeedsConflictAttention));
    assert_eq!(l.conflicts.lock().unwrap().len(), 1);
    assert!(
        !l.applied
            .lock()
            .unwrap()
            .iter()
            .any(|o| matches!(o, ApplyOp::Purge { .. })),
        "冲突情形不得静默永久删除本地"
    );
}

#[tokio::test]
async fn behind_beyond_window_fetches_changed_segments_only() {
    let l = FakeLocal::default();
    l.seq.store(1, Ordering::SeqCst); // 远远落后于窗口
    let r = FakeRemote::default();
    let mut m = base_manifest();
    m.segments = vec![SegmentRef {
        n: "seg-0000".into(),
        cover: ["a".into(), "z".into()],
        count: 2,
        hash12: "123456789012".into(),
        bytes: 20,
    }];
    m.window = Window {
        since_seq: 99,
        complete: true,
        entries: vec![],
    };
    m.refresh_checksum();
    r.seed(m);
    r.segments.lock().unwrap().insert(
        "seg-0000".into(),
        vec![ent("s1", 1, "aaaaaaaaaaaa"), ent("s2", 2, "bbbbbbbbbbbb")],
    );
    r.records
        .lock()
        .unwrap()
        .insert(path("n", "s1"), b"{\"i\":\"s1\"}".to_vec());
    r.records
        .lock()
        .unwrap()
        .insert(path("n", "s2"), b"{\"i\":\"s2\"}".to_vec());
    let (st, _) = run(l, r.clone(), None).await;
    assert!(st.requests >= 3, "清单 + 分段 + 两条记录");
    assert_eq!(st.pulled, 2, "分段揭示的两条新笔记都应拉取");
    assert_eq!(r.req(), st.requests, "统计数必须与真实请求数一致");
}

#[tokio::test]
async fn uncached_segment_hash_forces_refetch() {
    let l = FakeLocal::default();
    l.seq.store(1, Ordering::SeqCst);
    let r = FakeRemote::default();
    let mut m = base_manifest();
    m.segments = vec![SegmentRef {
        n: "seg-0000".into(),
        cover: ["a".into(), "z".into()],
        count: 1,
        hash12: "newhash12345".into(),
        bytes: 10,
    }];
    m.window = Window {
        since_seq: 99,
        complete: true,
        entries: vec![],
    };
    m.refresh_checksum();
    r.seed(m);
    r.segments
        .lock()
        .unwrap()
        .insert("seg-0000".into(), vec![ent("s9", 1, "cccccccccccc")]);
    r.records
        .lock()
        .unwrap()
        .insert(path("n", "s9"), b"{}".to_vec());
    // 缓存里是同名的旧 hash → 必须重拉
    l.seg_hashes
        .lock()
        .unwrap()
        .insert("seg-0000".into(), "oldhash12345".into());
    let (st, _) = run(l, r.clone(), None).await;
    assert!(st.pulled >= 1, "hash 变化必须触发分段重取");
    assert!(r.req() >= 3);
}

#[tokio::test]
async fn auth_failure_halts_round_and_reports_reauth() {
    let l = FakeLocal::default();
    l.locals.lock().unwrap().push(local("n1", 2, 1, "dddd"));
    l.envelopes
        .lock()
        .unwrap()
        .insert(path("n", "n1"), b"{\"rev\":2}".to_vec());
    let r = FakeRemote::default();
    r.seed(base_manifest());
    r.deny_auth.store(true, Ordering::SeqCst);
    let (st, ev) = run(l, r.clone(), None).await;
    assert_eq!(st.outcome, RoundOutcome::Failed);
    assert_eq!(r.req(), 1, "401 必须停轮，不得继续刷请求，实际 {}", r.req());
    assert!(ev.iter().any(|x| matches!(
        x,
        SyncEvent::Failed {
            retryable: false,
            message_key: "sync.auth_required"
        }
    )));
}

#[tokio::test]
async fn no_push_means_no_manifest_commit() {
    // 纯拉取轮次不得重写清单（省一次 CAS 与一轮流量）
    let l = FakeLocal::default();
    let r = FakeRemote::default();
    let mut m = base_manifest();
    m.window = Window {
        since_seq: 1,
        complete: true,
        entries: vec![ent("only", 1, "dddddddddddd")],
    };
    m.refresh_checksum();
    let before = m.to_wire();
    r.seed(m);
    r.records
        .lock()
        .unwrap()
        .insert(path("n", "only"), b"{\"i\":\"only\"}".to_vec());
    let etag = r.etag();
    let (st, _) = run(l, r.clone(), etag.as_deref()).await;
    assert_eq!(st.pushed, 0);
    assert_eq!(r.manifest_bytes(), before, "无本地写入时清单必须原样不动");
}

// ---------------------------------------------------------------- §11.4 租约 ---

#[tokio::test]
async fn an_enabled_lease_is_published_before_the_round_reads_anything() {
    let (l, r) = (FakeLocal::default(), FakeRemote::default());
    r.seed(base_manifest());
    let etag = r.etag();
    let (st, _) = run_with(l.clone(), r.clone(), etag.as_deref(), lease_cfg()).await;
    assert_eq!(
        r.publishes.lock().unwrap().len(),
        1,
        "开了租约就得贴自己的，否则别人看不见我"
    );
    let (token, expires) = &r.publishes.lock().unwrap()[0];
    assert!(!token.is_empty());
    assert!(
        expires.ends_with('Z'),
        "过期时刻得是可解析的时间串：{expires}"
    );
    assert_eq!(
        l.leases.lock().unwrap().len(),
        1,
        "贴上的租约要记进本地状态（sync_state 的那两列）"
    );
    assert_eq!(
        st.outcome,
        RoundOutcome::NoOp,
        "空轮不该因为租约变成别的结论"
    );
}

#[tokio::test]
async fn a_fresh_peer_lease_defers_the_announcement_but_keeps_the_upload() {
    let (l, r) = (FakeLocal::default(), FakeRemote::default());
    r.seed(base_manifest());
    dirty_local(&l, "n1", 3, "aaaa");
    // 别人（不是 dev-1）还新鲜
    r.leases.lock().unwrap().push(PeerLease {
        device: "dev-2".into(),
        expires_at: "2099-01-01T00:00:00.000Z".into(),
        seq: 1,
    });
    let etag = r.etag();
    let (st, ev) = run_with(l.clone(), r.clone(), etag.as_deref(), lease_cfg()).await;

    assert_eq!(st.pushed, 1, "记录照旧先推上去（清单才是公告板）");
    let m = Manifest::parse(&r.manifest_bytes()).expect("清单仍要可解析");
    assert!(
        !m.window.entries.iter().any(|x| x.i == "n1"),
        "让路时**不能**提交清单，否则并发公告互相覆盖"
    );
    assert_eq!(m.seq, 1, "seq 不该前进，实际 {}", m.seq);
    assert!(
        !l.applied
            .lock()
            .unwrap()
            .iter()
            .any(|o| matches!(o, ApplyOp::MarkSynced { .. })),
        "没公告就不能把改动标成已同步 —— 那会变成静默丢失",
    );
    assert!(
        ev.contains(&SyncEvent::Deferred {
            device: "dev-2".into()
        }),
        "让路必须让用户看得见：{ev:?}"
    );
    assert_eq!(st.outcome, RoundOutcome::Partial);
}

#[tokio::test]
async fn an_expired_or_self_owned_lease_does_not_block_the_announcement() {
    for (label, lease) in [
        (
            "过期了（设备崩溃留下的那份）",
            PeerLease {
                device: "dev-2".into(),
                expires_at: "2020-01-01T00:00:00.000Z".into(),
                seq: 1,
            },
        ),
        (
            "是自己的",
            PeerLease {
                device: "dev-1".into(),
                expires_at: "2099-01-01T00:00:00.000Z".into(),
                seq: 1,
            },
        ),
        (
            "时间读不出来",
            PeerLease {
                device: "dev-2".into(),
                expires_at: "不是时间".into(),
                seq: 1,
            },
        ),
    ] {
        let (l, r) = (FakeLocal::default(), FakeRemote::default());
        r.seed(base_manifest());
        dirty_local(&l, "n1", 3, "aaaa");
        r.leases.lock().unwrap().push(lease);
        let etag = r.etag();
        let (st, ev) = run_with(l, r.clone(), etag.as_deref(), lease_cfg()).await;
        assert_eq!(st.pushed, 1, "{label}");
        let m = Manifest::parse(&r.manifest_bytes()).expect("清单");
        assert!(
            m.window.entries.iter().any(|x| x.i == "n1"),
            "{label}：这类租约不该挡住公告"
        );
        assert!(
            !ev.iter().any(|e| matches!(e, SyncEvent::Deferred { .. })),
            "{label}"
        );
    }
}

#[tokio::test]
async fn an_unreadable_lease_directory_must_not_stop_the_announcement() {
    // §11.4：读不到别人的租约 = 当作没人持有。这一层坏了绝不能变成"永远不同步"。
    let (l, r) = (FakeLocal::default(), FakeRemote::default());
    r.seed(base_manifest());
    dirty_local(&l, "n1", 3, "aaaa");
    r.fail_holders.store(true, Ordering::SeqCst);
    let etag = r.etag();
    let (st, ev) = run_with(l, r.clone(), etag.as_deref(), lease_cfg()).await;
    let m = Manifest::parse(&r.manifest_bytes()).expect("清单");
    assert!(
        m.window.entries.iter().any(|x| x.i == "n1"),
        "租约读不出来仍要公告：{st:?} {ev:?}"
    );
    assert!(!ev.iter().any(|e| matches!(e, SyncEvent::Deferred { .. })));
}

#[tokio::test]
async fn a_failed_publish_never_aborts_the_round() {
    // 贴不上自己的租约 = 别人看不见我；本轮该照做，不能因此不同步。
    let (l, r) = (FakeLocal::default(), FakeRemote::default());
    r.seed(base_manifest());
    dirty_local(&l, "n1", 3, "aaaa");
    r.fail_publish.store(true, Ordering::SeqCst);
    let etag = r.etag();
    let (st, _) = run_with(l.clone(), r.clone(), etag.as_deref(), lease_cfg()).await;
    assert_eq!(st.pushed, 1, "贴不上租约也要照常上传");
    let m = Manifest::parse(&r.manifest_bytes()).expect("清单");
    assert!(
        m.window.entries.iter().any(|x| x.i == "n1"),
        "贴不上租约也要照常公告"
    );
    assert!(r.publishes.lock().unwrap().is_empty());
    assert!(
        l.leases.lock().unwrap().is_empty(),
        "没贴成功就别记本地状态"
    );
}

#[tokio::test]
async fn with_the_lease_off_the_round_touches_no_locks_at_all() {
    let (l, r) = (FakeLocal::default(), FakeRemote::default());
    r.seed(base_manifest());
    dirty_local(&l, "n1", 3, "aaaa");
    r.leases.lock().unwrap().push(PeerLease {
        device: "dev-2".into(),
        expires_at: "2099-01-01T00:00:00.000Z".into(),
        seq: 1,
    });
    let etag = r.etag();
    let (_, ev) = run(l, r.clone(), etag.as_deref()).await;
    assert!(
        r.publishes.lock().unwrap().is_empty(),
        "没开就不要发任何租约请求（省请求，也省一次误判）"
    );
    let m = Manifest::parse(&r.manifest_bytes()).expect("清单");
    assert!(m.window.entries.iter().any(|x| x.i == "n1"));
    assert!(!ev.iter().any(|e| matches!(e, SyncEvent::Deferred { .. })));
}

#[tokio::test]
async fn a_failed_manifest_commit_leaves_the_edit_pending_so_the_next_round_replays_it() {
    // §11.3 C4：记录已提交、清单未提交 → 清单重放。前提是本地**还没**把自己标成已同步，
    // 否则"重放"无从发生，别的设备就永远看不见这条改动。
    let (l, r) = (FakeLocal::default(), FakeRemote::default());
    r.seed(base_manifest());
    dirty_local(&l, "n1", 3, "aaaa");
    r.cas_failures.store(99, Ordering::SeqCst);
    let etag = r.etag();
    let (st, _) = run(l.clone(), r.clone(), etag.as_deref()).await;

    assert_eq!(st.pushed, 1, "记录该推还是推了");
    assert!(
        !l.applied
            .lock()
            .unwrap()
            .iter()
            .any(|o| matches!(o, ApplyOp::MarkSynced { .. })),
        "公告没成功就不许标 synced —— 这是这一条测试唯一真正要证的东西",
    );
    assert!(
        !l.applied
            .lock()
            .unwrap()
            .iter()
            .any(|o| matches!(o, ApplyOp::StoreManifest { .. })),
        "公告没成功也不该缓存一份新清单"
    );
    assert_eq!(st.outcome, RoundOutcome::Partial, "本轮得如实报『没做完』");
}
