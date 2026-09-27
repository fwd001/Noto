//! §5 能力探测：跑在真 TCP 服务器上，用 `/_control/inject` 逐项关掉能力。
//!
//! 这里要证的不是"探测返回了什么"，而是**每一位的缺失都能被单独认出来**，
//! 以及最要紧的一条：服务器不可达时报错，而不是悄悄当成"什么都不支持"——
//! 后者会把一个本来支持条件写的服务器降级到 S3 盲写，那正是会覆盖丢数据的方向。
use std::sync::Arc;
use std::time::Duration;

use notera_core::DeviceId;
use notera_net::{HttpClient, ProxyProfile, Timeouts, TlsPolicy};
use notera_test_webdav::{Backend, Injection, Started, TestServer};
use notera_webdav::{Caps, Credentials, ProbeReport, WebDavConfig, WebDavRemote};

struct Ctx {
    srv: Started,
    http: Arc<HttpClient>,
}

async fn ctx() -> Ctx {
    let srv = TestServer::start(Backend::Mem).await;
    let http = Arc::new(
        HttpClient::build(
            &ProxyProfile::direct(),
            &TlsPolicy::Strict,
            Timeouts::short(Duration::from_secs(10)),
        )
        .expect("出口客户端"),
    );
    Ctx { srv, http }
}

impl Ctx {
    fn remote(&self) -> WebDavRemote {
        let cfg = WebDavConfig::new(self.srv.base_url.clone())
            .with_credentials(Credentials::new("notera-test", "sup3r-s3cr3t").expect("凭据"))
            .with_device(DeviceId::new());
        WebDavRemote::new(cfg, Arc::clone(&self.http)).expect("合法配置")
    }
}

#[tokio::test]
async fn a_normal_server_earns_the_strong_write_strategies() {
    let c = ctx().await;
    let r = c.remote().probe_caps().await.expect("探测不该失败");
    // 这两位决定 S1/S2。它们为真是"默认能走条件写"的依据；为假就必须是真观察到不支持。
    assert!(
        r.conditional_put,
        "测试服务器实现 If-Match，探测却说没有：{:?}",
        r.describe()
    );
    assert!(
        r.overwrite_f_move,
        "测试服务器实现 Overwrite:F，探测却说没有：{:?}",
        r.describe()
    );
    assert!(
        r.strong_etag,
        "PUT 后 If-None-Match 应能拿到 304：{:?}",
        r.describe()
    );
    let caps = r.to_caps();
    assert!(
        caps.has(Caps::CONDITIONAL_PUT)
            && caps.has(Caps::OVERWRITE_F_MOVE)
            && caps.has(Caps::STRONG_ETAG)
    );
    assert!(
        !caps.has(Caps::CHUNKED),
        "发不出真 chunked 请求，就不该声称探到它"
    );
}

#[tokio::test]
async fn each_absence_is_detected_on_its_own() {
    struct Case {
        name: &'static str,
        inject: Injection,
        cleared: fn(&ProbeReport) -> bool,
        still_true: fn(&ProbeReport) -> bool,
    }
    let cases = vec![
        Case {
            name: "没有强 ETag：GET 永不回 304",
            inject: Injection::status("GET /.notes/probe/etag.json", 200),
            cleared: |r: &ProbeReport| !r.strong_etag,
            still_true: |r: &ProbeReport| r.conditional_put && r.overwrite_f_move,
        },
        Case {
            name: "条件 PUT 被当装饰：照样 200",
            inject: Injection::status("PUT /.notes/probe/cput.json", 200),
            cleared: |r| !r.conditional_put,
            still_true: |r| r.strong_etag && r.overwrite_f_move,
        },
        Case {
            name: "不支持 MOVE",
            inject: Injection::status("MOVE *", 501),
            cleared: |r| !r.overwrite_f_move,
            still_true: |r| r.conditional_put && r.strong_etag,
        },
        Case {
            name: "不支持 Range：忽略头部回整份 200",
            inject: Injection::status("GET /.notes/probe/range.bin", 200),
            cleared: |r| !r.range,
            still_true: |r| r.conditional_put,
        },
        Case {
            name: "不支持 Depth:infinity",
            inject: Injection::status("PROPFIND *", 400),
            cleared: |r| !r.depth_infinity,
            still_true: |r| r.conditional_put && r.overwrite_f_move,
        },
    ];
    for case in cases {
        let c = ctx().await;
        c.srv.inject(case.inject).await;
        let r = c
            .remote()
            .probe_caps()
            .await
            .unwrap_or_else(|e| panic!("{}：探测本身失败 {e}", case.name));
        assert!(
            (case.cleared)(&r),
            "{}：该位没被认出来 —— {:?}",
            case.name,
            r.describe()
        );
        assert!(
            (case.still_true)(&r),
            "{}：别的能力被误伤了 —— {:?}",
            case.name,
            r.describe()
        );
    }
}

#[tokio::test]
async fn an_unreachable_server_is_an_error_not_a_capability_verdict() {
    let c = ctx().await;
    let remote = c.remote();
    c.srv.stop().await;
    let err = remote.probe_caps().await.expect_err("服务器停了必须报错");
    // 关键是"报的是错"而不是"回一个全 false 的结论"：后者会把好服务器降级到 S3 盲写
    assert!(
        matches!(err, notera_sync::RemoteError::Offline),
        "实际 {err:?}"
    );
    c.srv.restart().await;
}

#[tokio::test]
async fn probe_leaves_the_real_library_alone() {
    // 探测对象只准待在 probe/ 下，且用完要清掉：
    // 探测在真实库上留垃圾 = 污染清单，那是要跟着同步到所有设备的。
    let c = ctx().await;
    let remote = c.remote();
    remote.probe_caps().await.unwrap();
    let dump = c.srv.fs_dump();
    let leftover = dump["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .filter(|e| e["path"].as_str().unwrap_or_default().contains("/probe/"))
        .collect::<Vec<_>>();
    assert!(leftover.is_empty(), "探测对象没清理：{leftover:?}");
    // 探测只准在 probe/ 里活动：碰到 records/manifest/attachments 就是污染了要同步的数据
    let touched_real = dump["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .filter_map(|e| e["path"].as_str())
        .filter(|p| {
            p.contains("/records/") || p.contains("/manifest/") || p.contains("/attachments/")
        })
        .collect::<Vec<_>>();
    assert!(
        touched_real.is_empty(),
        "探测动了真实数据区：{touched_real:?}"
    );
}

#[test]
fn caps_bits_map_exactly_as_the_table_says() {
    let all = ProbeReport {
        strong_etag: true,
        conditional_put: true,
        overwrite_f_move: true,
        depth_infinity: true,
        range: true,
    };
    assert_eq!(
        all.to_caps().mask(),
        Caps::STRONG_ETAG
            | Caps::CONDITIONAL_PUT
            | Caps::OVERWRITE_F_MOVE
            | Caps::DEPTH_INFINITY
            | Caps::RANGE
    );
    assert_eq!(ProbeReport::default().to_caps(), Caps::none());
    // 探到条件写 ⇒ 选 S1；什么都没探到 ⇒ 只能 S3 盲写复验（§5 的写入策略表）
    assert_eq!(
        all.to_caps().write_strategy(),
        notera_webdav::WriteStrategy::S1
    );
    assert_eq!(
        ProbeReport::default().to_caps().write_strategy(),
        notera_webdav::WriteStrategy::S3
    );
}
