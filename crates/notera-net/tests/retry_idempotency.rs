//! `notera-net` 重试层**幂等护栏**自己的门禁（PROXY.md §7 第一句、SYNC-PROTOCOL §11.1）。
//!
//! ```text
//! 重试只作用于幂等请求 + 显式标记 idempotent 的写
//! ```
//!
//! 这批门禁是被**变异测试逼出来的**：`notera-webdav/tests/retry_backoff.rs` 那条"条件写不被重放"
//! 绿着扛住了 M5（把 `budget = if spec.idempotent { … } else { 0 }` 改成无条件给预算）。原因不是
//! 那条测试写错了，而是它盯的是**调用点的选择**（`put_cond` 走 `send_once`，压根没进重试层），
//! 而这一层的护栏当时**一条门禁都没有** —— 摘掉它，全仓没有任何读数会变。
//!
//! 护栏这条路的判据只能是**差分**：同一份 503 注入、同一个预算，只有 `spec.idempotent` 这一维变，
//! 源站收到的请求条数必须不同。三条各钉一格：
//! * GET（天然幂等）⇒ 预算放开，4 条；
//! * MOVE（天然非幂等）⇒ **1 条**，一次都不许多发；
//! * POST 显式标 `idempotent = true`（§11.1 那种"内容哈希可复验的写"）⇒ 预算放开，3 条。
//!
//! 为什么 MOVE 那一格要紧：一次盲重放可能就是"源站被搬了两次"或"目标已存在"的假冲突，
//! 而它在客户端看起来都像一次普通的同步失败 —— 数据安全优先级排在体验之前。
//!
//! 跑法：`cargo test -p notera-net --test retry_idempotency`

use std::time::Duration;

use notera_net::{
    HttpClient, HttpMethod, ProxyProfile, RequestSpec, RetryPolicy, Timeouts, TlsPolicy,
};
use notera_test_webdav::{Backend, Injection, TestServer};

const TARGET: &str = "/.notes/retry-guard";

fn client() -> HttpClient {
    HttpClient::build(
        &ProxyProfile::direct(),
        &TlsPolicy::Strict,
        Timeouts::short(Duration::from_secs(10)),
    )
    .expect("出口客户端")
}

/// 打到 `TARGET` 的请求条数 —— 判据落在源站的账上，不是"重试函数被调用过"。
fn hits(srv: &TestServer, method: &str) -> usize {
    srv.request_log()
        .iter()
        .filter(|r| r.method == method && r.path == TARGET)
        .count()
}

/// 天然幂等的读：预算放开 ⇒ 1 次原始 + 3 次重试 = 4 条，且最后一条仍是 503（错误如实交回）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_idempotent_read_gets_the_whole_budget() {
    let srv = TestServer::start(Backend::Mem).await;
    srv.inject(Injection {
        status_for: vec![(format!("GET {TARGET}#99"), 503)],
        ..Default::default()
    })
    .await;
    let http = client();
    let spec = RequestSpec::new(HttpMethod::Get, format!("http://{}{}", srv.addr(), TARGET));
    let resp = http
        .send_with_retry(spec, &RetryPolicy::deterministic(5, 3))
        .await
        .expect("503 是响应不是传输失败，应原样交回");
    assert_eq!(resp.status, 503);
    assert_eq!(
        hits(&srv, "GET"),
        4,
        "预算 3 却发了 {} 条：这一层的重试次数与 PROXY.md §7 不符",
        hits(&srv, "GET")
    );
    srv.stop().await;
}

/// 护栏本体：非幂等的 MOVE 在**同样**的 503、**同样**的预算下必须一条不多。
/// M5（把 `spec.idempotent` 的判断摘掉）就是红在这里。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_non_idempotent_move_is_never_replayed_by_the_retry_layer() {
    let srv = TestServer::start(Backend::Mem).await;
    srv.inject(Injection {
        status_for: vec![(format!("MOVE {TARGET}#99"), 503)],
        ..Default::default()
    })
    .await;
    let http = client();
    let mut spec = RequestSpec::new(HttpMethod::Move, format!("http://{}{}", srv.addr(), TARGET));
    spec = spec.with_header(
        "destination",
        format!("http://{}/.notes/retry-dest", srv.addr()),
    );
    assert!(
        !spec.idempotent,
        "前置：MOVE 必须是天然非幂等的，否则这条门禁在测别的东西"
    );
    let resp = http
        .send_with_retry(spec, &RetryPolicy::deterministic(5, 3))
        .await
        .expect("503 原样交回");
    assert_eq!(resp.status, 503);
    assert_eq!(
        hits(&srv, "MOVE"),
        1,
        "重试层把非幂等的写重放了 {} 次 —— 幂等护栏失效，一次同步可能搬两趟源站",
        hits(&srv, "MOVE")
    );
    srv.stop().await;
}

/// 差分那一维要真的是 `spec.idempotent`，不是"方法名在不在读列表里"：
/// POST 显式标成幂等（§11.1 的"内容哈希可复验的写"）⇒ 预算放开。
/// 第 3 次注入额度用完，请求真进了处理器，所以最终状态**不是** 503。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_write_marked_idempotent_is_the_one_lever_that_opens_the_budget() {
    let srv = TestServer::start(Backend::Mem).await;
    srv.inject(Injection {
        status_for: vec![(format!("POST {TARGET}#2"), 503)],
        ..Default::default()
    })
    .await;
    let http = client();
    let mut spec = RequestSpec::new(HttpMethod::Post, format!("http://{}{}", srv.addr(), TARGET));
    assert!(!spec.idempotent, "前置：POST 默认不是幂等的");
    spec.idempotent = true;
    let resp = http
        .send_with_retry(spec, &RetryPolicy::deterministic(5, 3))
        .await
        .expect("POST 有响应");
    assert_eq!(
        hits(&srv, "POST"),
        3,
        "标了 idempotent 却没被重放（{} 条）：这一层认的是方法名而不是这个标记",
        hits(&srv, "POST")
    );
    assert_ne!(
        resp.status, 503,
        "第 3 次没打到处理器上（注入额度没用尽？）"
    );
    srv.stop().await;
}
