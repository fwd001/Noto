//! 服务器生命周期与连接处理：真 `TcpListener`、真 socket、无 mock。

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::net::TcpListener;
use tokio::sync::watch;

use crate::control::{self, Action};
use crate::handler;
use crate::handler::Reply;
use crate::http::{self, ReadError, Request};
use crate::inject::{Injection, InjectionCounters, LoggedRequest};
use crate::state::{BackendKind, Store};

/// 状态后端。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Backend {
    /// 纯内存。
    Mem,
    /// 权威状态在磁盘目录里；`restart()` 会**从磁盘重建**内存表，
    /// 于是"重启后数据仍在"是真的磁盘事实而不是"其实没重启"。
    Fs(PathBuf),
}

impl Backend {
    fn kind(&self) -> BackendKind {
        match self {
            Backend::Mem => BackendKind::Mem,
            Backend::Fs(p) => BackendKind::Fs(p.clone()),
        }
    }
}

/// `TestServer::start()` 的结果。
///
/// `Deref<Target = TestServer>` 让 `started.request_log()` / `started.reset()`
/// 这类写法直接可用；也要显式取句柄时用 [`Started::server`] / [`Started::into_server`]。
pub struct Started {
    pub addr: std::net::SocketAddr,
    pub base_url: String,
    server: TestServer,
}

impl Started {
    pub fn server(&self) -> &TestServer {
        &self.server
    }
    pub fn into_server(self) -> TestServer {
        self.server
    }
}

impl std::ops::Deref for Started {
    type Target = TestServer;
    fn deref(&self) -> &TestServer {
        &self.server
    }
}

/// 控制句柄（可克隆；克隆体指向同一个服务器）。
#[derive(Clone)]
pub struct TestServer {
    inner: Arc<Shared>,
}

/// 共享状态。字段对 crate 内可见（控制面要用）。
pub(crate) struct Shared {
    pub(crate) cell: Mutex<Cell>,
    /// true = 已停止接受新连接；接受循环据此退出并 drop 掉 listener。
    pub(crate) stopped: watch::Sender<bool>,
}

/// 一把锁保护的服务器状态。
pub(crate) struct Cell {
    pub(crate) backend: Backend,
    pub(crate) store: Store,
    pub(crate) injection: Injection,
    pub(crate) counters: InjectionCounters,
    pub(crate) log: Vec<LoggedRequest>,
    pub(crate) seq: u64,
    /// 已服务的**数据**请求数（控制面不计），驱动 `drop_after_n` / `reset_after`。
    pub(crate) served_data: usize,
    pub(crate) addr: std::net::SocketAddr,
    pub(crate) generation: u64,
    pub(crate) running: bool,
    pub(crate) accept: Option<tokio::task::JoinHandle<()>>,
    pub(crate) started_at: Instant,
}

fn lock(shared: &Arc<Shared>) -> MutexGuard<'_, Cell> {
    shared.cell.lock().expect("test-webdav 状态锁中毒")
}

impl TestServer {
    /// 起一个真实 TCP 服务器在 `127.0.0.1:0`。
    pub async fn start(backend: Backend) -> Started {
        let listener = match TcpListener::bind(("127.0.0.1", 0)).await {
            Ok(l) => l,
            Err(e) => panic!("无法绑定 127.0.0.1:0 —— {e}"),
        };
        let addr = listener.local_addr().expect("local_addr");
        let store = match Store::open(backend.kind()) {
            Ok(s) => s,
            Err(e) => panic!("后端初始化失败: {e}"),
        };
        let (stopped, _rx) = watch::channel(false);
        let shared = Arc::new(Shared {
            cell: Mutex::new(Cell {
                backend,
                store,
                injection: Injection::default(),
                counters: InjectionCounters::default(),
                log: Vec::new(),
                seq: 0,
                served_data: 0,
                addr,
                generation: 1,
                running: true,
                accept: None,
                started_at: Instant::now(),
            }),
            stopped,
        });
        let server = TestServer {
            inner: Arc::clone(&shared),
        };
        spawn_accept(listener, Arc::clone(&shared), addr);
        Started {
            addr,
            base_url: format!("http://{addr}"),
            server,
        }
    }

    /// 当前监听地址（`restart` 后不变）。
    pub fn addr(&self) -> std::net::SocketAddr {
        lock(&self.inner).addr
    }

    pub fn base_url(&self) -> String {
        format!("http://{}", self.addr())
    }

    pub fn backend(&self) -> Backend {
        lock(&self.inner).backend.clone()
    }

    pub fn fs_root(&self) -> Option<PathBuf> {
        match self.backend() {
            Backend::Mem => None,
            Backend::Fs(p) => Some(p),
        }
    }

    /// 安装注入配置；注入相关计数器归零 —— 两次同样配置的运行才可逐条比对。
    pub async fn inject(&self, i: Injection) {
        let mut c = lock(&self.inner);
        c.injection = i;
        c.counters = InjectionCounters::default();
        c.served_data = 0;
    }

    pub fn injection(&self) -> Injection {
        lock(&self.inner).injection.clone()
    }

    pub async fn clear_injection(&self) {
        self.inject(Injection::default()).await;
    }

    /// `RESET`：清空权威状态（注入与日志保留；日志用 [`TestServer::clear_log`] 清）。
    pub async fn reset(&self) {
        let mut c = lock(&self.inner);
        if let Err(e) = c.store.clear() {
            tracing::error!(?e, "reset 清空后端失败");
        }
        c.served_data = 0;
        c.counters = InjectionCounters::default();
    }

    pub async fn clear_log(&self) {
        let mut c = lock(&self.inner);
        c.log.clear();
        c.seq = 0;
    }

    /// `OFF(close-listener)`：停止接受新连接（已建立的连接不受影响）。
    pub async fn stop(&self) {
        let handle = {
            let mut c = lock(&self.inner);
            c.running = false;
            c.accept.take()
        };
        let _ = self.inner.stopped.send(true);
        if let Some(h) = handle {
            h.abort();
            let _ = h.await;
        }
    }

    /// `ON`：在同一地址重新监听。
    pub async fn start_server(&self) {
        if lock(&self.inner).running {
            return;
        }
        self.rebind().await;
    }

    /// `RESTART`：关监听 → 重建状态（Fs 模式**从磁盘 reload**）→ 同地址重新监听。
    pub async fn restart(&self) {
        self.stop().await;
        {
            let mut c = lock(&self.inner);
            if matches!(c.backend, Backend::Fs(_)) {
                if let Err(e) = c.store.reload() {
                    tracing::error!(?e, "restart reload 磁盘状态失败");
                }
            }
            c.generation += 1;
            c.served_data = 0;
            c.counters = InjectionCounters::default();
        }
        self.rebind().await;
    }

    async fn rebind(&self) {
        let addr = lock(&self.inner).addr;
        let mut last = None;
        for attempt in 0..80u32 {
            match TcpListener::bind(addr).await {
                Ok(listener) => {
                    let _ = self.inner.stopped.send(false);
                    spawn_accept(listener, Arc::clone(&self.inner), addr);
                    return;
                }
                Err(e) => {
                    last = Some(e);
                    tokio::time::sleep(Duration::from_millis(10 + u64::from(attempt) * 5)).await;
                }
            }
        }
        panic!("无法在同一地址 {addr} 重新监听（端口复用失败）: {last:?}");
    }

    /// 请求序列（`STATS` 的内存版）。控制面调用不进日志，因此可直接做计数断言。
    ///
    /// `status == 0` 表示"没有响应"（被丢弃 / 挂起 / 半上传切断）。
    pub fn request_log(&self) -> Vec<LoggedRequest> {
        lock(&self.inner).log.clone()
    }

    /// 服务端权威快照（`DUMP` 的内存版）—— 测试唯一允许的服务端状态断言手段。
    pub fn fs_dump(&self) -> serde_json::Value {
        let c = lock(&self.inner);
        control::dump_json(&c.store, None, c.generation, &c.backend)
    }

    /// 带 prefix 的权威快照。
    pub fn dump_prefix(&self, prefix: &str) -> serde_json::Value {
        let c = lock(&self.inner);
        control::dump_json(&c.store, Some(prefix), c.generation, &c.backend)
    }

    /// 注入 + 计数器 + 日志快照（`/_control/inspect` 的内存版）。
    pub fn inspect(&self) -> serde_json::Value {
        let c = lock(&self.inner);
        serde_json::json!({
            "addr": c.addr.to_string(),
            "backend": format!("{:?}", c.backend),
            "generation": c.generation,
            "running": c.running,
            "uptime_ms": c.started_at.elapsed().as_millis() as u64,
            "injection": c.injection,
            "counters": c.counters,
            "served_data": c.served_data,
            "requests": c.log,
        })
    }

    pub fn generation(&self) -> u64 {
        lock(&self.inner).generation
    }

    pub fn is_running(&self) -> bool {
        lock(&self.inner).running
    }
}

fn spawn_accept(listener: TcpListener, shared: Arc<Shared>, addr: std::net::SocketAddr) {
    let mut rx = shared.stopped.subscribe();
    let loop_shared = Arc::clone(&shared);
    let handle = tokio::spawn(async move {
        loop {
            tokio::select! {
                changed = rx.changed() => {
                    if changed.is_err() || *rx.borrow_and_update() { break; }
                }
                acc = listener.accept() => {
                    match acc {
                        Ok((stream, _peer)) => {
                            let s = Arc::clone(&loop_shared);
                            tokio::spawn(async move { serve_conn(s, stream).await; });
                        }
                        Err(e) => {
                            tracing::debug!(?e, "accept 失败");
                            tokio::time::sleep(Duration::from_millis(2)).await;
                        }
                    }
                }
            }
        }
        // listener 在此 drop —— 端口真的关掉了。
        drop(listener);
    });
    let mut c = lock(&shared);
    c.accept = Some(handle);
    c.running = true;
    c.addr = addr;
    c.started_at = Instant::now();
}

/// 单条连接：keep-alive 循环。
async fn serve_conn(shared: Arc<Shared>, stream: tokio::net::TcpStream) {
    let (mut r, mut w) = http::split(stream);
    // 该连接是否"经代理到达"：CONNECT 隧道 或 绝对形式请求目标。
    let mut proxied = false;
    loop {
        let truncate = lock(&shared).injection.truncate_upload_at;
        let req = match http::read_request(&mut r, truncate).await {
            Ok(Some(req)) => req,
            Ok(None) => break,
            Err(ReadError::TruncatedAfter(info)) => {
                // 半上传：读够 N 字节即断开。服务端从未拿到完整 body，
                // 因此不存在"半个有效文件"。
                let mut c = lock(&shared);
                c.counters.truncated += 1;
                c.served_data += 1;
                push_log(
                    &mut c,
                    &info.method,
                    if info.path.is_empty() { "?" } else { &info.path },
                    0,
                    info.bytes as u64,
                );
                break;
            }
            Err(e) => {
                tracing::debug!(?e, "报文解析失败，关闭连接");
                break;
            }
        };

        // CONNECT：隧道握手。回 200 后，本连接上的后续请求算"经代理到达"。
        if req.method == "CONNECT" {
            proxied = true;
            let headers = vec![("content-length".to_string(), "0".to_string())];
            if http::write_response(&mut w, &req.version, 200, &headers, &[], false)
                .await
                .is_err()
            {
                break;
            }
            continue;
        }
        if req.absolute_form {
            proxied = true;
        }

        // 控制面：永不受注入影响，也不进请求日志。
        if control::is_control(&req.path) {
            let (reply, action) = control::handle(&shared, &req);
            let close = req.wants_close();
            let io_bad = write(&mut w, &req, &reply).await.is_err();
            drop(reply);
            match action {
                Action::None => {}
                Action::Stop => server_of(&shared).stop().await,
                Action::Start => server_of(&shared).start_server().await,
                Action::Restart => server_of(&shared).restart().await,
            }
            if io_bad || close {
                break;
            }
            continue;
        }

        // 延迟注入：在锁外 sleep，别把别的连接一起卡住。
        let latency = lock(&shared).injection.latency_ms;
        if latency > 0 {
            tokio::time::sleep(Duration::from_millis(latency)).await;
        }

        match decide(&shared, &req, proxied) {
            Decision::Reply(reply) => {
                let close = req.wants_close();
                if write(&mut w, &req, &reply).await.is_err() || close {
                    break;
                }
            }
            Decision::Silent => {
                // `FAIL(hang)`：不回应、挂住连接，直到客户端超时放走。
                tokio::time::sleep(Duration::from_secs(3600)).await;
                break;
            }
            Decision::Drop => break,
        }
    }
}

/// 从 `Arc<Shared>` 造一个句柄（控制面端点需要 async 生命周期操作）。
fn server_of(shared: &Arc<Shared>) -> TestServer {
    TestServer {
        inner: Arc::clone(shared),
    }
}

enum Decision {
    Reply(Reply),
    /// 永不回应，挂住连接。
    Silent,
    /// 直接切断。
    Drop,
}

fn decide(shared: &Arc<Shared>, req: &Request, proxied: bool) -> Decision {
    let mut c = lock(shared);
    let inj = c.injection.clone();

    // FAIL(hang)
    if inj.timeout_all {
        c.counters.hung += 1;
        c.served_data += 1;
        push_log(&mut c, &req.method, &req.path, 0, req.body.len() as u64);
        return Decision::Silent;
    }
    // FAIL(abort)：第 n+1 个数据请求起直接断连。
    if let Some(n) = inj.drop_after_n {
        if c.served_data >= n {
            c.counters.connections_dropped += 1;
            c.served_data += 1;
            push_log(&mut c, &req.method, &req.path, 0, req.body.len() as u64);
            return Decision::Drop;
        }
    }
    if inj.require_proxy && !proxied {
        c.counters.rejections_403_not_proxied += 1;
    }

    let mut reply = handler::handle(&mut c.store, req, &inj, proxied);

    // FAIL(corrupt-body)：只改**响应**字节，落盘内容不动 —— 客户端 sha256 复算必失败。
    if inj.corrupt_manifest
        && reply.status == 200
        && !reply.body.is_empty()
        && (req.path.contains("manifest") || req.path.ends_with("index.json"))
    {
        let mid = reply.body.len() / 2;
        reply.body[mid] ^= 0xaa;
        c.counters.corrupted += 1;
    }

    c.served_data += 1;
    push_log(
        &mut c,
        &req.method,
        &req.path,
        reply.status,
        req.body.len() as u64,
    );

    // 服务端被清空（模拟运维误删 / 挂错目录）。
    if let Some(n) = inj.reset_after {
        if c.served_data >= n {
            if let Err(e) = c.store.clear() {
                tracing::error!(?e, "reset_after 清空失败");
            }
        }
    }
    Decision::Reply(reply)
}

async fn write(w: &mut http::Writer, req: &Request, reply: &Reply) -> std::io::Result<()> {
    let head_only = req.method == "HEAD";
    let mut headers: Vec<(String, String)> = Vec::with_capacity(reply.headers.len() + 1);
    let mut has_len = false;
    for (k, v) in &reply.headers {
        if k.eq_ignore_ascii_case("content-length") {
            has_len = true;
        }
        headers.push((k.clone(), v.clone()));
    }
    if !has_len && !matches!(reply.status, 204 | 304 | 205) {
        // HEAD 也报出 GET 会有的正文长度（只抑制正文字节）。
        headers.push(("content-length".into(), reply.body.len().to_string()));
    }
    http::write_response(
        w,
        &req.version,
        reply.status,
        &headers,
        &reply.body,
        head_only,
    )
    .await
}

fn push_log(c: &mut Cell, method: &str, path: &str, status: u16, bytes: u64) {
    c.seq += 1;
    let ts_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    c.log.push(LoggedRequest {
        seq: c.seq,
        method: method.to_string(),
        path: path.to_string(),
        status,
        bytes,
        ts_ms,
    });
}
