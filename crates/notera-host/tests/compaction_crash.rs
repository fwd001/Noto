//! 「分段已经写到服务器上、索引还没换版」这一刀真的崩过之后，还能恢复吗。
//!
//! 压实（SYNC-PROTOCOL §4.1）引入的这个崩溃面是新的：写侧一旦压实，就有**两次**远端写入
//! 要先后来（先 `seg-*.json`，后 `index.json`），中间那一刀落下时服务器上会存在"没有任何
//! 索引引用的孤儿分段"，而索引仍指向旧分段。顺序对了才不会坏（INV-09：清单不得引用尚未
//! 写入的对象），但"对了"必须由真死一次来证明，不能靠我嘴上论证。
//!
//! 9 点崩溃矩阵的子进程夹具只写 1 条笔记，窗口离 `WINDOW_MAX`(200) 远得很，永远走不到
//! 压实这一步 —— 所以这个注入点单独用一座大库夹具来打，名单来自
//! `notera_core::CRASH_POINTS_NEED_LARGE_LIBRARY`（减法在 `crash_recovery.rs` 那边做，
//! 两边合起来才等于全表，漏一格就红）。
//!
//! 跑法：`cargo test -p notera-host --test compaction_crash`
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

use notera_host::commands::AccountDraftCmd;
use notera_host::App;
use notera_store::NoteQuery;
use notera_sync::RoundOutcome;
use notera_test_webdav::{Backend, TestServer};
use serde_json::json;

const SECRET: &str = "sup3r-s3cr3t";
const CHILD: &str = "NOTERA_CC_CHILD";
/// 超过窗口上限，压实这一支才会被走到
const NOTES: usize = 240;
static SEQ: AtomicUsize = AtomicUsize::new(0);

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("notera-ccrash-{tag}-{}-{n}", std::process::id()));
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
    // 子进程已经配好账户了；重启同一座库不能再配第二个（ADR-0018 的单一活跃账户约束
    // 会直接拒绝，而这正是我们要的行为 —— 所以这里按"有没有"决定要不要配）。
    if app.current_account().unwrap().is_none() {
        let draft: AccountDraftCmd = serde_json::from_value(json!({
            "label": "压实崩溃盘", "baseUrl": url, "username": "notera-test",
        }))
        .unwrap();
        app.configure_account(draft).expect("配置账户");
    }
    app
}

fn fingerprints(app: &App) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = app
        .store()
        .list_notes(&NoteQuery::all())
        .unwrap()
        .into_iter()
        .map(|r| (r.title, r.content_hash))
        .collect();
    rows.sort();
    rows
}

/// 子进程：写满 {NOTES} 条笔记，然后一轮一轮同步，直到被 `NOTERA_CRASH_AT` 打死。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn child_writes_a_big_library_until_it_is_killed() {
    let Ok(spec) = std::env::var(CHILD) else { return };
    let Some((dir, url)) = spec.split_once('|') else { return };
    let app = boot(Path::new(dir), url);
    app.sync_once().await.expect("入伙");
    let folder = app.default_folder_id().unwrap();
    for i in 0..NOTES {
        app.create_note(&folder, doc(&format!("压实崩溃笔记 {i:03}"))).unwrap();
    }
    // 打死我们之前应该至少提交过几版清单（窗口要长过 200 才会压实）
    for round in 0..40 {
        let stats = app.sync_once().await.expect("子进程的一轮");
        if stats.outcome != RoundOutcome::Partial {
            eprintln!("子进程没死在点上：第 {round} 轮 {stats:?}");
            return;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_crash_right_after_the_segment_write_leaves_a_readable_library() {
    let dav = Tmp::new("dav");
    let srv = TestServer::start(Backend::Fs(dav.path().to_path_buf())).await;
    let url = srv.base_url();
    let a_dir = Tmp::new("a");

    for point in notera_core::CRASH_POINTS_NEED_LARGE_LIBRARY {
        assert!(
            notera_core::CRASH_POINTS.contains(point),
            "大库名单里的 {point} 忘了登记进 CRASH_POINTS"
        );
        let code = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "child_writes_a_big_library_until_it_is_killed", "--nocapture", "--test-threads=1"])
            .env(CHILD, format!("{}|{}", a_dir.path().display(), url))
            .env("NOTERA_CRASH_AT", *point)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn")
            .wait()
            .expect("wait")
            .code()
            .unwrap_or(-1);
        assert_eq!(code, notera_core::CRASH_EXIT_CODE, "{point} 没让进程死在那里（退出码 {code}）—— 这个注入点没被走到，下面的一切断言都是空的");

        // 崩完之后服务器必须仍然是"自洽"的：索引引用的分段都要在盘上
        let root = srv.fs_root().expect("fs 后端应有根目录");
        let index: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(root.join(".notes/manifest/index.json")).expect("索引在盘上"),
        )
        .expect("索引是 JSON");
        let window = index["window"]["entries"].as_array().map(|a| a.len()).unwrap_or(0);
        let refs = index["segments"].as_array().cloned().unwrap_or_default();
        for r in &refs {
            let name = r["n"].as_str().unwrap_or_default();
            assert!(
                root.join(format!(".notes/manifest/{name}.json")).exists(),
                "崩在 {point} 之后，索引引用了一个不存在的分段 {name}（INV-09 破了）"
            );
        }
        assert!(window <= notera_sync::manifest::WINDOW_MAX, "窗口 {window} 超过上限却没压实");
    }

    // 重启同一座库：这一刀不能丢数据，也不能让改动永远悬着
    let a = boot(a_dir.path(), &url);
    let mut trace = Vec::new();
    let mut settled = false;
    for _ in 0..14 {
        let stats = a.sync_once().await.expect("重启后的一轮");
        let st = a.store().stats().unwrap();
        trace.push(format!("{:?} pushed={} pulled={} dirty={} pending={}", stats.outcome, stats.pushed, stats.pulled, st.dirty_notes, st.outbox_pending));
        if stats.outcome != RoundOutcome::Partial && st.dirty_notes == 0 && st.outbox_pending == 0 {
            settled = true;
            break;
        }
    }
    assert!(settled, "崩过一次之后本机一直结不清：\n  {}", trace.join("\n  "));
    let st = a.store().stats().unwrap();
    assert_eq!(st.notes, NOTES as u32, "崩在压实那一刀之后本机笔记数就变了：{st:?}");

    // 换设备：孤儿分段不影响读，另一台设备要拿到全部 240 条
    let b_dir = Tmp::new("b");
    let b = boot(b_dir.path(), &url);
    let mut btrace = Vec::new();
    for _ in 0..14 {
        let stats = b.sync_once().await.expect("B 的一轮");
        btrace.push(format!("{:?} pulled={}", stats.outcome, stats.pulled));
        if stats.outcome != RoundOutcome::Partial {
            break;
        }
    }
    assert_eq!(b.store().stats().unwrap().notes, NOTES as u32, "另一台设备没追平：{btrace:?}");
    assert_eq!(fingerprints(&b), fingerprints(&a), "崩过一次之后两台设备的标题/内容哈希不一致");

    srv.stop().await;
}
