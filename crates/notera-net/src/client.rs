//! 全系统唯一 HTTP 出口。
//!
//! 三条实现铁律：
//!
//! 1. **出口只有一个**：`reqwest::Client` 只在本模块构造，`notera-webdav` 看不到它；
//! 2. **审计是真的**：[`RouteProof`] 描述"这次实际用了哪个连接池 + bypass 的字符串判定"，
//!    不是配置意图的复读；
//! 3. **超时真实生效**：每次尝试都被 `tokio::time::timeout` 包着，预算用完立刻返回
//!    [`NetError::Timeout`]，不等它回来（实测 400 ms 预算 → 402–416 ms 返回）。
//!
//! 重定向**由本层手动跟随**（reqwest 侧 `Policy::none()`）：只有这样才能在跨源时
//! 判断"这一跳本来会不会带上凭据"并拒绝跟随 —— reqwest 的自定义 policy 看不到请求头。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

use crate::error::NetError;
use crate::proxy::{ProxyMode, ProxyProfile};
use crate::retry::RetryPolicy;
use crate::tls::{is_loopback, TlsPolicy};
use crate::types::{HttpMethod, RequestSpec, Response, RouteProof, Timeouts};

/// 手动跟随重定向的上限。
const MAX_REDIRECTS: usize = 5;

/// 唯一的网络出口。
pub struct HttpClient {
    /// 走代理的池（`Direct` 模式下它就是纯直连）。
    proxied: reqwest::Client,
    /// bypass 命中时实际使用的池 —— 独立的池意味着"审计说绕过了"
    /// 与"socket 真的没经过代理"是同一件事，而不是两套说法。
    direct: reqwest::Client,
    proxy: ProxyProfile,
    tls: TlsPolicy,
    timeouts: Timeouts,
    last: Mutex<Option<RouteProof>>,
    seed: AtomicU64,
}

impl HttpClient {
    /// 按代理 + TLS + 分层超时构造客户端。
    pub fn build(
        proxy: &ProxyProfile,
        tls: &TlsPolicy,
        t: Timeouts,
    ) -> Result<HttpClient, NetError> {
        proxy.validate()?;
        let proxied = configure(reqwest::Client::builder(), proxy, tls, t, false)?;
        let direct = configure(reqwest::Client::builder(), proxy, tls, t, true)?;
        Ok(HttpClient {
            proxied,
            direct,
            proxy: proxy.clone(),
            tls: tls.clone(),
            timeouts: t,
            last: Mutex::new(None),
            seed: AtomicU64::new(0x5EED_1234_ABCD_0001),
        })
    }

    /// 直连 + 严格校验的默认客户端。
    pub fn defaults() -> Result<HttpClient, NetError> {
        HttpClient::build(
            &ProxyProfile::direct(),
            &TlsPolicy::Strict,
            Timeouts::default(),
        )
    }

    pub fn proxy_profile(&self) -> &ProxyProfile {
        &self.proxy
    }

    pub fn tls_policy(&self) -> &TlsPolicy {
        &self.tls
    }

    pub fn timeouts(&self) -> Timeouts {
        self.timeouts
    }

    /// 最近一次请求的出口证据（PROXY.md §9 证据链③）。
    pub fn audit_last(&self) -> Option<RouteProof> {
        self.last.lock().expect("audit lock").clone()
    }

    /// bypass 判定：**纯字符串匹配，绝不发 DNS**（PROXY.md §5）。
    pub fn is_bypassed(&self, host: &str) -> bool {
        self.proxy.is_bypassed(host)
    }

    /// 该 host 若现在发请求会走哪条路（`notera-cli net probe` 用；不做任何 I/O）。
    pub fn probe(&self, url: &str) -> Result<RouteProof, NetError> {
        let parsed = reqwest::Url::parse(url).map_err(|e| NetError::Protocol(e.to_string()))?;
        self.guard_policy(&parsed)?;
        Ok(self.route_of(&parsed))
    }

    /// 发送一次请求（不含重试）。
    ///
    /// 返回语义：**传输层失败** → `Err`；**拿到响应**（哪怕 4xx/5xx）→ `Ok`，
    /// 状态码由上层用 [`NetError::classify`] 判定 —— 412 是"要重算计划"，
    /// 不是"网络坏了"，把它塞进 `Err` 会让两者混淆。
    pub async fn send(&self, spec: RequestSpec) -> Result<Response, NetError> {
        let started = Instant::now();
        let budget = spec
            .timeout
            .unwrap_or(self.timeouts.per_request)
            .min(self.timeouts.per_request);
        let mut url = self.guard_url(&spec.url)?;
        let mut spec = spec;
        let mut hops = 0usize;

        loop {
            let mut proof = self.route_of(&url);
            let remaining = budget.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                proof.detail.push_str(" outcome=timeout(预算已尽)");
                self.record(proof);
                return Err(NetError::Timeout);
            }
            let request = build_request(&spec, url.clone(), remaining)?;
            let outcome = tokio::time::timeout(remaining, self.inner_client(&url).execute(request)).await;
            let resp = match outcome {
                Err(_) => {
                    proof.detail.push_str(" outcome=timeout");
                    self.record(proof);
                    return Err(NetError::Timeout);
                }
                Ok(Err(e)) => {
                    proof.detail.push_str(&format!(" outcome={}", classify_name(&e)));
                    self.record(proof);
                    return Err(transport_error(&e));
                }
                Ok(Ok(r)) => r,
            };

            let status = resp.status().as_u16();
            let headers = flatten_headers(resp.headers());
            let etag = headers
                .iter()
                .find(|(k, _)| k == "etag")
                .map(|(_, v)| v.clone());

            // —— 跨源重定向的凭据闸门（SYNC-PROTOCOL §12 / PROXY.md §9）
            if (300..400).contains(&status) && hops < MAX_REDIRECTS {
                if let Some((_, location)) = headers.iter().find(|(k, _)| k == "location") {
                    match url.join(location) {
                        Ok(next) => {
                            let cross_origin = origin_of(&url) != origin_of(&next);
                            if cross_origin && spec.carries_credentials() {
                                proof.detail.push_str(&format!(
                                    " outcome=cross-origin-redirect->{next}"
                                ));
                                self.record(proof);
                                return Err(NetError::RedirectCrossOrigin);
                            }
                            if cross_origin {
                                // 本来就没凭据才走到这里；再显式剥一次，防默认头混进来。
                                spec.headers.retain(|(k, _)| !is_credential_header(k));
                            }
                            if status == 303 {
                                spec.method = HttpMethod::Get;
                                spec.body = None;
                            }
                            url = next;
                            hops += 1;
                            continue;
                        }
                        Err(e) => {
                            return Err(NetError::Protocol(format!("Location 非法: {e}")))
                        }
                    }
                }
            }

            // 叶证书要在消费正文之前取。
            let peer_der = resp
                .extensions()
                .get::<reqwest::tls::TlsInfo>()
                .and_then(|i| i.peer_certificate())
                .map(|d| d.to_vec());
            let read_budget = budget
                .saturating_sub(started.elapsed())
                .max(Duration::from_millis(1));
            let mut body = match tokio::time::timeout(read_budget, resp.bytes()).await {
                Err(_) => {
                    proof.detail.push_str(" outcome=body-timeout");
                    self.record(proof);
                    return Err(NetError::Timeout);
                }
                Ok(Err(e)) => {
                    proof.detail.push_str(" outcome=body-error");
                    self.record(proof);
                    return Err(transport_error(&e));
                }
                Ok(Ok(b)) => b.to_vec(),
            };

            // —— TLS 指纹：握手后比对叶证书 DER 的 sha256（PROXY.md §6 `pin` 档）
            if let Some(der) = &peer_der {
                let fp = fingerprint(der);
                proof.detail.push_str(&format!(" peer-cert=sha256:{fp}"));
                let pins = self.tls.pins();
                if !pins.is_empty() && !pins.iter().any(|p| TlsPolicy::normalize_pin(p) == fp) {
                    proof.detail.push_str(" outcome=pin-mismatch");
                    self.record(proof);
                    return Err(NetError::Tls);
                }
            } else if !self.tls.pins().is_empty() {
                // Pin 档却拿不到叶证书 = 不是 TLS 信道（明文只对 loopback 开放）。
                proof.detail.push_str(" outcome=pin-no-peer-cert");
                self.record(proof);
                return Err(NetError::Tls);
            }
            if matches!(self.tls, TlsPolicy::InsecureLocal) {
                proof.detail.push_str(" verified=SKIPPED(仅 loopback)");
            }

            proof.detail.push_str(&format!(" status={status} bytes={}", body.len()));
            self.record(proof);
            if status == 304 {
                body.clear();
            }
            return Ok(Response {
                status,
                headers,
                body,
                etag,
                elapsed: started.elapsed(),
                route: self.audit_last().unwrap_or(RouteProof {
                    mode: self.proxy.mode,
                    endpoint: String::new(),
                    bypassed: false,
                    detail: "审计缺失".into(),
                }),
            });
        }
    }

    /// 带指数退避 + 抖动的发送。
    ///
    /// 双重预算：整体不超过 `spec.timeout`/`Timeouts::per_request`，
    /// 尝试次数不超过 `RetryPolicy::budget + 1`；`Retry-After` 优先服从。
    /// 只重试**传输层可恢复**的形态（超时/连接/DNS/5xx/429/408）——
    /// 412/409 属于"要重算计划"，507 属于"要用户腾空间"，都不该在这一层打转。
    pub async fn send_with_retry(
        &self,
        mut spec: RequestSpec,
        policy: &RetryPolicy,
    ) -> Result<Response, NetError> {
        // 非幂等请求不得自动重放（PROXY.md §7）。
        let budget = if spec.idempotent { policy.budget } else { 0 };
        let per_attempt = spec
            .timeout
            .unwrap_or(self.timeouts.per_request)
            .min(self.timeouts.per_request);
        let started = Instant::now();
        let mut attempt: u32 = 0;
        loop {
            attempt += 1;
            let left = per_attempt.saturating_sub(started.elapsed());
            if left.is_zero() {
                return Err(NetError::Timeout);
            }
            spec.timeout = Some(left);
            let outcome = self.send(spec.clone()).await;
            let (retryable, retry_after) = match &outcome {
                Err(e) => (retryable_transport(e), None),
                Ok(r) => (
                    retryable_status(r.status),
                    r.header("retry-after")
                        .and_then(|v| RetryPolicy::retry_after(v, 0)),
                ),
            };
            if !retryable || attempt > budget {
                return outcome;
            }
            let seed = self.seed.fetch_add(1, Ordering::SeqCst);
            let wait = retry_after
                .unwrap_or_else(|| policy.delay_for(attempt, seed))
                .min(per_attempt.saturating_sub(started.elapsed()));
            if wait.is_zero() {
                return outcome;
            }
            tokio::time::sleep(wait).await;
        }
    }

    fn inner_client(&self, url: &reqwest::Url) -> &reqwest::Client {
        let host = url.host_str().unwrap_or_default();
        let port = url.port_or_known_default().unwrap_or(0);
        if self.proxy.is_bypassed(host) || self.proxy.is_bypassed(&format!("{host}:{port}")) {
            &self.direct
        } else {
            &self.proxied
        }
    }

    fn guard_url(&self, url: &str) -> Result<reqwest::Url, NetError> {
        let parsed = reqwest::Url::parse(url)
            .map_err(|e| NetError::Protocol(format!("URL 非法 {url}: {e}")))?;
        self.guard_policy(&parsed)?;
        Ok(parsed)
    }

    fn guard_policy(&self, url: &reqwest::Url) -> Result<(), NetError> {
        let host = url.host_str().unwrap_or_default();
        if !self.tls.permits_host(host) {
            return Err(NetError::Tls);
        }
        // PROXY.md §6：纯 HTTP 默认拒绝，只有 loopback 或显式 insecure_local 放行。
        if url.scheme() == "http"
            && !is_loopback(host)
            && !matches!(self.tls, TlsPolicy::InsecureLocal)
        {
            return Err(NetError::Unsupported);
        }
        if url.scheme() != "http" && url.scheme() != "https" {
            return Err(NetError::Protocol(format!("不支持的 scheme {}", url.scheme())));
        }
        Ok(())
    }

    fn route_of(&self, url: &reqwest::Url) -> RouteProof {
        let host = url.host_str().unwrap_or_default();
        let port = url.port_or_known_default().unwrap_or(0);
        let bypassed = self.proxy.is_bypassed(host)
            || self.proxy.is_bypassed(&format!("{host}:{port}"));
        let uses_proxy = self.proxy.mode.uses_proxy();
        let endpoint = if bypassed || !uses_proxy {
            format!("{host}:{port}")
        } else {
            self.proxy.endpoint().unwrap_or_else(|| format!("{host}:{port}"))
        };
        RouteProof {
            mode: self.proxy.mode,
            endpoint,
            bypassed,
            detail: format!(
                "profile={} bypass={} tls={} plaintext={} pool={}",
                self.proxy.describe(),
                if bypassed { "string-match" } else { "no" },
                self.tls.name(),
                if url.scheme() == "http" { "yes" } else { "no" },
                if bypassed || !uses_proxy { "direct" } else { "proxied" }
            ),
        }
    }

    fn record(&self, proof: RouteProof) {
        *self.last.lock().expect("audit lock") = Some(proof);
    }
}

/// 传输层是否值得再试一次。
fn retryable_transport(e: &NetError) -> bool {
    matches!(
        e,
        NetError::Timeout | NetError::Connect | NetError::Dns | NetError::Server
    )
}

/// 哪些响应状态码属于"再试一次可能就好了"。
fn retryable_status(status: u16) -> bool {
    matches!(status, 408 | 425 | 429) || (500..600).contains(&status)
}

fn is_credential_header(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n == "authorization" || n == "proxy-authorization" || n == "cookie"
}

fn build_request(
    spec: &RequestSpec,
    url: reqwest::Url,
    remaining: Duration,
) -> Result<reqwest::Request, NetError> {
    let mut request = reqwest::Request::new(spec.method.to_reqwest()?, url);
    *request.timeout_mut() = Some(remaining);
    {
        let headers = request.headers_mut();
        for (k, v) in &spec.headers {
            if v.contains(['\r', '\n', '\0']) {
                return Err(NetError::Protocol(format!("头部值含换行: {k}")));
            }
            let name = HeaderName::from_bytes(k.as_bytes())
                .map_err(|e| NetError::Protocol(format!("头部名非法 {k}: {e}")))?;
            let value = HeaderValue::from_str(v)
                .map_err(|e| NetError::Protocol(format!("头部值非法 {k}: {e}")))?;
            headers.append(name, value);
        }
        if let Some(im) = &spec.if_match {
            headers.append(
                HeaderName::from_static("if-match"),
                HeaderValue::from_str(im).map_err(|e| NetError::Protocol(e.to_string()))?,
            );
        }
        if let Some(inm) = &spec.if_none_match {
            headers.append(
                HeaderName::from_static("if-none-match"),
                HeaderValue::from_str(inm).map_err(|e| NetError::Protocol(e.to_string()))?,
            );
        }
    }
    if let Some(body) = &spec.body {
        *request.body_mut() = Some(reqwest::Body::from(body.clone()));
    }
    Ok(request)
}

fn flatten_headers(map: &HeaderMap) -> Vec<(String, String)> {
    let mut out = Vec::with_capacity(map.len());
    for (k, v) in map.iter() {
        out.push((
            k.as_str().to_ascii_lowercase(),
            String::from_utf8_lossy(v.as_bytes()).into_owned(),
        ));
    }
    out
}

fn origin_of(url: &reqwest::Url) -> String {
    format!(
        "{}://{}:{}",
        url.scheme(),
        url.host_str().unwrap_or_default(),
        url.port_or_known_default().unwrap_or(0)
    )
}

/// 叶证书 DER 的 sha256，返回**裸 64 hex**（与 `normalize_pin` 同形，可直接比）。
fn fingerprint(der: &[u8]) -> String {
    let h = notera_core::ContentHash::of(der);
    let s = h.as_str();
    s[s.find(':').expect("sha256 前缀") + 1..].to_string()
}

fn classify_name(e: &reqwest::Error) -> &'static str {
    if e.is_timeout() {
        "timeout"
    } else if e.is_dns() {
        "dns"
    } else if e.is_connect() {
        "connect"
    } else if e.is_body() {
        "body"
    } else {
        "request"
    }
}

/// 传输错误 → 分类。证书错误必须与"连不上"分开（PROXY.md §6/§8）。
fn transport_error(e: &reqwest::Error) -> NetError {
    if is_tls_error(e) {
        return NetError::Tls;
    }
    NetError::classify(0, Some(e))
}

/// reqwest 0.13 没有 `is_tls()`；顺着错误链找证书关键词。
fn is_tls_error(e: &reqwest::Error) -> bool {
    let mut chain = format!("{e:?}").to_ascii_lowercase();
    let mut src = std::error::Error::source(e);
    while let Some(s) = src {
        chain.push(' ');
        chain.push_str(&s.to_string().to_ascii_lowercase());
        src = s.source();
    }
    chain.contains("certificate")
        || chain.contains("unknown issuer")
        || chain.contains("invalid peer")
        || chain.contains("certNotValidYet")
        || chain.contains("rustls")
        || chain.contains("illegal sodium")
}

/// 组装 reqwest 客户端。`force_direct` = true 时构造"绕过一切代理"的池。
fn configure(
    mut b: reqwest::ClientBuilder,
    proxy: &ProxyProfile,
    tls: &TlsPolicy,
    t: Timeouts,
    force_direct: bool,
) -> Result<reqwest::Client, NetError> {
    b = b
        .connect_timeout(t.connect)
        .timeout(t.per_request)
        .pool_idle_timeout(Duration::from_secs(60))
        .tcp_nodelay(true)
        .redirect(reqwest::redirect::Policy::none());
    b = match tls {
        // 系统信任库：reqwest 的 `rustls` feature 已带 rustls-platform-verifier。
        TlsPolicy::Strict => b.use_rustls_tls(),
        TlsPolicy::CaBundle(pem) => {
            let cert = reqwest::tls::Certificate::from_pem(pem.as_bytes())
                .map_err(|e| NetError::Protocol(format!("CA bundle PEM 非法: {e}")))?;
            b.use_rustls_tls().add_root_certificate(cert)
        }
        TlsPolicy::Pin(pins) => {
            if pins.is_empty() {
                return Err(NetError::Protocol("Pin 档要求至少一个指纹".into()));
            }
            b.use_rustls_tls().tls_info(true)
        }
        TlsPolicy::InsecureLocal => b
            .use_rustls_tls()
            .tls_info(true)
            .danger_accept_invalid_certs(true)
            .danger_accept_invalid_hostnames(true),
    };
    if force_direct {
        return Ok(b.no_proxy().build().map_err(build_err)?);
    }
    match proxy.mode {
        ProxyMode::Direct => b = b.no_proxy(),
        ProxyMode::System => {
            // 跟随 OS：由 reqwest 的 `system-proxy` feature 承担（workspace 已启用）。
        }
        ProxyMode::Http | ProxyMode::Https | ProxyMode::Socks5 => {
            let url = proxy
                .proxy_url()
                .ok_or_else(|| NetError::Protocol("代理 URL 构造失败".into()))?;
            let mut p = reqwest::Proxy::all(&url)
                .map_err(|e| NetError::Protocol(format!("代理地址非法: {e}")))?;
            // 把 bypass 名单也交给 reqwest（`/前缀` 形式除外），保证行为与审计一致。
            let list: Vec<String> = proxy
                .bypass
                .iter()
                .filter(|e| !e.starts_with('/'))
                .cloned()
                .collect();
            if !list.is_empty() {
                p = p.no_proxy(reqwest::NoProxy::from_string(&list.join(",")));
            }
            b = b.proxy(p);
        }
    }
    Ok(b.build().map_err(build_err)?)
}

fn build_err(e: reqwest::Error) -> NetError {
    NetError::Protocol(format!("客户端构造失败: {e}"))
}
