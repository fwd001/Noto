//! §28「取消」那一格从"只有 PROBE"推成门禁。
//!
//! PROXY.md §7 原来写的是**实测结论**却没有门禁：「400 ms 预算下停滞服务器请求在 402–416 ms 内
//! 返回，说明取消路径真实生效（不是"等它回来再看"）」—— 那个数是某次手工 PROBE 打出来的，
//! 谁改坏了都不会红。这一文件把它变成会红的判据，并且加了一条 §7 后半句的判据：
//!
//! ```text
//! 一轮内重试预算 3 次，超出即让位给下一轮，避免"一轮卡死 5 分钟"。
//! ```
//!
//! 三格各自的判据与为什么必须是这个样子：
//!
//! 1. **挂死的请求要自己放手**：源站用 `hang_on` 永不回应，客户端必须在 `per_request` 之内报
//!    `NetError::Timeout`。两头都有界：下界证明它**真的等过**（不是连不上而秒失败 —— 那种坏法会
//!    让"超时"这一格绿在另一个错误上），上界证明它没有"等它回来再看"。
//! 2. **一轮挂起不许把总时长推过单次请求预算**：同一个挂死的源站，配上预算 3 的
//!    `send_with_retry`，客户端**仍然**在 `per_request` 处放手（实测 915–916 ms）。
//!    这条的**钝处也写在它自己的注释里**：拿两次变异试过"每次尝试各拿一份预算"那个形状，
//!    都没抓住（后续尝试被 reqwest 的连接池挡在服务器之外）—— 所以它钉的是"一轮不卡死"，
//!    不是"每次尝试各有多少预算"。台账与测试注释都按这个口径写，不写成三条全有牙。
//! 3. **放手不等于写了一半**：挂在 `PUT` 上被超时掉的写，服务器上**不许**出现那个对象。
//!    这条是数据安全侧的：一次取消若留下半份记录，下一轮就会把它当成"远端已有"。
//!
//! **墙钟判据的口径**（本仓的规矩：带时间的门禁要独占机器跑、要多轮）：预算取 900 ms，
//! 上下界留足余量（下界 300 ms、上界 2500 ms），既躲开调度抖动，又足以抓住"重试各发一份预算"
//! 那种 4× 的回归。台账里记的是**实测区间**，不是"应该 900 ms"。
//!
//! 跑法：`cargo test -p notera-net --test cancel_within_budget -- --test-threads=1`

use std::time::{Duration, Instant};

use notera_net::{
    HttpClient, HttpMethod, NetError, ProxyProfile, RequestSpec, RetryPolicy, Timeouts, TlsPolicy,
};
use notera_test_webdav::{Backend, Injection, TestServer};

/// 单次请求预算。
const BUDGET_MS: u64 = 900;
/// 下界：至少等过这么久才算"放手"，不是秒失败。
const LOWER_MS: u64 = 300;
/// 上界：再慢就算没取消。留了余量，但仍然远小于 4 次尝试的量级。
const UPPER_MS: u64 = 2500;

fn client() -> HttpClient {
    HttpClient::build(
        &ProxyProfile::direct(),
        &TlsPolicy::Strict,
        Timeouts::short(Duration::from_millis(BUDGET_MS)),
    )
    .expect("出口客户端")
}

/// 源站自己数到的"被我挂住的请求"条数（`/_control/inspect` 的 counters.hung）。
fn hung_count(srv: &TestServer) -> u64 {
    srv.inspect()["counters"]["hung"]
        .as_u64()
        .unwrap_or(u64::MAX)
}

fn path(name: &str) -> String {
    format!("/.notes/records/note/{name}")
}

/// ① 挂死的读必须在预算内自己放手，并且源站那边确实记到一次挂起。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hung_request_gives_up_inside_the_per_request_budget() {
    let srv = TestServer::start(Backend::Mem).await;
    let p = path("slow.json");
    srv.inject(Injection::hang_on("GET *slow.json")).await;

    let started = Instant::now();
    let err = client()
        .send(RequestSpec::new(
            HttpMethod::Get,
            format!("http://{}{p}", srv.addr()),
        ))
        .await
        .expect_err("服务器永不回应：这一条必须超时，不许挂着等");
    let took = started.elapsed().as_millis() as u64;
    // 打出来是给台账用的实测数（这一格的判据是区间，区间本身不含"实际是多少"）。
    eprintln!(
        "CANCEL① took={took}ms budget={BUDGET_MS}ms hung={}",
        hung_count(&srv)
    );

    assert!(
        matches!(err, NetError::Timeout),
        "放手是放手了，报的却是 {err:?} —— §8 那一行写的是超时分类"
    );
    assert!(
        (LOWER_MS..=UPPER_MS).contains(&took),
        "取消发生在 {took} ms（预算 {BUDGET_MS} ms，应在 {LOWER_MS}–{UPPER_MS} ms 内）：\
         太早=压根没等（那测的是连不上），太晚=在等它回来再看"
    );
    assert_eq!(
        hung_count(&srv),
        1,
        "源站记到的挂起数是 {} 而不是 1 —— 这条超时不是发生在那台挂死的服务器上",
        hung_count(&srv)
    );
    srv.stop().await;
}

/// ② 带预算 3 的重试**不会把一轮推过单次请求预算**。
///
/// 实测三遍都是 915–916 ms（预算 900 ms），且源站只挂起一次。
///
/// **这条的钝处照实记（两次变异都没抓住它）**：把循环顶上的 `left.is_zero()` 让位判断去掉、
/// 再把每次尝试的 `spec.timeout` 从"剩余"改成"整份预算"（= 我说的这个回归本来的形状），
/// `hung` 仍然是 1、耗时仍然 916 ms —— 因为第 2 次尝试根本没能走到源站：第一条挂起的连接被
/// reqwest 的连接池占着，后续尝试在连接层就死了（`Connect` 一类，同样是可重试的，所以循环照走、
/// 只是到不了服务器）。所以这一条能钉住的是"一轮挂起不会把总时长推过 per_request"，
/// **不能**钉住"每次尝试各拿一份预算"这种形状 —— 后者要另一条工装（每连接一份预算的回归需要在
/// 没有连接池的路径上测，或在 `decide()` 那侧统计尝试数）。这条按实写在台账里，不写成"三条全有牙"。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_retry_layer_cannot_outlive_the_per_request_budget() {
    let srv = TestServer::start(Backend::Mem).await;
    let p = path("slow.json");
    srv.inject(Injection::hang_on("GET *slow.json")).await;

    let started = Instant::now();
    let err = client()
        .send_with_retry(
            RequestSpec::new(HttpMethod::Get, format!("http://{}{p}", srv.addr())),
            &RetryPolicy::deterministic(50, 3),
        )
        .await
        .expect_err("服务器一直不回话：再多重试预算也救不了它");
    let took = started.elapsed().as_millis() as u64;
    eprintln!(
        "CANCEL② took={took}ms hung={} (retry budget 3)",
        hung_count(&srv)
    );

    assert!(matches!(err, NetError::Timeout), "{err:?}");
    assert_eq!(
        hung_count(&srv),
        1,
        "源站被挂起了 {} 次：每次尝试都各拿一份预算的话，一轮同步就能卡死好几分钟（PROXY.md §7）",
        hung_count(&srv)
    );
    assert!(
        took <= UPPER_MS,
        "带预算 3 的重试跑了 {took} ms，超过上限 {UPPER_MS} ms：总预算没管住单次尝试"
    );
    srv.stop().await;
}

/// ③ 被超时掉的写，服务器上不许留下痕迹。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancelled_write_leaves_no_trace_on_the_server() {
    let srv = TestServer::start(Backend::Mem).await;
    let p = path("half.json");
    srv.inject(Injection::hang_on("PUT *half.json")).await;

    let err = client()
        .send(
            RequestSpec::new(HttpMethod::Put, format!("http://{}{p}", srv.addr())).with_body(
                "{\"rev\":7,\"content\":\"half-written should leave nothing\"}"
                    .as_bytes()
                    .to_vec(),
            ),
        )
        .await
        .expect_err("PUT 挂死：客户端必须自己放手");
    assert!(matches!(err, NetError::Timeout), "{err:?}");

    let dump = srv.dump_prefix("/.notes").to_string();
    assert!(
        !dump.contains("half.json"),
        "取消之后服务器上出现了那个对象：下一轮会把它当成远端已有 —— 半份记录比没有更难收拾：{dump}"
    );
    assert_eq!(
        hung_count(&srv),
        1,
        "这次超时不是发生在挂死的 PUT 上（挂起数 {} ）",
        hung_count(&srv)
    );
    srv.stop().await;
}
