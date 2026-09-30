//! §28 里最后那格"代理错误密码"此前的形状：需要一个**会回 `407 Proxy-Authenticate` 的代理**，
//! 而工装的"代理"一直是源站自己扮的 —— 它认识的口令是**源站**口令（`Authorization`），不是
//! **代理**口令（`Proxy-Authorization`）。于是产品那一档"代理要求认证"从头到尾没有一条门禁走过：
//! 407 会不会被吞成"同步成功"、凭据会不会半路丢掉、被拒的时候会不会悄悄回退成直连。
//!
//! 工装现在有了：`notera_test_webdav::HttpForwardProxy`（真 TCP、真绝对形式解析、真转发，
//! 可选按 RFC 7235 要求 `Basic`）。
//!
//! **判据的形状，这里一次说清**（与 `proxy_routing.rs` 那条不同，别混）：真转发代理交给源站的是
//! origin-form 请求行，所以**源站分不出"经代理"与"直连"** —— 这一点与 SOCKS5 一样。推论：
//! `Injection::require_proxy` 不能与本工装同机用（它靠"看到绝对形式"判定，而那只在"源站扮演代理"
//! 的老夹具里成立；用它测真代理，测到的是我自己的写法）。这里的牙齿是**两边对照**：
//! 代理的 `forwarded()` / `auth_rejects()` vs 源站的 `request_log()`。被 407 挡住那一刻必须是
//! "代理记到一次拒绝 + 代理零转发 + **源站一条都没有**" —— 第三样才是"没偷偷直连"的证据。
//!
//! 跑法：`cargo test -p notera-webdav --test proxy_http_407`
use std::time::Duration;

use notera_net::{HttpClient, HttpMethod, ProxyProfile, RequestSpec, Timeouts, TlsPolicy};
use notera_test_webdav::{Backend, HttpForwardProxy, TestServer};

const USER: &str = "davuser";
const PASS: &str = "d4vpass";

fn client(profile: &ProxyProfile) -> HttpClient {
    HttpClient::build(
        profile,
        &TlsPolicy::Strict,
        Timeouts::short(Duration::from_secs(10)),
    )
    .expect("出口客户端")
}

/// 指向某个 HTTP 代理的配置。`pass = None` 是"配了代理而一个凭据都没带"那种形态
/// （等价于配置里的口令在 plumbing 里丢了），不是"随便省略的默认值"。
fn http_profile(port: u16, pass: Option<&str>) -> ProxyProfile {
    let mut p = ProxyProfile::http("127.0.0.1", port);
    if let Some(pass) = pass {
        p.username = Some(USER.to_string());
        p.password = Some(pass.to_string());
    }
    p
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

/// 源站收到过几条打到这个路径的请求。
fn origin_hits(srv: &TestServer, path: &str) -> usize {
    srv.request_log().iter().filter(|r| r.path == path).count()
}

/// 主腿：代理要求口令，而客户端给了正确口令 —— 必须成，并且是真的**经过**这个代理成的。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_proxy_that_demands_a_password_still_carries_the_request() {
    let srv = TestServer::start(Backend::Mem).await;
    let proxy = HttpForwardProxy::start_requiring(USER, PASS)
        .await
        .expect("起带 407 策略的 HTTP 转发代理");
    let url = format!("http://{}/.notes/proxy-407-proof.txt", srv.addr());

    let status = put(
        &client(&http_profile(proxy.port(), Some(PASS))),
        &url,
        b"right password",
    )
    .await
    .unwrap_or_else(|e| panic!("代理口令正确却失败：{e}"));
    assert!(
        (200..300).contains(&status),
        "配了正确的代理口令仍被拒：{status}"
    );

    // 三件一起才算"这趟真走了代理"：代理转发了、代理没拒、源站数到那一条。
    assert!(
        proxy.forwarded() >= 1,
        "请求成功而代理的转发数是 0 ⇒ 产品压根没把请求交给代理（或 reqwest 没把凭据发出去）"
    );
    assert_eq!(
        proxy.auth_rejects(),
        0,
        "正确口令被判成拒绝：那这条腿就没在测口令匹配"
    );
    assert_eq!(
        proxy.last_target(),
        srv.addr().to_string(),
        "代理被要求去的目标不是源站"
    );
    assert_eq!(
        origin_hits(&srv, "/.notes/proxy-407-proof.txt"),
        1,
        "源站没数到那一条 PUT"
    );
    let dumped = srv.dump_prefix("/.notes").to_string();
    assert!(
        dumped.contains("proxy-407-proof.txt"),
        "请求回 2xx 而服务器快照里没有那份字节：{dumped}"
    );
    proxy.shutdown().await;
    srv.stop().await;
}

/// 口令不对：必须在代理那一层断掉，**且不许源站收到任何东西**。
/// 这里最容易被糊过去的是"失败了，但失败在别处"，所以三条读数一起钉：
/// 代理记到一次拒绝、代理零转发、源站零请求 —— 第三条同时排除了"静默回退成直连"。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_wrong_proxy_password_is_refused_at_the_proxy_and_never_reaches_the_origin() {
    let srv = TestServer::start(Backend::Mem).await;
    let proxy = HttpForwardProxy::start_requiring(USER, PASS)
        .await
        .expect("带 407 策略的代理");
    let path = "/.notes/never-should-land.txt";
    let url = format!("http://{}{path}", srv.addr());

    let wrong = client(&http_profile(proxy.port(), Some("not-the-password")));
    // 失败可以是"传输层被断"也可以是"读到 407"，两种都算被拒；这里唯一不许的是它成功。
    if let Ok(status) = put(&wrong, &url, b"wrong password").await {
        assert!(
            !(200..300).contains(&status),
            "代理口令不对，请求居然成功（{status}）—— 那 407 就只是个数字"
        );
    }
    assert!(
        proxy.auth_rejects() >= 1,
        "失败了但代理没记成「口令被拒」⇒ 断在别处，这条判据没指到 407 那一格"
    );
    assert_eq!(proxy.forwarded(), 0, "被拒的那一条还是转给了源站");
    assert_eq!(
        origin_hits(&srv, path),
        0,
        "被 407 拒掉的请求出现在了源站 —— 产品把代理拒答当成'没代理'，静默回退成直连"
    );
    proxy.shutdown().await;
    srv.stop().await;
}

/// 凭据一个都不给（等价于"配置里有口令而出口那一路把它丢了"）：同样必须在代理那一层断。
/// 与上一条分开写，是因为两者的**修法**不同：口令错是用户的配置问题（界面要能显示原因），
/// 凭据丢失是产品的 plumbing 坏了（用户改什么都没用）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_proxy_profile_that_sends_no_credentials_is_refused_too() {
    let srv = TestServer::start(Backend::Mem).await;
    let proxy = HttpForwardProxy::start_requiring(USER, PASS)
        .await
        .expect("带 407 策略的代理");
    let path = "/.notes/no-creds.txt";
    let url = format!("http://{}{path}", srv.addr());

    let bare = client(&http_profile(proxy.port(), None));
    if let Ok(status) = put(&bare, &url, b"x").await {
        assert!(
            !(200..300).contains(&status),
            "一个凭据都没发，请求居然成功（{status}）"
        );
    }
    assert!(
        proxy.auth_rejects() >= 1,
        "没发凭据的腿必须被代理判成拒绝（不然这条与上一条是同一件事）"
    );
    assert_eq!(origin_hits(&srv, path), 0, "没凭据的请求落到了源站");
    proxy.shutdown().await;
    srv.stop().await;
}

/// 差分腿：同一个代理、同一个客户端配置，**只改代理自己的口令策略**（要 / 不要）。
/// 这一条存在的原因是：上面三条如果全部红在"代理一直拒"，也能凑出一份看起来合格的账。
/// 只有"去掉 407 策略后同一条腿立刻成"才证明那些红是被策略挡的，不是被工装挡的。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_same_request_goes_through_once_the_proxy_stops_demanding_a_password() {
    let srv = TestServer::start(Backend::Mem).await;
    let path = "/.notes/differential.txt";
    let url = format!("http://{}{path}", srv.addr());

    // ① 要口令而客户端不给 —— 与上一条同形态，这里作为差分的"拒"腿。
    let strict = HttpForwardProxy::start_requiring(USER, PASS)
        .await
        .expect("带 407 策略的代理");
    let bare = client(&http_profile(strict.port(), None));
    if let Ok(status) = put(&bare, &url, b"x").await {
        assert!(
            !(200..300).contains(&status),
            "差分的'拒'腿居然回 {status} —— 代理的 407 策略没生效，后面那句'红是被策略挡的'就没根据"
        );
    }
    let rejected_hits = origin_hits(&srv, path);
    let rejected_forwards = strict.forwarded();

    // ② 同一个源站、同一份客户端配置，只把代理的策略换成"不要口令" —— 必须成。
    let open = HttpForwardProxy::start().await.expect("不要口令的代理");
    let status = put(
        &client(&http_profile(open.port(), None)),
        &url,
        b"now it is allowed",
    )
    .await
    .unwrap_or_else(|e| panic!("差分的'通'腿失败：{e}"));
    assert!(
        (200..300).contains(&status),
        "代理不要口令后同一条腿仍失败（{status}）⇒ 前面那些红是工装挡的，不是策略挡的"
    );
    assert!(open.forwarded() >= 1, "'通'腿没有真的转发");
    assert_eq!(
        origin_hits(&srv, path),
        rejected_hits + 1,
        "'拒'腿记到的源站请求数应当是 0，且'通'腿恰好 +1：实测 {rejected_hits} → {}",
        origin_hits(&srv, path)
    );
    assert_eq!(rejected_forwards, 0, "'拒'腿记到了转发数");

    strict.shutdown().await;
    open.shutdown().await;
    srv.stop().await;
}
