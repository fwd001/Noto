//! 缺口 G50：**配好账户之后，不重启也要真的开始同步。**
//!
//! 这条测的是"配置 → 引擎"那条边，不是引擎本身（引擎在 `sync_once.rs` / `engine.rs` 里
//! 早就绿了）。旧形状下这条边是断的：调度器只在桌面壳 `setup()` 里、按"开机那一刻
//! 已配好账户"起一次，而 `configure_account` 全程不碰它 ⇒
//! 全新安装填好 WebDAV → 点同步 → 命令面回 `null`、没人消费、也没有事件来纠正徽标，
//! 那颗"正在同步"会转到用户重启为止；而界面那句文案写的是"配好凭据后会自动开始同步"。
//!
//! 判据因此打在**服务器收到的请求**与**本地待办被结清**这两处结果上：
//! 中间不重启、不手动 `sync-once`、不发 `sync_now`。
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use notera_host::commands::AccountDraftCmd;
use notera_host::App;
use notera_test_webdav::{Backend, TestServer};
use serde_json::json;

const SECRET: &str = "sup3r-s3cr3t";
static SEQ: AtomicUsize = AtomicUsize::new(0);

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("notera-autostart-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn boot(dir: &Path) -> App {
    std::env::set_var("NOTERA_DEV_WEBDAV_SECRET", SECRET);
    App::boot(dir).expect("核心启动")
}

fn draft(url: &str) -> AccountDraftCmd {
    serde_json::from_value(json!({
        "label": "自动起跑", "baseUrl": url, "username": "notera-test",
    }))
    .unwrap()
}

/// 最多等 `secs` 秒，直到 `hit()` 为真。轮询而不是 sleep 固定值：CI 上快慢差一个量级。
async fn wait_until(secs: u64, hit: impl Fn() -> bool) -> bool {
    for _ in 0..(secs * 5) {
        if hit() {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    false
}

#[tokio::test(flavor = "current_thread")]
async fn configuring_an_account_starts_the_engine_without_a_restart() {
    let srv = TestServer::start(Backend::Mem).await;
    let url = srv.base_url();
    let dir = Tmp::new("autostart");
    let app = boot(&dir.0);

    // 壳在首帧之后就是这么调的：先开托管，此时**还没有账户**。
    app.enable_background_sync();
    // 没配账户时不该有任何网络动作（"装完是干净的"这条底线，含 dev 桥）
    assert!(
        srv.request_log().is_empty(),
        "没配账户就不该碰服务器：{:?}",
        srv.request_log().iter().take(3).collect::<Vec<_>>()
    );
    tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
    assert!(
        srv.request_log().is_empty(),
        "监督循环不该在没有账户时自己发起网络请求"
    );

    // 配好（**不重启**、不发 sync_now、不调 sync-once）
    app.configure_account(draft(&url)).expect("配置账户");
    let put_seen = wait_until(30, || {
        srv.request_log().iter().any(|r| r.method == "PUT")
    })
    .await;
    let log = srv.request_log();
    assert!(
        put_seen,
        "配好之后引擎必须自己起跑：30 秒内服务器没收到一次 PUT（清单/记录提交）。请求日志 {} 条：{:?}",
        log.len(),
        log.iter().take(6).map(|r| format!("{} {}", r.method, r.path)).collect::<Vec<_>>()
    );

    // 起跑还不算"有始有终"：本地待办要被这一轮**结清**，否则用户看到的还是"待发操作 N"。
    let drained = wait_until(30, || app.sync_status().unwrap().pending_ops == 0).await;
    let st = app.sync_status().unwrap();
    assert!(
        drained,
        "这一轮要把默认本确认掉，待发操作必须归零：实际 pending_ops={}",
        st.pending_ops
    );
    assert_eq!(st.badge, "synced", "结清之后徽标该落在已同步：{st:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn removing_the_account_stops_the_engine_without_a_restart() {
    let srv = TestServer::start(Backend::Mem).await;
    let url = srv.base_url();
    let dir = Tmp::new("autostop");
    let app = boot(&dir.0);
    app.enable_background_sync();
    let saved = app.configure_account(draft(&url)).expect("配置账户");
    assert!(
        wait_until(30, || !srv.request_log().is_empty()).await,
        "前置不成立：引擎没起来"
    );

    app.remove_account(&saved.id).expect("删账户");
    tokio::time::sleep(std::time::Duration::from_millis(3000)).await;
    let before = srv.request_log().len();
    tokio::time::sleep(std::time::Duration::from_millis(6000)).await;
    let after = srv.request_log().len();
    assert_eq!(
        before, after,
        "删掉账户之后还在打服务器（{} → {} 条）—— 那台服务器的数据归谁说了算已经不清楚了",
        before,
        after
    );
}
