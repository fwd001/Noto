//! 「落后设备追上大库」——§53 的"换设备自动恢复"在有真实体量的库上到底成不成立。
//!
//! 为什么值得单独一条：清单是"基线分段 ⊕ 变更窗口"两段式（SYNC-PROTOCOL §4.1），
//! 窗口上限 200 条（`WINDOW_MAX`）。库里超过 200 条变更之后走的是**另一条码路**：
//! `window.complete == false` 或本机 `seq_applied` 落后 → 回退去拉分段基线。小库测试
//! 全绿并不能证明那条路是通的 —— 它一次都没被走过。
//!
//! 这条测试要的证据很硬：260 条笔记的库，一台干净设备入伙之后必须**一条不少、内容
//! 一字不差**，而且不能靠"多点几次同步"才凑齐。
//!
//! 跑法：`cargo test -p notera-host --test late_device`
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use notera_host::commands::AccountDraftCmd;
use notera_host::App;
use notera_store::NoteQuery;
use notera_test_webdav::{Backend, TestServer};
use serde_json::json;

const SECRET: &str = "sup3r-s3cr3t";
/// 比 `WINDOW_MAX`（200）多出一截，确保溢出那条码路被踩到。
const NOTES: usize = 260;
static SEQ: AtomicUsize = AtomicUsize::new(0);

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("notera-late-{tag}-{}-{n}", std::process::id()));
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

struct Device {
    app: App,
    _dir: Tmp,
}

impl Device {
    fn boot(tag: &str, base_url: &str) -> Self {
        let dir = Tmp::new(tag);
        let app = App::boot(dir.path()).expect("核心启动");
        std::env::set_var("NOTERA_DEV_WEBDAV_SECRET", SECRET);
        let draft: AccountDraftCmd = serde_json::from_value(json!({
            "label": "大库", "baseUrl": base_url, "username": "notera-test",
        }))
        .unwrap();
        app.configure_account(draft).expect("配置账户");
        Self { app, _dir: dir }
    }
    fn fingerprints(&self) -> Vec<(String, String)> {
        let mut rows: Vec<(String, String)> = self
            .app
            .store()
            .list_notes(&NoteQuery::all())
            .unwrap()
            .into_iter()
            .map(|r| (r.title, r.content_hash))
            .collect();
        rows.sort();
        rows
    }
    /// 按调度器的节奏跑到**这一轮不再需要接着干**。注意"账目结清"只说明本机没有要推的
    /// 改动，**不等于追平**：拉的一侧本机永远是干净的， backlog 体现在 `Partial` 上。
    async fn settle(&self, cap: usize) -> Vec<String> {
        let mut trace = Vec::new();
        for _ in 0..cap {
            let stats = self.app.sync_once().await.expect("一轮同步");
            let st = self.app.store().stats().unwrap();
            trace.push(format!("{:?} pushed={} pulled={} dirty={} pending={}", stats.outcome, stats.pushed, stats.pulled, st.dirty_notes, st.outbox_pending));
            let drained = stats.outcome != notera_sync::RoundOutcome::Partial;
            if drained && st.dirty_notes == 0 && st.outbox_pending == 0 {
                return trace;
            }
        }
        trace
    }
}

fn doc(text: &str) -> serde_json::Value {
    json!({ "v": 1, "content": [{ "id": "blk000001", "type": "paragraph", "content": [{ "text": text }] }] })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fresh_device_joining_an_over_window_library_gets_every_note() {
    let dav = Tmp::new("dav");
    let srv = TestServer::start(Backend::Fs(dav.path().to_path_buf())).await;
    let url = srv.base_url();
    let a = Device::boot("late-a", &url);
    a.app.sync_once().await.expect("A 入伙");

    let folder = a.app.default_folder_id().unwrap();
    let titles: Vec<String> = (0..NOTES).map(|i| format!("大库笔记 {i:03}")).collect();
    for t in &titles {
        a.app.create_note(&folder, doc(t)).unwrap();
    }

    let trace = a.settle(12).await;
    let st = a.app.store().stats().unwrap();
    assert_eq!(st.notes, NOTES as u32, "A 本机就该有 {NOTES} 条：{st:?}");
    assert_eq!(st.dirty_notes, 0, "A 还没公告完：{trace:?}");
    assert_eq!(st.outbox_pending, 0, "A 的 outbox 没结清：{trace:?}");

    // 第二台设备：一座空库入伙，直接面对"变更比窗口还多"的清单
    let b = Device::boot("late-b", &url);
    // 服务器侧先排除"写漏"：窗口里就得是 261 条（260 篇 + 1 个文件夹）
    let root = srv.fs_root().expect("fs 后端应有根目录");
    let index: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(root.join(".notes/manifest/index.json")).expect("清单在盘上"),
    )
    .expect("清单是 JSON");
    let announced = index["window"]["entries"].as_array().map(|a| a.len()).unwrap_or(0);
    assert_eq!(announced, NOTES + 1, "清单公告的条目数就不对，后面怎么追都是徒劳");

    let btrace = b.settle(12).await;
    let bs = b.app.store().stats().unwrap();
    assert_eq!(bs.notes, NOTES as u32, "干净设备追完之后应有 {NOTES} 条，实际 {}：\n  A {}\n  B {}", bs.notes, trace.join("\n  "), btrace.join("\n  "));
    assert_eq!(b.fingerprints(), a.fingerprints(), "两台设备的标题/内容哈希不一致（漏了或变了）：\n  B {btrace:?}");
    assert_eq!(bs.fts_rows, bs.notes, "搜索索引没跟着笔记一起到位（{} vs {}）", bs.fts_rows, bs.notes);

    // 追平之后再来一条：不能因为已经踩过"全量回退"就再也推不动
    let extra = "追平之后又写的一条";
    b.app.create_note(&b.app.default_folder_id().unwrap(), doc(extra)).unwrap();
    b.settle(6).await;
    a.settle(6).await;
    assert_eq!(a.fingerprints().len(), NOTES + 1, "A 没收到 B 后来写的那条");
    assert!(a.fingerprints().iter().any(|(t, _)| t == extra), "后来那条标题不对");

    srv.stop().await;
}
