//! 千条规模的库追平 —— 把"大库"从一次性实测变成常驻门禁。
//!
//! 260 条那两条（`late_device` / `compaction`）钉的是"跨过窗口上限"这个**边界**；
//! 它们不能回答两个只有体量才暴露的问题：
//! 1. **追平要转多少轮**：每轮请求预算 `round_request_cap=200`，1000 条的下载如果
//!    预算账算错，新设备可以一直停在 `Partial` 转圈 —— 徽标会诚实地写"正在同步"，
//!    但用户等的是"什么时候完"。这里给一个硬上界，超了就红。
//! 2. **清单结构在千条下还成不成立**：索引必须只装引用（分段列表 + ≤200 条窗口），
//!    否则每轮空转都要搬整份库；同时一条新改动不许回头重写基线分段。
//!
//! 断言全部打在**外部结果**上：盘上的清单文件、真 TCP 服务器的请求日志、两台设备
//! 逐条 (标题, 内容哈希) 一致 —— 不是"某个函数被调用过"。
//!
//! 跑法：`cargo test -p notera-host --test big_library`
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use notera_host::commands::AccountDraftCmd;
use notera_host::App;
use notera_store::NoteQuery;
use notera_sync::manifest::{SEGMENT_TARGET, WINDOW_MAX};
use notera_sync::RoundOutcome;
use notera_test_webdav::{Backend, TestServer};
use serde_json::json;

const SECRET: &str = "sup3r-s3cr3t";
const NOTES: usize = 1000;
/// 逐条比对时一次读满。`limit: 0` 在 store 里是"默认 500 条"，千条库上只比前 500 行
/// 等于没比 —— 本轮实测：两台设备各 1001 条，截断后都恰好回 500 行并且"相等"。
const ROW_CAP: u32 = 100_000;

/// 追平的轮数上界。1000 条 + 1 个文件夹 = 1001 个下载，每轮预算 200 条写入 / 请求，
/// 理论上 6~8 轮；留一倍余量，仍然远小于"转不停"。
const CATCH_UP_ROUND_CAP: usize = 16;
/// 索引只许装引用：千条若整份摊在索引里，这里就是几百 KiB。
const INDEX_BUDGET_BYTES: usize = 8 * 1024;

static SEQ: AtomicUsize = AtomicUsize::new(0);

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("notera-big-{tag}-{}-{n}", std::process::id()));
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
            "label": "千库", "baseUrl": base_url, "username": "notera-test",
        }))
        .unwrap();
        app.configure_account(draft).expect("配置账户");
        Self { app, _dir: dir }
    }
    fn fingerprints(&self) -> Vec<(String, String)> {
        let mut rows: Vec<(String, String)> = self
            .app
            .store()
            .list_notes(&NoteQuery {
                limit: ROW_CAP,
                ..NoteQuery::all()
            })
            .unwrap()
            .into_iter()
            .map(|r| (r.title, r.content_hash))
            .collect();
        rows.sort();
        rows
    }
    /// 按调度器的节奏跑到"这一轮不需要接着干"且本机账目结清；返回每一轮的痕迹。
    async fn settle(&self, cap: usize) -> Vec<String> {
        let mut trace = Vec::new();
        for _ in 0..cap {
            let stats = self.app.sync_once().await.expect("一轮同步");
            let st = self.app.store().stats().unwrap();
            trace.push(format!(
                "{:?} pushed={} pulled={} dirty={} pending={}",
                stats.outcome, stats.pushed, stats.pulled, st.dirty_notes, st.outbox_pending
            ));
            if stats.outcome != RoundOutcome::Partial
                && st.dirty_notes == 0
                && st.outbox_pending == 0
            {
                return trace;
            }
        }
        trace
    }
}

fn doc(text: &str) -> serde_json::Value {
    json!({ "v": 1, "content": [{ "id": "blk000001", "type": "paragraph", "content": [{ "text": text }] }] })
}

/// 清单实际公告的条目数：窗口里的 + 每个分段里真实读盘数出来的条目。
/// 不用索引自报的 count，免得它自己就对不上账。
fn announced_entries(index: &serde_json::Value, dir: &Path) -> usize {
    let mut n = index["window"]["entries"]
        .as_array()
        .map(|v| v.len())
        .unwrap_or(0);
    for s in index["segments"].as_array().cloned().unwrap_or_default() {
        let name = s["n"].as_str().unwrap_or_default();
        let body =
            std::fs::read(dir.join(format!("{name}.json"))).expect("索引引用的分段必须在盘上");
        let listed: serde_json::Value = serde_json::from_slice(&body).expect("分段是 JSON");
        n += listed
            .as_array()
            .cloned()
            .or_else(|| listed.get("entries").and_then(|v| v.as_array()).cloned())
            .map(|v| v.len())
            .unwrap_or(0);
    }
    n
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_thousand_note_library_catches_up_within_a_bounded_number_of_rounds() {
    let started = Instant::now();
    let dav = Tmp::new("dav");
    let srv = TestServer::start(Backend::Fs(dav.path().to_path_buf())).await;
    let url = srv.base_url();

    // ── 写侧：一台设备把 1000 条推上去 ──────────────────────────────────
    let a = Device::boot("big-a", &url);
    a.app.sync_once().await.expect("A 入伙");
    let folder = a.app.default_folder_id().unwrap();
    for i in 0..NOTES {
        a.app
            .create_note(&folder, doc(&format!("千库笔记 {i:04}")))
            .unwrap();
    }
    let atrace = a.settle(CATCH_UP_ROUND_CAP).await;
    let ast = a.app.store().stats().unwrap();
    assert_eq!(ast.notes, NOTES as u32, "A 本机就该有 {NOTES} 条：{ast:?}");
    assert_eq!(ast.dirty_notes, 0, "A 公告没结清：{atrace:?}");
    assert_eq!(ast.outbox_pending, 0, "A 的 outbox 没结清：{atrace:?}");
    let pushed_in = atrace.len();
    assert!(
        pushed_in <= CATCH_UP_ROUND_CAP,
        "{NOTES} 条公告用了 {} 轮，超过上界 {CATCH_UP_ROUND_CAP}：{atrace:?}",
        pushed_in
    );

    // ── 清单结构：索引只装引用 ────────────────────────────────────────
    let root = srv.fs_root().expect("fs 后端应有根目录");
    let index_raw = std::fs::read(root.join(".notes/manifest/index.json")).expect("索引在盘上");
    let index: serde_json::Value = serde_json::from_slice(&index_raw).expect("索引是 JSON");
    assert!(
        index_raw.len() <= INDEX_BUDGET_BYTES,
        "{NOTES} 条的索引有 {} B（预算 {INDEX_BUDGET_BYTES} B）—— 索引在装条目而不是装引用，每轮空转都要整份搬",
        index_raw.len()
    );
    let window = index["window"]["entries"]
        .as_array()
        .map(|v| v.len())
        .unwrap_or(usize::MAX);
    assert!(
        window <= WINDOW_MAX,
        "压实后窗口 {window} 条，超过上限 {WINDOW_MAX}"
    );
    let refs = index["segments"].as_array().cloned().unwrap_or_default();
    assert!(
        !refs.is_empty(),
        "1000 条早超过窗口上限，索引里却没有分段 —— 基线没被折出去：{atrace:?}"
    );
    let mut announced = window;
    let dir = root.join(".notes/manifest");
    for r in &refs {
        let name = r["n"].as_str().unwrap_or_default();
        let body =
            std::fs::read(dir.join(format!("{name}.json"))).expect("索引引用的分段必须在盘上");
        let listed: serde_json::Value = serde_json::from_slice(&body).expect("分段是 JSON");
        let arr = listed
            .as_array()
            .cloned()
            .or_else(|| listed.get("entries").and_then(|v| v.as_array()).cloned())
            .unwrap_or_default();
        assert_eq!(
            r["count"].as_u64().unwrap_or(0) as usize,
            arr.len(),
            "分段 {name} 的 count 与实际条目数不一致"
        );
        assert!(
            arr.len() <= SEGMENT_TARGET,
            "分段 {name} 有 {} 条，超过每段目标 {SEGMENT_TARGET}",
            arr.len()
        );
        announced += arr.len();
    }
    assert_eq!(
        announced,
        NOTES + 1,
        "清单公告的条目总数（分段 + 窗口）不等于 {NOTES} 篇 + 1 个文件夹"
    );

    // ── 读侧：空库入伙，轮数有界、徽标说实话、逐条一致 ─────────────────
    let b = Device::boot("big-b", &url);
    let first = b.app.sync_once().await.expect("B 的第一轮");
    assert_eq!(
        first.outcome,
        RoundOutcome::Partial,
        "夹具要改：千条的第一轮该被预算截断：{first:?}"
    );
    assert_eq!(
        b.app.sync_status().unwrap().badge,
        "syncing",
        "被截断的一轮之后界面仍报「已同步」—— 库里差着几百条"
    );
    let btrace = b.settle(CATCH_UP_ROUND_CAP).await;
    let caught_up = btrace.len() + 1;
    assert!(
        caught_up <= CATCH_UP_ROUND_CAP,
        "{NOTES} 条追平用了 {caught_up} 轮，超过上界 {CATCH_UP_ROUND_CAP}：\n  A {}\n  B {}",
        atrace.join("\n  "),
        btrace.join("\n  ")
    );

    let bst = b.app.store().stats().unwrap();
    assert_eq!(
        bst.notes,
        NOTES as u32,
        "干净设备追完之后应有 {NOTES} 条，实际 {}：\n  B {}",
        bst.notes,
        btrace.join("\n  ")
    );
    assert_eq!(
        bst.fts_rows, bst.notes,
        "搜索索引没跟着笔记到位（{} vs {}）",
        bst.fts_rows, bst.notes
    );
    assert_eq!(
        b.fingerprints(),
        a.fingerprints(),
        "两台设备标题/内容哈希不一致（漏了或变了）：\n  B {}",
        btrace.join("\n  ")
    );
    assert_eq!(
        b.app.sync_status().unwrap().badge,
        "synced",
        "追平之后徽标没回到「已同步」"
    );

    // ── 角色实体：默认本在两台设备上必须是**同一个**条目 ────────────────
    // 默认本是"每个账户有且只有一个"的角色实体。它以前由各台设备各造一个随机 id，
    // 于是第二台入伙之后：本机出现两个都叫「默认本」的文件夹，远端清单里也多出一条
    // folder 条目，并且双方的那一份会互相拉回来。实测（修 id 之前）A=1 / B=2、清单 1002 条。
    let atrace2 = a.settle(6).await;
    let fa = a.app.store().list_folders().unwrap();
    let fb = b.app.store().list_folders().unwrap();
    assert_eq!(
        fa.len(),
        1,
        "A 上有 {} 个文件夹，默认本被复制了：{:?} {}",
        fa.len(),
        fa.iter().map(|f| f.name.clone()).collect::<Vec<_>>(),
        atrace2.join(" | ")
    );
    assert_eq!(
        fb.len(),
        1,
        "B 追平之后有 {} 个都叫默认本的文件夹：{:?}",
        fb.len(),
        fb.iter()
            .map(|f| (f.id.to_string(), f.name.clone()))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        fa[0].id.as_str(),
        fb[0].id.as_str(),
        "两台设备的默认本 id 不同 —— 它们在清单里就是两个条目，迟早各自长出一批笔记"
    );

    // 远端清单公告的条目数：{NOTES} 篇 + 1 个默认本，不多不少。
    let post: serde_json::Value = serde_json::from_slice(
        &std::fs::read(root.join(".notes/manifest/index.json")).expect("索引仍在盘上"),
    )
    .expect("索引是 JSON");
    let announced_after = announced_entries(&post, &dir);
    assert_eq!(
        announced_after,
        NOTES + 1,
        "追平之后清单公告的条目数涨了 —— 有实体被复制：{announced_after}"
    );

    // ── 千条规模下的空轮代价（PERF-05 的量级版）────────────────────────
    // sync_cost 用 12 条钉的是"一次请求"；那条在 506 B 的索引上过得很轻松。
    // 真正会咬人的是**清单变大之后**缓存/条件请求失效：每 25 秒搬一次 9~17 KiB 的
    // 索引，笔记本 idle 也在烧流量与电。这里要的是零正文。
    srv.clear_log().await;
    let idle = b.app.sync_once().await.expect("追平后的空轮");
    let idle_log = srv.request_log();
    let idle_lines: Vec<String> = idle_log
        .iter()
        .filter(|r| r.path.contains("/manifest/"))
        .map(|r| format!("{} {} -> {}", r.method, r.path, r.status))
        .collect();
    assert_eq!(
        idle.outcome,
        RoundOutcome::NoOp,
        "什么都没改，千库上这一轮却有活干：{idle:?}"
    );
    assert_eq!(
        idle.requests, 1,
        "千库空轮用了 {} 次请求（应为 1 次条件请求）：{idle_lines:?}",
        idle.requests
    );
    assert_eq!(
        idle.bytes_down, 0,
        "千库空轮下载了 {} B 正文 —— 条件请求没命中，每轮都在重搬清单",
        idle.bytes_down
    );
    assert_eq!(
        idle.bytes_up, 0,
        "千库空轮上行 {} B —— 没改动却公告了东西",
        idle.bytes_up
    );
    assert!(
        idle_log
            .iter()
            .any(|r| r.status == 304 && r.path.ends_with("/manifest/index.json")),
        "千库空轮没走 304：{idle_lines:?}"
    );

    // ── 收敛后的增量：一条改动只该花一条的代价 ────────────────────────
    let extra = "千库追平之后又写的一条";
    b.app
        .create_note(&b.app.default_folder_id().unwrap(), doc(extra))
        .unwrap();
    srv.clear_log().await;
    let incr = b.settle(6).await;
    a.settle(6).await;
    assert_eq!(
        a.fingerprints().len(),
        NOTES + 1,
        "A 没收到 B 后来写的那条：{incr:?}"
    );
    assert!(
        a.fingerprints().iter().any(|(t, _)| t == extra),
        "后来那条标题不对"
    );

    let rewritten: Vec<String> = srv
        .request_log()
        .iter()
        .filter(|r| r.method == "PUT" && r.path.contains("/manifest/seg-"))
        .map(|r| format!("{} ({} B)", r.path, r.bytes))
        .collect();
    assert!(
        rewritten.is_empty(),
        "{NOTES} 条的库里只改一条，就把基线分段重写了：{rewritten:?}"
    );
    let up_after = srv
        .request_log()
        .iter()
        .filter(|r| r.method == "PUT")
        .map(|r| r.bytes)
        .sum::<u64>();
    assert!(
        up_after < index_raw.len() as u64 * 8,
        "一条增量的上行量 {up_after} B 相对索引 {} B 过大 —— 改动在放大成整份重写",
        index_raw.len()
    );

    tracing::info!(
        notes = NOTES,
        push_rounds = pushed_in,
        catch_up_rounds = caught_up,
        index_bytes = index_raw.len(),
        segments = refs.len(),
        secs = started.elapsed().as_secs_f64(),
        "big library catch-up"
    );
    srv.stop().await;
}
