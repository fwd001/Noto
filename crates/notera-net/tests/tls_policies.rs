//! §28 最后一格「真 TLS 握手失败」，以及 `docs/PROXY.md` §6 四档策略与 §8 那一行
//! "`Tls` 分类、**不降级重试**、`! 证书不受信任`"的门禁。
//!
//! **此前一格都没有**：全仓每条测试都是 `TlsPolicy::Strict` 打在**明文 loopback** 上，
//! 四档里没有任何一档真做过握手。工装现在有了 `TlsOrigin`（每次运行现造一张自签 CA 与一张
//! 叶证书，盘上不落密钥），这一批才有判据可写。
//!
//! 判据的形状（这是本文件最要紧的一段）：**不读客户端自述，读源站的两本账**：
//!
//! * `accepted` —— TCP 连上来过几条（含握手当场失败的）；
//! * `handled` —— 握手成、且真读到一个请求并回应了的条数。
//!
//! 于是"证书不受信任"这一格的可证形态是 **`accepted ≥ 1` 而 `handled == 0`**：
//! 连上了、握手没成、**一个字节的应用数据都没交换**。而 §8 那句"不降级重试"的可证形态是
//! **`accepted == 1`**（配着 `send_with_retry` 预算 3 也一样只上一次）—— 要是哪天 TLS 失败被
//! 误当成可重试形态，这里会数出 4 条。
//!
//! 跑法：`cargo test -p notera-net --test tls_policies`
//!
//! **这批把缺口 G35 修掉了**（0.0.47）：`TlsPolicy::CaBundle` 原先走 reqwest 已废弃的
//! `add_root_certificate`，而 reqwest 0.13 只要见到用户加的根就把**整条校验**推给
//! `rustls_platform_verifier`（Windows 上报 `NTE_BAD_SIGNATURE` 而不是 `UnknownIssuer`）
//! ⇒ 内网自签端点填对 CA 也连不上。改走 `tls_certs_only` 之后校验留在 rustls 内，而"系统根"
//! 要我们自己读进来一起交（否则 §6 那句"追加"会变成"替换"）。根因、两侧判据与实测读数见
//! `client.rs` 的 `ca_bundle_roots` 与 `docs/PRODUCTION-READINESS.md` 的 G35 那一格。

use std::time::Duration;

use notera_net::{
    HttpClient, HttpMethod, NetError, ProxyProfile, RequestSpec, RetryPolicy, Timeouts, TlsPolicy,
};
use notera_test_webdav::TlsOrigin;

fn client(tls: &TlsPolicy) -> HttpClient {
    HttpClient::build(
        &ProxyProfile::direct(),
        tls,
        Timeouts::short(Duration::from_secs(10)),
    )
    .expect("出口客户端")
}

async fn get(tls: &TlsPolicy, url: &str) -> Result<u16, NetError> {
    client(tls)
        .send(RequestSpec::new(HttpMethod::Get, url))
        .await
        .map(|r| r.status)
}

/// ⓪ **工装自检**：两种链形状（自签根 / 两级私有 CA）在"跳过校验"下都必须真完成握手。
///
/// 这条是给 G35 的读数兜底的：如果我的 TLS 源站本身就完不成握手，那"CaBundle 失败"这件事
/// 一直在测我的生成器而不是产品的验证器 —— 那种结论会把用户往"换验证器"的方向带。
/// 它同时是"两级链那套生成代码有没有写错"的唯一自动检查（G35 的定性靠的是那台两级源站）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn both_chain_shapes_handshake_when_verification_is_skipped() {
    for (label, srv) in [
        ("自签根", TlsOrigin::start().await.expect("起自签根源站")),
        (
            "两级私有 CA",
            TlsOrigin::start_two_level().await.expect("起两级 CA 源站"),
        ),
    ] {
        let url = format!("{}/protocol.json", srv.base_url());
        let status = get(&TlsPolicy::InsecureLocal, &url)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "{label}：这台源站连跳过校验都完不成握手（{e}）—— \
                 那 G35 的所有读数测的都是工装，不是产品"
                )
            });
        assert_eq!(status, 200, "{label}：握手成了却没拿到 200");
        assert_eq!(
            srv.handled(),
            1,
            "{label}：源站没数到那条真请求，那这个 200 是别处来的"
        );
    }
}

/// ① `Strict` 对自签链：必须拒绝，而且**什么都没交换**。
///
/// 这一条守的是最坏的那种坏法：链校验被哪天不小心关掉（`danger_accept_invalid_certs` 之类），
/// 表现不是报错而是"同步照常成功"，而路上任何人都能改用户的笔记。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn strict_policy_rejects_a_self_signed_chain_without_exchanging_any_data() {
    let srv = TlsOrigin::start().await.expect("起 TLS 源站");
    let url = format!("{}/protocol.json", srv.base_url());

    let err = get(&TlsPolicy::Strict, &url)
        .await
        .expect_err("自签证书在 Strict 档必须被拒；这一条要哪天绿了，就是校验被关掉了");
    assert!(
        matches!(err, NetError::Tls),
        "拒是拒了，但报错了种类：{err:?} —— §8 那一行写的是 `Tls` 分类，\
         错种会被上层折成'稍后重试'，用户就永远查不到该换证书这件事"
    );
    assert!(
        srv.accepted() >= 1,
        "源站连一条 TCP 都没数到：那上面那句『握手被拒』是客户端自述，没有对照"
    );
    assert_eq!(
        srv.handled(),
        0,
        "TLS 校验失败之后应用层还交换了数据（{} 条）—— 那等于校验形同虚设",
        srv.handled()
    );
}

/// ② `Pin` 档不是"绕过 CA 校验的后门"。
///
/// §6 写的是：链校验照做，指纹只是**叠在上面的白名单**（"连接后比对"）。这一条钉的就是那句话 ——
/// 要是哪天有人为了让自签服务器过 `Pin` 而把链校验关掉（那正是"看起来更安全、其实形同虚设"的
/// 改法），这里会立刻从 `Tls` 变成 `Ok(200)` 而红。
///
/// 顺手钉两样：空指纹表是**配置错误**不是"不校验"；指纹不符同样 `Tls`。
///
/// **这一档的正腿没在这儿验**（"链可信 + 指纹相符 ⇒ 放行"）：那要一张系统信任库里就认得的证书，
/// 而我不往用户的系统信任库装测试 CA。见缺口 G35 与下面那条 `#[ignore]` 探针。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pin_is_not_a_way_to_bypass_chain_validation() {
    let srv = TlsOrigin::start().await.expect("起 TLS 源站");
    let url = format!("{}/protocol.json", srv.base_url());
    let leaf = srv.leaf_sha256().to_string();

    // 空表：配置错误要**在建客户端那一刻**就拒（比"发出去再失败"更早），不许当成"这一档不检查"。
    let empty = HttpClient::build(
        &ProxyProfile::direct(),
        &TlsPolicy::Pin(vec![]),
        Timeouts::short(Duration::from_secs(5)),
    );
    let e = empty.err().expect(
        "Pin 空表被当成了放行：建客户端就该失败（HttpClient 不是 Debug，这里只打是否有错）",
    );
    assert!(matches!(e, NetError::Protocol(_)), "空表的拒因不对：{e}");

    // 指纹相符，但链不可信 ⇒ 仍然必须失败（这一句是这条门禁的全部意义）。
    let matched = get(&TlsPolicy::Pin(vec![leaf]), &url)
        .await
        .expect_err("Pin 不许替链校验开门：相符的指纹 + 不受信的链，仍然要拒");
    assert!(
        matches!(matched, NetError::Tls),
        "受信链这一关被 Pin 绕过了：{matched:?}"
    );

    // 指纹不符 ⇒ 同样 Tls，且响应不交给上层。
    let bogus = "aa".repeat(32);
    let err = get(&TlsPolicy::Pin(vec![bogus]), &url)
        .await
        .expect_err("指纹不符必须拒绝 —— 否则中间人换一张证书就能过");
    assert!(matches!(err, NetError::Tls), "不符时没报 Tls：{err:?}");
    // 链校验在握手阶段就把连接断了 ⇒ 源站一条应用请求都不该收到。
    // 这顺带说明 `Pin` 的正腿（相符 ⇒ 放行）在链不可信时**根本走不到**，G35 的范围包括它。
    assert_eq!(
        srv.handled(),
        0,
        "源站收到了 {} 条真请求：那链校验就没在握手阶段断，上面两句的依据都不成立",
        srv.handled()
    );
}

/// ④ `InsecureLocal` 的"仅 loopback"不是一句注释：非 loopback 主机要在**发出任何连接之前**
/// 就被挡住。判据用差分：给一个必然解析不了的主机，若闸门生效报的是 `Tls`（策略拒绝），
/// 而漏了才会去解析并报 `Dns`。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn insecure_local_still_refuses_any_host_that_is_not_loopback() {
    assert!(
        TlsPolicy::InsecureLocal.permits_host("127.0.0.1")
            && TlsPolicy::InsecureLocal.permits_host("localhost")
            && TlsPolicy::InsecureLocal.permits_host("[::1]"),
        "loopback 形态被自己的判定挡了：那这一档在开发机上根本没法用"
    );
    assert!(
        !TlsPolicy::InsecureLocal.permits_host("dav.corp.example"),
        "非 loopback 被判成允许：那 danger_accept_invalid_certs 就是全局开关，\
         生产配置里一旦误用这档，任何人都能改用户的笔记"
    );

    let err = get(
        &TlsPolicy::InsecureLocal,
        "https://notera-tls-gate.invalid/.notes/protocol.json",
    )
    .await
    .expect_err("InsecureLocal 打到公网主机必须被拒");
    assert!(
        matches!(err, NetError::Tls),
        "报的是 {err:?}：出现 Dns/Connect 说明闸门没生效、请求真的发出去了"
    );

    // 而 loopback 上这一档确实能用（否则 §6 那行"仅 loopback"是个没人能走通的路径）。
    let srv = TlsOrigin::start().await.expect("起 TLS 源站");
    let url = format!("{}/protocol.json", srv.base_url());
    let status = get(&TlsPolicy::InsecureLocal, &url)
        .await
        .expect("loopback 上 InsecureLocal 应当放行");
    assert_eq!(status, 200);
}

/// ⑤ §8 那行"**不降级重试**"的数：TLS 失败不属于可重试形态，配着预算 3 的 `send_with_retry`
/// 也只该上一场。源站的 `accepted` 就是那个次数 —— 这条是防"把证书问题当成抖动"的退化：
/// 那种退化不会改错误码，只会让一轮多花几次握手，最后仍以同样的一句话报出去，
/// 所以只有数连接才看得见。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tls_failure_is_not_retried_even_with_a_full_budget() {
    let srv = TlsOrigin::start().await.expect("起 TLS 源站");
    let url = format!("{}/protocol.json", srv.base_url());
    let before = srv.accepted();

    let http = client(&TlsPolicy::Strict);
    let err = http
        .send_with_retry(
            RequestSpec::new(HttpMethod::Get, url),
            &RetryPolicy::deterministic(5, 3),
        )
        .await
        .expect_err("自签链在 Strict 档必须失败");
    assert!(matches!(err, NetError::Tls), "{err:?}");
    assert_eq!(
        srv.accepted() - before,
        1,
        "TLS 失败被重试了 {} 次：§8 写的是不降级重试（预算 3 也不许多上）",
        srv.accepted() - before
    );
    assert_eq!(srv.handled(), 0, "重试期间真的交换过应用数据");
}

/// ③ `CaBundle` 的**正腿**：这一档只认我递给它的那一份 CA，两种链形状都要真完成握手。
///
/// 为什么这条值一次改动：§6 把 `CaBundle` 写成"内网/警务网自签或私有 CA 的主路径"，而在此之前
/// 这一档在本机**从没握成过一次手**（缺口 G35）—— 也就是验收标准那句"WebDAV 同步可用"对
/// 自签端点是空的。判据两半，少一半都能撒谎：
///
/// * 递对 CA ⇒ 200 **并且源站数到 `handled == 1`**（客户端自述成功不算数）；
/// * 递**另一台**的 CA ⇒ 必须 `Tls` 且那台源站一条请求都收不到 —— 这一半守的是
///   "只要用户填了点什么就一律放行"那种退化（把校验关掉最容易写成这样）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ca_bundle_trusts_exactly_the_pem_it_was_given() {
    for (label, srv) in [
        ("自签根", TlsOrigin::start().await.expect("起自签根源站")),
        (
            "两级私有 CA",
            TlsOrigin::start_two_level().await.expect("起两级 CA 源站"),
        ),
    ] {
        let other = TlsOrigin::start().await.expect("起第二台（另一张 CA）");
        let url = format!("{}/protocol.json", srv.base_url());

        let status = get(&TlsPolicy::CaBundle(srv.ca_pem().to_string()), &url)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "{label}：递给 CaBundle 的就是这台源站的 CA，仍然握不成手（{e}）—— \
                     这一条就是缺口 G35 本身，§6 那条内网主路径是空的"
                )
            });
        assert_eq!(status, 200, "{label}：握手成了却没拿到 200");
        assert_eq!(
            srv.handled(),
            1,
            "{label}：客户端说成功，可源站一条请求都没数到 —— 那这个 200 不是从这台机器来的"
        );

        let Err(err) = get(&TlsPolicy::CaBundle(other.ca_pem().to_string()), &url).await else {
            panic!("{label}：递进去的是**另一台**的 CA 却仍然放行 —— 这一档变成了填了就信");
        };
        assert!(
            matches!(err, NetError::Tls),
            "{label}：错链的拒因不对（{err:?}）—— §8 那一行写的是 `Tls` 分类"
        );
        assert_eq!(
            other.handled(),
            0,
            "{label}：CA 不对的那台源站居然服务过请求 —— 信任锚没生效"
        );
        assert_eq!(
            srv.handled(),
            1,
            "{label}：换成错 CA 之后仍然拿到了服务 —— 校验被绕开"
        );
    }
}
