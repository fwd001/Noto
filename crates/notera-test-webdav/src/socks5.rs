//! 迷你 **SOCKS5 转发器**（RFC 1928 + RFC 1929 的一个够用子集）。
//!
//! 存在的理由：TEST-PLAN §28 里"SOCKS5 端到端"这一格此前是 **BLOCKED**，原因写得很具体 ——
//! 本工装的"代理"一直是**源站自己扮的**（它认识 CONNECT 与绝对形式请求行，但不认识 SOCKS
//! 握手）。于是产品里那条 `socks5` 通路从头到尾没有一条门禁走过：`reqwest` 的 `socks` feature
//! 有没有真打开、`ProxyProfile::socks5()` 造出来的 URL 能不能被认、`socks5h` 那支（让代理侧
//! 解析域名）走的是不是同一条路 —— 全都是"配置解析单测绿，没人真发过一个包"。
//!
//! 这一格要的红法与 HTTP 代理那条**不一样**，得说清楚：源站分不出"经 SOCKS5 到达"和"直连"
//! （SOCKS5 是裸 TCP 隧道，源站看到的请求行长一个样），所以 `Injection::require_proxy` 在这里
//! 用不上。这里的牙齿是**转发器自己的计数**：只有真走完 SOCKS5 握手的连接才会被记成一条隧道。
//! 哪天产品把 `socks5` 配置静默忽略掉去直连源站，源站照样回 200，而 `tunnels()` 停在 0 —— 红。
//!
//! 依赖与铁律：本 crate 不许依赖 `notera-net`/`reqwest`（见 `lib.rs` 头部），所以这里
//! 自己解析协议字节；真 TCP、真握手、真转发，不 mock。

use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// 握手里允许的最多方法数与域名长度：不是"防攻击"，是不让一个写错的长度字节把测试变成 OOM。
const MAX_METHODS: u8 = 32;
const MAX_DOMAIN: u8 = 253;

const VER: u8 = 0x05;
const CMD_CONNECT: u8 = 0x01;
const ATYP_IPV4: u8 = 0x01;
const ATYP_DOMAIN: u8 = 0x03;
const ATYP_IPV6: u8 = 0x04;
const M_NO_AUTH: u8 = 0x00;
const M_USER_PASS: u8 = 0x02;

/// 转发器看到的事实。判据只读这些数 —— 它们就是"这一腿真经代理走过了"的全部证据。
#[derive(Debug, Default)]
struct Counters {
    /// 完成握手并成功连上目标、开始转发的连接数。
    tunnels: AtomicU64,
    /// 问候阶段被拒：版本不是 05、方法列表里没有可接受的、或用户名/口令不对。
    handshake_rejects: AtomicU64,
    /// 用户名/口令**子协商**被拒（区别于"客户端压根没提供该方法的凭据"）。
    auth_rejects: AtomicU64,
    /// 两个方向合计转发的字节数（0 字节的隧道不算"把请求发出去了"）。
    bytes: AtomicU64,
    /// 客户端要求连的目标（最后一次）。域名形式记成 `host:port`，IP 形式记成 `ip:port`。
    last_target: std::sync::Mutex<String>,
}

/// 一个跑在 tokio 上的 SOCKS5 转发器。`shutdown()` 之后端口就释放。
pub struct Socks5Forwarder {
    addr: SocketAddr,
    counters: Arc<Counters>,
    stop: tokio::sync::watch::Sender<bool>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl Socks5Forwarder {
    /// 不要求认证（只接受方法 `00`）。
    pub async fn start() -> std::io::Result<Self> {
        Self::with_auth(None).await
    }

    /// 要求 RFC 1929 的用户名/口令：不匹配就拒，并且**一个字节都不往目标转**。
    pub async fn start_requiring(
        user: impl Into<String>,
        pass: impl Into<String>,
    ) -> std::io::Result<Self> {
        Self::with_auth(Some((user.into(), pass.into()))).await
    }

    async fn with_auth(creds: Option<(String, String)>) -> std::io::Result<Self> {
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
    /// 真走完握手并转发过字节的隧道数。
    pub fn tunnels(&self) -> u64 {
        self.counters.tunnels.load(Ordering::SeqCst)
    }
    pub fn handshake_rejects(&self) -> u64 {
        self.counters.handshake_rejects.load(Ordering::SeqCst)
    }
    pub fn auth_rejects(&self) -> u64 {
        self.counters.auth_rejects.load(Ordering::SeqCst)
    }
    pub fn bytes_forwarded(&self) -> u64 {
        self.counters.bytes.load(Ordering::SeqCst)
    }
    /// 客户端要求连的目标 —— 断"它确实是要去源站，而不是去别处"。
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

impl Drop for Socks5Forwarder {
    fn drop(&mut self) {
        // 忘了调 shutdown 也不要把任务漏在运行时里：发停止信号，让 accept 循环自己退。
        let _ = self.stop.send(true);
        if let Some(t) = self.task.take() {
            t.abort();
        }
    }
}

/// 一条连接：握手 → （可选）用户名/口令 → CONNECT → 双向拷贝。任何一步不对就断开，不转一个字节。
async fn serve(mut client: TcpStream, counters: Arc<Counters>, creds: Option<(String, String)>) {
    // --- 问候：VER, NMETHODS, METHODS... ---
    let mut head = [0u8; 2];
    if client.read_exact(&mut head).await.is_err() {
        return;
    }
    if head[0] != VER || head[1] == 0 || head[1] > MAX_METHODS {
        // 不是 SOCKS5 问候（例如把 HTTP CONNECT 打到了这个端口）：记一笔再走。
        counters.handshake_rejects.fetch_add(1, Ordering::SeqCst);
        return;
    }
    let mut methods = vec![0u8; head[1] as usize];
    if client.read_exact(&mut methods).await.is_err() {
        return;
    }
    let wants_password = creds.is_some() && methods.contains(&M_USER_PASS);
    let accepts_anon = methods.contains(&M_NO_AUTH) && creds.is_none();
    if creds.is_some() && !wants_password {
        // 配了凭据要求而客户端不肯用用户名/口令 —— 回"没有可接受的方法"。
        let _ = client.write_all(&[VER, 0xFF]).await;
        counters.handshake_rejects.fetch_add(1, Ordering::SeqCst);
        return;
    }
    if creds.is_none() && !accepts_anon {
        let _ = client.write_all(&[VER, 0xFF]).await;
        counters.handshake_rejects.fetch_add(1, Ordering::SeqCst);
        return;
    }
    if wants_password {
        let _ = client.write_all(&[VER, M_USER_PASS]).await;
        if !password_handshake(&mut client, &counters, creds.as_ref().unwrap()).await {
            return;
        }
    } else {
        let _ = client.write_all(&[VER, M_NO_AUTH]).await;
    }

    // --- CONNECT 请求：VER, CMD, RSV, ATYP, DST.ADDR, DST.PORT ---
    // 只有 4 个定长字节：ATYP **后面**就是地址，第 5 个字节属于地址。多读一个会把 IPv4 的第一个
    // 八位组吞掉，于是转发的目标是错位地址（这一版就是这样把三条腿都跑成超时的）。
    let mut req = [0u8; 4];
    if client.read_exact(&mut req).await.is_err() {
        return;
    }
    if req[0] != VER || req[1] != CMD_CONNECT {
        // 只实现 CONNECT；BIND/UDP 一律按"拒绝"回（0x07 = command not supported）。
        let _ = client
            .write_all(&[VER, 0x07, 0x00, ATYP_IPV4, 0, 0, 0, 0, 0, 0])
            .await;
        counters.handshake_rejects.fetch_add(1, Ordering::SeqCst);
        return;
    }
    let Some((candidates, asked)) = read_target(&mut client, req[3]).await else {
        let _ = client
            .write_all(&[VER, 0x01, 0x00, ATYP_IPV4, 0, 0, 0, 0, 0, 0])
            .await;
        counters.handshake_rejects.fetch_add(1, Ordering::SeqCst);
        return;
    };
    // 域名形式可能解析出多个地址（`localhost` → ::1 与 127.0.0.1，而工装里的源站只听 IPv4）。
    // 真代理也是这个做法：按解析顺序试到能连上的那一个。
    let mut upstream = None;
    for cand in &candidates {
        if let Ok(s) = TcpStream::connect(cand).await {
            upstream = Some(s);
            break;
        }
    }
    let Some(upstream) = upstream else {
        // 目标连不上：0x05 = connection refused。隧道没成，不算 tunnels。
        let _ = client
            .write_all(&[VER, 0x05, 0x00, ATYP_IPV4, 0, 0, 0, 0, 0, 0])
            .await;
        counters.handshake_rejects.fetch_add(1, Ordering::SeqCst);
        return;
    };
    if let Ok(mut g) = counters.last_target.lock() {
        // 记的是**客户端要的那个名字**（域名形式就是域名），不是解析后的 IP ——
        // 这一格区分得开 `socks5`（客户端自己解析，发 IPv4）与 `socks5h`（代理侧解析）。
        *g = asked;
    }
    // 成功回包：VER, REP=0, RSV, ATYP=1, BND.ADDR(4), BND.PORT(2)。地址给零即可，客户端不读它。
    let _ = client
        .write_all(&[VER, 0x00, 0x00, ATYP_IPV4, 0, 0, 0, 0, 0, 0])
        .await;
    counters.tunnels.fetch_add(1, Ordering::SeqCst);

    let (mut cr, mut cw) = client.into_split();
    let (mut ur, mut uw) = upstream.into_split();
    let a = Arc::clone(&counters);
    let b = Arc::clone(&counters);
    let t1 = tokio::spawn(async move { pump(&mut cr, &mut uw, &a).await });
    let t2 = tokio::spawn(async move { pump(&mut ur, &mut cw, &b).await });
    let _ = t1.await;
    let _ = t2.await;
}

/// RFC 1929 的子协商：`VER=01, ULEN, USER, PLEN, PASS` → `01 00` 或 `01 FF`。
async fn password_handshake(
    client: &mut TcpStream,
    counters: &Arc<Counters>,
    (user, pass): &(String, String),
) -> bool {
    let mut ver = [0u8; 1];
    if client.read_exact(&mut ver).await.is_err() || ver[0] != 0x01 {
        counters.handshake_rejects.fetch_add(1, Ordering::SeqCst);
        return false;
    }
    let Ok(ulen) = client.read_u8().await else {
        counters.handshake_rejects.fetch_add(1, Ordering::SeqCst);
        return false;
    };
    let mut uname = vec![0u8; ulen as usize];
    if client.read_exact(&mut uname).await.is_err() {
        counters.handshake_rejects.fetch_add(1, Ordering::SeqCst);
        return false;
    }
    let Ok(plen) = client.read_u8().await else {
        counters.handshake_rejects.fetch_add(1, Ordering::SeqCst);
        return false;
    };
    let mut pwd = vec![0u8; plen as usize];
    if client.read_exact(&mut pwd).await.is_err() {
        counters.handshake_rejects.fetch_add(1, Ordering::SeqCst);
        return false;
    }
    let ok = uname == user.as_bytes() && pwd == pass.as_bytes();
    let _ = client
        .write_all(&[0x01, if ok { 0x00 } else { 0xFF }])
        .await;
    if !ok {
        counters.auth_rejects.fetch_add(1, Ordering::SeqCst);
        counters.handshake_rejects.fetch_add(1, Ordering::SeqCst);
    }
    ok
}

/// 解析 `ATYP` 后面的地址 + 端口，返回（可试的目标地址列表, 客户端**要的那个名字**）。
/// 域名形式**由本转发器解析** —— 那正是 `socks5h` 的语义，与 `socks5`（客户端自己解析、
/// 发 IPv4/IPv6）是两条不同的路，各自都要有测试走到。
async fn read_target(client: &mut TcpStream, atyp: u8) -> Option<(Vec<SocketAddr>, String)> {
    match atyp {
        ATYP_IPV4 => {
            let mut octets = [0u8; 4];
            client.read_exact(&mut octets).await.ok()?;
            let mut port = [0u8; 2];
            client.read_exact(&mut port).await.ok()?;
            let addr = SocketAddr::new(IpAddr::V4(octets.into()), u16::from_be_bytes(port));
            Some((vec![addr], addr.to_string()))
        }
        ATYP_IPV6 => {
            let mut octets = [0u8; 16];
            client.read_exact(&mut octets).await.ok()?;
            let mut port = [0u8; 2];
            client.read_exact(&mut port).await.ok()?;
            let addr = SocketAddr::new(IpAddr::V6(octets.into()), u16::from_be_bytes(port));
            Some((vec![addr], addr.to_string()))
        }
        ATYP_DOMAIN => {
            let len = client.read_u8().await.ok()?;
            if len == 0 || len > MAX_DOMAIN {
                return None;
            }
            let mut buf = vec![0u8; len as usize];
            client.read_exact(&mut buf).await.ok()?;
            let host = String::from_utf8(buf).ok()?;
            let mut port = [0u8; 2];
            client.read_exact(&mut port).await.ok()?;
            let asked = format!("{host}:{}", u16::from_be_bytes(port));
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host(&asked).await.ok()?.collect();
            if addrs.is_empty() {
                return None;
            }
            Some((addrs, asked))
        }
        _ => None,
    }
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
