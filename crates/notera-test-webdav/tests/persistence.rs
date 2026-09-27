//! test-webdav 自测（三）：`Fs` 后端与 `RESTART` 的**真实持久性**。

mod support;

use notera_test_webdav::{Backend, TestServer};
use support::{cleanup, send, tmp_dir};

const ALLPROP: &[u8] =
    b"<?xml version=\"1.0\"?><d:propfind xmlns:d=\"DAV:\"><d:allprop/></d:propfind>";

#[tokio::test]
async fn fs_backend_survives_restart_with_identical_etags() {
    let dir = tmp_dir("restart");
    let s = TestServer::start(Backend::Fs(dir.clone())).await;
    let payload = b"{\"rev\":3}".to_vec();
    put(&s, "/.notes/records/note/a.json", &payload).await;
    put(&s, "/.notes/manifest/index.json", b"{\"seq\":9}").await;
    let before = get(&s, "/.notes/records/note/a.json").await;
    assert_eq!(before.status, 200);
    let etag_before = before.etag().unwrap_or_default().to_string();

    // 磁盘上真的有文件（不是"其实没重启"的假象）
    assert!(
        dir.join(".notes")
            .join("records")
            .join("note")
            .join("a.json")
            .exists(),
        "Fs 模式必须真的落盘: {:?}",
        dir
    );

    s.restart().await;
    assert_eq!(s.generation(), 2, "RESTART 要换代");

    let after = get(&s, "/.notes/records/note/a.json").await;
    assert_eq!(after.status, 200, "重启后资源必须还在");
    assert_eq!(after.body, payload);
    assert_eq!(
        after.etag().unwrap_or_default(),
        etag_before,
        "ETag 由内容派生 → 重启后必须逐字节一致（SY-INT-07）"
    );

    // 304 空轮在重启后仍然可用
    let re = send(
        s.addr,
        "GET",
        "/.notes/records/note/a.json",
        &[support::h("if-none-match", etag_before.clone())],
        &[],
    )
    .await;
    assert_eq!(re.status, 304, "重启后 If-None-Match 仍须命中");

    // 集合结构也从磁盘恢复
    let pf = send(
        s.addr,
        "PROPFIND",
        "/.notes",
        &[support::h("depth", "1")],
        ALLPROP,
    )
    .await;
    let t = String::from_utf8_lossy(&pf.body).into_owned();
    assert!(t.contains("<D:href>/.notes/records/</D:href>"), "{t}");
    assert!(t.contains("<D:href>/.notes/manifest/</D:href>"), "{t}");

    s.stop().await;
    cleanup(&dir);
}

#[tokio::test]
async fn fs_state_is_readable_by_a_fresh_server_handle() {
    let dir = tmp_dir("reopen");
    let s = TestServer::start(Backend::Fs(dir.clone())).await;
    put(&s, "/.notes/protocol.json", b"{\"protocol\":1}").await;
    let dump1 = s.fs_dump();
    s.stop().await;

    // 换一个进程内句柄重新打开同一目录：这就是"客户端不得依赖服务端内存态"
    let s2 = TestServer::start(Backend::Fs(dir.clone())).await;
    let dump2 = s2.fs_dump();
    assert_eq!(
        dump1["entries"], dump2["entries"],
        "两个句柄看到的权威快照必须一致: {dump1} vs {dump2}"
    );
    assert_eq!(
        get(&s2, "/.notes/protocol.json").await.body,
        b"{\"protocol\":1}"
    );
    s2.stop().await;
    cleanup(&dir);
}

#[tokio::test]
async fn fs_reset_wipes_the_disk_too() {
    let dir = tmp_dir("fsreset");
    let s = TestServer::start(Backend::Fs(dir.clone())).await;
    put(&s, "/.notes/records/note/a.json", b"x").await;
    assert!(dir.join(".notes").exists());
    s.reset().await;
    assert_eq!(s.fs_dump()["count"], serde_json::json!(0));
    assert!(!dir.join(".notes").exists(), "RESET 必须连磁盘一起清");
    assert!(dir.exists(), "但 root 本身保留");
    s.stop().await;
    cleanup(&dir);
}

#[tokio::test]
async fn delete_and_move_are_reflected_on_disk() {
    let dir = tmp_dir("diskops");
    let s = TestServer::start(Backend::Fs(dir.clone())).await;
    put(&s, "/.notes/tmp/dev-a.json", b"tmp").await;
    assert!(dir.join(".notes").join("tmp").join("dev-a.json").exists());
    let dest = format!("http://{}/.notes/records/note/a.json", s.addr);
    let mv = send(
        s.addr,
        "MOVE",
        "/.notes/tmp/dev-a.json",
        &[support::h("destination", dest)],
        &[],
    )
    .await;
    assert_eq!(mv.status, 201);
    assert!(
        !dir.join(".notes").join("tmp").join("dev-a.json").exists(),
        "MOVE 后旧文件必须消失"
    );
    assert!(dir
        .join(".notes")
        .join("records")
        .join("note")
        .join("a.json")
        .exists());

    let del = send(s.addr, "DELETE", "/.notes/records/note/a.json", &[], &[]).await;
    assert_eq!(del.status, 204);
    assert!(!dir
        .join(".notes")
        .join("records")
        .join("note")
        .join("a.json")
        .exists());
    s.stop().await;
    cleanup(&dir);
}

#[tokio::test]
async fn mem_restart_keeps_state_but_bumps_generation() {
    // 契约里 `RESTART` 只对 Fs 有持久性含义；Mem 模式明确说明行为。
    let s = TestServer::start(Backend::Mem).await;
    put(&s, "/a.json", b"x").await;
    s.restart().await;
    assert_eq!(get(&s, "/a.json").await.status, 200);
    assert_eq!(s.generation(), 2);
}

async fn put(s: &TestServer, path: &str, body: &[u8]) -> support::RawResp {
    send(s.addr(), "PUT", path, &[], body).await
}

async fn get(s: &TestServer, path: &str) -> support::RawResp {
    send(s.addr(), "GET", path, &[], &[]).await
}
