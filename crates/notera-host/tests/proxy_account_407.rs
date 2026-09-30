//! §28「错误密码」剩下的那一半：**设置页里输入的代理口令，这一轮同步到底用没用上**，
//! 以及口令错的时候，用户拿到的是"凭据不对"还是"暂时读不到协议信息"。
//!
//! 为什么单独一个文件：`notera-webdav/tests/proxy_http_407.rs` 那 4 条是**手搓 `HttpClient`**
//! 跑的，它只能证明"出口层会把凭据交给代理、407 会在代理那一层断"。而从"设置页填了代理口令"
//! 到那台客户端被建出来，中间隔着 `configure_account → 系统凭据引用 → net_proxy() → build_remote`
//! —— 这正是本项目踩过三次的那个形状（实现齐全、单测全绿、**没人调用它**）。
//! 更要紧的是失败那一侧：`negotiate` 原先把**所有**远端错误折成 `sync.protocol_unreadable`
//! （"暂时读不到服务器上的协议信息，请稍后重试"），于是**代理口令错**也长成"等一会儿再试"的样子 ——
//! 用户会一直点重试，而永远不会好。PROXY.md §8 那行写的却是 `! 代理需要登录`：文档与代码分叉（§45）。
//!
//! 本机证据依赖系统凭据库（ADR-0020：Windows 真、其余按 BLOCKED 记），所以两条都有一道
//! `available()` 的前置守卫 —— 守卫不成立时这条**不算通过**，台账里按实记它是 Windows 证据。
//!
//! 跑法：`cargo test -p notera-host --test proxy_account_407`
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use notera_host::commands::SyncStatusDto;
use notera_host::{credential_store, App};
use notera_test_webdav::{Backend, HttpForwardProxy, TestServer};
use serde_json::json;

const PUSER: &str = "pu";
const PPASS: &str = "pp-proxy-pass";
const SECRET: &str = "sup3r-s3cr3t";
static SEQ: AtomicUsize = AtomicUsize::new(0);

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("notera-p407-{tag}-{}-{n}", std::process::id()));
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

/// 账户草稿：代理地址/端口固定指到那台 407 代理，口令由调用方给（`None` = 一个凭据都不给）。
fn draft(id: &str, url: &str, proxy_port: u16, proxy_pass: Option<&str>) -> serde_json::Value {
    let mut obj = json!({
        "label": "407 代理账户",
        "baseUrl": url,
        "username": "notera-test",
        "id": id,
        "proxyMode": "http",
        "proxyHost": "127.0.0.1",
        "proxyPort": proxy_port,
    });
    if let Some(pass) = proxy_pass {
        obj["proxyUsername"] = json!(PUSER);
        obj["proxyPassword"] = json!(pass);
    }
    obj
}

async fn boot(dir: &Path, url: &str, proxy_port: u16, proxy_pass: Option<&str>) -> App {
    std::env::set_var("NOTERA_DEV_WEBDAV_SECRET", SECRET);
    let app = App::boot(dir).expect("核心启动");
    if app.current_account().unwrap().is_none() {
        let cmd: notera_host::commands::AccountDraftCmd =
            serde_json::from_value(draft("", url, proxy_port, proxy_pass)).expect("草稿合法");
        app.configure_account(cmd).expect("配置账户");
    }
    app
}

fn status(app: &App) -> SyncStatusDto {
    app.sync_status().expect("状态读得到")
}

/// 主腿：设置页输入的代理口令，真被这一轮同步用上了 —— 而且**只有用它才成得了**。
///
/// 判据全打在边界上（代理自己的计数 + 源站收到的字节），不看客户端自述：
/// 那台代理没有正确凭据就一律 407，所以"这一轮成了"本身不含牙齿，牙齿在代理的 `auth_rejects==0`
/// 与 `forwarded≥1` 上 —— 产品若把口令半路丢掉，这里必然红。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_proxy_password_typed_in_settings_carries_the_real_round() {
    if !credential_store::available() {
        eprintln!("系统凭据库不可用：这一格按 BLOCKED 记（ADR-0020），不算通过");
        return;
    }
    let srv = TestServer::start(Backend::Mem).await;
    let proxy = HttpForwardProxy::start_requiring(PUSER, PPASS)
        .await
        .expect("起带 407 策略的代理");
    let url = srv.base_url();

    let dir = Tmp::new("good");
    let app = boot(dir.path(), &url, proxy.port(), Some(PPASS)).await;
    let folder = app.default_folder_id().unwrap();
    let note = app
        .create_note(
            &folder,
            json!({ "v": 1, "content": [{ "id": "blk000001", "type": "paragraph", "content": [{ "text": "代理口令写下的笔记" }] }] }),
        )
        .expect("本机建笔记");

    app.sync_once()
        .await
        .expect("设置里填了正确的代理口令，这一轮应当走得通");

    assert!(
        proxy.forwarded() >= 1,
        "这一轮成了而代理的转发数是 0 ⇒ 同步根本没经过那台要求认证的代理（配置里的代理被静默忽略）"
    );
    assert_eq!(
        proxy.auth_rejects(),
        0,
        "凭据是对的却被供应商拒过 ⇒ 界面上那次成功是蒙的（先失败再撞对一次）"
    );
    assert_eq!(
        status(&app).badge,
        "synced",
        "同步成功了而状态徽章没跟上（用户看不出这一轮到底成没成）"
    );
    let dump = srv.dump_prefix("/.notes/records").to_string();
    assert!(
        dump.contains(note.id.as_str()),
        "账户说同步成了，服务器上却没有这条笔记的记录（{note_id}）：{dump}",
        note_id = note.id.as_str()
    );
    assert_eq!(
        status(&app).pending_ops,
        0,
        "这一轮之后还留着待办 ⇒ 成功是部分的，界面却在说已同步"
    );
    proxy.shutdown().await;
    srv.stop().await;
}

/// 失败那一侧：口令不对必须说成**凭据问题**，不许说成"暂时读不到协议信息"；
/// 并且失败不许伤到本机数据、不许把待办吞掉，改对口令之后重试要真能推上去。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_wrong_proxy_password_says_credentials_are_wrong_not_wait_and_retry() {
    if !credential_store::available() {
        eprintln!("系统凭据库不可用：这一格按 BLOCKED 记（ADR-0020），不算通过");
        return;
    }
    let srv = TestServer::start(Backend::Mem).await;
    let proxy = HttpForwardProxy::start_requiring(PUSER, PPASS)
        .await
        .expect("带 407 策略的代理");
    let url = srv.base_url();

    let dir = Tmp::new("wrong");
    let app = boot(dir.path(), &url, proxy.port(), Some("typo-in-settings")).await;
    let folder = app.default_folder_id().unwrap();
    let n1 = app
        .create_note(
            &folder,
            json!({ "v": 1, "content": [{ "id": "blk000002", "type": "paragraph", "content": [{ "text": "口令错了也要写得下去" }] }] }),
        )
        .expect("本机建笔记");

    let err = app
        .sync_once()
        .await
        .expect_err("代理口令不对，这一轮必须失败");
    // 这一句是本条测试存在的理由：此前这里报的是 sync_refused + reason=sync.protocol_unreadable，
    // 界面文案是"暂时读不到服务器上的协议信息，请稍后重试" —— 让用户去等，而该改的是口令。
    assert_eq!(
        err.code, "sync_auth_failed",
        "凭据问题被报成了别的东西（{}）：界面会给出与原因无关的处置建议",
        err.code
    );
    assert!(
        proxy.auth_rejects() >= 1,
        "失败了但代理没记成口令被拒 ⇒ 这一轮断在别处，上面那句结论没有依据"
    );
    let st = status(&app);
    assert_eq!(st.badge, "failed", "失败没落到徽章上");
    assert_eq!(
        st.message_key.as_deref(),
        Some("sync.auth_failed"),
        "状态里那行可读的话不是凭据问题：用户看到的是猜，不是原因"
    );
    assert!(
        st.pending_ops >= 1,
        "一次失败的同步把待办吞掉了（那才是真丢数据的前置）：{}",
        st.pending_ops
    );

    // 保证句的那一半：网络/凭据问题不许让本地不可用。
    let folder2 = app.default_folder_id().unwrap();
    let n2 = app
        .create_note(
            &folder2,
            json!({ "v": 1, "content": [{ "id": "blk000003", "type": "paragraph", "content": [{ "text": "改对之前还要写得动" }] }] }),
        )
        .expect("同步失败之后本机照样能写");

    // 重试腿：把口令改对（同一个账户 id，不是新增账户），下一轮要真把两条都推上去。
    let existing = app
        .current_account()
        .unwrap()
        .expect("前置：账户已经配好")
        .id;
    let cmd = serde_json::from_value(draft(&existing, &url, proxy.port(), Some(PPASS)))
        .expect("草稿合法");
    app.configure_account(cmd).expect("改口令");
    // 被 407 挡住的那一轮不该在服务器上留下任何痕迹（这一句读在重试之前，
    // 少了它，"重试之后两条都在"就无法区分"之前推上去了"与"重试才推上去"）。
    let after_failure = srv.dump_prefix("/.notes/records").to_string();
    assert!(
        !after_failure.contains(n1.id.as_str()),
        "代理把这一轮拒了，服务器上却已经出现这条记录：那 407 什么都没挡住"
    );

    app.sync_once().await.expect("改对代理口令之后重试应当成功");
    assert_eq!(status(&app).pending_ops, 0, "重试成功了却还有待办");
    let dump = srv.dump_prefix("/.notes/records").to_string();
    // 两条都要在：失败那一轮写的那条 + 失败之后新写的那条 —— 少一条就是"重试只推了增量"。
    assert!(
        dump.contains(n1.id.as_str()) && dump.contains(n2.id.as_str()),
        "重试之后服务器上少了东西（期望两条都在）：n1={} n2={} 快照={dump}",
        n1.id.as_str(),
        n2.id.as_str()
    );
    proxy.shutdown().await;
    srv.stop().await;
}
