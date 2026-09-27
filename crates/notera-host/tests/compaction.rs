//! 清单压实到底有没有发生 —— 以及压实之后新设备还能不能追平。
//!
//! SYNC-PROTOCOL §4.1 设计的是"基线分段 ⊕ 变更窗口"：窗口上限 200 条，超过就该把窗口
//! 折进分段、重写受影响分段、再落一版只引用分段的索引。`compact()` 与 `put_segment`
//! 早就写好了，但**引擎一次都没调用过** —— 实测 260 条变更后服务器上是 `segments=0`、
//! 窗口 261 条：清单会在每次提交时整份重写，库越大每轮越贵，而文档写着"清单两段式压实"。
//!
//! 这条测试同时钉两侧，缺一不可信：
//! 1. **写侧**：超过窗口上限后，分段真的落盘、索引里的窗口回落到上限以内，且索引引用的
//!    分段在盘上都存在（INV-09：清单不得引用尚未写入的对象）；
//! 2. **读侧**：一座空库入伙，必须靠"分段基线"把 260 条全部追平 —— 压实之后能读回来，
//!    才算这个结构真的可用，而不是只把数据换了个地方藏起来。
//!
//! 跑法：`cargo test -p notera-host --test compaction`
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use notera_host::commands::AccountDraftCmd;
use notera_host::App;
use notera_store::NoteQuery;
use notera_sync::RoundOutcome;
use notera_test_webdav::{Backend, TestServer};
use serde_json::json;

const SECRET: &str = "sup3r-s3cr3t";
const NOTES: usize = 260;
static SEQ: AtomicUsize = AtomicUsize::new(0);

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("notera-compaction-{tag}-{}-{n}", std::process::id()));
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
            "label": "压实盘", "baseUrl": base_url, "username": "notera-test",
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
    /// 跑到"这一轮不再需要接着干"且本机账目结清。
    async fn settle(&self, cap: usize) -> Vec<String> {
        let mut trace = Vec::new();
        for _ in 0..cap {
            let stats = self.app.sync_once().await.expect("一轮同步");
            let st = self.app.store().stats().unwrap();
            trace.push(format!(
                "{:?} pushed={} pulled={} dirty={} pending={}",
                stats.outcome, stats.pushed, stats.pulled, st.dirty_notes, st.outbox_pending
            ));
            if stats.outcome != RoundOutcome::Partial && st.dirty_notes == 0 && st.outbox_pending == 0 {
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
async fn the_manifest_compacts_and_a_fresh_device_still_converges() {
    let dav = Tmp::new("dav");
    let srv = TestServer::start(Backend::Fs(dav.path().to_path_buf())).await;
    let url = srv.base_url();
    let a = Device::boot("comp-a", &url);
    a.app.sync_once().await.expect("A 入伙");
    let folder = a.app.default_folder_id().unwrap();
    for i in 0..NOTES {
        a.app.create_note(&folder, doc(&format!("压实笔记 {i:03}"))).unwrap();
    }
    let trace = a.settle(12).await;
    let st = a.app.store().stats().unwrap();
    assert_eq!(st.notes, NOTES as u32, "A 本机应有 {NOTES} 条：{st:?}");
    assert_eq!(st.dirty_notes, 0, "A 还没公告完：{trace:?}");

    let root = srv.fs_root().expect("fs 后端应有根目录");
    let index: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(root.join(".notes/manifest/index.json")).expect("索引在盘上"),
    )
    .expect("索引是 JSON");
    let window = index["window"]["entries"].as_array().map(|a| a.len()).unwrap_or(usize::MAX);
    let refs = index["segments"].as_array().cloned().unwrap_or_default();
    assert!(
        !refs.is_empty(),
        "260 条变更早超过窗口上限 200，索引里却一条分段都没有 —— 说明写侧从不压实，清单会一直长大：{trace:?}"
    );
    assert!(window <= notera_sync::manifest::WINDOW_MAX, "压实后窗口该回落到上限以内，实际 {window} 条");

    // INV-09：索引引用的每个分段都必须真的在盘上
    let dir = root.join(".notes/manifest");
    for r in &refs {
        let name = r["n"].as_str().unwrap_or_default();
        assert!(dir.join(format!("{name}.json")).exists(), "索引引用了不存在的分段 {name}");
        let on_disk = std::fs::read(dir.join(format!("{name}.json"))).expect("分段可读");
        assert_eq!(
            r["bytes"].as_u64().unwrap_or(usize::MAX as u64),
            on_disk.len() as u64,
            "分段 {name} 的 bytes 与盘上长度不一致"
        );
        let count = r["count"].as_u64().unwrap_or(0) as usize;
        let listed: serde_json::Value = serde_json::from_slice(&on_disk).expect("分段是 JSON");
        let arr = listed
            .as_array()
            .cloned()
            .or_else(|| listed.get("entries").and_then(|v| v.as_array()).cloned())
            .unwrap_or_default();
        assert_eq!(arr.len(), count, "分段 {name} 的 count 与实际条目数不一致");
    }

    // 读侧：空库入伙必须靠分段基线把 260 条全追回来
    let b = Device::boot("comp-b", &url);
    let btrace = b.settle(14).await;
    let bs = b.app.store().stats().unwrap();
    assert_eq!(bs.notes, NOTES as u32, "压实后新设备应追平到 {NOTES} 条，实际 {}：\n  A {}\n  B {}", bs.notes, trace.join("\n  "), btrace.join("\n  "));
    assert_eq!(b.fingerprints(), a.fingerprints(), "两台设备标题/内容哈希不一致");
    assert_eq!(bs.fts_rows, bs.notes, "搜索索引没跟着到位：{} vs {}", bs.fts_rows, bs.notes);

    // 压实过一次之后，任何一条新改动的 id 都必然落在已有分段的 cover 里。
    // 重叠率规则若只看比率不看量，这里就会为了一个字节的新笔记重写整份基线分段。
    a.app.create_note(&folder, doc("压实之后又写的一条")).unwrap();
    srv.clear_log().await;
    a.settle(6).await;
    let rewritten: Vec<String> = srv
        .request_log()
        .iter()
        .filter(|r| r.method == "PUT" && r.path.contains("/manifest/seg-"))
        .map(|r| format!("{} ({} B)", r.path, r.bytes))
        .collect();
    assert!(rewritten.is_empty(), "只改了一条笔记就把基线分段重写了：{rewritten:?}");

    srv.stop().await;
}
