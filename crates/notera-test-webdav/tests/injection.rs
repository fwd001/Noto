//! test-webdav 自测（二）：故障注入必须**确定可复现**。

mod support;

use notera_core::ContentHash;
use notera_test_webdav::{Backend, Injection, TestServer};
use support::{build_request, h, send, send_raw_bytes, RawResp};

const ALLPROP: &[u8] =
    b"<?xml version=\"1.0\"?><d:propfind xmlns:d=\"DAV:\"><d:allprop/></d:propfind>";

fn sha_hex(b: &[u8]) -> String {
    let h = ContentHash::of(b);
    let s = h.as_str();
    s[s.find(':').unwrap() + 1..].to_string()
}

async fn mem_with(i: Injection) -> notera_test_webdav::Started {
    let s = TestServer::start(Backend::Mem).await;
    s.inject(i).await;
    s
}

/// `FAIL(status=…,target=…)`：只回状态、**不落盘**，且原样进日志。
#[tokio::test]
async fn injected_statuses_short_circuit_without_side_effects() {
    for (code, pattern) in [
        (401u16, "PUT /.notes/manifest/*"),
        (403, "PUT /.notes/manifest/*"),
        (404, "PUT /.notes/manifest/*"),
        (405, "PUT /.notes/manifest/*"),
        (412, "PUT /.notes/manifest/*"),
        (500, "PUT /.notes/manifest/*"),
        (503, "PUT /.notes/manifest/*"),
        (507, "PUT /.notes/manifest/*"),
    ] {
        let s = mem_with(Injection::status(pattern, code)).await;
        let r = send(
            s.addr,
            "PUT",
            "/.notes/manifest/index.json",
            &[],
            b"{\"seq\":1}",
        )
        .await;
        assert_eq!(r.status, code, "pattern={pattern}");
        let g = get(&s.addr, "/.notes/manifest/index.json").await;
        assert_eq!(g.status, 404, "注入的 4xx/5xx 不得落盘");
        let log = s.request_log();
        assert_eq!(log.len(), 2, "注入请求也要进 STATS: {log:?}");
        assert_eq!(log[0].status, code);
        assert_eq!(log[0].method, "PUT");
        assert_eq!(log[0].path, "/.notes/manifest/index.json");
        assert_eq!(log[0].bytes, 9);
        assert_eq!(log[1].status, 404);
    }
}

/// 按动词区分 + 第一条命中生效（规则有序）。
#[tokio::test]
async fn status_rules_are_method_scoped_and_first_match_wins() {
    let s = mem_with(Injection {
        status_for: vec![
            ("PUT /.notes/records/*".into(), 412),
            ("* ".trim().to_string(), 500), // 兜底：任意路径，排在后面
        ],
        ..Default::default()
    })
    .await;
    // 第一条命中即生效：PUT 落在 records 上 → 412 而非兜底 500
    let p = put(&s.addr, "/.notes/records/note/a.json", b"v").await;
    assert_eq!(p.status, 412, "规则按序第一条命中生效");
    assert_eq!(get(&s.addr, "/.notes/records/note/a.json").await.status, 500, "其余请求走兜底规则");

    let s2 = mem_with(Injection {
        status_for: vec![("PUT /.notes/records/*".into(), 503)],
        ..Default::default()
    })
    .await;
    let p = put(&s2.addr, "/.notes/records/note/b.json", b"v").await;
    assert_eq!(p.status, 503);
    // GET 不受规则影响
    let g2 = get(&s2.addr, "/.notes/records/note/b.json").await;
    assert_eq!(g2.status, 404);
    let rf = send(s2.addr, "PROPFIND", "/.notes", &[h("depth", "0")], ALLPROP).await;
    assert_eq!(rf.status, 404, "/.notes 未被创建（PUT 被 503 短路）");
}

/// `FAIL(partial-write)`：服务端**已落盘**但返回 500（SY-FAULT-09 的形态）。
#[tokio::test]
async fn partial_write_lands_on_disk_but_returns_error() {
    let s = mem_with(Injection::partial_write("PUT /.notes/records/*", 500)).await;
    let r = put(&s.addr, "/.notes/records/note/x.json", b"payload").await;
    assert_eq!(r.status, 500, "响应必须是失败");
    // 但权威状态里已经有了 —— 这正是"幂等重放"要处理的形态
    let dump = s.fs_dump();
    let entry = dump["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["path"] == serde_json::json!("/.notes/records/note/x.json"))
        .expect("DUMP 里应有该对象");
    assert_eq!(entry["bytes"], serde_json::json!(7));
    assert_eq!(
        entry["sha256"],
        serde_json::json!(format!("sha256:{}", sha_hex(b"payload")))
    );
    let g = get(&s.addr, "/.notes/records/note/x.json").await;
    assert_eq!(g.body, b"payload");
}

/// 半上传：读够 N 字节就断开 → 客户端报错，且服务端**不留半个有效文件**。
#[tokio::test]
async fn truncate_upload_at_leaves_no_partial_object() {
    let s = mem_with(Injection {
        truncate_upload_at: Some(8),
        ..Default::default()
    })
    .await;
    let body = vec![b'x'; 4096];
    let req = build_request("PUT", "/.notes/records/note/big.json", &[], &body);
    // 服务端只读 8 字节就关连接：客户端要么写失败、要么拿不到响应。
    let r = send_raw_bytes(s.addr, &req).await;
    assert_eq!(r.status, 0, "半上传不得有正常响应: {r:?}");

    let g = get(&s.addr, "/.notes/records/note/big.json").await;
    assert_eq!(g.status, 404, "半上传后目标必须不存在");
    assert_eq!(
        s.fs_dump()["count"],
        serde_json::json!(0),
        "DUMP 里不能有任何残留对象: {}",
        s.fs_dump()
    );
    let log = s.request_log();
    assert_eq!(log.len(), 2, "半上传 + 复验 GET: {log:?}");
    assert_eq!(log[0].status, 0, "半上传那一条必须是'无响应'");
    assert_eq!(log[0].method, "PUT");
    assert_eq!(log[0].bytes, 8, "日志要如实记录只读到 8 字节");
    assert_eq!(log[1].status, 404);
    assert_eq!(s.inspect()["counters"]["truncated"], serde_json::json!(1));
}

/// `FAIL(corrupt-body)`：落盘内容不变，只有**响应**被改 → sha256 复算必然失败。
#[tokio::test]
async fn corrupt_manifest_is_detected_by_sha256_and_disk_stays_clean() {
    let s = mem_with(Injection {
        corrupt_manifest: true,
        ..Default::default()
    })
    .await;
    let payload = b"{\"seq\":1324,\"checksum\":\"sha256:aaaa\"}";
    put(&s.addr, "/.notes/manifest/index.json", payload).await;
    let g = get(&s.addr, "/.notes/manifest/index.json").await;
    assert_eq!(g.status, 200);
    assert_eq!(g.body.len(), payload.len());
    assert_ne!(&g.body, payload.as_slice(), "响应必须被篡改");
    // 客户端唯一的发现手段就是复算 sha256
    assert_ne!(sha_hex(&g.body), sha_hex(payload));
    // ETag 仍描述真实内容 → 与响应正文不一致，也是可检出的信号
    assert_eq!(
        g.etag().unwrap_or_default().trim_matches('"'),
        sha_hex(payload),
        "ETag 必须仍是落盘内容的哈希"
    );
    // 服务端权威快照未被污染
    let dump = s.fs_dump();
    let stored = dump["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["path"] == serde_json::json!("/.notes/manifest/index.json"))
        .expect("DUMP 里应有 index.json");
    assert_eq!(
        stored["sha256"],
        serde_json::json!(format!("sha256:{}", sha_hex(payload))),
        "篡改只发生在响应上，落盘内容必须一字不差"
    );
    assert_eq!(s.inspect()["counters"]["corrupted"], serde_json::json!(1));

    // 关掉注入后同一请求即干净
    s.clear_injection().await;
    let clean = get(&s.addr, "/.notes/manifest/index.json").await;
    assert_eq!(&clean.body, payload.as_slice());
}

#[tokio::test]
async fn reset_empties_the_store() {
    let s = TestServer::start(Backend::Mem).await;
    put(&s.addr, "/.notes/records/note/a.json", b"1").await;
    put(&s.addr, "/.notes/records/folder/b.json", b"2").await;
    assert_ne!(s.fs_dump()["count"], serde_json::json!(0));
    s.reset().await;
    assert_eq!(s.fs_dump()["count"], serde_json::json!(0), "{}", s.fs_dump());
    assert!(s.fs_dump()["entries"].as_array().unwrap().is_empty());
    let pf = send(s.addr, "PROPFIND", "/.notes", &[h("depth", "infinity")], ALLPROP).await;
    assert_eq!(pf.status, 404, "RESET 后根前缀必须不存在");
}

/// `reset_after`：第 N 个数据请求之后服务端被清空。
#[tokio::test]
async fn reset_after_n_requests_wipes_state() {
    let s = mem_with(Injection {
        reset_after: Some(2),
        ..Default::default()
    })
    .await;
    put(&s.addr, "/a.json", b"1").await;
    put(&s.addr, "/b.json", b"2").await;
    assert_eq!(s.fs_dump()["count"], serde_json::json!(0), "两次之后应被清空");
    assert_eq!(get(&s.addr, "/a.json").await.status, 404);
}

/// `OFF(close-listener)` 之后新连接被拒；`ON` 之后恢复。
#[tokio::test]
async fn stop_and_start_refuse_and_restore_connections() {
    let s = TestServer::start(Backend::Mem).await;
    put(&s.addr, "/.notes/records/note/a.json", b"1").await;
    let addr = s.addr;
    s.stop().await;
    assert!(!s.is_running());
    let err = TcpConnect::attempt(addr).await;
    assert!(err.is_err(), "关闭监听后连接必须失败");
    s.start_server().await;
    assert!(s.is_running());
    let g = get(&addr, "/.notes/records/note/a.json").await;
    assert_eq!(g.status, 200, "ON 之后同一地址继续可用");
    assert_eq!(g.body, b"1");
}

struct TcpConnect;
impl TcpConnect {
    async fn attempt(addr: std::net::SocketAddr) -> std::io::Result<tokio::net::TcpStream> {
        tokio::time::timeout(
            std::time::Duration::from_secs(3),
            tokio::net::TcpStream::connect(addr),
        )
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "超时"))?
    }
}

/// `FAIL(hang)`：不回应也不关闭连接 → 客户端只能靠超时自救。
#[tokio::test]
async fn timeout_all_hangs_the_connection() {
    let s = mem_with(Injection::hang()).await;
    let res = tokio::time::timeout(
        std::time::Duration::from_millis(300),
        get(&s.addr, "/.notes/manifest/index.json"),
    )
    .await;
    assert!(res.is_err(), "挂起注入下不得在预算内返回");
    let log = s.request_log();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].status, 0, "无响应也要留下一行事实");
    assert_eq!(s.inspect()["counters"]["hung"], serde_json::json!(1));
}

/// `FAIL(abort)`：第 n+1 个数据请求起直接断连。
#[tokio::test]
async fn drop_after_n_cuts_connections() {
    let s = mem_with(Injection::abort_after(2)).await;
    let a = put(&s.addr, "/1.json", b"x").await;
    let b = put(&s.addr, "/2.json", b"x").await;
    assert_eq!(a.status, 201);
    assert_eq!(b.status, 201);
    let c = get(&s.addr, "/3-check.json").await;
    assert_eq!(c.status, 0, "第三次应被切断");
    let log = s.request_log();
    assert_eq!(
        log.iter().map(|r| r.status).collect::<Vec<_>>(),
        vec![201, 201, 0]
    );
    assert_eq!(s.inspect()["counters"]["connections_dropped"], serde_json::json!(1));
}

/// `FAIL(latency)`：延迟真实作用于响应时间（可控且可测）。
#[tokio::test]
async fn latency_injection_delays_responses() {
    let s = mem_with(Injection::latency(120)).await;
    let t0 = std::time::Instant::now();
    let r = get(&s.addr, "/nothing").await;
    let elapsed = t0.elapsed();
    assert_eq!(r.status, 404);
    assert!(
        elapsed >= std::time::Duration::from_millis(100),
        "延迟未生效: {elapsed:?}"
    );
    // 控制面不受延迟影响（否则挂掉的服务器管不回来）
    let t1 = std::time::Instant::now();
    let d = send(s.addr, "GET", "/_fs/dump", &[], &[]).await;
    assert_eq!(d.status, 200);
    assert!(t1.elapsed() < std::time::Duration::from_millis(100), "控制面必须即时");
}

/// 代理独占（PROXY.md §9 证据链②）：直连 403，经代理才放行。
#[tokio::test]
async fn require_proxy_forbids_direct_and_admits_proxied_traffic() {
    let s = mem_with(Injection {
        require_proxy: true,
        ..Default::default()
    })
    .await;

    // ① 直连（origin-form）→ 403
    let direct = put(&s.addr, "/.notes/records/note/a.json", b"x").await;
    assert_eq!(direct.status, 403, "直连必须被拒");
    assert_eq!(s.fs_dump()["count"], serde_json::json!(0));

    // ② 绝对形式请求目标（= HTTP 代理转发 http:// 目标的样子）→ 放行
    let target = format!("http://{}/.notes/records/note/a.json", s.addr);
    let via_proxy = support::send_raw_bytes(
        s.addr,
        &build_request("PUT", &target, &[], b"via-proxy"),
    )
    .await;
    assert_eq!(via_proxy.status, 201, "经代理转发的请求应放行");
    assert_eq!(s.fs_dump()["count"], serde_json::json!(1), "{}", s.fs_dump());

    // ③ CONNECT 隧道：握手 200 之后，同一连接上的请求算"经代理到达"→ 放行
    let tunnel = support::send_pipeline(
        s.addr,
        &[
            build_request("CONNECT", &s.addr.to_string(), &[], &[]),
            build_request(
                "PUT",
                "/.notes/records/note/b.json",
                &[],
                b"via-connect",
            ),
            build_request("GET", "/.notes/records/note/a.json", &[], &[]),
        ],
    )
    .await;
    assert_eq!(tunnel[0].status, 200, "CONNECT 握手应回 200");
    assert_eq!(tunnel[1].status, 201, "隧道内的 PUT 必须被放行");
    assert_eq!(tunnel[2].status, 200, "隧道内的 GET 必须被放行");
    assert_eq!(tunnel[2].body, b"via-proxy");
    assert_eq!(get(&s.addr, "/.notes/records/note/b.json").await.status, 403,
        "换了直连连接就必须再次被拒 —— 放行判定是按连接的");
}

/// **确定性**：同一注入两次运行，request_log（去掉时间戳）逐条一致。
#[tokio::test]
async fn same_injection_produces_identical_logs_twice() {
    async fn run() -> Vec<(u64, String, String, u16, u64)> {
        let inj = Injection {
            status_for: vec![("GET /.notes/records/note/c.json".into(), 503)],
            corrupt_manifest: true,
            reset_after: Some(6),
            ..Default::default()
        };
        let s = mem_with(inj).await;
        put(&s.addr, "/.notes/manifest/index.json", b"{\"seq\":1}").await;
        put(&s.addr, "/.notes/records/note/a.json", b"aa").await;
        get(&s.addr, "/.notes/records/note/c.json").await;
        get(&s.addr, "/.notes/manifest/index.json").await;
        send(
            s.addr,
            "PROPFIND",
            "/.notes",
            &[h("depth", "infinity")],
            ALLPROP,
        )
        .await;
        let sig = s
            .request_log()
            .iter()
            .map(|r| (r.seq, r.method.clone(), r.path.clone(), r.status, r.bytes))
            .collect::<Vec<_>>();
        sig
    }
    let a = run().await;
    let b = run().await;
    assert_eq!(a, b, "两次同样注入的日志必须一致");
    assert_eq!(a.len(), 5, "{a:?}");
    assert_eq!(a[2].3, 503);
    assert_eq!(a[1].3, 201);
}

/// 控制面也走真 HTTP：`/_control/*` 与 `/_fs/dump`。
#[tokio::test]
async fn control_plane_works_over_http() {
    let s = TestServer::start(Backend::Mem).await;
    put(&s.addr, "/.notes/records/note/a.json", b"hello").await;

    let dump = send(s.addr, "GET", "/_fs/dump", &[], &[]).await;
    assert_eq!(dump.status, 200);
    let v: serde_json::Value = serde_json::from_slice(&dump.body).unwrap();
    assert_eq!(v["count"], serde_json::json!(1), "count 只数文件: {v}");
    assert_eq!(v["backend"], serde_json::json!("mem"));
    let sha = v["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["path"] == serde_json::json!("/.notes/records/note/a.json"))
        .unwrap();
    assert_eq!(sha["sha256"], serde_json::json!(format!("sha256:{}", sha_hex(b"hello"))));
    assert_eq!(sha["bytes"], serde_json::json!(5));

    // prefix 过滤
    let pfx = send(s.addr, "GET", "/_fs/dump?prefix=/.notes/manifest", &[], &[]).await;
    let pv: serde_json::Value = serde_json::from_slice(&pfx.body).unwrap();
    assert_eq!(pv["count"], serde_json::json!(0), "{pv}");

    // 注入一个 401，然后 STATS 里能看到
    let inj_body = serde_json::to_vec(&Injection::status("PUT /*", 401)).unwrap();
    let ir = send(s.addr, "POST", "/_control/inject", &[], &inj_body).await;
    assert_eq!(ir.status, 200, "{}", String::from_utf8_lossy(&ir.body));
    let blocked = put(&s.addr, "/.notes/records/note/b.json", b"x").await;
    assert_eq!(blocked.status, 401);

    let stats = send(s.addr, "GET", "/_control/inspect", &[], &[]).await;
    let sv: serde_json::Value = serde_json::from_slice(&stats.body).unwrap();
    let reqs = sv["requests"].as_array().unwrap();
    // STATS 只含协议流量：a.json 的 PUT 成功 + b.json 的 PUT 被注入 401；
    // 控制面调用（dump/inject/inspect）一律不进日志。
    assert_eq!(reqs.len(), 2, "{sv}");
    assert_eq!(reqs[0]["status"], serde_json::json!(201));
    assert_eq!(reqs[1]["status"], serde_json::json!(401));
    assert_eq!(sv["injection"]["status_for"][0][1], serde_json::json!(401));
    assert_eq!(sv["injection"]["status_for"][0][0], serde_json::json!("PUT /*"));

    // RESET 走 HTTP，随后 DUMP 为空
    let rr = send(s.addr, "POST", "/_control/reset", &[], &[]).await;
    assert_eq!(rr.status, 200);
    let after = send(s.addr, "GET", "/_fs/dump", &[], &[]).await;
    let av: serde_json::Value = serde_json::from_slice(&after.body).unwrap();
    assert_eq!(av["count"], serde_json::json!(0));
}

async fn get(addr: &std::net::SocketAddr, path: &str) -> RawResp {
    send(*addr, "GET", path, &[], &[]).await
}

async fn put(addr: &std::net::SocketAddr, path: &str, body: &[u8]) -> RawResp {
    send(*addr, "PUT", path, &[], body).await
}
