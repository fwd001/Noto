//! test-webdav 自测（一）：动词与语义。全部走真 socket，无 mock。

mod support;

use notera_core::ContentHash;
use notera_test_webdav::{Backend, Started, TestServer};
use std::net::SocketAddr;
use support::{build_chunked_request, build_request, h, send, send_pipeline, RawResp};

async fn put(addr: SocketAddr, path: &str, body: &[u8]) -> RawResp {
    send(addr, "PUT", path, &[], body).await
}

async fn get(addr: SocketAddr, path: &str) -> RawResp {
    send(addr, "GET", path, &[], &[]).await
}

async fn mem() -> Started {
    TestServer::start(Backend::Mem).await
}

fn sha256_hex(bytes: &[u8]) -> String {
    let h = ContentHash::of(bytes);
    let s = h.as_str();
    // 权威形态是 `sha256:<64hex>`，这里只要 hex 部分。
    s[s.find(':').expect("sha256 前缀") + 1..].to_string()
}

const ALLPROP: &[u8] =
    b"<?xml version=\"1.0\"?><d:propfind xmlns:d=\"DAV:\"><d:allprop/></d:propfind>";

#[tokio::test]
async fn options_advertises_every_required_verb() {
    let s = mem().await;
    let r = send(s.addr, "OPTIONS", "/.notes", &[], &[]).await;
    let allow = r.header("allow").unwrap_or_default().to_string();
    let dav = r.header("dav").unwrap_or_default().to_string();
    for v in [
        "OPTIONS", "GET", "HEAD", "PUT", "DELETE", "MOVE", "COPY", "PROPFIND", "PROPPATCH",
        "MKCOL",
    ] {
        assert!(allow.contains(v), "Allow 缺 {v}: {allow}");
    }
    assert!(dav.contains('1') && dav.contains('2'), "DAV 头: {dav}");
}

#[tokio::test]
async fn propfind_returns_207_multistatus_for_depth_zero_one_infinity() {
    let s = mem().await;
    put(s.addr, "/.notes/manifest/index.json", b"{\"seq\":1}")
        .await;
    put(s.addr, "/.notes/records/note/a.json", b"aa").await;
    put(s.addr, "/.notes/records/folder/b.json", b"bb").await;

    let d0 = send(
        s.addr,
        "PROPFIND",
        "/.notes",
        &[h("depth", "0"), h("content-type", "application/xml")],
        ALLPROP,
    )
    .await;
    assert_eq!(d0.status, 207, "body={}", String::from_utf8_lossy(&d0.body));
    let t0 = String::from_utf8_lossy(&d0.body).into_owned();
    assert!(t0.starts_with("<?xml"), "{t0}");
    assert!(t0.contains("<D:multistatus xmlns:D=\"DAV:\">"), "{t0}");
    assert_eq!(count_occurrences(&t0, "<D:response>"), 1, "Depth:0 只回自己");
    assert!(t0.contains("<D:href>/.notes/</D:href>"));
    assert!(t0.contains("<D:propstat>"));
    assert!(t0.contains("<D:prop>"));

    let d1 = send(s.addr, "PROPFIND", "/.notes", &[h("depth", "1")], ALLPROP).await;
    assert_eq!(d1.status, 207);
    let t1 = String::from_utf8_lossy(&d1.body).into_owned();
    // /.notes + manifest + records（不含孙子）
    assert_eq!(count_occurrences(&t1, "<D:response>"), 3, "{t1}");
    assert!(t1.contains("<D:collection/>"));

    let di = send(
        s.addr,
        "PROPFIND",
        "/.notes",
        &[h("depth", "infinity")],
        ALLPROP,
    )
    .await;
    assert_eq!(di.status, 207);
    let ti = String::from_utf8_lossy(&di.body).into_owned();
    assert_eq!(count_occurrences(&ti, "<D:response>"), 8, "{ti}");
    assert!(ti.contains("<D:href>/.notes/records/note/a.json</D:href>"));
    assert!(ti.contains("<D:getetag>"));
    assert!(ti.contains("<D:getcontentlength>2</D:getcontentlength>"));

    // Depth 缺失 → 按 infinity 处理（并回 207）
    let dn = send(s.addr, "PROPFIND", "/.notes", &[], ALLPROP).await;
    assert_eq!(dn.status, 207);
    // 不存在的资源 → 404
    let miss = send(
        s.addr,
        "PROPFIND",
        "/.notes/nope",
        &[h("depth", "0")],
        ALLPROP,
    )
    .await;
    assert_eq!(miss.status, 404);
}

#[tokio::test]
async fn put_then_get_is_byte_identical_and_etag_is_content_sha256() {
    let s = mem().await;
    let payload = b"{\"rev\":7,\"hash\":\"sha256:deadbeef\"}".to_vec();
    let p = put(s.addr, "/.notes/records/note/x.json", &payload).await;
    assert_eq!(p.status, 201, "新建应 201");
    let etag = p.etag().unwrap_or_default().to_string();
    assert_eq!(
        etag,
        format!("\"{}\"", sha256_hex(&payload)),
        "ETag 必须是内容 sha256 派生的强 ETag: {etag}"
    );
    assert!(!etag.starts_with("W/"), "必须是强 ETag: {etag}");

    let g = get(s.addr, "/.notes/records/note/x.json").await;
    assert_eq!(g.status, 200);
    assert_eq!(g.body, payload);
    assert_eq!(g.etag().unwrap_or_default(), etag);
    assert_eq!(
        g.header("content-length")
            .unwrap_or_default()
            .parse::<usize>()
            .unwrap(),
        payload.len()
    );

    // 覆盖写 → 204，ETag 随内容变
    let p2 = put(s.addr, "/.notes/records/note/x.json", b"other").await;
    assert_eq!(p2.status, 204);
    assert_ne!(p2.etag().unwrap_or_default(), etag);
    assert_eq!(
        get(s.addr, "/.notes/records/note/x.json").await.body,
        b"other"
    );
}

#[tokio::test]
async fn if_match_mismatch_is_412_and_match_proceeds() {
    let s = mem().await;
    put(s.addr, "/.notes/records/note/a.json", b"v1").await;
    let cur = get(s.addr, "/.notes/records/note/a.json").await;
    let etag = cur.etag().unwrap_or_default().to_string();

    let bad = send(
        s.addr,
        "PUT",
        "/.notes/records/note/a.json",
        &[h(
            "if-match",
            "\"0000000000000000000000000000000000000000000000000000000000000000\"",
        )],
        b"v2",
    )
    .await;
    assert_eq!(bad.status, 412, "If-Match 不匹配必须 412");
    // 412 之后内容未变
    assert_eq!(
        get(s.addr, "/.notes/records/note/a.json").await.body,
        b"v1"
    );

    let good = send(
        s.addr,
        "PUT",
        "/.notes/records/note/a.json",
        &[h("if-match", etag.clone())],
        b"v2",
    )
    .await;
    assert_eq!(good.status, 204, "匹配则写入成功");
    assert_eq!(get(s.addr, "/.notes/records/note/a.json").await.body, b"v2");

    // If-Match 打在不存在的资源上 → 412（非 `*` 需要现值匹配）
    let fresh = send(
        s.addr,
        "PUT",
        "/.notes/records/note/new.json",
        &[h("if-match", etag)],
        b"x",
    )
    .await;
    assert_eq!(fresh.status, 412);
}

#[tokio::test]
async fn if_none_match_star_rejects_existing_target() {
    let s = mem().await;
    put(s.addr, "/.notes/records/note/dup.json", b"first").await;
    let r = send(
        s.addr,
        "PUT",
        "/.notes/records/note/dup.json",
        &[h("if-none-match", "*")],
        b"second",
    )
    .await;
    assert_eq!(r.status, 412, "已存在 + If-None-Match:* → 412");
    assert_eq!(
        get(s.addr, "/.notes/records/note/dup.json").await.body,
        b"first"
    );

    // 不存在时同一请求成立（这就是 protocol.json 的创建语义）
    let ok = send(
        s.addr,
        "PUT",
        "/.notes/protocol.json",
        &[h("if-none-match", "*")],
        b"{\"protocol\":1}",
    )
    .await;
    assert_eq!(ok.status, 201);
}

#[tokio::test]
async fn get_if_none_match_hit_returns_304_with_empty_body() {
    let s = mem().await;
    put(s.addr, "/.notes/manifest/index.json", b"{\"seq\":1324}").await;
    let g = get(s.addr, "/.notes/manifest/index.json").await;
    let etag = g.etag().unwrap_or_default().to_string();
    assert_eq!(g.status, 200);

    let re = send(
        s.addr,
        "GET",
        "/.notes/manifest/index.json",
        &[h("if-none-match", etag.clone())],
        &[],
    )
    .await;
    assert_eq!(re.status, 304, "空轮快路径的前提");
    assert!(re.body.is_empty(), "304 不得带正文");
    assert_eq!(re.etag().unwrap_or_default(), etag);

    // 弱 ETag 与多值列表也要命中
    let weak = etag.replace('"', "");
    let re2 = send(
        s.addr,
        "GET",
        "/.notes/manifest/index.json",
        &[h("if-none-match", format!("W/\"{weak}\", \"other\""))],
        &[],
    )
    .await;
    assert_eq!(re2.status, 304);

    // 不匹配 → 200 + 正文
    let re3 = send(
        s.addr,
        "GET",
        "/.notes/manifest/index.json",
        &[h("if-none-match", "\"stale\"")],
        &[],
    )
    .await;
    assert_eq!(re3.status, 200);
    assert_eq!(re3.body, b"{\"seq\":1324}");
}

#[tokio::test]
async fn range_requests_return_206_and_bad_range_416() {
    let s = mem().await;
    put(s.addr, "/attachments/ab/abcdef", b"0123456789").await;
    let r = send(
        s.addr,
        "GET",
        "/attachments/ab/abcdef",
        &[h("range", "bytes=2-4")],
        &[],
    )
    .await;
    assert_eq!(r.status, 206);
    assert_eq!(r.body, b"234");
    assert_eq!(r.header("content-range").unwrap_or_default(), "bytes 2-4/10");

    let tail = send(
        s.addr,
        "GET",
        "/attachments/ab/abcdef",
        &[h("range", "bytes=8-")],
        &[],
    )
    .await;
    assert_eq!(tail.status, 206);
    assert_eq!(tail.body, b"89");

    let bad = send(
        s.addr,
        "GET",
        "/attachments/ab/abcdef",
        &[h("range", "bytes=99-120")],
        &[],
    )
    .await;
    assert_eq!(bad.status, 416);
}

#[tokio::test]
async fn move_honours_overwrite_f_and_if_match() {
    let s = mem().await;
    put(s.addr, "/.notes/tmp/dev1-nonce.json", b"new").await;
    put(s.addr, "/.notes/records/note/r.json", b"old").await;
    let dest = format!("http://{}", s.addr);

    // 目标存在 + Overwrite: F → 412（RFC 4918 §9.8.5）
    let blocked = send(
        s.addr,
        "MOVE",
        "/.notes/tmp/dev1-nonce.json",
        &[
            h(
                "destination",
                format!("{dest}/.notes/records/note/r.json"),
            ),
            h("overwrite", "F"),
        ],
        &[],
    )
    .await;
    assert_eq!(blocked.status, 412, "Overwrite:F 命中已存在目标必须 412");
    // 源与目标都还在
    assert_eq!(get(s.addr, "/.notes/tmp/dev1-nonce.json").await.status, 200);
    assert_eq!(
        get(s.addr, "/.notes/records/note/r.json").await.body,
        b"old"
    );

    // Overwrite: T → 覆盖，204
    let forced = send(
        s.addr,
        "MOVE",
        "/.notes/tmp/dev1-nonce.json",
        &[h(
            "destination",
            format!("{dest}/.notes/records/note/r.json"),
        )],
        &[],
    )
    .await;
    assert_eq!(forced.status, 204);
    assert_eq!(get(s.addr, "/.notes/tmp/dev1-nonce.json").await.status, 404);
    assert_eq!(get(s.addr, "/.notes/records/note/r.json").await.body, b"new");

    // 新目标 → 201
    let fresh = send(
        s.addr,
        "MOVE",
        "/.notes/records/note/r.json",
        &[h(
            "destination",
            format!("{dest}/.notes/records/note/r2.json"),
        )],
        &[],
    )
    .await;
    assert_eq!(fresh.status, 201);
    assert_eq!(
        get(s.addr, "/.notes/records/note/r2.json").await.body,
        b"new"
    );

    // MOVE + If-Match 不匹配 → 412，源不动
    put(s.addr, "/.notes/tmp/x.json", b"tmp").await;
    let pre = send(
        s.addr,
        "MOVE",
        "/.notes/tmp/x.json",
        &[
            h("destination", format!("{dest}/.notes/records/note/z.json")),
            h("if-match", "\"bogus\""),
        ],
        &[],
    )
    .await;
    assert_eq!(pre.status, 412);
    assert_eq!(get(s.addr, "/.notes/tmp/x.json").await.status, 200);
}

#[tokio::test]
async fn copy_keeps_source_and_honours_overwrite_f() {
    let s = mem().await;
    put(s.addr, "/attachments/00/sha-a", b"AAA").await;
    let dest = format!("http://{}", s.addr);
    let c = send(
        s.addr,
        "COPY",
        "/attachments/00/sha-a",
        &[h("destination", format!("{dest}/attachments/00/sha-b"))],
        &[],
    )
    .await;
    assert_eq!(c.status, 201);
    assert_eq!(
        get(s.addr, "/attachments/00/sha-a").await.status,
        200,
        "源保留"
    );
    assert_eq!(get(s.addr, "/attachments/00/sha-b").await.body, b"AAA");
    assert_eq!(
        get(s.addr, "/attachments/00/sha-b").await.etag(),
        get(s.addr, "/attachments/00/sha-a").await.etag()
    );
    let again = send(
        s.addr,
        "COPY",
        "/attachments/00/sha-a",
        &[
            h("destination", format!("{dest}/attachments/00/sha-b")),
            h("overwrite", "F"),
        ],
        &[],
    )
    .await;
    assert_eq!(again.status, 412);
}

#[tokio::test]
async fn mkcol_is_strict_about_parents_and_existing() {
    let s = mem().await;
    let ok = send(s.addr, "MKCOL", "/.notes", &[], &[]).await;
    assert_eq!(ok.status, 201);
    let dup = send(s.addr, "MKCOL", "/.notes", &[], &[]).await;
    assert_eq!(dup.status, 405, "已存在 → 405");
    let orphan = send(s.addr, "MKCOL", "/.notes/a/b/c", &[], &[]).await;
    assert_eq!(orphan.status, 409, "父集合不存在 → 409");
    let nested = send(s.addr, "MKCOL", "/.notes/locks", &[], &[]).await;
    assert_eq!(nested.status, 201);
    // 集合可被 PROPFIND 看到
    let pf = send(s.addr, "PROPFIND", "/.notes", &[h("depth", "1")], ALLPROP).await;
    let t = String::from_utf8_lossy(&pf.body).into_owned();
    assert!(t.contains("<D:href>/.notes/locks/</D:href>"), "{t}");
}

#[tokio::test]
async fn delete_reports_404_and_cascades_collections() {
    let s = mem().await;
    let miss = send(
        s.addr,
        "DELETE",
        "/.notes/records/note/none.json",
        &[],
        &[],
    )
    .await;
    assert_eq!(miss.status, 404, "远端确实缺失必须与'配额满'区分（§10）");
    put(s.addr, "/.notes/records/note/a.json", b"1").await;
    put(s.addr, "/.notes/records/note/b.json", b"2").await;
    let d = send(s.addr, "DELETE", "/.notes/records", &[], &[]).await;
    assert_eq!(d.status, 204);
    assert_eq!(get(s.addr, "/.notes/records/note/a.json").await.status, 404);
    assert_eq!(get(s.addr, "/.notes/manifest").await.status, 404);
}

#[tokio::test]
async fn proppatch_stores_property_and_propfind_returns_it() {
    let s = mem().await;
    put(s.addr, "/.notes/protocol.json", b"{\"protocol\":1}").await;
    let patch = br#"<?xml version="1.0"?><d:propertyupdate xmlns:d="DAV:" xmlns:x="urn:notera"><d:set><d:prop><x:generator>notera 0.1.0</x:generator></d:prop></d:set></d:propertyupdate>"#;
    let r = send(
        s.addr,
        "PROPPATCH",
        "/.notes/protocol.json",
        &[h("content-type", "application/xml")],
        patch,
    )
    .await;
    assert_eq!(r.status, 207, "PROPPATCH 必须回 207");
    let t = String::from_utf8_lossy(&r.body).into_owned();
    assert!(t.contains("<D:propstat>"), "{t}");
    assert!(t.contains("<generator>notera 0.1.0</generator>"), "{t}");

    let pf = send(
        s.addr,
        "PROPFIND",
        "/.notes/protocol.json",
        &[h("depth", "0")],
        ALLPROP,
    )
    .await;
    let pt = String::from_utf8_lossy(&pf.body).into_owned();
    assert!(pt.contains("<generator>notera 0.1.0</generator>"), "{pt}");
}

#[tokio::test]
async fn head_returns_headers_without_body() {
    let s = mem().await;
    put(s.addr, "/.notes/records/folder/f.json", b"1234567").await;
    let h0 = send(s.addr, "HEAD", "/.notes/records/folder/f.json", &[], &[]).await;
    assert_eq!(h0.status, 200);
    assert!(h0.body.is_empty(), "HEAD 不得有正文");
    assert_eq!(
        h0.header("content-length").unwrap_or_default(),
        "7",
        "HEAD 要报出 GET 会有的长度"
    );
    assert!(h0.etag().is_some());
}

#[tokio::test]
async fn chunked_upload_is_stored_byte_identically() {
    let s = mem().await;
    let req = build_chunked_request(
        "PUT",
        "/.notes/records/note/chunked.json",
        &[],
        &[b"{\"a\":", b"1,\"b\":", b"[1,2]}"],
    );
    let r = support::send_raw_bytes(s.addr, &req).await;
    assert_eq!(r.status, 201, "chunked PUT 应成功: {r:?}");
    let g = get(s.addr, "/.notes/records/note/chunked.json").await;
    assert_eq!(g.body, b"{\"a\":1,\"b\":[1,2]}");
    assert_eq!(g.body.len(), 17);
}

#[tokio::test]
async fn traversal_and_malformed_requests_are_refused() {
    let s = mem().await;
    let r = send(s.addr, "GET", "/.notes/../../Windows/system32", &[], &[]).await;
    assert_eq!(r.status, 400, "目录穿越必须拒绝");
    let r2 = send(s.addr, "PUT", "/a/./../../b", &[], b"x").await;
    assert_eq!(r2.status, 400);
    // 未实现的方法 → 405/501，且不崩
    let r3 = send(s.addr, "TRACE", "/.notes", &[], &[]).await;
    assert!(matches!(r3.status, 405 | 501), "{}", r3.status);
    // 畸形请求行 → 直接关连接，无响应
    let raw = support::send_raw_bytes(s.addr, b"GARBAGE\r\n\r\n").await;
    assert_eq!(raw.status, 0, "畸形报文不得有响应: {raw:?}");
}

#[tokio::test]
async fn keep_alive_serves_several_requests_on_one_connection() {
    let s = mem().await;
    let reqs = vec![
        build_request("PUT", "/.notes/records/note/k1.json", &[], b"one"),
        build_request("GET", "/.notes/records/note/k1.json", &[], &[]),
        build_request("PROPFIND", "/.notes", &[h("depth", "0")], ALLPROP),
    ];
    let rs = send_pipeline(s.addr, &reqs).await;
    assert_eq!(rs.len(), 3);
    assert_eq!(rs[0].status, 201);
    assert_eq!(rs[1].status, 200);
    assert_eq!(rs[1].body, b"one");
    assert_eq!(rs[2].status, 207);
}

#[tokio::test]
async fn get_and_put_on_collection_are_clean_errors() {
    let s = mem().await;
    send(s.addr, "MKCOL", "/.notes", &[], &[]).await;
    let mk = send(s.addr, "MKCOL", "/.notes/locks", &[], &[]).await;
    assert_eq!(mk.status, 201);
    let g = get(s.addr, "/.notes/locks").await;
    assert_eq!(g.status, 405, "GET 集合 → 405");
    let p = put(s.addr, "/.notes/locks", b"x").await;
    assert_eq!(p.status, 405, "PUT 集合 → 405");
}

fn count_occurrences(hay: &str, needle: &str) -> usize {
    hay.matches(needle).count()
}
