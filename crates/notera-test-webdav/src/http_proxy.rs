//! 迷你 **HTTP/1.1 转发代理**（绝对形式请求 + `CONNECT` 隧道，可按 RFC 7235 要求 `407`）。
//!
//! 存在的理由：TEST-PLAN §28「错误密码」那一格此前是 BLOCKED，原因写得很具体 ——
//! 需要一个**会回 `407 Proxy-Authenticate` 的代理**，而本工装的"代理"一直是源站自己扮的
//! （它认识 CONNECT 与绝对形式请求行，可它的"要口令"是**源站**口令，不是代理口令）。
//! 于是产品里"代理要求认证"这一档从来没有一条门禁走过：407 会不会被当成"没代理"而悄悄
//! 回退成直连？代理口令错了会不会报成成功？这两条一旦错了都是**谎报**的形状。
//!
//! 与 `socks5.rs` 的分工不是重复，而是**同一件事的两档**（认证发生在哪一层）：
//! * SOCKS5 在握手层拒（`01 FF`，连不上目标之前就没出去一个字节）。
//! * 这一个在 HTTP 层拒（`407` + `Proxy-Authenticate`，产品侧读到的是一个 HTTP 状态）。
//!
//! **一个必须记着的对称性**：真转发代理交给源站的是 **origin-form** 请求行，于是源站同样分不出
//! "经代理"与"直连" —— 这一点与 SOCKS5 一样。推论有两条，都影响门禁怎么写：
//! 1. `Injection::require_proxy` **不能**与本代理同机使用：它靠"看到绝对形式请求行"判定经代理，
//!    而那只在"源站自己扮演代理"的老夹具里成立。用它去测真转发代理，测到的是我自己的写法。
//! 2. 所以这里的牙齿是**两边的对照**：代理记到几条（`forwarded()`）vs 源站收到几条
//!    （`request_log()`）。407 挡住的那一刻必须 `auth_rejects ≥ 1`、`forwarded == 0`、
//!    **且源站一条都没有** —— 只有"源站空"这一半能证"产品没有偷偷绕过代理去直连"，
//!    而那正是用户撞上"代理口令错了却看着像同步好了"的那种形态。
//!
//! 实现取舍（显式的，不是"忘了做"）：
//! * **一条连接只服务一个请求**：转发给源站的请求里写 `Connection: close`，回给客户端时把
//!   `Connection` 换成本代理自己的 `close`。这是合法的 HTTP/1.1 行为，换来"连接寿命由本代理
//!   说了算"，reqwest 不会把一个已经关掉的连接当成可复用。
//! * **只实现 `Basic` 这一支代理认证**（被测的就是它），其余方案一律按"不匹配"处理成 407。
//! * `CONNECT` 只做隧道，不解 TLS。
//! * 报文解析复用本 crate 的 `http` 模块（`read_request` / `write_response`），不另写一份请求
//!   解析器 —— 两处各写一遍迟早分叉。响应侧没有现成解析器（本 crate 是服务器），故这里自带
//!   一个只读响应头的小函数。
//!
//! 依赖与铁律：本 crate 不许依赖 `notera-net`/`reqwest`（见 `lib.rs` 头部），真 TCP、真握手、
//! 真转发，不 mock。

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use base64::Engine as _;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

use crate::http;

/// 代理认证要求（用户名, 口令）。`None` = 不要求。
type Required = Option<(String, String)>;

#[derive(Debug, Default)]
struct Counters {
    /// 过了认证并真把请求转给源站的数量。
    forwarded: AtomicU64,
    /// `CONNECT` 握手成了隧道的数量。
    tunnels: AtomicU64,
    /// 回了 `407` 的次数（没有 `Proxy-Authorization`，或不匹配）。
    auth_rejects: AtomicU64,
    /// 两个方向合计转发的字节数。
    bytes: AtomicU64,
    /// 最后一次转发的目标（`host:port`）—— 断"它确实是去源站，而不是去别处"。
    last_target: std::sync::Mutex<String>,
}

/// 一个跑在 tokio 上的 HTTP 转发代理。`shutdown()` 之后端口就释放。
pub struct HttpForwardProxy {
    addr: SocketAddr,
    counters: Arc<Counters>,
    stop: tokio::sync::watch::Sender<bool>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl HttpForwardProxy {
    /// 不要求认证。
    pub async fn start() -> std::io::Result<Self> {
        Self::with_auth(None).await
    }

    /// 要求 `Proxy-Authorization: Basic`：不匹配就回 407，并且**一个字节都不往源站转**。
    pub async fn start_requiring(
        user: impl Into<String>,
        pass: impl Into<String>,
    ) -> std::io::Result<Self> {
        Self::with_auth(Some((user.into(), pass.into()))).await
    }

    async fn with_auth(creds: Required) -> std::io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let addr = listener.local_addr()?;
        let counters = Arc::new(Counters::default());
        let (stop, mut rx) = tokio::sync::watch::channel(false);
        let task_counters = Arc::clone(&counters);
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = rx.changed() => break,
                    accepted = listener.accept() => {
                        let Ok((sock, _peer)) = accepted else { break };
                        let c = Arc::clone(&task_counters);
                        let creds = creds.clone();
                        tokio::spawn(async move { serve(sock, c, creds).await; });
                    }
                }
            }
        });
        Ok(Self {
            addr,
            counters,
            stop,
            task: Some(task),
        })
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }
    pub fn host(&self) -> String {
        self.addr.ip().to_string()
    }
    pub fn port(&self) -> u16 {
        self.addr.port()
    }
    /// 过了认证、真被转到源站的请求数。
    pub fn forwarded(&self) -> u64 {
        self.counters.forwarded.load(Ordering::SeqCst)
    }
    pub fn tunnels(&self) -> u64 {
        self.counters.tunnels.load(Ordering::SeqCst)
    }
    pub fn auth_rejects(&self) -> u64 {
        self.counters.auth_rejects.load(Ordering::SeqCst)
    }
    pub fn bytes_forwarded(&self) -> u64 {
        self.counters.bytes.load(Ordering::SeqCst)
    }
    pub fn last_target(&self) -> String {
        self.counters
            .last_target
            .lock()
            .map(|g| g.clone())
            .unwrap_or_default()
    }

    pub async fn shutdown(mut self) {
        let _ = self.stop.send(true);
        if let Some(t) = self.task.take() {
            let _ = t.await;
        }
    }
}

impl Drop for HttpForwardProxy {
    fn drop(&mut self) {
        // 忘了调 shutdown 也不要把任务漏在运行时里：发停止信号，让 accept 循环自己退。
        let _ = self.stop.send(true);
        if let Some(t) = self.task.take() {
            t.abort();
        }
    }
}

/// 一条连接：读请求 →（可选）407 → 转发 → 回传响应 → 关闭。任何一步不对就断开，不转一个字节。
async fn serve(client: TcpStream, counters: Arc<Counters>, creds: Required) {
    let (mut r, mut w) = http::split(client);
    let Ok(Some(req)) = http::read_request(&mut r, None).await else {
        return;
    };

    // --- 代理认证（RFC 7235）：口令不对就到此为止，源站一个字节都收不到 ---
    if let Some((user, pass)) = creds.as_ref() {
        if !presents_valid_basic(&req, user, pass) {
            counters.auth_rejects.fetch_add(1, Ordering::SeqCst);
            let headers = vec![
                (
                    "proxy-authenticate".to_string(),
                    "Basic realm=\"notera-test\"".to_string(),
                ),
                ("content-length".to_string(), "0".to_string()),
                ("connection".to_string(), "close".to_string()),
            ];
            let _ = http::write_response(&mut w, &req.version, 407, &headers, &[], false).await;
            return;
        }
    }

    if req.method == "CONNECT" {
        // 把读写两半合回去再拆，隧道要的是原始字节流。
        // `buffer()` 里可能留着已经读进来的尾字节（客户端在 CONNECT 之后紧接着发了隧道数据）：
        // 那些字节属于隧道，必须先交给源站再开始双向拷贝，否则就丢了开头一段。
        let leftover = r.buffer().to_vec();
        let Ok(merged) = w.reunite(r.into_inner()) else {
            return;
        };
        let (cr, cw) = merged.into_split();
        tunnel(req.target.clone(), cr, cw, leftover, &counters).await;
        return;
    }

    // 转发代理只服务绝对形式请求目标；origin-form 说明客户端没把我们当代理。
    let Some(authority) = authority_of(&req.target) else {
        let h = [("connection".to_string(), "close".to_string())];
        let _ = http::write_response(
            &mut w,
            &req.version,
            400,
            &h,
            b"origin-form to a proxy",
            false,
        )
        .await;
        return;
    };
    let Some(mut upstream) = connect_candidates(&authority).await else {
        let h = [("connection".to_string(), "close".to_string())];
        let _ = http::write_response(
            &mut w,
            &req.version,
            502,
            &h,
            b"proxy cannot reach origin",
            false,
        )
        .await;
        return;
    };
    if let Ok(mut g) = counters.last_target.lock() {
        *g = authority.clone();
    }
    counters.forwarded.fetch_add(1, Ordering::SeqCst);

    // 转出去的请求：origin-form 请求行 + 去掉逐跳头（**尤其 Proxy-Authorization：代理口令不许
    // 出现在源站收到的请求里**）+ 本代理自己声明的 Connection: close。
    let target = if req.query.is_empty() {
        req.path.clone()
    } else {
        format!("{}?{}", req.path, req.query)
    };
    let mut head = format!("{} {target} HTTP/1.1\r\n", req.method);
    for (k, v) in &req.headers {
        if matches!(
            k.as_str(),
            "proxy-authorization" | "connection" | "host" | "content-length" | "transfer-encoding"
        ) {
            continue;
        }
        head.push_str(k);
        head.push_str(": ");
        head.push_str(v);
        head.push_str("\r\n");
    }
    head.push_str("Host: ");
    head.push_str(&authority);
    head.push_str("\r\nConnection: close\r\n");
    if !req.body.is_empty() {
        head.push_str(&format!("Content-Length: {}\r\n", req.body.len()));
    }
    head.push_str("\r\n");

    if write_all_counting(&mut upstream, head.as_bytes(), &counters)
        .await
        .is_err()
        || (!req.body.is_empty()
            && write_all_counting(&mut upstream, &req.body, &counters)
                .await
                .is_err())
    {
        return;
    }
    let _ = upstream.flush().await;

    // 回传：读源站响应头，按 Content-Length（或读到 EOF，因为请求里写了 close）取 body，
    // 再用本代理自己的 Connection: close 写给客户端。
    //
    // **HEAD 必须单独走这一档**（第一版就是没做这件事，症状出现在另一条不相干的判据上）：
    // HEAD 的响应头里带着 `Content-Length`（"GET 会给多少字节"），而**没有 body**。按长度去
    // `read_exact` 必然读到 EOF ⇒ 本代理一个字节都没回，客户端看到的是"连接被提前关闭"。
    // 所以：body 一律为空，而源站给的那个 `Content-Length` 原样转出去（改写它等于伪造尺寸）。
    let is_head = req.method == "HEAD";
    let Some(rsp) = read_response(&mut upstream, is_head).await else {
        return;
    };
    let mut out_headers: Vec<(String, String)> = rsp
        .headers
        .iter()
        .filter(|(k, _)| {
            let hop_by_hop = matches!(k.as_str(), "connection" | "transfer-encoding");
            // 非 HEAD 时 `content-length` 由本代理按真实 body 重新给；HEAD 时**原样保留源站那一个**
            // （它说的是"GET 会给多少字节"，而这正是客户端要知道的）。
            let rewritten = !is_head && k == "content-length";
            !(hop_by_hop || rewritten)
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    if !is_head {
        out_headers.push(("content-length".to_string(), rsp.body.len().to_string()));
    }
    out_headers.push(("connection".to_string(), "close".to_string()));
    counters
        .bytes
        .fetch_add(rsp.body.len() as u64, Ordering::SeqCst);
    let _ = http::write_response(
        &mut w,
        "HTTP/1.1",
        rsp.status,
        &out_headers,
        &rsp.body,
        is_head,
    )
    .await;
}

/// `CONNECT host:port` → 连上就回 200 并双向拷贝；连不上回 502 且**不算隧道**。
/// `leftover` 是读 CONNECT 请求时已经被缓冲进来的尾字节，属于隧道内容，先交给源站。
async fn tunnel(
    authority: String,
    mut cr: http::OwnedReadHalf,
    mut cw: http::OwnedWriteHalf,
    leftover: Vec<u8>,
    counters: &Arc<Counters>,
) {
    let Some(upstream) = connect_candidates(&authority).await else {
        let _ = cw
            .write_all(
                b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .await;
        return;
    };
    if let Ok(mut g) = counters.last_target.lock() {
        *g = authority;
    }
    if cw
        .write_all(b"HTTP/1.1 200 Connection Established\r\nContent-Length: 0\r\n\r\n")
        .await
        .is_err()
    {
        return;
    }
    counters.tunnels.fetch_add(1, Ordering::SeqCst);
    let (mut ur, mut uw) = upstream.into_split();
    if !leftover.is_empty() {
        if write_all_counting(&mut uw, &leftover, counters)
            .await
            .is_err()
        {
            return;
        }
        let _ = uw.flush().await;
    }
    let a = Arc::clone(counters);
    let b = Arc::clone(counters);
    let t1 = tokio::spawn(async move { pump(&mut cr, &mut uw, &a).await });
    let t2 = tokio::spawn(async move { pump(&mut ur, &mut cw, &b).await });
    let _ = t1.await;
    let _ = t2.await;
}

/// 域名可能解析出多个地址（`localhost` → `::1` 与 `127.0.0.1`，而工装源站只听 IPv4）：
/// 按 IPv4 优先试到能连上的那一个，与真代理的做法一致。
async fn connect_candidates(authority: &str) -> Option<TcpStream> {
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host(authority).await.ok()?.collect();
    if addrs.is_empty() {
        return None;
    }
    for a in &addrs {
        if a.is_ipv4() {
            if let Ok(s) = TcpStream::connect(a).await {
                return Some(s);
            }
        }
    }
    for a in &addrs {
        if a.is_ipv6() {
            if let Ok(s) = TcpStream::connect(a).await {
                return Some(s);
            }
        }
    }
    None
}

/// `http://host:port/path` → `host:port`；绝对形式没写端口时按 scheme 补默认端口。
/// `CONNECT` 的 target 本来就是 `host:port`，走"含冒号原样返回"那一支。
fn authority_of(target: &str) -> Option<String> {
    let is_https = target.starts_with("https://");
    let rest = target
        .strip_prefix("http://")
        .or_else(|| target.strip_prefix("https://"))
        .unwrap_or(target);
    let host_port = rest.split(['/', '?']).next().filter(|s| !s.is_empty())?;
    if host_port.contains(':') {
        return Some(host_port.to_string());
    }
    Some(format!("{host_port}:{}", if is_https { 443 } else { 80 }))
}

/// `Proxy-Authorization: Basic base64(user:pass)` —— 只有精确匹配才算过。
fn presents_valid_basic(req: &http::Request, user: &str, pass: &str) -> bool {
    let Some(v) = req.header("proxy-authorization") else {
        return false;
    };
    let Some(rest) = v.strip_prefix("Basic ") else {
        return false;
    };
    let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(rest.trim()) else {
        return false;
    };
    decoded.as_slice() == format!("{user}:{pass}").as_bytes()
}

struct Response {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

/// 读源站的响应：状态行 + 头部，然后**有 `Content-Length` 就按长度读**，没有就读到 EOF
/// （本代理在请求里写了 `Connection: close`，所以"读到 EOF"不会挂住）。
/// `head_only` 时**不读 body**（HEAD 按协议没有 body，而它的 `Content-Length` 是"GET 会给多少"）。
async fn read_response(sock: &mut TcpStream, head_only: bool) -> Option<Response> {
    let mut buf = BufReader::new(sock);
    let mut line = String::new();
    if buf.read_line(&mut line).await.ok()? == 0 {
        return None;
    }
    let status = line
        .split_whitespace()
        .nth(1)?
        .trim_end_matches(['\r', '\n'])
        .parse::<u16>()
        .ok()?;
    let mut headers: Vec<(String, String)> = Vec::new();
    loop {
        let mut l = String::new();
        let n = buf.read_line(&mut l).await.ok()?;
        if n == 0 || l.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = l.split_once(':') {
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
    }
    let mut body = match headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.trim().parse::<usize>().ok())
    {
        // HEAD：按协议没有 body，一个字节都不读（读了就永远等不到那 Content-Length 个字节）。
        _ if head_only => Vec::new(),
        Some(len) => {
            let mut b = vec![0u8; len];
            if buf.read_exact(&mut b).await.is_err() {
                return None;
            }
            b
        }
        None => {
            let mut b = Vec::new();
            if buf.read_to_end(&mut b).await.is_err() {
                return None;
            }
            b
        }
    };
    // 304 按协议没有 body：源站有时会带一个 `Content-Length: 0` 之外的怪头，这里显式清空，
    // 免得"读到 EOF"把别的字节当成 304 的 body 转出去（那会破坏 reqwest 的缓存复验判据）。
    if status == 304 {
        body.clear();
    }
    Some(Response {
        status,
        headers,
        body,
    })
}

async fn write_all_counting<W: AsyncWriteExt + Unpin>(
    w: &mut W,
    bytes: &[u8],
    counters: &Arc<Counters>,
) -> std::io::Result<()> {
    counters
        .bytes
        .fetch_add(bytes.len() as u64, Ordering::SeqCst);
    w.write_all(bytes).await
}

async fn pump<R, W>(reader: &mut R, writer: &mut W, counters: &Arc<Counters>)
where
    R: AsyncReadExt + Unpin,
    W: AsyncWriteExt + Unpin,
{
    let mut buf = vec![0u8; 8 * 1024];
    loop {
        match reader.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => {
                counters.bytes.fetch_add(n as u64, Ordering::SeqCst);
                if writer.write_all(&buf[..n]).await.is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    let _ = writer.flush().await;
}
