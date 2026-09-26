//! §8 的附件断点续传（`resume` / `range`）—— 两台设备 + 真 TCP 服务器上跑。
//!
//! 要证的不是"能下下来"（那上一条 `sync_once` 已经验过），而是**中途没下完时
//! 进度留在磁盘上，重启之后从断点接着要**：
//! 1. 第一轮只拿到一个窗口（4 MiB），`.part` 恰好那么多，正式 blob 不存在；
//! 2. 进程重启（换一个 `App` 实例开同一座库）再跑一轮 → 补齐、哈希相符、`.part` 清掉；
//! 3. 附件路径上总共只有 2 次 GET —— 若没有续传，第二轮会整块重下一遍。
//!
//! 第二条测试换的是**服务器的脾气**：探得出 206 却对正式请求忽略 `Range`（答 200 +
//! 全文）。那种答复必须当整份覆盖，不能往半截文件后面追加。
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use notera_host::commands::AccountDraftCmd;
use notera_host::App;
use notera_test_webdav::{Backend, Injection, TestServer};
use serde_json::json;

const SECRET: &str = "sup3r-s3cr3t";
const WINDOW: u64 = 4 * 1024 * 1024;
static SEQ: AtomicUsize = AtomicUsize::new(0);

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("notera-resume-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn doc(text: &str) -> serde_json::Value {
    json!({ "v": 1, "content": [{ "id": "blk000001", "type": "paragraph", "content": [{ "text": text }] }] })
}

fn boot(dir: &Path, url: &str) -> App {
    std::env::set_var("NOTERA_DEV_WEBDAV_SECRET", SECRET);
    let app = App::boot(dir).expect("核心启动");
    if app.current_account().unwrap().is_none() {
        let draft: AccountDraftCmd = serde_json::from_value(json!({
            "label": "续传盘", "baseUrl": url, "username": "notera-test",
        }))
        .unwrap();
        app.configure_account(draft).expect("配置账户");
    }
    app
}

/// 按编辑器的真实顺序把一张"大图"挂上去：先落 blob，再把引用写进正文，然后同步 + 传附件。
async fn seed_source(dir: &Path, url: &str, blob: &[u8]) -> String {
    let app = boot(dir, url);
    app.sync_once().await.expect("A 入伙");
    let folder = app.default_folder_id().unwrap();
    let note = app.create_note(&folder, doc("带大图的笔记")).unwrap();
    let id = notera_core::EntityId::parse(&note.id).unwrap();
    let sha = app
        .store()
        .attach_blob(&id, blob, "image/png", Some("big.png"), "blk000002")
        .unwrap()
        .sha256;
    let head = app.store().get_note(&id).unwrap().unwrap();
    app.store()
        .edit_note(
            &id,
            json!({ "v": 1, "content": [
                { "id": "blk000001", "type": "paragraph", "content": [{ "text": "带大图的笔记" }] },
                { "id": "blk000002", "type": "image", "attrs": {
                    "sha256": &sha, "ref": &sha, "role": "inline", "pending": false,
                    "size": blob.len(), "mediaType": "image/png", "name": "big.png" } },
            ] }),
            head.rev,
        )
        .unwrap();
    app.sync_once().await.expect("A 推正文");
    let remote = app.remote_for_sync().await.unwrap().expect("A 有适配器");
    let round = app.run_attachment_round(&remote).await;
    assert_eq!(round, (1, 0, 0), "源设备要把这张图传上去：{round:?}");
    sha
}

/// 到某一刻为止，附件对象上发生过哪些请求（`seq 方法 -> 状态`）。
fn attachment_requests(srv: &TestServer, sha: &str, up_to: usize) -> Vec<String> {
    srv.request_log()
        .iter()
        .take(up_to)
        .filter(|r| r.path.contains(sha))
        .map(|r| format!("{} {} -> {}", r.seq, r.method, r.status))
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_partial_attachment_keeps_its_progress_and_finishes_after_a_restart() {
    let srv = TestServer::start(Backend::Mem).await;
    let url = srv.base_url();
    // 比一个窗口多 1234 字节：正好逼出"两次请求、中间留在磁盘上"
    let blob: Vec<u8> = (0..WINDOW as usize + 1234).map(|i| (i % 251) as u8).collect();

    let a_dir = Tmp::new("source");
    let sha = seed_source(a_dir.path(), &url, &blob).await;

    let b_dir = Tmp::new("target");
    let b = boot(b_dir.path(), &url);
    b.sync_once().await.expect("B 拉到正文");
    assert_eq!(
        b.store().attachment_downloads(4).unwrap().len(),
        1,
        "正文里的引用要在 B 这边排成一条下载任务"
    );
    let remote_b = b.remote_for_sync().await.unwrap().expect("B 有适配器");
    let first = b.run_attachment_round(&remote_b).await;
    let part = b.store().blob_part_path(&sha);
    let final_path = b.store().blob_path(&sha);
    assert_eq!(first.1, 0, "第一轮只该拿一个窗口，不该算下完：{first:?}");
    assert_eq!(std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0), WINDOW, "半截文件应恰好推进一个窗口");
    assert!(!final_path.exists(), "没校验通过之前，正式 blob 不许出现（§8：不把半上传文件当成完整文件）");
    // 到此为止，附件对象上只该出现"源设备上传之后自己核对一次"的那一回
    let after_first = srv.request_log().len();
    drop(b);

    // 重启：换一个 App 实例开同一座库，进度必须还在
    let b2 = boot(b_dir.path(), &url);
    let remote2 = b2.remote_for_sync().await.unwrap().expect("重启后仍有适配器");
    let second = b2.run_attachment_round(&remote2).await;
    assert_eq!(second.1, 1, "第二轮要从断点补齐：{second:?}");
    assert_eq!(std::fs::read(&final_path).unwrap(), blob, "续传拼出来的字节要一字不差");
    assert!(!part.exists(), "补齐之后半截文件要清掉，否则它会被当成还在下载的东西");
    let (local, _remote) = b2.store().attachment_for_state(&sha);
    assert_eq!(local.as_str(), "available", "账上要认这份附件已经在本机");

    let seen: Vec<String> = srv
        .request_log()
        .iter()
        .filter(|r| r.path.contains(&sha))
        .map(|r| format!("{} {} -> {}", r.seq, r.method, r.status))
        .collect();
    let in_round2 = attachment_requests(&srv, &sha, srv.request_log().len())
        .into_iter()
        .skip(attachment_requests(&srv, &sha, after_first).len())
        .collect::<Vec<_>>();
    assert_eq!(in_round2.len(), 1, "重启后那一轮只该再要一个窗口（不是整块重来）：{seen:?}");
    assert!(in_round2.first().is_some_and(|l| l.ends_with("GET -> 206")), "重启后那次必须是带 Range 的续取：{in_round2:?}");
    let windows = seen.iter().filter(|l| l.contains("206")).count();
    assert_eq!(windows, 2, "两次 206 才对（一轮一个窗口）：{seen:?}");
    srv.stop().await;
}

/// §14 兼容矩阵里最难缠的一类：这台服务器**探得出 206，正式请求却忽略 `Range`**。
///
/// 客户端于是收到"我明明从 4 MiB 续要，你却把整份给我了"。如果照 `from=4 MiB`
/// 追加上去，拼出来就是一份**内容重复**的文件 —— 哈希对不上，用户那边的表现是
/// "这张图永远下不下来"。正确做法是按整份覆盖。这一条只有真服务器能证：
/// 单元测的是判据，这里测的是磁盘上真的落对了字节。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_server_that_ignores_range_still_lands_the_right_bytes() {
    let srv = TestServer::start(Backend::Mem).await;
    let url = srv.base_url();
    let blob: Vec<u8> = (0..WINDOW as usize + 1234).map(|i| (i % 197) as u8).collect();

    let a_dir = Tmp::new("lie-source");
    let sha = seed_source(a_dir.path(), &url, &blob).await;

    let b_dir = Tmp::new("lie-target");
    let b = boot(b_dir.path(), &url);
    b.sync_once().await.expect("B 拉到正文");
    let remote = b.remote_for_sync().await.unwrap().expect("B 有适配器");
    let first = b.run_attachment_round(&remote).await;
    let part = b.store().blob_part_path(&sha);
    assert_eq!(first.1, 0, "第一轮按窗口停在半截：{first:?}");
    assert_eq!(std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0), WINDOW);
    drop(b);

    // 从这里开始，服务器不再理 Range 头（探测记录还在，客户端仍按"支持 Range"发请求）。
    srv.clear_log().await;
    srv.inject(Injection::ignore_range()).await;

    let b2 = boot(b_dir.path(), &url);
    let remote2 = b2.remote_for_sync().await.unwrap().expect("重启后仍有适配器");
    let second = b2.run_attachment_round(&remote2).await;
    assert_eq!(second.1, 1, "整份答复也要把这一条结清：{second:?}");

    let got = std::fs::read(b2.store().blob_path(&sha)).unwrap_or_default();
    assert_eq!(got, blob, "服务器给的是全文就必须当全文用：追加会拼出一份内容重复的文件");
    assert!(!part.exists(), "半截文件不能留在盘上");
    let (local, _remote) = b2.store().attachment_for_state(&sha);
    assert_eq!(local.as_str(), "available");

    let seen: Vec<String> = srv
        .request_log()
        .iter()
        .filter(|r| r.path.contains(&sha))
        .map(|r| format!("{} -> {}", r.method, r.status))
        .collect();
    assert_eq!(seen.as_slice(), ["GET -> 200".to_string()].as_slice(), "第二轮该被答成整份 200，而且只要一次：{seen:?}");
    srv.stop().await;
}
