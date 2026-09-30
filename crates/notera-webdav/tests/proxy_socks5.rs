//! §28 里那条一直 **BLOCKED** 的"SOCKS5 端到端"。
//!
//! 之前的状态很尴尬：`ProxyProfile::socks5()` 有配置解析单测、`proxy_url()` 有构造单测、
//! 界面设置页也真给用户提供 SOCKS5 那一档 —— 但**没有一个测试真发过一个走 SOCKS5 的包**。
//! 工装里的"代理"一直是源站自己扮的（它认识 CONNECT 与绝对形式请求行，不认识 SOCKS 握手），
//! 所以那条通路是"实现齐全、单测全绿、没人调用"这个老形状的又一例。
//! 解除条件就是 TEST-PLAN 写的那件东西：一个会答 `05 00` 并真转发的迷你 SOCKS5 应答器，
//! 代理与源站分开 —— 现在有了（`notera_test_webdav::Socks5Forwarder`）。
//!
//! 判据的形状与 HTTP 代理那条（`proxy_routing.rs`）**不一样，这里说清为什么**：
//! SOCKS5 是裸 TCP 隧道，源站看到的请求行与直连完全一样，所以 `require_proxy` 在这里用不上。
//! 这里的牙齿是转发器自己的计数 —— 只有真完成握手（并按需过了用户名/口令子协商）的连接才会
//! 被记成一条隧道。哪天 reqwest 的 `socks` feature 掉了、或产品把 SOCKS5 配置静默忽略去直连，
//! 源站照样回 200 而 `tunnels()` 停在 0 ⇒ 红。
//!
//! 跑法：`cargo test -p notera-webdav --test proxy_socks5`
use std::time::Duration;

use notera_net::{HttpClient, HttpMethod, ProxyProfile, RequestSpec, Timeouts, TlsPolicy};
use notera_test_webdav::{Backend, Socks5Forwarder, TestServer};

fn client(profile: &ProxyProfile) -> HttpClient {
    HttpClient::build(
        profile,
        &TlsPolicy::Strict,
        Timeouts::short(Duration::from_secs(10)),
    )
    .expect("出口客户端")
}

async fn put(http: &HttpClient, url: &str, body: &[u8]) -> Result<u16, String> {
    match http
        .send(RequestSpec::new(HttpMethod::Put, url).with_body(body.to_vec()))
        .await
    {
        Ok(r) => Ok(r.status),
        Err(e) => Err(e.to_string()),
    }
}

fn socks_profile(port: u16, user: Option<&str>, pass: Option<&str>) -> ProxyProfile {
    let mut p = ProxyProfile::socks5("127.0.0.1", port);
    p.username = user.map(str::to_string);
    p.password = pass.map(str::to_string);
    p
}

/// 主腿：配了 SOCKS5 就必须真经隧道过去，而且源站确实收到了那一条。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_socks5_tunnel_really_carries_the_request() {
    let srv = TestServer::start(Backend::Mem).await;
    let proxy = Socks5Forwarder::start().await.expect("起 SOCKS5 转发器");
    let origin = srv.addr();
    let url = format!("http://{origin}/.notes/socks-proof.txt");

    let http = client(&socks_profile(proxy.port(), None, None));
    let status = put(&http, &url, b"through socks5")
        .await
        .unwrap_or_else(|e| panic!("经 SOCKS5 的 PUT 失败了：{e}"));
    assert!(
        (200..300).contains(&status),
        "经 SOCKS5 写被源站拒掉：{status}"
    );

    // 这两行才是本格的牙齿：请求成功了，还要证明它是**经隧道**成功的。
    assert!(
        proxy.tunnels() >= 1,
        "请求成功而 SOCKS5 隧道数是 0 ⇒ 产品压根没把请求交给代理（直连源站），SOCKS5 配置被静默忽略"
    );
    assert!(
        proxy.bytes_forwarded() > 0,
        "隧道开了而零字节转发 ⇒ 转发的不是这条请求"
    );
    assert_eq!(
        proxy.last_target(),
        origin.to_string(),
        "代理被要求去的目标不是源站"
    );
    let seen = srv
        .request_log()
        .iter()
        .filter(|r| r.method == "PUT" && r.path == "/.notes/socks-proof.txt")
        .count();
    assert_eq!(seen, 1, "源站没数到那一条 PUT：{seen}");

    // 内容真在服务器上（读回来），不是"隧道开了但没人写进去"。
    let dumped = srv.dump_prefix("/.notes").to_string();
    assert!(
        dumped.contains("socks-proof.txt"),
        "隧道过了而服务器快照里没有那份字节：{dumped}"
    );
    proxy.shutdown().await;
    srv.stop().await;
}

/// 用户名/口令那一档：口令不对必须在握手阶段就断，**一个字节都不许到源站**。
/// 这是 §28 里"407 错误密码"那一格在 SOCKS5 侧的等价物（RFC 1929 的子协商）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn socks5_password_mismatch_stops_before_the_origin_sees_anything() {
    let srv = TestServer::start(Backend::Mem).await;
    let proxy = Socks5Forwarder::start_requiring("davuser", "d4vpass")
        .await
        .expect("起带认证的 SOCKS5 转发器");
    let origin = srv.addr();
    let good = format!("http://{origin}/.notes/good.txt");
    let bad = format!("http://{origin}/.notes/bad.txt");

    // ① 口令对：必须成，并且记一条隧道。
    let ok = client(&socks_profile(
        proxy.port(),
        Some("davuser"),
        Some("d4vpass"),
    ));
    let status = put(&ok, &good, b"right password")
        .await
        .unwrap_or_else(|e| panic!("口令正确却失败：{e}"));
    assert!((200..300).contains(&status), "口令正确仍被拒：{status}");
    assert_eq!(proxy.auth_rejects(), 0, "正确口令被判成拒绝");
    let tunnels_after_good = proxy.tunnels();
    assert!(tunnels_after_good >= 1);

    // ② 口令错：必须失败，且失败发生在**到源站之前**。
    let wrong = client(&socks_profile(
        proxy.port(),
        Some("davuser"),
        Some("wrong-password"),
    ));
    let err = put(&wrong, &bad, b"wrong password")
        .await
        .expect_err("口令不对居然成功了 —— 要么转发器没在验，要么客户端没真用凭据");
    assert!(
        proxy.auth_rejects() >= 1,
        "失败了但没记成「口令被拒」⇒ 断在别处，这条判据就没指到那一格：{err}"
    );
    assert_eq!(
        proxy.tunnels(),
        tunnels_after_good,
        "口令不对还是开成了隧道"
    );
    let reached = srv
        .request_log()
        .iter()
        .filter(|r| r.path == "/.notes/bad.txt")
        .count();
    assert_eq!(
        reached, 0,
        "被拒的那一条居然到了源站 —— 那「认证」就只是装饰"
    );
    proxy.shutdown().await;
    srv.stop().await;
}

/// `socks5` 与 `socks5h` 是两条不同的路（谁去解析主机名），必须各自走通、且**可区分**。
///
/// 两腿用的 URL 形式不同，这一条按实写清原因：`socks5h` 交给代理的是**主机名**（这正是要验的那件事），
/// 而 `socks5` 那一腿客户端自己解析 —— 这台机器上 `localhost` 先给 `::1`，而工装的源站只听 IPv4，
/// 于是那条腿会红在"连不上"上，而那**是夹具的绑定属性，不是产品路径**。所以 `socks5` 这一腿
/// 用 IP 字面量的 URL：它要证的仍然是"代理收到的是已经解析好的 IP 形式，不是名字"。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn socks5h_lets_the_proxy_resolve_the_host_while_socks5_does_not() {
    let srv = TestServer::start(Backend::Mem).await;
    let origin = srv.addr();

    // socks5h：客户端把主机名交给代理 → 代理侧解析 → 记到的目标是**那个名字**。
    let named = Socks5Forwarder::start().await.expect("转发器");
    let mut remote_dns = ProxyProfile::socks5("127.0.0.1", named.port());
    remote_dns.resolve_remote_dns = true;
    let url = format!("http://localhost:{}/.notes/by-name.txt", origin.port());
    let status = put(&client(&remote_dns), &url, b"proxy resolves")
        .await
        .unwrap_or_else(|e| panic!("socks5h 这一腿失败：{e}"));
    assert!((200..300).contains(&status), "socks5h 被拒：{status}");
    assert!(named.tunnels() >= 1, "socks5h 没经隧道");
    assert_eq!(
        named.last_target(),
        format!("localhost:{}", origin.port()),
        "socks5h 声称由代理解析，而代理收到的却是解析好的 IP（那等于没走这条分支）"
    );

    // socks5（客户端自己解析）：代理收到的是 IP 形式，与上面**必须不同**。
    let ipish = Socks5Forwarder::start().await.expect("转发器");
    let mut local_dns = ProxyProfile::socks5("127.0.0.1", ipish.port());
    local_dns.resolve_remote_dns = false;
    let url2 = format!("http://{origin}/.notes/by-ip.txt");
    let status2 = put(&client(&local_dns), &url2, b"client resolves")
        .await
        .unwrap_or_else(|e| panic!("socks5 这一腿失败：{e}"));
    assert!((200..300).contains(&status2), "socks5 被拒：{status2}");
    assert!(ipish.tunnels() >= 1, "socks5 没经隧道");
    assert_eq!(
        ipish.last_target(),
        origin.to_string(),
        "socks5 这一腿代理收到的目标形式不对"
    );
    assert_ne!(
        named.last_target(),
        ipish.last_target(),
        "两腿给出同一个目标形式 ⇒ 代理侧/客户端侧解析这条边没被走到"
    );
    // **一条按实记的钝处**：这一格本来想再钉一句"`resolve_remote_dns` 翻，交给 reqwest 的
    // scheme 就翻"。拿变异 **M117**（把两档合成一档，永远输出 `socks5h`）试过：上面那两条腿
    // **照样绿** —— 因为两腿的 URL 主机形式本来就不同（一条给名字、一条给 IP 字面量），
    // `assert_ne!` 的差可以完全来自 URL 而来自不了那个布尔。那是一条恒真的差分，写它不如不写。
    // 也不能在这里直接读 `proxy_url()`：它是 `pub(crate)`，**故意的** —— 那串里带凭据。
    // 更不能拿 `describe()` 顶：那是同一个决定的**第二处实现**，它绿不代表 URL 对（两处互相兜底
    // 正是本仓库反复踩过的形状）。所以这一档的真门在 `notera-net` 自己的单测里：
    // `proxy::tests::proxy_url_carries_credentials_and_scheme` —— M117 在那儿是红的，实测过。
    // 这里两腿各自证的是另一半：**两种 scheme 真的都能把包经 SOCKS5 隧道送出去**。

    named.shutdown().await;
    ipish.shutdown().await;
    srv.stop().await;
}

/// 代理端口在、但后面那座"代理"连不上源站：必须失败并给出可显示的错，
/// 不许把失败吞成"成功但没同步"。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_socks5_proxy_that_cannot_reach_the_origin_fails_loudly() {
    let srv = TestServer::start(Backend::Mem).await;
    let origin_port = srv.addr().port();
    // 转发器先起，然后**关掉源站**：隧道握手会成功，CONNECT 阶段连不上目标。
    let proxy = Socks5Forwarder::start().await.expect("转发器");
    srv.stop().await;
    let url = format!("http://127.0.0.1:{origin_port}/.notes/gone.txt");
    let err = put(
        &client(&socks_profile(proxy.port(), None, None)),
        &url,
        b"x",
    )
    .await
    .expect_err("源站已经不在了，这一腿必须失败");
    assert!(
        !err.is_empty(),
        "失败了却没给出任何原因 —— 界面只能显示通用兜底"
    );
    assert_eq!(
        proxy.tunnels(),
        0,
        "目标连不上却记成了隧道：隧道数就不再是「经代理成功」的证据了"
    );
    proxy.shutdown().await;
}
