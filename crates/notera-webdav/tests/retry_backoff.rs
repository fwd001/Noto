//! §28「重试」那一格，以及 `docs/PROXY.md` §7 那两句承诺的端到端门禁：
//!
//! ```text
//! 重试只作用于幂等请求（GET/HEAD/PROPFIND）与可判定重放的写。
//! 一轮内重试预算 3 次，超出即让位给下一轮。
//! ```
//!
//! **这两句此前只有单测级别的证据**（`notera-net` 里退避算法自身、`retry_after_accepts_both_forms`），
//! 没有一条门禁看过"源站到底收到几次请求"。也就是说：`times`（规则命中次数）在工装文法里
//! **从未实现**，"两次 503 然后好"这个形态根本造不出来 —— 于是 §28 那一格长期只能记"部分覆盖"，
//! SY-FAULT-03/04 写着 `times=2 / times=99` 却按字面跑不起来（这条账本文 TEST-PLAN 记法表自己写过）。
//! 现在 `Injection` 的规则串支持 `pathglob#N`，这批门禁才写得出来。
//!
//! **判据打在哪儿**：打在**源站的请求日志**上（收到几条 GET / 几条 PUT），不是打在"重试函数被调用"。
//! 走的是产品公开入口 `fetch_protocol` / `provision_protocol` —— 启动期协商（SYNC-PROTOCOL §2）
//! 真调用的就是这两个方法，所以这条调用边是通的，不是我为测试另开的一条。
//!
//! **这条门禁钉的是哪一层，说清楚（不然会误以为它管住了全部）**：写"不被重放"有**两道**闸门 ——
//! ① 调用点选择（`put_cond` / `move_raw` / `delete_raw` 走 `send_once`，压根不进重试层）；
//! ② 重试层自己的护栏（`send_with_retry` 里 `if spec.idempotent { budget } else { 0 }`）。
//! 这里第三条（`a_conditional_write_is_never_replayed`）验的是 **①**。②是我先写成 M5 才发现的：
//! 把护栏整个摘掉，这一文件的三条**全绿** —— 因为条件写根本不路过它。护栏那一条的门禁另立在
//! `notera-net/tests/retry_idempotency.rs`（同注入、同预算，只变 `idempotent` 这一维看差分）。
//!
//! 退避时长用 `RetryPolicy::deterministic(5, budget)`：这里要验的是**次数与形状**；
//! 真实 2 s × 1.85^n 的口径由 `notera-net` 自己的单测钉着，不在这条门禁上重复计费。
//!
//! **注意这条量的是"重试层"，不是"产品这一轮"**：这里我自己 `with_retry` 给了预算 3。
//! 产品装配 `WebDavRemote` 时把政策硬编成 `deterministic(40, 1)`（40 ms、预算 1、无抖动），
//! 于是 §7 那两句在 shipped 路径上并不成立 —— 缺口 **G34**，它的整轮读数在
//! `notera-host/tests/retry_in_round.rs`（一次抖动挡得住、两次挡不住）。这条文件不替 G34 作证。
//!
//! 跑法：`cargo test -p notera-webdav --test retry_backoff`

use std::sync::Arc;
use std::time::Duration;

use notera_net::{HttpClient, ProxyProfile, RetryPolicy, Timeouts, TlsPolicy};
use notera_sync::RemoteError;
use notera_test_webdav::{Backend, Injection, TestServer};
use notera_webdav::{WebDavConfig, WebDavRemote};

const PROTO: &str = "/.notes/protocol.json";

fn http() -> Arc<HttpClient> {
    Arc::new(
        HttpClient::build(
            &ProxyProfile::direct(),
            &TlsPolicy::Strict,
            Timeouts::short(Duration::from_secs(10)),
        )
        .expect("出口客户端"),
    )
}

/// 用给定重试预算包一个远端句柄。`budget` 是**额外**尝试次数（PROXY.md §7 的"3 次"）。
fn remote(srv: &TestServer, budget: u32) -> WebDavRemote {
    WebDavRemote::new(WebDavConfig::new(srv.base_url()), http())
        .expect("合法配置")
        .with_retry(RetryPolicy::deterministic(5, budget))
}

/// 源站收到过几条打到这个 method+path 的请求 —— 这条门禁唯一可信的读数。
fn hits(srv: &TestServer, method: &str, path: &str) -> usize {
    srv.request_log()
        .iter()
        .filter(|r| r.method == method && r.path == path)
        .count()
}

fn protocol_doc() -> serde_json::Value {
    serde_json::json!({
        "protocol": 1,
        "root_id": "retry-backoff",
        "device_id": "01H9-retry",
    })
}

/// SY-FAULT-03：`FAIL(status=503, target=protocol.json, times=2)` —— 前两次 503、第三次放行，
/// 调用方**看不见**这次故障（读成功、内容正确），而源站日志里确实有 3 条 GET。
///
/// 两侧都要：只有"调用方成功"会放过一种坏情况 —— 客户端根本没重试，而是第 1 次就拿到了
/// 缓存/别的东西；只有"源站 3 条"会放过另一种 —— 重试了但错误仍冒到调用方，用户看到的是失败。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_injected_503s_on_a_read_are_absorbed_before_the_caller_sees_them() {
    let srv = TestServer::start(Backend::Mem).await;
    let r = remote(&srv, 3);
    r.provision_protocol(&protocol_doc())
        .await
        .expect("前置：protocol.json 要真的在服务器上");
    let before = hits(&srv, "GET", PROTO);

    srv.inject(Injection {
        status_for: vec![(format!("GET {PROTO}#2"), 503)],
        ..Default::default()
    })
    .await;

    let got = r
        .fetch_protocol()
        .await
        .expect("两次 503 之后的第三次成功不该冒到调用方");
    let doc = got.expect("protocol.json 存在，不该读成 None");
    assert_eq!(
        doc["root_id"].as_str(),
        Some("retry-backoff"),
        "重试是成了，读回来的却不是你写的那一份"
    );

    let after = hits(&srv, "GET", PROTO) - before;
    assert_eq!(
        after, 3,
        "源站侧应当看到 2 次 503 + 1 次放行 = 3 条 GET；实际 {after} 条"
    );
    srv.stop().await;
}

/// SY-FAULT-04：`FAIL(status=503, times=99)` —— 一直在坏。预算 3 ⇒ 总共 **4** 次尝试就收手，
/// 并把失败如实交给调用方（`RemoteError::Server`：链路通、服务器不回话，不是"离线"）。
///
/// 上限这一半是 INV-06 的字面判据（"请求总数 = 上限，不无限循环"）：少了说明没重试够，
/// 多了说明一轮能卡死 5 分钟 —— 两个都是用户能直接感觉到的坏。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retries_stop_at_the_budget_and_the_failure_reaches_the_caller() {
    let srv = TestServer::start(Backend::Mem).await;
    let r = remote(&srv, 3);
    r.provision_protocol(&protocol_doc())
        .await
        .expect("前置：protocol.json 要真的在服务器上");
    let before = hits(&srv, "GET", PROTO);

    srv.inject(Injection {
        status_for: vec![(format!("GET {PROTO}#99"), 503)],
        ..Default::default()
    })
    .await;

    let err = r
        .fetch_protocol()
        .await
        .expect_err("503 一直在，重试耗尽必须把失败交给调用方");
    let tried = hits(&srv, "GET", PROTO) - before;
    assert_eq!(
        tried, 4,
        "预算 3 ⇒ 恰好 4 次尝试（1 次原始 + 3 次重试）；实际 {tried} 次"
    );
    assert!(
        matches!(err, RemoteError::Server),
        "失败是到了，但报错了种类：{err:?} —— 503 该是 Server（服务器不回话），不是离线或不存在"
    );
    srv.stop().await;
}

/// PROXY.md §7 那句"重试只作用于幂等请求"的**另一半**：带条件头的写**不许**被自动重放。
/// `FAIL(status=503, target=protocol.json, times=99)` 打在创建语义的 `PUT`（`If-None-Match: *`）上，
/// 源站必须只看到 **1 条** PUT。
///
/// 为什么这一条比前两条更要紧：前两条坏在"用户多等一会儿"，这一条坏在**重复写用户数据** ——
/// 一次盲重放可能把 412 变成一次覆盖，或让远端多出一个半提交的对象。
/// 断言同时钉住"没有落盘"：注入的 503 不该有副作用。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_conditional_write_is_never_replayed() {
    let srv = TestServer::start(Backend::Mem).await;
    srv.inject(Injection {
        status_for: vec![(format!("PUT {PROTO}#99"), 503)],
        ..Default::default()
    })
    .await;
    let r = remote(&srv, 3);

    let err = r
        .provision_protocol(&protocol_doc())
        .await
        .expect_err("PUT 一直 503，创建必须失败");
    assert_eq!(
        hits(&srv, "PUT", PROTO),
        1,
        "条件写被自动重放了：源站看到 {} 条 PUT，PROXY.md §7 承诺的是 1 条",
        hits(&srv, "PUT", PROTO)
    );
    assert!(matches!(err, RemoteError::Server), "失败种类不对：{err:?}");
    let dump = srv.dump_prefix("/.notes").to_string();
    assert!(
        !dump.contains("protocol.json"),
        "注入的 503 却有副作用（服务器上出现了 protocol.json）：{dump}"
    );
    srv.stop().await;
}
