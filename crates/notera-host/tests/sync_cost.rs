//! 空轮的代价（PERF-05 与 PERF-14）—— 一行为什么也要有门禁。
//!
//! 后台调度器每 25 秒就要跑一轮。库里没变化的时候这一轮必须是"一次请求、几乎零字节"，
//! 否则笔记本上什么都没干也在满负荷打网络：`≤2 请求 / 总传输 ≤2 KiB / manifest 拿 304`
//! 是 SYNC-PROTOCOL §6.3 写死的，PROPFIND 每轮 ≤1 是 PERF-14。
//!
//! 这一条还是**本轮 304 改动的回归护栏**：为了让"被预算截断的下载"不再被误判成已同步，
//! 304 快路径现在要多问一句本机结清没有；问错了方向，代价就是每台设备每 25 秒多下一次
//! 全量清单（5000 条时那是 186 KiB 一轮）。
//!
//! 跑法：`cargo test -p notera-host --test sync_cost`
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use notera_host::commands::AccountDraftCmd;
use notera_host::App;
use notera_sync::RoundOutcome;
use notera_test_webdav::{Backend, TestServer};
use serde_json::json;

const SECRET: &str = "sup3r-s3cr3t";
static SEQ: AtomicUsize = AtomicUsize::new(0);

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("notera-cost-{tag}-{}-{n}", std::process::id()));
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

fn boot(dir: &Path, url: &str) -> App {
    std::env::set_var("NOTERA_DEV_WEBDAV_SECRET", SECRET);
    let app = App::boot(dir).expect("核心启动");
    let draft: AccountDraftCmd = serde_json::from_value(json!({
        "label": "空轮盘", "baseUrl": url, "username": "notera-test",
    }))
    .unwrap();
    app.configure_account(draft).expect("配置账户");
    app
}

fn doc(text: &str) -> serde_json::Value {
    json!({ "v": 1, "content": [{ "id": "blk000001", "type": "paragraph", "content": [{ "text": text }] }] })
}

async fn converge(app: &App, cap: usize) {
    for _ in 0..cap {
        let stats = app.sync_once().await.expect("一轮同步");
        let st = app.store().stats().unwrap();
        if stats.outcome != RoundOutcome::Partial && st.dirty_notes == 0 && st.outbox_pending == 0 {
            return;
        }
    }
    panic!("{cap} 轮还没结清");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_idle_round_costs_one_request_and_no_body() {
    let srv = TestServer::start(Backend::Mem).await;
    let url = srv.base_url();
    let a_dir = Tmp::new("cost-a");
    let b_dir = Tmp::new("cost-b");
    let a = boot(a_dir.path(), &url);
    let b = boot(b_dir.path(), &url);

    // 两边都有内容并互相收敛 —— 之后才是"什么都没改"的空轮
    let folder = a.default_folder_id().unwrap();
    for i in 0..12 {
        a.create_note(&folder, doc(&format!("空轮笔记 {i}")))
            .unwrap();
    }
    converge(&a, 8).await;
    converge(&b, 8).await;
    let folder_b = b.default_folder_id().unwrap();
    b.create_note(&folder_b, doc("对面后来写的一条")).unwrap();
    converge(&b, 8).await;
    converge(&a, 8).await;

    // 第一版：对面刚写过，这一轮"读到变化"是应该的（整份清单一次）
    let warm = a.sync_once().await.expect("读到变化的一轮");
    assert!(
        warm.requests <= 2,
        "读到变化的一轮用了 {} 次请求",
        warm.requests
    );
    // 空轮：只许一次请求、零正文、清单走 304
    srv.clear_log().await;
    let stats = a.sync_once().await.expect("空轮");
    assert_eq!(
        stats.outcome,
        RoundOutcome::NoOp,
        "什么都没改，这一轮却有活干：{stats:?}"
    );
    assert!(
        stats.requests <= 2,
        "空轮用了 {} 次请求（预算 ≤2）",
        stats.requests
    );
    let transferred = stats.bytes_up + stats.bytes_down;
    assert!(
        transferred <= 2048,
        "空轮传输了 {transferred} 字节（预算 ≤2 KiB）"
    );
    let log = srv.request_log();
    let index_304 = log
        .iter()
        .any(|r| r.status == 304 && r.path.contains("index.json"));
    assert!(
        index_304,
        "清单没走条件请求（304），每轮都在整份下载：{}",
        log.iter()
            .map(|r| format!("{} {} -> {}", r.method, r.path, r.status))
            .collect::<Vec<_>>()
            .join(" | ")
    );
    let propfinds = log.iter().filter(|r| r.method == "PROPFIND").count();
    assert!(
        propfinds <= 1,
        "一轮里发了 {propfinds} 次 PROPFIND（预算 ≤1）"
    );

    // 对面也没活干：同样便宜
    srv.clear_log().await;
    let bs = b.sync_once().await.expect("对端的空轮");
    assert_eq!(bs.outcome, RoundOutcome::NoOp);
    assert!(bs.requests <= 2, "对端空轮用了 {} 次请求", bs.requests);

    srv.stop().await;
}
