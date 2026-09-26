//! 生产 `RemotePort` 适配器：WebDAV over `notera-net`（docs/SYNC-PROTOCOL.md）。
//!
//! 本模块只做三件事：拼路径（见 [`crate::path`]）、按 `cap_mask` 选写入策略（§5）、
//! 把 HTTP 形态换算成端口词汇（§12）。**所有字节都经 [`HttpClient`] 出门**：这里没有
//! 底层客户端，也没有第二个出口（PROXY.md §1 铁律）。
//!
//! 关于 gzip：§4.2 的 gzip 数字是**线上传输尺寸**的实测（`Accept-Encoding` 与解码由
//! `notera-net` 承担），不是"服务器上的存储编码"。规范从未要求落盘压缩，因此本适配器
//! **按明文写出** `wire` 原文 —— 于是复验可以逐字节比对，不必跟"某台服务器到底压不压"
//! 对赌（§1 布局里 `records/note/<id>.json` 就是信封本体）。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use notera_core::DeviceId;
use notera_net::{HttpClient, HttpMethod, NetError, RequestSpec, Response, RetryPolicy};
use notera_sync::manifest::{EntryRef, Manifest};
use notera_sync::{Commit, RemoteError, RemotePort};
use serde_json::Value;

use crate::auth::Credentials;
use crate::caps::{Caps, WriteStrategy};
use crate::error::{map_net, map_status, WebDavError};
use crate::path::{assert_url_sane, kind_dir, kind_matches, RecordPath, RemotePath, DEFAULT_ROOT_PREFIX};
use crate::record::WireMeta;

/// 单条记录的体积上限：把 §6.3 的"单轮硬上限 8 MiB"落到单对象粒度。
/// 超了就是调用方把附件当记录写了 —— 附件走 §13 的独立队列，不经过本 trait。
pub const MAX_RECORD_BYTES: usize = 8 * 1024 * 1024;

/// 构造参数（§1 的 base_url + root_prefix，加设备身份与能力位）。
#[derive(Clone, Debug)]
pub struct WebDavConfig {
    pub base_url: String,
    pub root_prefix: String,
    pub credentials: Option<Credentials>,
    pub device: DeviceId,
    pub caps: Caps,
}

impl WebDavConfig {
    /// 默认根前缀 `/.notes`、默认能力位、无凭据。
    pub fn new(base_url: impl Into<String>) -> WebDavConfig {
        WebDavConfig {
            base_url: base_url.into(),
            root_prefix: DEFAULT_ROOT_PREFIX.to_string(),
            credentials: None,
            device: DeviceId::new(),
            caps: Caps::conventional(),
        }
    }

    pub fn with_root_prefix(mut self, p: impl Into<String>) -> Self {
        self.root_prefix = p.into();
        self
    }
    pub fn with_base_url(mut self, u: impl Into<String>) -> Self {
        self.base_url = u.into();
        self
    }
    pub fn with_credentials(mut self, c: Credentials) -> Self {
        self.credentials = Some(c);
        self
    }
    pub fn with_device(mut self, d: DeviceId) -> Self {
        self.device = d;
        self
    }
    pub fn with_caps(mut self, c: Caps) -> Self {
        self.caps = c;
        self
    }
}

/// 真实远端。一次构造、多轮复用：内部只有不可变配置与一个原子计数器，故 `Send + Sync`。
pub struct WebDavRemote {
    http: Arc<HttpClient>,
    paths: RemotePath,
    credentials: Option<Credentials>,
    device: String,
    caps: Caps,
    nonce: AtomicU64,
    retry: RetryPolicy,
}

impl WebDavRemote {
    /// 校验配置并建好适配器。路径/URL 非法在这里就拒，不留到第一轮。
    pub fn new(cfg: WebDavConfig, http: Arc<HttpClient>) -> Result<WebDavRemote, WebDavError> {
        let paths = RemotePath::new(&cfg.base_url, &cfg.root_prefix)?;
        notera_core::assert_id_valid(&cfg.device.0).map_err(|e| WebDavError::Config(e.detail))?;
        let device = cfg.device.0.as_str().to_string();
        Ok(WebDavRemote {
            http,
            paths,
            credentials: cfg.credentials,
            device,
            caps: cfg.caps,
            nonce: AtomicU64::new(1),
            retry: RetryPolicy::deterministic(40, 1),
        })
    }

    /// 生效中的能力位（诊断面板用）。
    pub fn caps(&self) -> Caps {
        self.caps
    }

    /// 路径拼法（只读，供 host 做 `/_fs` 之外的探测与测试断言）。
    pub fn paths(&self) -> &RemotePath {
        &self.paths
    }

    /// 本设备的远端身份（tmp 对象名前缀）。
    pub fn device_id(&self) -> &str {
        &self.device
    }

    /// 覆盖读/幂等写的重试预算。
    #[must_use]
    pub fn with_retry(mut self, p: RetryPolicy) -> Self {
        self.retry = p;
        self
    }

    fn next_nonce(&self) -> u64 {
        self.nonce.fetch_add(1, Ordering::Relaxed)
    }

    // ------------------------------------------------------ 请求构造 ---

    fn spec(&self, method: HttpMethod, url: &str) -> Result<RequestSpec, RemoteError> {
        assert_url_sane(url).map_err(RemoteError::from)?;
        let mut s = RequestSpec::new(method, url);
        if let Some(c) = &self.credentials {
            s = s.with_header("authorization", c.basic_header());
        }
        Ok(s)
    }

    async fn send_once(&self, s: RequestSpec) -> Result<Response, RemoteError> {
        self.http.send(s).await.map_err(map_net)
    }

    async fn send_retry(&self, s: RequestSpec) -> Result<Response, RemoteError> {
        self.http.send_with_retry(s, &self.retry).await.map_err(map_net)
    }

    async fn get_raw(&self, url: &str, inm: Option<&str>) -> Result<Response, RemoteError> {
        let mut s = self.spec(HttpMethod::Get, url)?;
        if let Some(e) = inm {
            s = s.with_if_none_match(e);
        }
        self.send_retry(s).await
    }

    async fn head_raw(&self, url: &str) -> Result<Response, RemoteError> {
        let s = self.spec(HttpMethod::Head, url)?;
        self.send_retry(s).await
    }

    /// 无条件 `PUT`（用于我们自己独占命名的暂存对象）。
    async fn put_plain(&self, url: &str, body: &[u8]) -> Result<Response, RemoteError> {
        let s = self
            .spec(HttpMethod::Put, url)?
            .with_header("content-type", "application/json")
            .with_body(body.to_vec());
        self.send_retry(s).await
    }

    /// 带条件形态的 `PUT`。条件写**不**交给重试层自动重放：412 是"要重算计划"，不是"再试一次"。
    async fn put_cond(&self, url: &str, body: &[u8], cond: Cond) -> Result<Response, RemoteError> {
        let s = self
            .spec(HttpMethod::Put, url)?
            .with_header("content-type", "application/json")
            .with_body(body.to_vec());
        let retryable = matches!(cond, Cond::None);
        let s = match cond {
            Cond::None => s,
            Cond::IfMatch(e) => s.with_if_match(e),
            Cond::CreateOnly => s.with_if_none_match("*"),
        };
        if retryable {
            self.send_retry(s).await
        } else {
            self.send_once(s).await
        }
    }

    async fn move_raw(&self, from_url: &str, dest_url: &str, overwrite: bool, if_match_src: Option<&str>) -> Result<Response, RemoteError> {
        // Destination 越出根 = 把库里的东西搬到库外，或把库外的东西搬进来。绝不发出。
        if !self.paths.is_in_root(dest_url) || !self.paths.is_in_root(from_url) {
            return Err(RemoteError::Protocol(format!("MOVE 端点越出远端根: {dest_url}")));
        }
        let mut s = self
            .spec(HttpMethod::Move, from_url)?
            .with_header("destination", dest_url)
            .with_header("overwrite", if overwrite { "T" } else { "F" });
        // MOVE 的条件头按 RFC 作用于**源**，因此只有"源就是我们要 CAS 的那个对象"时才附带。
        if let Some(e) = if_match_src {
            s = s.with_if_match(e);
        }
        self.send_once(s).await
    }

    async fn delete_raw(&self, url: &str) -> Result<Response, RemoteError> {
        let s = self.spec(HttpMethod::Delete, url)?;
        self.send_once(s).await
    }

    /// 清理我们自己写的暂存对象。404 是正常结局，任何失败都不影响正确性（§11.3 C2）。
    async fn best_effort_delete(&self, url: &str) {
        match self.delete_raw(url).await {
            Ok(_) => {}
            Err(e) => tracing::debug!(%e, tmp = url, "暂存对象清理失败，交给维护轮"),
        }
    }

    // ------------------------------------------------------ 读对象 ---

    pub(crate) async fn read_object(&self, url: &str, inm: Option<&str>) -> Result<Object, RemoteError> {
        let r = self.get_raw(url, inm).await?;
        Ok(Object::of(&r))
    }

    /// HEAD 取强 etag；404 → `None`（与 §10 的"远端确实没有"同形）。
    pub(crate) async fn etag_of(&self, url: &str) -> Result<Option<String>, RemoteError> {
        let r = self.head_raw(url).await?;
        match r.status {
            404 => Ok(None),
            200..=299 => Ok(etag_of(&r)),
            s => Err(map_status(s)),
        }
    }

    // -------------------------------------------------- 原子写与降级 ---

    /// 按 `cap_mask` 排出的尝试序列（§5：S1 → S2 → S3）。
    pub(crate) fn attempts(&self) -> Vec<WriteStrategy> {
        let mut v = Vec::with_capacity(3);
        if self.caps.has(Caps::CONDITIONAL_PUT) {
            v.push(WriteStrategy::S1);
        }
        if self.caps.has(Caps::OVERWRITE_F_MOVE) {
            v.push(WriteStrategy::S2);
        }
        v.push(WriteStrategy::S3);
        v
    }

    /// tmp+MOVE / 条件 PUT / 盲写三选一，服务器回 405/501 就降级到下一级。
    pub(crate) async fn atomic_write(&self, w: &AtomicWrite<'_>) -> Result<Option<String>, RemoteError> {
        let mut degraded = false;
        for strat in self.attempts() {
            match self.try_write(strat, w).await {
                Try::Done(etag) => return Ok(etag),
                Try::Lost => return Err(RemoteError::Precondition),
                Try::Unsupported => {
                    degraded = true;
                    tracing::debug!(?strat, dest = w.dest, "服务器不接受该写入语义，降级");
                }
                Try::Failed(e) => return Err(e),
            }
        }
        Err(if degraded {
            RemoteError::Protocol("服务器不支持任何可用的原子写入语义（405/501）".into())
        } else {
            RemoteError::Server
        })
    }

    async fn try_write(&self, strat: WriteStrategy, w: &AtomicWrite<'_>) -> Try {
        match strat {
            WriteStrategy::S1 => {
                let cond = match w.cas_etag {
                    Some(e) => Cond::IfMatch(e.to_string()),
                    None => Cond::CreateOnly,
                };
                let r = match self.put_cond(w.dest, w.body, cond).await {
                    Ok(r) => r,
                    Err(e) => return Try::Failed(e),
                };
                match judge_write(&r) {
                    Ok(()) => Try::Done(etag_of(&r)),
                    Err(WriteFail::Lost) => Try::Lost,
                    Err(WriteFail::Unsupported) => Try::Unsupported,
                    Err(WriteFail::Other(e)) => Try::Failed(map_net(e)),
                }
            }
            WriteStrategy::S2 => {
                if w.overwrite_existing {
                    // `Overwrite: F` 只对"目标不存在"成立，所以更新必须先让目标消失。
                    // 诚实记录残留风险：腾空与落新之间没有被服务器判定的条件（这类服务器
                    // 也没有条件 DELETE），因此 S2 的 CAS 只覆盖到第 ① 步读到的那一瞬 ——
                    // 窗口比 S1 大，靠 §11.2 第一层 rev 闸门与本方法的写后复验收住。
                    match self.delete_raw(w.dest).await {
                        Ok(r) if r.status == 404 || r.is_success() => {}
                        Ok(r) => return Try::Failed(map_status(r.status)),
                        Err(e) => return Try::Failed(e),
                    }
                }
                if let Err(e) = self.put_plain(w.tmp, w.body).await {
                    return Try::Failed(e);
                }
                match self.move_raw(w.tmp, w.dest, false, None).await {
                    Ok(r) => match judge_write(&r) {
                        Ok(()) => Try::Done(etag_of(&r)),
                        Err(f) => {
                            self.best_effort_delete(w.tmp).await;
                            match f {
                                // 目标在 DELETE 与 MOVE 之间被别人建了 —— 让路，不覆盖。
                                WriteFail::Lost => Try::Lost,
                                WriteFail::Unsupported => Try::Unsupported,
                                WriteFail::Other(e) => Try::Failed(map_net(e)),
                            }
                        }
                    },
                    Err(e) => {
                        self.best_effort_delete(w.tmp).await;
                        Try::Failed(e)
                    }
                }
            }
            WriteStrategy::S3 => {
                // 降级路径：盲写。并发保护只剩写后复验（§5"存在覆盖窗口，靠复验检出"）。
                let r = match self.put_cond(w.dest, w.body, Cond::None).await {
                    Ok(r) => r,
                    Err(e) => return Try::Failed(e),
                };
                match judge_write(&r) {
                    Ok(()) => Try::Done(etag_of(&r)),
                    Err(WriteFail::Lost) => Try::Lost,
                    Err(WriteFail::Unsupported) => Try::Unsupported,
                    Err(WriteFail::Other(e)) => Try::Failed(map_net(e)),
                }
            }
        }
    }

    /// 写后复验，`Commit.verified` 的**唯一**来源。
    ///
    /// 三条策略都在这里读回目标并逐字节比对：§5 把 S2 的"回读校验"画在 tmp 阶段，但
    /// [`notera_sync::Commit::verified`] 要的是"远端这一条就是我发的这一条"，所以 S2 也补一次。
    /// `verified = false` 只出现在**复验没能确认**的情形（读回 404，或内容与发出不一致 ——
    /// S3 的覆盖窗口就靠这一条检出）；传输/权限类失败直接以 `Err` 上抛，不伪装成"写完了"。
    async fn confirm_record(&self, url: &str, want: &WireMeta, wire: &[u8], etag: Option<String>) -> Result<Commit, RemoteError> {
        let got = self.read_object(url, None).await?;
        if got.status == 404 {
            return Ok(Commit { etag, verified: false });
        }
        if !got.is_success() {
            return Err(map_status(got.status));
        }
        let stored = WireMeta::parse_lenient(&got.body);
        let verified = got.body == wire && stored.as_ref().is_some_and(|m| m.rev == want.rev && m.hash == want.hash);
        if !verified {
            tracing::warn!(url, "写后复验不一致：判定被覆盖，交回上层重算计划");
        }
        Ok(Commit {
            etag: got.etag.or(etag),
            verified,
        })
    }

    /// prev 的退化写法：没有可用的 MOVE 时，把第 ① 步读到的旧正文原样 PUT 上去。
    async fn put_prev_from_observed(&self, prev_url: &str, bytes: Option<&[u8]>) -> Result<(), RemoteError> {
        let Some(b) = bytes else { return Ok(()) };
        let r = self.put_plain(prev_url, b).await?;
        match judge_write(&r) {
            Ok(()) => Ok(()),
            Err(WriteFail::Unsupported) => Err(RemoteError::Protocol("服务器无法保留上一版清单（prev 写不进去）".into())),
            Err(WriteFail::Lost) => Err(RemoteError::Precondition),
            Err(WriteFail::Other(e)) => Err(map_net(e)),
        }
    }

    /// 没有可用的 `MOVE`（或它被服务器拒了）时的清单发布。
    ///
    /// 第 ④ 步已经把 `index.json` 腾空，所以这里"应当不存在"是成立的前提 ——
    /// 有条件 PUT 就用 `If-None-Match: *` 把它变成真的 CAS（有人抢建就 412）；
    /// 连条件 PUT 都没有才退到 §5 的盲写，覆盖风险由第 ⑥ 步复验检出。
    async fn publish_manifest_by_put(&self, index: &str, wire: &[u8]) -> Result<Option<String>, RemoteError> {
        let cond = if self.caps.has(Caps::CONDITIONAL_PUT) { Cond::CreateOnly } else { Cond::None };
        if matches!(cond, Cond::None) {
            tracing::debug!("MOVE 与条件 PUT 都不可用，清单降级为盲 PUT 发布");
        }
        let r = self.put_cond(index, wire, cond).await?;
        match judge_write(&r) {
            Ok(()) => Ok(etag_of(&r)),
            Err(WriteFail::Unsupported) => Err(RemoteError::Protocol("服务器不接受清单写入（405/501）".into())),
            Err(WriteFail::Lost) => Err(RemoteError::Precondition),
            Err(WriteFail::Other(e)) => Err(map_net(e)),
        }
    }
}

/// 一次原子写的输入。
pub(crate) struct AtomicWrite<'a> {
    pub dest: &'a str,
    pub tmp: &'a str,
    pub body: &'a [u8],
    /// 我们**已经读到**的目标 etag；`None` = 目标应当不存在（创建语义）。
    pub cas_etag: Option<&'a str>,
    /// `true` = 目标当前存在且正被换新内容（只有 S2 需要为此先 DELETE 目标）。
    pub overwrite_existing: bool,
}

/// `PUT` 的条件形态。
pub(crate) enum Cond {
    None,
    IfMatch(String),
    /// `If-None-Match: *` = 只允许创建，存在即 412。
    CreateOnly,
}

pub(crate) enum Try {
    Done(Option<String>),
    /// 服务器判定 CAS 失败：让路，不覆盖。
    Lost,
    /// 405/501：语义不被支持，降级到下一策略（§12）。
    Unsupported,
    Failed(RemoteError),
}

pub(crate) enum WriteFail {
    Lost,
    Unsupported,
    Other(NetError),
}

/// 写请求的判决：412/409/404/423 都是"前置条件不成立"，只有 405/501 才是"不支持"。
fn judge_write(resp: &Response) -> Result<(), WriteFail> {
    match resp.status {
        200..=299 => Ok(()),
        404 | 409 | 412 | 423 | 428 => Err(WriteFail::Lost),
        405 | 501 => Err(WriteFail::Unsupported),
        s => Err(WriteFail::Other(NetError::classify_status(s))),
    }
}

fn etag_of(resp: &Response) -> Option<String> {
    resp.etag.clone().or_else(|| resp.header("etag").map(str::to_string))
}

/// 一个远端对象的读结果（状态 + 原文 + etag）。
#[derive(Clone, Debug)]
pub(crate) struct Object {
    pub status: u16,
    pub body: Vec<u8>,
    pub etag: Option<String>,
}

impl Object {
    fn of(resp: &Response) -> Object {
        Object {
            status: resp.status,
            body: resp.body.clone(),
            etag: etag_of(resp),
        }
    }
    fn is_success(&self) -> bool {
        (200..=299).contains(&self.status)
    }
}

// ------------------------------------------------------------ RemotePort ---

#[async_trait::async_trait]
impl RemotePort for WebDavRemote {
    /// `GET manifest/index.json`（带 `If-None-Match`）。
    ///
    /// `Ok(None)` = "本轮拿不到新的清单正文"，涵盖两种形态：304（远端未变）与 404
    /// （根还没初始化）。清单是公告板不是数据源（§0 R1），所以两者都**绝不**被解读成
    /// "远端是空的"；引擎在 `None` 分支回落到 `LocalPort::cached_manifest`。
    async fn fetch_manifest(&self, etag: Option<&str>) -> Result<Option<(Vec<u8>, Option<String>)>, RemoteError> {
        let url = self.paths.manifest_index();
        let o = self.read_object(&url, etag).await?;
        match o.status {
            304 | 404 => Ok(None),
            200..=299 if o.body.is_empty() => Err(RemoteError::Protocol("index.json 为空，拒绝当成空清单".into())),
            200..=299 => Ok(Some((o.body, o.etag))),
            s => Err(map_status(s)),
        }
    }

    /// `GET manifest/<name>.json` → 条目数组（§4.1）。
    async fn fetch_segment(&self, name: &str) -> Result<Vec<EntryRef>, RemoteError> {
        let url = self.paths.segment(name)?;
        let o = self.read_object(&url, None).await?;
        match o.status {
            404 => Err(RemoteError::NotFound),
            200..=299 if o.body.is_empty() => Err(RemoteError::Protocol(format!("分段 {name} 内容为空"))),
            200..=299 => parse_segment(&o.body, name),
            s => Err(map_status(s)),
        }
    }

    /// `GET records/<kind>/<id>.json`；404 → `Ok(None)`（§10 的分类归上层）。
    async fn fetch_record(&self, kind: &str, id: &str) -> Result<Option<Vec<u8>>, RemoteError> {
        let rp = self.paths.record(kind, id)?;
        let o = self.read_object(&rp.url, None).await?;
        match o.status {
            404 => Ok(None),
            200..=299 => Ok(Some(o.body)),
            s => Err(map_status(s)),
        }
    }

    /// 写一条实体记录：§11.2 的 rev 闸门 + §5 的策略选择 + 写后复验。
    ///
    /// rev 闸门需要知道远端当前的 rev，而 HTTP 没有 `If-Rev` —— 因此"更新写"
    /// （`if_match` 非空）与 S3 一律先 `GET` 一次；纯创建写不花这一趟（服务器用
    /// `If-None-Match: *` / `Overwrite: F` 替我们守住"不许覆盖"）。
    async fn put_record(&self, kind: &str, id: &str, wire: &[u8], if_match: Option<&str>) -> Result<Commit, RemoteError> {
        let rp: RecordPath = self.paths.record(kind, id)?;
        if wire.len() > MAX_RECORD_BYTES {
            return Err(RemoteError::Protocol(format!("记录 {} 字节，超过单对象上限", wire.len())));
        }
        let want = WireMeta::parse_outgoing(wire)?;
        check_envelope_identity(&want, kind, id)?;

        let strategy = self.caps.write_strategy();
        let mut cas: Option<String> = if_match.map(str::to_string);
        let mut overwrite_existing = false;
        if strategy == WriteStrategy::S3 || if_match.is_some() {
            let o = self.read_object(&rp.url, None).await?;
            match o.status {
                404 => {
                    cas = None;
                }
                200..=299 => {
                    match WireMeta::parse_lenient(&o.body) {
                        Some(m) => {
                            if m.is_same_commit(&want) {
                                // §11.1：上一轮写了、确认丢了。内容一致即直接判成功，不重复写。
                                return Ok(Commit { etag: o.etag, verified: true });
                            }
                            if let (Some(e), Some(fresh)) = (if_match, o.etag.as_deref()) {
                                if fresh != e {
                                    // 调用方手里的 etag 已经不是服务器那一版了：有人在探测与写入
                                    // 之间动过这一条（§11.2 第二层）。让路，不覆盖。
                                    return Err(RemoteError::Precondition);
                                }
                            }
                            if want.rev <= m.rev {
                                // 绝不把更旧的一条盖到更新的上面（R3：只认 rev + hash）。
                                return Err(RemoteError::Precondition);
                            }
                            overwrite_existing = true;
                            // 引擎的 HEAD 探测可能比这一趟 GET 更旧：以刚读到的 etag 为准。
                            cas = o.etag.clone().or_else(|| if_match.map(str::to_string));
                        }
                        None => {
                            return Err(RemoteError::Protocol(format!(
                                "远端记录 {} 不是可复验的信封（缺 rev/hash），已拒绝覆盖",
                                rp.on_server
                            )))
                        }
                    }
                }
                s => return Err(map_status(s)),
            }
        }
        let tmp = self.paths.record_tmp(kind, id, &self.device, self.next_nonce())?;
        let w = AtomicWrite {
            dest: &rp.url,
            tmp: &tmp,
            body: wire,
            cas_etag: cas.as_deref(),
            overwrite_existing,
        };
        let etag = self.atomic_write(&w).await?;
        self.confirm_record(&rp.url, &want, wire, etag).await
    }

    /// §4.4 的六步原子提交。
    ///
    /// 与文档唯一的偏离在第 ④ 步：文档写的是"把 `.tmp-*` 移进 `.prev`"，但同一行的注释
    /// "保留上一版"、§1 的 `index.json.prev 上一版清单` 与 D1 的回退语义都要求 `.prev`
    /// 存的是**上一版**。把新内容移进去会当场毁掉那份唯一能在清单损坏时回退的副本，
    /// 所以这里做的是轮转：`index.json → index.json.prev`，再把暂存移进 `index.json`
    /// （也因此只需一次暂存上传，不需要文档里的 `.tmp-*2`）。
    async fn commit_manifest(&self, wire: &[u8], cas_etag: Option<&str>) -> Result<Option<String>, RemoteError> {
        let staged = Manifest::parse(wire).map_err(|e| RemoteError::Protocol(format!("清单自校验失败，拒绝公告: {e}")))?;
        let index = self.paths.manifest_index();

        // ① 读当前 index.json 取 E0 —— 调用方已给出 E0 时不重读（§6.3 的请求预算）。
        let observed: Option<Object> = match cas_etag {
            Some(_) => None,
            None => Some(self.read_object(&index, None).await?),
        };
        if let Some(o) = &observed {
            match o.status {
                404 => {}
                200..=299 => {
                    let cur = Manifest::parse(&o.body).map_err(|e| RemoteError::Protocol(format!("服务器上的清单不可信: {e}")))?;
                    if staged.seq <= cur.seq {
                        // 清单落后于实体是安全的（R2），倒退不是：拒绝并让上层重算。
                        return Err(RemoteError::Precondition);
                    }
                }
                s => return Err(map_status(s)),
            }
        }
        let e0: Option<String> = match cas_etag {
            Some(e) => Some(e.to_string()),
            None => observed.as_ref().filter(|o| o.is_success()).and_then(|o| o.etag.clone()),
        };
        let prev_bytes: Option<Vec<u8>> = observed.as_ref().filter(|o| o.status == 200).map(|o| o.body.clone());

        // ③ PUT 暂存 → 回读校验 checksum：半写/截断在这里被拦住，而不是在别的设备上。
        let tmp = self.paths.manifest_tmp(&self.device, self.next_nonce())?;
        let staged_resp = self.put_plain(&tmp, wire).await?;
        if !staged_resp.is_success() {
            return Err(map_status(staged_resp.status));
        }
        let back = self.read_object(&tmp, None).await?;
        if !back.is_success() || back.body != wire || !checksum_matches(&back.body) {
            self.best_effort_delete(&tmp).await;
            return Err(RemoteError::Protocol("清单暂存回读与提交内容不一致".into()));
        }

        // ④ 轮转上一版：index.json → index.json.prev（Overwrite:T，If-Match: E0）。
        let prev = self.paths.manifest_prev();
        if let Some(e) = &e0 {
            let rot = self.move_raw(&index, &prev, true, Some(e)).await?;
            match judge_write(&rot) {
                Ok(()) => {}
                Err(WriteFail::Lost) if rot.status == 404 => {
                    // 目标已不在：没什么可保留的，继续发布。
                }
                Err(WriteFail::Lost) => {
                    self.best_effort_delete(&tmp).await;
                    return Err(RemoteError::Precondition);
                }
                Err(WriteFail::Unsupported) => {
                    // 没有可用的 MOVE：退化用 prev 的写入保住上一版，再把 index 腾空。
                    let keep = match &prev_bytes {
                        Some(b) => b.clone(),
                        None => self.read_object(&index, None).await?.body,
                    };
                    self.put_prev_from_observed(&prev, Some(&keep)).await?;
                    self.delete_raw(&index).await?;
                }
                Err(WriteFail::Other(err)) => {
                    self.best_effort_delete(&tmp).await;
                    return Err(map_net(err));
                }
            }
        } else {
            self.put_prev_from_observed(&prev, prev_bytes.as_deref()).await?;
            if prev_bytes.is_some() {
                // 服务器不肯给 ETag → 没有 If-Match 可用的轮转。保住 prev 之后把 index 腾空，
                // 让第 ⑤ 步的"创建语义"仍然是一个真的 CAS（抢不过别人就 412）。
                let d = self.delete_raw(&index).await?;
                if !d.is_success() && d.status != 404 {
                    return Err(map_status(d.status));
                }
            }
        }

        // ⑤ 暂存 → index.json。`Overwrite: F` = 创建语义，存在即 412 → 让路。
        let final_etag = if self.caps.has(Caps::OVERWRITE_F_MOVE) {
            let mv = self.move_raw(&tmp, &index, false, None).await?;
            match judge_write(&mv) {
                Ok(()) => etag_of(&mv),
                Err(f) => {
                    self.best_effort_delete(&tmp).await;
                    match f {
                        WriteFail::Lost => return Err(RemoteError::Precondition),
                        WriteFail::Unsupported => self.publish_manifest_by_put(&index, wire).await?,
                        WriteFail::Other(err) => return Err(map_net(err)),
                    }
                }
            }
        } else {
            self.publish_manifest_by_put(&index, wire).await?
        };

        // ⑥ GET 复算 checksum，比对 seq。
        let done = self.read_object(&index, None).await?;
        if !done.is_success() {
            return Err(map_status(done.status));
        }
        let published = Manifest::parse(&done.body).map_err(|e| RemoteError::Protocol(format!("提交后的清单不可信: {e}")))?;
        if done.body != wire || published.seq != staged.seq {
            // 公告被别人覆盖了（或服务器只落了半份）：判"远端领先"，重算计划。
            return Err(RemoteError::Precondition);
        }
        Ok(done.etag.or(final_etag))
    }

    /// 基线分段写入（§4.3：只有压实会重写它；CAS 由随后的清单提交仲裁）。
    async fn put_segment(&self, name: &str, wire: &[u8]) -> Result<(), RemoteError> {
        let url = self.paths.segment(name)?;
        let existing = self.etag_of(&url).await?;
        let tmp = self.paths.tmp(&self.device, self.next_nonce())?;
        let w = AtomicWrite {
            dest: &url,
            tmp: &tmp,
            body: wire,
            cas_etag: existing.as_deref(),
            overwrite_existing: existing.is_some(),
        };
        let written = self.atomic_write(&w).await?;
        let back = self.read_object(&url, None).await?;
        if !back.is_success() {
            if written.is_some() {
                return Err(map_status(back.status));
            }
            return Err(RemoteError::Precondition);
        }
        if back.body != wire {
            // 读回来的不是我们那一份：分段被并发重写过，让上层重算而不是将错就错。
            return Err(RemoteError::Precondition);
        }
        Ok(())
    }

    /// HEAD 一条记录取强 etag；404 → `Ok(None)`。
    async fn probe_record_etag(&self, kind: &str, id: &str) -> Result<Option<String>, RemoteError> {
        let rp = self.paths.record(kind, id)?;
        self.etag_of(&rp.url).await
    }
}

/// §3：信封的 `kind` 必须与目录一致、`id` 必须与文件名一致。写出去之前先自查。
fn check_envelope_identity(want: &WireMeta, kind: &str, id: &str) -> Result<(), RemoteError> {
    if let Some(k) = &want.kind {
        if !kind_matches(kind, k) {
            return Err(RemoteError::Protocol(format!("信封 kind {k:?} 与目录 {kind:?} 不一致")));
        }
    }
    if let Some(i) = &want.id {
        if i != id {
            return Err(RemoteError::Protocol(format!("信封 id {i:?} 与文件名 {id:?} 不一致")));
        }
    }
    if want.purged && kind_dir(kind).is_err() {
        return Err(RemoteError::Protocol("墓碑信封缺少 kind".into()));
    }
    Ok(())
}

/// 暂存回读的自校验：能解析、checksum 形态正确即算通过（内容一致性由逐字节比对兜住）。
fn checksum_matches(bytes: &[u8]) -> bool {
    match Manifest::parse(bytes) {
        Ok(m) => m.checksum.len() == "sha256:".len() + 64 && m.checksum.starts_with("sha256:"),
        Err(_) => false,
    }
}

/// 分段文件的两种可接受形态：裸条目数组，或 `{"entries": [...]}`。
fn parse_segment(body: &[u8], name: &str) -> Result<Vec<EntryRef>, RemoteError> {
    let v: Value = serde_json::from_slice(body).map_err(|e| RemoteError::Protocol(format!("分段 {name} 不是合法 JSON: {e}")))?;
    let arr = match &v {
        Value::Array(a) => a.clone(),
        Value::Object(o) => o.get("entries").and_then(Value::as_array).cloned().unwrap_or_default(),
        _ => Vec::new(),
    };
    let mut out = Vec::with_capacity(arr.len());
    for raw in arr {
        let e: EntryRef = serde_json::from_value(raw).map_err(|e| RemoteError::Protocol(format!("分段 {name} 条目非法: {e}")))?;
        notera_core::EntityId::parse(&e.i)
            .map_err(|_| RemoteError::Protocol(format!("分段 {name} 含非 UUID 条目 id: {:?}", e.i)))?;
        out.push(e);
    }
    Ok(out)
}
