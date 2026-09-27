//! notera-sync —— 同步引擎。
//!
//! 规范：docs/SYNC-PROTOCOL.md（本文所有分支都对应其小节）。
//!
//! ## 为什么用端口（trait）而不是直接依赖 store / webdav
//!
//! 同步引擎的正确性与"底层是 SQLite 还是别的"无关。把它对世界的全部依赖收敛成
//! 两个 trait，收益是决定性的：
//! * P1..P18 判定、清单 CAS、崩溃点恢复、退避节奏都能在**毫秒级单测**里跑满分支；
//! * 引擎不再可能在"网络等待中持有写锁"，因为 `LocalPort` 的方法全是本地操作；
//! * 集成时由 `notera-host` 把真实 store / webdav 适配进来，接口不匹配会在
//!   一处（适配层）暴露，而不是散落在引擎里。

pub mod manifest;
pub mod plan;

use manifest::{EntryRef, Manifest, ManifestError};
use plan::{Action, ConflictKind, Decision, LocalView, Plan, RemoteView};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration;

pub use manifest::PROTOCOL as SYNC_PROTOCOL_VERSION;

// ------------------------------------------------------------------ 端口 ---

/// 本地存储侧能力。**全部是本地操作，实现者不得在其中等待网络**（I8/P1）。
pub trait LocalPort: Send + Sync {
    fn account_id(&self) -> String;
    fn device_id(&self) -> String;
    fn now(&self) -> String;
    /// 本地视图（含 dirty 判定所需的 rev/sync_rev/hash）
    fn local_views(&self) -> Result<Vec<LocalView>, LocalError>;
    /// 已缓存的远端索引（清单的本地投影）
    fn cached_remote(&self) -> Result<Vec<RemoteView>, LocalError>;
    fn cached_segment_hashes(&self) -> BTreeMap<String, String>;
    /// 本机已经存下、且已经与远端确认过的实体头（`(kind,id)` → `(rev, 全哈希)`）。
    ///
    /// 默认空表 = "一条都不跳过"，也就是本方法出现之前的行为。实现方**必须**给出干净行：
    /// `local_views()` 只报脏行，缺了这一份视图，一台已经追平的设备仍会把清单里每条
    /// 都当成新增重下一遍，直到把每轮请求预算吃光，真正缺的条目再也轮不上。
    fn synced_heads(&self) -> BTreeMap<(String, String), (u64, String)> {
        BTreeMap::new()
    }
    fn seq_applied(&self) -> u64;
    /// 取某笔记在某 rev 的内容（三方合并的 base，DATA-MODEL §4.4 保证存在）
    fn revision_json(&self, id: &str, rev: u64) -> Result<Option<serde_json::Value>, LocalError>;
    /// 待上传实体的 wire 字节（信封 JSON）
    fn envelope_wire(&self, kind: &str, id: &str) -> Result<Option<Vec<u8>>, LocalError>;
    /// 应用远端结果：单事务
    fn apply(&self, ops: Vec<ApplyOp>) -> Result<ApplyReport, LocalError>;
    /// 记录冲突（保留双方）
    fn record_conflict(&self, d: &Decision, local: &LocalView, remote: &RemoteView) -> Result<(), LocalError>;
    fn outbox_take(&self, limit: usize) -> Result<Vec<OutboxItem>, LocalError>;
    /// 结清某个实体某 rev 的待办。参数刻意是 `kind/id/rev` 而不是某个"键"：
    /// 引擎手里只有这三样（`Decision.key` 就是 `(kind, id)`），以前把它当
    /// `dedupe_key` 传下去，实现方要么接不上、要么猜错行 —— 猜错就是把别人的
    /// 待办标成已完成。
    fn outbox_settle(&self, kind: &str, id: &str, rev: u64, st: OutboxState) -> Result<(), LocalError>;
    /// §11.4：把本轮贴上的租约记进本地状态（诊断用：出问题时能看出"当时我以为谁在写"）。
    /// 允许空实现 —— 它不丢数据，只少一条线索。
    fn record_lease(&self, _token: &str, _expires_at: &str) -> Result<(), LocalError> {
        Ok(())
    }
    /// 上一轮已应用并成功提交的清单字节。
    ///
    /// 304（远端未变）时引擎手里**没有**清单正文，但本地若有改动仍必须追加公告 ——
    /// 没有这个缓存，"改了却没人看得见"就是静默同步失败。
    fn cached_manifest(&self) -> Option<Vec<u8>>;
}

/// 远端能力。实现者（webdav 适配层）负责重试/代理/TLS，本 crate 只做**决策**。
#[async_trait::async_trait]
pub trait RemotePort: Send + Sync {
    /// GET manifest/index.json，带 If-None-Match。None = 304 未变。
    async fn fetch_manifest(&self, etag: Option<&str>) -> Result<Option<(Vec<u8>, Option<String>)>, RemoteError>;
    async fn fetch_segment(&self, name: &str) -> Result<Vec<EntryRef>, RemoteError>;
    async fn fetch_record(&self, kind: &str, id: &str) -> Result<Option<Vec<u8>>, RemoteError>;
    /// 写实体记录（内部按 cap_mask 选 S1/S2/S3 并回读校验）
    async fn put_record(&self, kind: &str, id: &str, wire: &[u8], if_match: Option<&str>) -> Result<Commit, RemoteError>;
    /// CAS 提交清单（先写 prev，再提交 index）
    async fn commit_manifest(&self, wire: &[u8], cas_etag: Option<&str>) -> Result<Option<String>, RemoteError>;
    async fn put_segment(&self, name: &str, wire: &[u8]) -> Result<(), RemoteError>;
    async fn probe_record_etag(&self, kind: &str, id: &str) -> Result<Option<String>, RemoteError>;
    /// §11.4：贴上/续上本机租约（尽力而为，失败只影响"别人看不看得见我"）。
    /// 过期时刻由引擎算：它才掌握"这一轮是什么时候"，也便于把同一个值记进本地状态。
    /// 刻意**不给默认实现**：默认 = 这一层静默空转，而"空转的并发保护"比没有更糟。
    async fn lease_publish(&self, token: &str, expires_at: &str, seq: u64) -> Result<(), RemoteError>;
    /// §11.4：读别人的租约。`known` 是清单 `generated_by` 学到的对手设备 id ——
    /// 服务器列目录能力坏掉时至少还能给它让路。读不出一律按"没人持有"处理。
    async fn lease_holders(&self, known: &[String]) -> Result<Vec<PeerLease>, RemoteError>;
}

/// 别人贴着的一份租约（适配器负责从 `locks/<device>.json` 翻译过来）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerLease {
    pub device: String,
    pub expires_at: String,
    pub seq: u64,
}

impl PeerLease {
    /// 还挡不挡路。时间读不出来按"过期"算 —— 挡不住事小，永久挡住同步是大。
    pub fn is_fresh(&self, now_ms: i64) -> bool {
        match notera_core::Timestamp::parse(&self.expires_at).and_then(|t| t.as_millis()) {
            Some(ms) => ms > now_ms,
            None => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub etag: Option<String>,
    pub verified: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApplyOp {
    Upsert { kind: String, id: String, wire: Vec<u8> },
    /// 冲突采纳：把远端那一版换成本机正文（本地那份已由 `record_conflict` 存成副本）。
    /// 只有引擎在判出 `UpdateUpdate` 并真的取回记录时才发这条（CONFLICT-RESOLUTION §6.1）。
    AdoptConflict { kind: String, id: String, wire: Vec<u8> },
    SetRemote { kind: String, id: String, rev: u64, hash12: String },
    MarkSynced { kind: String, id: String, rev: u64 },
    Delete { kind: String, id: String, rev: u64 },
    Purge { kind: String, id: String },
    Tombstone { kind: String, id: String, rev: u64, deleted_at: Option<String>, purged: bool },
    /// 提交成功后把清单正文与 etag 缓存下来，供下一轮 304 路径使用
    StoreManifest { wire: Vec<u8>, etag: Option<String>, seq: u64 },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ApplyReport {
    pub applied: usize,
    pub rejected: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutboxItem {
    pub dedupe_key: String,
    pub kind: String,
    pub id: String,
    pub rev: u64,
    pub op: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutboxState {
    Pending,
    Inflight,
    Done,
    Failed,
    Superseded,
    Blocked,
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum LocalError {
    #[error("本地存储失败: {0}")]
    Storage(String),
    #[error("库版本过新，已只读")]
    ReadOnly,
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum RemoteError {
    #[error("离线")]
    Offline,
    #[error("需要凭据")]
    Auth,
    #[error("无权限")]
    Forbidden,
    #[error("空间不足")]
    Quota,
    #[error("服务器不可用")]
    Server,
    #[error("前置条件失败（并发），需重算计划")]
    Precondition,
    #[error("记录不存在")]
    NotFound,
    #[error("协议/校验失败: {0}")]
    Protocol(String),
    #[error("已取消")]
    Cancelled,
}

impl RemoteError {
    /// 该错误是否值得本轮之后尽快重试（SYNC-PROTOCOL §12）。
    pub fn retryable(&self) -> bool {
        matches!(self, RemoteError::Offline | RemoteError::Server | RemoteError::Quota | RemoteError::Precondition)
    }
    /// 是否应停止继续本轮（避免对着坏链路刷请求）。
    pub fn halts_round(&self) -> bool {
        matches!(self, RemoteError::Auth | RemoteError::Forbidden | RemoteError::Cancelled | RemoteError::Protocol(_))
    }
}

// ---------------------------------------------------------------- 轮次 ---

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    Unconfigured,
    Provisioning,
    BootstrapPull,
    BootstrapPush,
    Online,
    ReadOnly,
    NeedsCredentials,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoundStats {
    pub requests: u32,
    pub bytes_up: u64,
    pub bytes_down: u64,
    pub pushed: usize,
    pub pulled: usize,
    pub conflicts: usize,
    pub cas_retries: u8,
    pub outcome: RoundOutcome,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RoundOutcome {
    NoOp,
    Converged,
    Partial,
    Failed,
}

/// 引擎对外的唯一事件面。host 把它折叠成 UI 的四态徽标 —— **协议细节不出本 crate**。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SyncEvent {
    Phase(Phase),
    Progress { done: u32, total: u32 },
    NeedsConflictAttention,
    /// §11.4：本轮让路给另一台设备（改动仍是 dirty，下一轮会再试）。
    /// 单独一个事件而不是"失败"：让路是正常协作，报成失败会吓到人，
    /// 静默不说又等于"看着同步其实没公告"。
    Deferred { device: String },
    Completed(RoundOutcome),
    Failed { retryable: bool, message_key: &'static str },
}

#[derive(Clone, Debug)]
pub struct EngineConfig {
    pub max_cas_retries: u8,
    pub round_request_cap: u32,
    pub pull_concurrency: usize,
    pub bootstrap_batch: usize,
    /// §11.4 的开关。由 host 按 §5 的探测结果决定：**只在保护缺位时开**
    /// （写入策略 S3，或探不到强 ETag ⇒ 清单 CAS 不可信）。
    /// 有 S1/S2 且强 ETag 时服务器自己就拦并发写，开租约只是每轮多两个请求。
    pub lease: LeasePolicy,
}

/// 租约策略。默认关：它只在保护缺位时才有意义。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LeasePolicy {
    #[default]
    Off,
    On { ttl_ms: i64 },
}


impl Default for EngineConfig {
    fn default() -> Self {
        Self { max_cas_retries: 3, round_request_cap: 200, pull_concurrency: 8, bootstrap_batch: 500, lease: LeasePolicy::Off }
    }
}

pub struct SyncEngine<L: LocalPort, R: RemotePort> {
    local: L,
    remote: R,
    cfg: EngineConfig,
}

impl<L: LocalPort, R: RemotePort> SyncEngine<L, R> {
    pub fn new(local: L, remote: R, cfg: EngineConfig) -> Self {
        Self { local, remote, cfg }
    }
    pub fn local(&self) -> &L {
        &self.local
    }
    pub fn remote(&self) -> &R {
        &self.remote
    }

    /// 一轮同步。空轮必须是 1 请求 0 字节正文（SYNC-PROTOCOL §6.3）。
    pub async fn run_round(&self, etag: Option<&str>) -> (RoundStats, Vec<SyncEvent>) {
        let mut st = RoundStats {
            requests: 0,
            bytes_up: 0,
            bytes_down: 0,
            pushed: 0,
            pulled: 0,
            conflicts: 0,
            cas_retries: 0,
            outcome: RoundOutcome::NoOp,
        };
        let mut events = Vec::new();

        // ⓪ §6.1 的 `acquire_lease?`：先把自己贴上去，再开始读。
        // 贴不上不拦本轮 —— 那只是"别人看不见我"，正确性仍由 §11.2 前两层兜。
        if let LeasePolicy::On { ttl_ms } = self.cfg.lease {
            let now_ms = self.now_ms();
            let expires_at = notera_core::Timestamp::from_millis(now_ms + ttl_ms).to_string();
            let token = self.lease_token();
            if self.remote.lease_publish(&token, &expires_at, self.local.seq_applied()).await.is_ok() {
                let _ = self.local.record_lease(&token, &expires_at);
            }
        }

        // ① 读清单
        // 本轮是否被请求预算**截断**过：截断意味着还有活没干完（下载的半路上），
        // 与"条目 404 / 单个请求出错"要分开记 —— 见下面 seq 落账那道的判据。
        let mut capped = false;
        // 本轮提交出去的清单序号（纯拉的一侧没有）。收尾记 `seq_applied` 时它优先于读到的
        // 那个 seq —— 本机自己刚公告了一版，落账要落在新的那一版上。
        let mut committed_seq: Option<u64> = None;
        let (manifest, mut new_etag) = match self.remote.fetch_manifest(etag).await {
            Ok(None) => {
                // 304：远端未变。但**"远端未变"不等于"本机已追平"** —— 上一轮可能被
                // 请求预算截断在下载的中途，此时账上（脏）是干净的，界还会说"已同步",
                // 而库里其实少着一批实体。所以这里要用上一轮存下来的清单正文接着算。
                let cached = self.local.cached_manifest().and_then(|b| Manifest::parse(&b).ok());
                let backlog = cached.as_ref().is_some_and(|m| self.local.seq_applied() < m.seq);
                let dirty = self.local.local_views().map(|v| v.iter().any(|l| l.dirty())).unwrap_or(false);
                if !dirty && !backlog {
                    st.requests += 1;
                    return (st, vec![SyncEvent::Completed(RoundOutcome::NoOp)]);
                }
                st.requests += 1;
                (cached, etag.map(str::to_string))
            }
            Ok(Some((bytes, e))) => {
                st.requests += 1;
                st.bytes_down += bytes.len() as u64;
                match Manifest::parse(&bytes) {
                    Ok(m) => (Some(m), e),
                    // D1/D2：清单不可信 → 绝不"以空清单继续"（那等于清空用户库）
                    Err(ManifestError::Protocol { .. }) => {
                        return (
                            RoundStats { outcome: RoundOutcome::Failed, ..st },
                            vec![SyncEvent::Failed { retryable: false, message_key: "sync.protocol_mismatch" }],
                        );
                    }
                    Err(_) => {
                        return (
                            RoundStats { outcome: RoundOutcome::Failed, ..st },
                            vec![SyncEvent::Failed { retryable: true, message_key: "sync.corrupt_record" }],
                        );
                    }
                }
            }
            Err(e) => return (RoundStats { outcome: RoundOutcome::Failed, ..st }, vec![self.fail_event(&e)]),
        };

        // ② 生成计划
        let locals = match self.local.local_views() {
            Ok(v) => v,
            Err(_) => return (RoundStats { outcome: RoundOutcome::Failed, ..st }, vec![]),
        };
        let remotes: Vec<RemoteView> = match &manifest {
            Some(m) => {
                let mut out = self.local.cached_remote().unwrap_or_default();
                // 落后超过窗口 → 拉变化分段（SYNC-PROTOCOL §6.2 第三档）
                let window_covers = m.window.complete
                    && self.local.seq_applied() >= m.window.since_seq;
                if !window_covers {
                    for name in m.segments_needed_for(&self.local.cached_segment_hashes()) {
                        if st.requests >= self.cfg.round_request_cap {
                            capped = true;
                            break;
                        }
                        match self.remote.fetch_segment(&name).await {
                            Ok(entries) => {
                                st.requests += 1;
                                out.retain(|r| !entries.iter().any(|e| e.key() == (r.kind.clone(), r.id.clone())));
                                for e in entries {
                                    out.push(RemoteView {
                                        kind: e.t,
                                        id: e.i,
                                        rev: e.r,
                                        hash: Some(e.h),
                                        deleted_at: e.d,
                                        purged: e.p != 0,
                                    });
                                }
                            }
                            Err(_) => st.outcome = RoundOutcome::Partial,
                        }
                    }
                }
                if window_covers || manifest.is_some() {
                    for e in &m.window.entries {
                        out.retain(|r| r.key() != e.key());
                        out.push(RemoteView {
                            kind: e.t.clone(),
                            id: e.i.clone(),
                            rev: e.r,
                            hash: Some(e.h.clone()),
                            deleted_at: e.d.clone(),
                            purged: e.p != 0,
                        });
                    }
                }
                out
            }
            None => self.local.cached_remote().unwrap_or_default(),
        };
        let plan = Plan::build(&locals, &remotes);

        // P7「内容已经一样」不等于"本轮无事可做"就完事：本地这条可能**还挂着脏**
        // —— 上一轮公告已经落到服务器、却还没来得及把本地结清（进程正好崩在那里）。
        // 此时远端 rev 与内容哈希都对得上，就把这一条按已同步收尾；不收的话它永远
        // 不会被任何一轮再处理，设置页的"待同步"计数就此永久不掉（§18 要求诚实）。
        for l in &locals {
            if !l.dirty() || l.deleted_at.is_some() || l.purged_at.is_some() {
                continue;
            }
            let Some(r) = remotes.iter().find(|r| r.kind == l.kind && r.id == l.id) else { continue };
            if r.purged || r.deleted_at.is_some() || r.rev != l.rev {
                continue;
            }
            if !r.hash.as_deref().is_some_and(|h| notera_core::same_content_hash(h, &l.content_hash)) {
                continue;
            }
            let _ = self.local.apply(vec![ApplyOp::MarkSynced {
                kind: l.kind.clone(),
                id: l.id.clone(),
                rev: l.rev,
            }]);
            let _ = self.local.outbox_settle(&l.kind, &l.id, l.rev, OutboxState::Done);
        }

        // ③ push 本地变更（先实体，后清单 —— R2）
        notera_core::crash_point("before_records_push");
        let mut written: Vec<EntryRef> = Vec::new();
        // 推成功了但**公告还没提交**的一批：等清单 CAS 成功再落 `MarkSynced`。
        // 早于公告就标成已同步 = 清单提交一失败（网络抖一下、CAS 让路）这批改动
        // 再也不会被重新公告，别的设备永远看不见（§11.3 C4 要的正是"清单重放"）。
        let mut await_announce: Vec<(String, String, u64)> = Vec::new();
        let lmap: BTreeMap<(String, String), &LocalView> =
            locals.iter().map(|l| (l.key(), l)).collect();
        for d in plan.pushes() {
            if st.requests >= self.cfg.round_request_cap {
                capped = true;
                st.outcome = RoundOutcome::Partial;
                break;
            }
            let Some(l) = lmap.get(&d.key) else { continue };
            let wire = match self.local.envelope_wire(&l.kind, &l.id) {
                Ok(Some(w)) => w,
                Ok(None) => continue,
                Err(_) => continue,
            };
            let known_etag = self.remote.probe_record_etag(&l.kind, &l.id).await.ok().flatten();
            st.requests += 1;
            match self.remote.put_record(&l.kind, &l.id, &wire, known_etag.as_deref()).await {
                Ok(c) if c.verified => {
                    st.requests += 1;
                    st.bytes_up += wire.len() as u64;
                    st.pushed += 1;
                    written.push(plan::entry_of(l, wire.len() as u64));
                    await_announce.push((l.kind.clone(), l.id.clone(), l.rev));
                }
                Ok(_) => {
                    // 写未通过复验：不得记为已提交（webdav 层契约）
                    let _ = self.local.outbox_settle(&l.kind, &l.id, l.rev, OutboxState::Failed);
                }
                Err(RemoteError::Precondition) => {
                    // 有人先写了：本轮该实体让路，重算在下轮
                    let _ = self.local.outbox_settle(&l.kind, &l.id, l.rev, OutboxState::Pending);
                    st.outcome = RoundOutcome::Partial;
                }
                Err(e) => {
                    let _ = self.local.outbox_settle(&l.kind, &l.id, l.rev, OutboxState::Failed);
                    if e.halts_round() {
                        return (RoundStats { outcome: RoundOutcome::Failed, ..st }, vec![self.fail_event(&e)]);
                    }
                }
            }
            events.push(SyncEvent::Progress { done: st.pushed as u32, total: plan.pushes().len() as u32 });
        }

        // ④ pull 远端变更
        notera_core::crash_point("after_records_push");
        // 已经有了的那一版，就别再花请求去要了（见 `LocalPort::synced_heads`）。
        let held = self.local.synced_heads();
        let rmap: BTreeMap<(String, String), &RemoteView> = remotes.iter().map(|r| (r.key(), r)).collect();
        notera_core::crash_point("before_apply");
        for d in plan.pulls() {
            if st.requests >= self.cfg.round_request_cap {
                capped = true;
                st.outcome = RoundOutcome::Partial;
                break;
            }
            let (kind, id) = d.key.clone();
            if let (Some((rev, hash)), Some(r)) = (held.get(&d.key), rmap.get(&d.key)) {
                let same_rev = *rev == r.rev;
                let same_hash = r.hash.as_deref().is_some_and(|h| notera_core::same_content_hash(h, hash));
                if same_rev && same_hash {
                    continue;
                }
            }
            match self.remote.fetch_record(&kind, &id).await {
                Ok(Some(wire)) => {
                    st.requests += 1;
                    st.bytes_down += wire.len() as u64;
                    let rep = self.local.apply(vec![ApplyOp::Upsert { kind, id, wire }]).unwrap_or_default();
                    if rep.applied == 1 {
                        st.pulled += 1;
                    } else {
                        st.conflicts += 0; // 校验失败已在 store 侧丢弃（I6）
                    }
                }
                Ok(None) => {
                    // 清单说有、记录 404 → §10：记 missing，**不删本地**
                    st.outcome = RoundOutcome::Partial;
                }
                Err(e) if e.halts_round() => {
                    return (RoundStats { outcome: RoundOutcome::Failed, ..st }, vec![self.fail_event(&e)])
                }
                Err(_) => st.outcome = RoundOutcome::Partial,
            }
        }

        // ⑤ 墓碑与删除应用（不产生新写入的分支）
        let mut tombstone_ops = Vec::new();
        for d in &plan.decisions {
            match &d.action {
                Action::ApplyRemoteDelete | Action::ApplyRemotePurge => {
                    let purged = matches!(d.action, Action::ApplyRemotePurge);
                    let rev = remotes.iter().find(|r| r.key() == d.key).map(|r| r.rev).unwrap_or(0);
                    let deleted_at = remotes.iter().find(|r| r.key() == d.key).and_then(|r| r.deleted_at.clone());
                    tombstone_ops.push(ApplyOp::Tombstone {
                        kind: d.key.0.clone(),
                        id: d.key.1.clone(),
                        rev,
                        deleted_at,
                        purged,
                    });
                }
                Action::Conflict(ck) => {
                    if let (Some(l), Some(r)) = (lmap.get(&d.key), remotes.iter().find(|x| x.key() == d.key)) {
                        let _ = self.local.record_conflict(d, l, r);
                        st.conflicts += 1;
                        // §6.1：正文必须是**已被别的设备确认的那一份**，本机那份刚才已经进了副本。
                        // 所以这里必须把远端记录真的取回来 —— 只登记两个哈希的冲突卡片有三个后果：
                        //   1. 面板左右两栏显示的是同一份本机内容（"服务器那一版"根本没下来）；
                        //   2. 对面那台的编辑在本机不存在，用户按"用服务器那一版"替换是个空操作；
                        //   3. 最坏的一条：本机脏 head 还在，下一轮它带着更高的 rev 推上去，
                        //      把别人已确认的那一份**静默盖掉**。
                        // 只有 UpdateUpdate 走这条路：删除 vs 修改（P11）该保留哪一边由用户决定，
                        // 引擎不许替他选。取不到就照旧留卡片，下一轮再试。
                        if matches!(ck, ConflictKind::UpdateUpdate) && l.kind == "n" && st.requests < self.cfg.round_request_cap {
                            if let Ok(Some(wire)) = self.remote.fetch_record(&l.kind, &l.id).await {
                                st.requests += 1;
                                st.bytes_down += wire.len() as u64;
                                let rep = self
                                    .local
                                    .apply(vec![ApplyOp::AdoptConflict { kind: l.kind.clone(), id: l.id.clone(), wire }])
                                    .unwrap_or_default();
                                if rep.applied == 1 {
                                    st.pulled += 1;
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        if !tombstone_ops.is_empty() {
            let _ = self.local.apply(tombstone_ops);
        }

        // ⑥ CAS 提交清单（本轮最后一步 —— R2）
        notera_core::crash_point("after_apply");
        // 304 轮次手里没有清单正文：回落到上一轮缓存，否则本地变更将永远无法公告。
        let mut manifest = manifest;
        if manifest.is_none() && !written.is_empty() {
            manifest = self.local.cached_manifest().and_then(|b| Manifest::parse(&b).ok());
            if manifest.is_none() {
                // 缓存也没有（首次或缓存丢失）：以本轮变更新建一份，宁可多一次全量公告，
                // 也不能丢掉变更。seq 从 1 起由服务器 CAS 仲裁。
                manifest = Some(Manifest::initial("", &self.local.device_id(), &self.local.now(), "notera"));
            }
        }
        // 本轮是否要让路（让路只跳过"提交清单"这一步，其余收尾照常）
        let mut yielded: Option<String> = None;
        if !written.is_empty() {
            // §11.2 第三层：**写清单前**问一次有没有人正占着。让路只是延后一轮 ——
            // 记录已经推上去了但清单没公告 = 别人暂时看不见，也无害（清单是唯一入口）。
            if let LeasePolicy::On { .. } = self.cfg.lease {
                let known: Vec<String> = manifest.as_ref().map(|m| vec![m.generated_by.clone()]).unwrap_or_default();
                let now_ms = self.now_ms();
                let me = self.local.device_id();
                match self.remote.lease_holders(&known).await {
                    Ok(peers) => {
                        st.requests += 1;
                        yielded = peers.into_iter().find(|p| p.device != me && p.is_fresh(now_ms)).map(|p| p.device);
                    }
                    // 读不到别人的租约 = 当作没人持有（§11.4：这一层坏了不能变成永不同步）
                    Err(_) => {
                        st.requests += 1;
                    }
                }
            }
        }
        if let Some(device) = yielded {
            st.outcome = RoundOutcome::Partial;
            events.push(SyncEvent::Deferred { device });
        }
        if !written.is_empty() && events.iter().all(|e| !matches!(e, SyncEvent::Deferred { .. })) {
            if let Some(m) = manifest.as_ref() {
                let mut next = m.with_commit(&self.local.device_id(), &self.local.now(), &written, &[]);
                let mut tries = 0u8;
                loop {
                    st.requests += 1;
                    st.bytes_up += next.to_wire().len() as u64;
                    notera_core::crash_point("before_manifest_commit");
                    match self.remote.commit_manifest(&next.to_wire(), new_etag.as_deref()).await {
                        Ok(e) => {
                            // 公告已在服务器上、本地却还没结清 —— 这是最要命的一刀：
                            // 崩在这里之后下一轮必须既不再重复公告、也不把改动弄丢。
                            notera_core::crash_point("after_manifest_commit");
                            // 公告成功了，这批才算"已同步"；outbox 也在此刻结清。
                            for (kind, id, rev) in &await_announce {
                                let _ = self.local.apply(vec![ApplyOp::MarkSynced {
                                    kind: kind.clone(),
                                    id: id.clone(),
                                    rev: *rev,
                                }]);
                                let _ = self.local.outbox_settle(kind, id, *rev, OutboxState::Done);
                            }
                            let _ = self.local.apply(vec![
                                ApplyOp::StoreManifest { wire: next.to_wire(), etag: e.clone(), seq: next.seq },
                                ApplyOp::SetRemote {
                                    kind: "manifest".into(),
                                    id: "index".into(),
                                    rev: next.seq,
                                    hash12: e.unwrap_or_default(),
                                },
                            ]);
                            committed_seq = Some(next.seq);
                            st.cas_retries = tries;
                            break;
                        }
                        Err(RemoteError::Precondition) if tries < self.cfg.max_cas_retries => {
                            tries += 1;
                            st.cas_retries = tries;
                            // 重读 → 重新合并 → 重试（清单是公告板，不丢数据）
                            match self.remote.fetch_manifest(None).await {
                                Ok(Some((bytes, e2))) => match Manifest::parse(&bytes) {
                                    Ok(fresh) => {
                                        next = fresh.with_commit(&self.local.device_id(), &self.local.now(), &written, &[]);
                                        new_etag = e2;
                                    }
                                    Err(_) => break,
                                },
                                _ => break,
                            }
                        }
                        Err(_) => {
                            st.outcome = RoundOutcome::Partial;
                            break;
                        }
                    }
                }
            }
        }

        // 只在**本轮没有被请求预算截断**时才把 seq 记成"已应用"。
        //
        // 早先这里是无条件写的，于是 >窗口 的库会卡死：本轮被 `round_request_cap` 截断
        // （下载停在半路）→ seq 却已落账 → 下一轮读清单拿到 304 就直接判"无事可做"，
        // 那些还没落到本机的实体**永远没人管**，而账上干干净净、界面说"已同步"。
        // 实测：260 条笔记的库，第二台设备停在 196 条。见 `notera-host/tests/late_device.rs`。
        // 注意判据是 `capped` 而不是 `outcome != Partial`：条目 404 也会给 Partial，
        // 那种"远端说有、记录却没有"的实体不该让整轮永远追不平（§10 走 missing 修复）。
        if let Some(seq) = committed_seq.or_else(|| manifest.as_ref().map(|m| m.seq)) {
            if !capped {
                let _ = self.local.apply(vec![ApplyOp::SetRemote {
                    kind: "seq".into(),
                    id: "applied".into(),
                    rev: seq,
                    hash12: String::new(),
                }]);
            }
        }

        if st.outcome == RoundOutcome::NoOp {
            st.outcome = if st.pushed + st.pulled > 0 { RoundOutcome::Converged } else { RoundOutcome::NoOp };
        }
        if st.conflicts > 0 {
            events.push(SyncEvent::NeedsConflictAttention);
        }
        events.push(SyncEvent::Completed(st.outcome));
        (st, events)
    }

    /// 本地时钟的毫秒刻度。读不出来就按 0 —— 方向是"别人的租约都算过期"，
    /// 也就是不让路，而不是永久挡住自己。
    fn now_ms(&self) -> i64 {
        notera_core::Timestamp::parse(&self.local.now()).and_then(|t| t.as_millis()).unwrap_or_default()
    }

    /// 一轮一份。只用来区分同一台设备的两次运行，不参与任何判定。
    /// 借 `EntityId` 生成而不是给本 crate 加 uuid 依赖（它的依赖面是刻意最小的）。
    fn lease_token(&self) -> String {
        notera_core::EntityId::new().to_string()
    }

    fn fail_event(&self, e: &RemoteError) -> SyncEvent {
        SyncEvent::Failed {
            retryable: e.retryable(),
            message_key: match e {
                RemoteError::Offline => "sync.offline",
                RemoteError::Auth => "sync.auth_required",
                RemoteError::Forbidden => "sync.forbidden",
                RemoteError::Quota => "sync.quota_full",
                RemoteError::Server => "sync.server_unavailable",
                RemoteError::Precondition => "sync.precondition",
                RemoteError::NotFound => "sync.failed",
                RemoteError::Protocol(_) => "sync.corrupt_record",
                RemoteError::Cancelled => "sync.cancelled",
            },
        }
    }
}

// ---------------------------------------------------------------- 退避 ---

/// `delay = min(max, base × factor^n) × (1 ± jitter)`，实测节奏见 PROXY.md §7。
#[derive(Clone, Debug)]
pub struct Backoff {
    pub base: Duration,
    pub factor: f64,
    pub max: Duration,
    pub jitter: f64,
}

impl Default for Backoff {
    fn default() -> Self {
        Self { base: Duration::from_secs(2), factor: 1.85, max: Duration::from_secs(900), jitter: 0.2 }
    }
}

impl Backoff {
    /// `rand01` 由调用方提供（测试注入固定值即可断言精确区间）。
    pub fn delay_for(&self, attempt: u32, rand01: f64) -> Duration {
        let raw = self.base.as_secs_f64() * self.factor.powi(attempt.min(40) as i32);
        let capped = raw.min(self.max.as_secs_f64());
        let signed = (rand01.clamp(0.0, 1.0) * 2.0 - 1.0) * self.jitter;
        Duration::from_secs_f64((capped * (1.0 + signed)).max(0.05))
    }

    /// 尊重服务器 `Retry-After`（取较大者，避免比服务器要求更激进）。
    pub fn with_retry_after(&self, attempt: u32, rand01: f64, retry_after: Option<Duration>) -> Duration {
        let d = self.delay_for(attempt, rand01);
        match retry_after {
            Some(ra) => d.max(ra),
            None => d,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_monotonically_and_caps() {
        let b = Backoff::default();
        let mid = |n: u32| b.delay_for(n, 0.5); // rand01=0.5 → 无偏移
        assert_eq!(mid(0), Duration::from_secs(2));
        assert_eq!(mid(1), Duration::from_secs_f64(2.0 * 1.85));
        assert!(mid(2) > mid(1) && mid(3) > mid(2), "必须单调递增");
        assert_eq!(mid(60), Duration::from_secs(900), "必须封顶 15min");
    }

    #[test]
    fn backoff_jitter_stays_in_band() {
        let b = Backoff::default();
        let lo = b.delay_for(3, 0.0);
        let hi = b.delay_for(3, 1.0);
        let mid = b.delay_for(3, 0.5);
        assert!(lo < mid && mid < hi);
        assert!((hi.as_secs_f64() / lo.as_secs_f64()) < 1.6, "±20% jitter 区间不应过宽");
    }

    #[test]
    fn retry_after_is_never_more_aggressive_than_server_asks() {
        let b = Backoff::default();
        let d = b.with_retry_after(0, 0.5, Some(Duration::from_secs(30)));
        assert_eq!(d, Duration::from_secs(30));
        let d2 = b.with_retry_after(6, 0.5, Some(Duration::from_secs(1)));
        assert!(d2 > Duration::from_secs(1), "退避已大于 Retry-After 时不得缩短");
    }

    #[test]
    fn error_classification_matches_protocol() {
        assert!(RemoteError::Server.retryable());
        assert!(RemoteError::Quota.retryable());
        assert!(!RemoteError::Auth.retryable());
        assert!(RemoteError::Auth.halts_round(), "认证失败必须停轮，不能刷请求");
        assert!(RemoteError::Protocol("x".into()).halts_round());
        assert!(!RemoteError::Offline.halts_round(), "离线可继续下一轮，不是终止态");
    }

    #[test]
    fn phase_enum_is_serializable_for_diagnostics() {
        let p = Phase::NeedsCredentials;
        let j = serde_json::to_string(&p).unwrap();
        assert_eq!(serde_json::from_str::<Phase>(&j).unwrap(), p);
    }
}
