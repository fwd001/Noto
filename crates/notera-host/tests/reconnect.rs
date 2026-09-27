//! §19–23 要求的「反复断连重连最终收敛」，也是总指令 §53 那条主循环的机器证据：
//!
//! ```text
//! 断网 → 继续写 → 恢复网络 → 自动追平 → 换设备一条不差
//! ```
//!
//! 这里把三件容易被混为一谈的事分开钉住：
//! 1. **链路断着的那一轮绝不报"已同步"**（服务器是真的停监听，不是让它回一个错误码）；
//! 2. 断线期间写的笔记**本机立刻读得回来**，也不需要用户再点一次保存（I8）；
//! 3. 链路恢复后靠 outbox 自己追平，追平完两台设备的**标题与内容哈希逐条一致** ——
//!    既不少一条，也不重一份。
//!
//! 跑法：`cargo test -p notera-host --test reconnect`
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use notera_host::commands::AccountDraftCmd;
use notera_host::App;
use notera_store::NoteQuery;
use notera_sync::RoundOutcome;
use notera_test_webdav::{Backend, Injection, TestServer};
use serde_json::json;

const SECRET: &str = "sup3r-s3cr3t";
static SEQ: AtomicUsize = AtomicUsize::new(0);

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("notera-reconnect-{tag}-{}-{n}", std::process::id()));
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
            "label": "抖动盘", "baseUrl": base_url, "username": "notera-test",
        }))
        .unwrap();
        app.configure_account(draft).expect("配置账户");
        Self { app, _dir: dir }
    }
    /// 标题 + 内容哈希：哈希不同就是正文变了，标题相同不代表内容相同。
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
}

fn doc(text: &str) -> serde_json::Value {
    json!({ "v": 1, "content": [{ "id": "blk000001", "type": "paragraph", "content": [{ "text": text }] }] })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_flaky_link_still_converges_without_losing_or_duplicating_notes() {
    let srv = TestServer::start(Backend::Mem).await;
    let url = srv.base_url();
    let a = Device::boot("flaky-a", &url);
    let b = Device::boot("flaky-b", &url);
    // 两边都先入伙（空库握手），否则后面撞的是"根不一致"而不是本题要测的东西
    a.app.sync_once().await.expect("A 入伙");
    b.app.sync_once().await.expect("B 入伙");
    let folder = a.app.default_folder_id().unwrap();

    let mut down_rounds = 0usize;
    for round in 0..6 {
        // 三种"链路坏了"要分开测，因为它们在代码里走的是三条不同的路：
        // ① 服务器整个不在听（拔网线）→ 连接就被拒；
        // ② 连接建了又被掐（代理/服务器半路断）→ 握手阶段就死；
        // ③ 握手能过、**读清单时**服务器回 500 → 这一种才会真的进到引擎里那一支。
        // 只测①②会让"引擎把坏轮次报成功"这类缺陷从缝里漏掉 —— 变异验证过：只测①②
        // 时把"拉清单失败"改成"报 Converged"，这条测试照样绿；加上③才红。
        let mode = round % 3;
        match mode {
            0 => srv.stop().await,
            1 => srv.inject(Injection::abort_after(0)).await,
            _ => {
                srv.inject(Injection::status("GET /.notes/manifest/*", 500))
                    .await
            }
        }
        let text = format!("第 {round} 轮写的正文");
        a.app.create_note(&folder, doc(&text)).unwrap();

        match a.app.sync_once().await {
            Ok(stats) => assert!(
                matches!(stats.outcome, RoundOutcome::Failed | RoundOutcome::Partial),
                "链路坏着（模式 {mode}），这一轮却报成 {:?}：{stats:?} —— 用户会以为已经存到云上",
                stats.outcome
            ),
            // 探测/协商阶段就失败也算诚实：只要没被报成成功；库里欠账下面单独断言。
            Err(e) => assert!(e.code != "no_account", "坏链路不该让账户判定消失：{e:?}"),
        }
        // I8：网络失败绝不挡住本机使用 —— 刚写的那条现在就读得回来
        let readable = a
            .app
            .store()
            .list_notes(&NoteQuery::all())
            .unwrap()
            .iter()
            .any(|r| r.title == text);
        assert!(
            readable,
            "第 {round} 轮（模式 {mode}）写的笔记在本机读不回来：{text}"
        );
        // 更关键的是**账要挂着**：这一轮没上去，outbox 就必须还欠着，
        // 否则界面报"已同步"而服务器上没有，换设备时这条就消失了。
        let st = a.app.store().stats().unwrap();
        assert!(
            st.outbox_pending >= 1,
            "坏链路那轮之后本机没记账（模式 {mode}，outbox_pending={})",
            st.outbox_pending
        );
        assert!(
            st.dirty_notes >= 1,
            "坏链路那轮之后改动被判成已公告（模式 {mode}，dirty_notes={})",
            st.dirty_notes
        );
        down_rounds += 1;
        match mode {
            0 => srv.start_server().await,
            _ => srv.clear_injection().await,
        }
    }
    assert_eq!(down_rounds, 6, "六轮都应各坏一次（三种模式轮着来）");

    // 链路恢复：按调度器的节奏跑到**本机账上结清**。判据用库里的状态而不是轮次的
    // 返回值 —— 中间那些"其实是通的"轮次早就把东西推上去了，最后一轮报 NoOp 是对的，
    // 真正要防的是"报了成功而账没结"（P7 那一类）。
    let mut settled = false;
    let mut tail = Vec::new();
    for _ in 0..8 {
        let stats = a.app.sync_once().await.expect("恢复后的轮次");
        let st = a.app.store().stats().unwrap();
        tail.push(format!(
            "{:?} dirty={} pending={}",
            stats.outcome, st.dirty_notes, st.outbox_pending
        ));
        if st.dirty_notes == 0 && st.outbox_pending == 0 {
            settled = true;
            break;
        }
    }
    assert!(
        settled,
        "链路恢复后本机一直没能结清：\n  {}",
        tail.join("\n  ")
    );
    let st = a.app.store().stats().unwrap();
    assert_eq!(st.notes, 6, "本机应有六条（六轮各一条）：{st:?}");

    b.app.sync_once().await.expect("B 拉一轮");
    b.app.sync_once().await.expect("B 补一轮");
    assert_eq!(
        a.fingerprints(),
        b.fingerprints(),
        "两台设备的标题/内容哈希不一致（丢了或重了）"
    );
    assert_eq!(b.fingerprints().len(), 6);

    // 断线那几轮不许在服务器上留下半截东西：临时对象、或同一篇的第二份记录文件
    let entries = srv.fs_dump()["entries"]
        .as_array()
        .expect("entries")
        .clone();
    let strays: Vec<String> = entries
        .iter()
        .filter(|e| e["path"].as_str().unwrap_or_default().contains(".tmp-"))
        .map(|e| e["path"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(strays.is_empty(), "服务器上留着半截对象：{strays:?}");
    let records: Vec<String> = entries
        .iter()
        .filter_map(|e| e["path"].as_str().map(str::to_string))
        .filter(|p| p.contains("/records/n/"))
        .collect();
    let mut uniq = records.clone();
    uniq.sort();
    uniq.dedup();
    assert_eq!(
        records.len(),
        uniq.len(),
        "同一个 id 出现多份记录文件：{records:?}"
    );

    srv.stop().await;
}
