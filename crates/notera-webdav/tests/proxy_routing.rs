//! §28「代理故障注入」里此前**一条都没有进门禁**的那半：产品到底有没有真的把请求发给代理。
//!
//! PROXY.md §9 写的三条证据（差分测试 / 服务器只接受经代理到达的连接 / 脱敏审计）此前
//! 只以"手工跑过一次 `notera-cli net probe`"的形式存在 —— 那是 PROBE，不是回归门禁：
//! 任何一次改动把代理静默吞掉，全量测试都不会红。这一条把它变成会红的东西。
//!
//! 判据是**差分**的，这是唯一能证"用了代理"的形态：同一台服务器挂上"只接受经代理到达"
//! 的策略之后，配了代理必须成、没配代理必须 403。若产品偷偷忽略代理配置，第二条腿会红。
//!
//! 跑法：`cargo test -p notera-webdav --test proxy_routing`

use std::time::Duration;

use notera_net::{HttpClient, HttpMethod, ProxyProfile, RequestSpec, Timeouts, TlsPolicy};
use notera_test_webdav::{Backend, Injection, TestServer};

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

async fn get(http: &HttpClient, url: &str) -> Result<u16, String> {
    match http.send(RequestSpec::new(HttpMethod::Get, url)).await {
        Ok(r) => Ok(r.status),
        Err(e) => Err(e.to_string()),
    }
}

/// 一台只接受"经代理到达"的服务器：配了 HTTP 代理必须成，直连必须 403。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_http_proxy_route_is_the_only_way_through() {
    let srv = TestServer::start(Backend::Mem).await;
    srv.inject(Injection {
        require_proxy: true,
        ..Default::default()
    })
    .await;
    let (host, port) = (srv.addr().ip().to_string(), srv.addr().port());
    let url = format!("http://{host}:{port}/.notes/proxy-proof.txt");

    // ① 走代理：reqwest 对 http:// 目标发的是**绝对形式**请求行，服务器据此判定 proxied。
    let via_proxy = client(&ProxyProfile::http(&host, port));
    let wrote = put(&via_proxy, &url, b"through the proxy")
        .await
        .expect("经代理的请求不该失败");
    assert!(
        (200..300).contains(&wrote),
        "配了 HTTP 代理却被服务器拒掉：{wrote}"
    );
    let read_back = get(&via_proxy, &url).await.expect("读回也要经代理");
    assert_eq!(read_back, 200, "经代理写进去的东西读不回来：{read_back}");

    // ② 直连同一台服务器：必须被拒。**这一腿才是整条测试的牙齿** ——
    //    如果哪天产品把代理配置静默忽略掉（或直接连源站），上面那条还会绿，这条会红。
    let direct = client(&ProxyProfile::direct());
    let refused = get(&direct, &url).await.unwrap_or_else(|e| {
        panic!("直连应当拿到 403 而不是传输失败：{e}");
    });
    assert_eq!(
        refused, 403,
        "直连居然成功了 —— 要么服务器没在挡，要么请求没按我们以为的方式发出去"
    );
    let rejected = srv.inspect()["counters"]["rejections_403_not_proxied"]
        .as_u64()
        .unwrap_or(0);
    assert!(
        rejected >= 1,
        "服务器没有记下这条「未经代理」的拒绝，判据就成了猜测：{:?}",
        srv.inspect()["counters"]
    );
    srv.stop().await;
}

/// 死代理必须**失败**，而 `bypass` 命中同一目标时必须照常可用（PROXY.md §9 证据链①）。
///
/// 这条同时挡两种相反的错：代理被忽略（→ 死代理下仍然成功，静默降级）；
/// 以及 `bypass` 不生效（→ 内网直连目标被强行绕去代理，用户侧是"配了例外却没例外"）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dead_proxy_fails_and_a_bypassed_host_still_works() {
    let srv = TestServer::start(Backend::Mem).await;
    let (host, port) = (srv.addr().ip().to_string(), srv.addr().port());
    let url = format!("http://{host}:{port}/.notes/proxy-proof.txt");

    // 先把内容用直连写好，后面两腿读同一份，比较的才只是"路由方式"。
    let direct = client(&ProxyProfile::direct());
    assert!(
        (200..300).contains(&put(&direct, &url, b"origin is fine").await.unwrap()),
        "前置不成立：源站本身写不进"
    );
    assert_eq!(get(&direct, &url).await.unwrap(), 200);

    // ① 一个没人监听的端口当代理（1 端口基本不可能有人用）。
    let dead = ProxyProfile::http(&host, 1);
    let outcome = get(&client(&dead), &url).await;
    assert!(
        outcome.is_err(),
        "指向死代理的请求居然成功了 —— 代理配置被静默忽略（PROXY.md §9 证据链①的红线）：{outcome:?}"
    );

    // ①b 代理主机名根本解析不出来（内网里最常见的配错：host 写错一个字母）。
    let unresolvable = ProxyProfile::http("no-such-proxy-host.invalid", 3128);
    let dns = get(&client(&unresolvable), &url).await;
    assert!(
        dns.is_err(),
        "代理主机名解析失败却请求成功 —— 要么静默直连了，要么这条压根没走代理：{dns:?}"
    );

    // ② 同一个死代理 + 一条命中目标的 bypass：必须照常可用。
    let bypassed = ProxyProfile {
        bypass: vec![format!("{host}:{port}")],
        ..dead.clone()
    };
    assert_eq!(
        get(&client(&bypassed), &url).await.unwrap(),
        200,
        "bypass 没生效：内网直连目标被强行绕去一个死代理"
    );

    // ③ 对照组：一条**不相干**的 bypass 不许变成"只要配了例外就统统直连"。
    //    （那是 `is_bypassed` 最容易写错的一种：只要列表非空就 return true。）
    let unmatched = ProxyProfile {
        bypass: vec!["webdav.corp.internal".into()],
        ..dead.clone()
    };
    let still_proxied = get(&client(&unmatched), &url).await;
    assert!(
        still_proxied.is_err(),
        "bypass 列表里一条都不匹配的目标被放行了 —— 代理形同虚设：{still_proxied:?}"
    );

    // ④ 撤掉死代理（回到 direct）之后同一份内容立刻可读 —— "恢复"这一腿。
    assert_eq!(
        get(&client(&ProxyProfile::direct()), &url).await.unwrap(),
        200,
        "换回直连之后读不到，说明前面的失败在出口层留了脏状态"
    );
    srv.stop().await;
}

/// 代理凭据只能出现在 `Proxy-Authorization` 上，**不许**漏进 Debug / 审计 / URL 明文。
/// §28 的"错误密码"这一形态需要一台会回 407 的代理服务器（本工装没有），
/// 这里能证的、也是最容易出错的那一半是：密码不会因为我们自己的代码泄露出去。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn proxy_credentials_never_leak_into_the_route_proof() {
    let secret = "s3cr3t-pr0xy-pw";
    let profile = ProxyProfile {
        username: Some("ops-user".into()),
        password: Some(secret.into()),
        ..ProxyProfile::http("proxy.corp.internal", 3128)
    };
    let http = client(&profile);
    // 目标是 https：出口守卫不允许"非本机 + 明文 http"，那是另一条判据（PROXY.md §1）。
    let proof = http
        .probe("https://webdav.corp.internal/.notes/manifest/index.json")
        .expect("路由判定是纯计算，不该失败");
    let rendered = format!("{proof:?} {profile:?}");
    assert!(
        !rendered.contains(secret),
        "代理口令出现在了路由审计里：{rendered}"
    );
    assert!(
        proof.detail.contains("proxy.corp.internal:3128"),
        "脱敏不该把代理端点本身也抹掉，否则排查时看不出走了哪台：{}",
        proof.detail
    );
    // 凭据仍在 URL 里带上用户名这件事，由 notera-net 自己的
    // `proxy_url_carries_credentials_and_scheme` 钉；这里只管"不外泄"。
}
