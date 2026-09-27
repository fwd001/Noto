//! §28 的代理配置**从设置到出口**那条边：账户里配的代理，真的被同步用上了吗。
//!
//! 为什么单独一个文件：`notera-webdav/tests/proxy_routing.rs` 那三条是**手搓 `HttpClient`**
//! 跑的，它只能证明"notera-net 会把代理交给 reqwest"。而从"设置页填了代理"到"那个
//! `HttpClient` 被建出来"中间还隔着 `configure_account → ProxyProfile → net_proxy()`
//! 这一截 —— 那正是本项目踩过两次的形状（Range 探测、清单压实：实现齐全、单测全绿、
//! **没人调用它**）。这条测试站在组装根这一侧，把 `net_proxy` 换成
//! `ProxyProfile::direct()` 就得红。
//!
//! 跑法：`cargo test -p notera-host --test proxy_account`

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use notera_host::commands::AccountDraftCmd;
use notera_host::App;
use notera_test_webdav::{Backend, Injection, TestServer};
use serde_json::json;

const SECRET: &str = "sup3r-s3cr3t";
static SEQ: AtomicUsize = AtomicUsize::new(0);

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("notera-proxy-{tag}-{}-{n}", std::process::id()));
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

/// 一台只接受"经代理到达"的服务器 + 一个把代理填进配置的账户。
fn draft(url: &str, proxy: Option<(String, u16)>) -> AccountDraftCmd {
    draft_as("", url, proxy)
}

fn draft_as(id: &str, url: &str, proxy: Option<(String, u16)>) -> AccountDraftCmd {
    let mut obj = json!({
        "label": "代理账户", "baseUrl": url, "username": "notera-test", "id": id,
    });
    if let Some((host, port)) = proxy {
        obj["proxyMode"] = json!("http");
        obj["proxyHost"] = json!(host);
        obj["proxyPort"] = json!(port);
    }
    serde_json::from_value(obj).expect("草稿形态合法")
}

async fn boot_with(dir: &Path, url: &str, proxy: Option<(String, u16)>) -> App {
    std::env::set_var("NOTERA_DEV_WEBDAV_SECRET", SECRET);
    let app = App::boot(dir).expect("核心启动");
    if app.current_account().unwrap().is_none() {
        app.configure_account(draft(url, proxy)).expect("配置账户");
    }
    app
}

/// 走代理的账户能同步，且服务器侧看到的确实是"经代理到达"的请求。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_account_configured_with_a_proxy_syncs_through_it() {
    let srv = TestServer::start(Backend::Mem).await;
    srv.inject(Injection {
        require_proxy: true,
        ..Default::default()
    })
    .await;
    let url = srv.base_url();
    let (host, port) = (srv.addr().ip().to_string(), srv.addr().port());

    let dir = Tmp::new("via-proxy");
    let app = boot_with(dir.path(), &url, Some((host, port))).await;
    // 判据只用"服务器侧的事实"，不写 `pulled >= 0` 这种恒真断言（那正是本轮审查抓出来的毛病）
    app.sync_once()
        .await
        .expect("配了 HTTP 代理的账户应当能同步");
    // 服务器侧的自述：一条"未经代理"的拒绝都不该出现（出现了就是先直连失败再蒙对一次）
    let rejected = srv.inspect()["counters"]["rejections_403_not_proxied"]
        .as_u64()
        .unwrap_or(0);
    assert_eq!(
        rejected, 0,
        "这一轮里服务器拒过直连 —— 说明真正通路并不是配置里那条代理：{rejected}"
    );
    let served = srv.inspect()["served_data"].as_u64().unwrap_or(0);
    assert!(served >= 1, "前置不成立：服务器压根没收到请求");

    // 数据真的过去了：建一条笔记 → 同步 → 服务器上要有那条记录
    let folder = app.default_folder_id().unwrap();
    app.create_note(
        &folder,
        json!({ "v": 1, "content": [{ "id": "blk000001", "type": "paragraph", "content": [{ "text": "经代理写下的笔记" }] }] }),
    )
    .expect("本机建笔记");
    app.sync_once().await.expect("经代理推出去");
    let dump = srv.dump_prefix("/.notes/records");
    let count = dump["count"].as_u64().unwrap_or(0);
    assert!(
        count >= 1,
        "账户说同步成功了，服务器上却什么都没有（那这条代理通路是假的）：{dump}"
    );
    srv.stop().await;
}

/// 同一台服务器、同一份数据目录，**把代理撤掉**就必须同步失败。
///
/// 这一腿是差分的一半 —— 少了它，上一条可能只是"直连恰好也能通"。
/// 也顺手钉住另一件事：失败要落在同步状态上，而不是把本机数据弄坏。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropping_the_proxy_makes_the_same_account_fail() {
    let srv = TestServer::start(Backend::Mem).await;
    srv.inject(Injection {
        require_proxy: true,
        ..Default::default()
    })
    .await;
    let url = srv.base_url();
    let (host, port) = (srv.addr().ip().to_string(), srv.addr().port());

    let dir = Tmp::new("no-proxy");
    let app = boot_with(dir.path(), &url, Some((host, port))).await;
    app.sync_once().await.expect("带代理的那次应当成功");
    let with_proxy = srv.inspect()["counters"]["rejections_403_not_proxied"]
        .as_u64()
        .unwrap_or(0);
    assert_eq!(with_proxy, 0, "上一轮不该有直连被拒");

    // 重新配置成直连（**带原账户 id** —— 空 id 会被当成新增账户，而一次只允许一个启用账户，
    //    那条守卫在这里正好证明它自己有效）。
    let existing = app
        .current_account()
        .unwrap()
        .expect("前置：账户已经配好")
        .id;
    app.configure_account(draft_as(&existing, &url, None))
        .expect("改回直连");
    let direct = app.sync_once().await;
    assert!(
        direct.is_err(),
        "撤掉代理之后居然还能同步 —— 要么服务器没在挡，要么我们根本没按配置走：{direct:?}"
    );
    let rejected = srv.inspect()["counters"]["rejections_403_not_proxied"]
        .as_u64()
        .unwrap_or(0);
    assert!(
        rejected >= 1,
        "直连被拒这件事服务器没记下来，那上一条断言只是猜：{rejected}"
    );
    // 失败不外溢成本机不可用（§28 保证句的一半）
    let folder = app.default_folder_id().unwrap();
    app.create_note(
        &folder,
        json!({ "v": 1, "content": [{ "id": "blk000002", "type": "paragraph", "content": [{ "text": "断网也要写得下去" }] }] }),
    )
    .expect("同步失败之后本机照样能写");
    let notes = app
        .store()
        .list_notes(&notera_store::NoteQuery {
            folder: None,
            trash: false,
            limit: 50,
            offset: 0,
        })
        .expect("列表读得到");
    assert_eq!(notes.len(), 1, "本机数据被一次失败的同步弄坏了：{notes:?}");
    srv.stop().await;
}
