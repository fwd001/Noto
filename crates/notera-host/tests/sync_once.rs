//! §5 与 §6 的落点验收。跑在 `notera-test-webdav` 的真 TCP 服务器上，
//! 走的是 `App::sync_once` —— 产品调度器每 25 秒跑的同一条代码路径。
//!
//! 三件事必须被观测到，缺一件都算没接线：
//! 1. **探测发生在装配适配器之前**：装出来的那个 remote 必须已经带着实测位图。
//!    反过来说，如果先装再探，这次会话拿到的仍是保守默认，§5 的优化要等下次启动。
//! 2. **探测失败 ≠ 能力缺失**：连接被掐断时必须退回保守默认并如实提示，
//!    绝不能用"全 false 的结论"去写库（那会把好服务器降级成 S3 盲写）。
//! 3. 本地笔记真的落到服务器文件系统，并被**第二台设备**原样拉下来。
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use notera_host::commands::AccountDraftCmd;
use notera_host::{App, BusEvent};
use notera_store::NoteQuery;
use notera_test_webdav::{Backend, Injection, TestServer};
use notera_webdav::{Caps, WriteStrategy};
use serde_json::json;

const SECRET: &str = "sup3r-s3cr3t";
static SEQ: AtomicUsize = AtomicUsize::new(0);

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("notera-synconce-{tag}-{}-{n}", std::process::id()));
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

/// 一台设备 = 一个数据目录 + 一个 `App`。目录必须比 App 活得久，所以成对持有。
struct Device {
    app: App,
    _dir: Tmp,
}

impl Device {
    fn boot(tag: &str, base_url: &str) -> Self {
        let dir = Tmp::new(tag);
        let app = App::boot(dir.path()).expect("核心启动");
        // 凭据走 debug 专用的环境变量：OS 钥匙串是 PLATFORM.md §6 的活，
        // 在此之前 `secret_for` 只认这一个来源（发布版宁可显示"需要凭据"）。
        std::env::set_var("NOTERA_DEV_WEBDAV_SECRET", SECRET);
        // 用 JSON 构造而不是结构体字面量：顺便证明"前端发的那个形状"真的被接受。
        let draft: AccountDraftCmd = serde_json::from_value(json!({
            "label": "测试盘",
            "baseUrl": base_url,
            "username": "notera-test",
        }))
        .expect("最小账户草案");
        app.configure_account(draft).expect("配置账户");
        Self { app, _dir: dir }
    }
    fn account_id(&self) -> String {
        self.app.current_account().unwrap().expect("已配置账户").id.to_string()
    }
}

fn doc(text: &str) -> serde_json::Value {
    json!({ "v": 1, "content": [{ "id": "blk000001", "type": "paragraph", "content": [{ "text": text }] }] })
}

#[tokio::test]
async fn boot_probe_happens_before_the_adapter_is_built() {
    let srv = TestServer::start(Backend::Mem).await;
    let a = Device::boot("order", &srv.base_url());
    let acct = a.account_id();

    // 从未探测：cap_mask 必须是 NULL，而不是 0（0 = "探过且什么都不支持"，含义完全不同）
    assert!(a.app.store().account_caps(&acct).unwrap().is_none(), "新账户不该带探测结果");
    assert!(a.app.store().account_caps_probed_at(&acct).unwrap().is_none());

    // 让服务器"把条件头当装饰"：实测结论因此**低于**保守默认，
    // 于是"适配器带的是哪个位图"就成了一次可观测的顺序判定 ——
    // 若先装后探，这里会是默认的 S1，而真实测出来的应该是 S2。
    srv.inject(Injection::status("PUT /.notes/probe/cput.json", 200)).await;

    let remote = a.app.remote_for_sync().await.expect("启动期装配远端").expect("配了账户就该有适配器");
    let mask = a.app.store().account_caps(&acct).unwrap().expect("探测结果要落库");
    let caps = remote.caps();
    assert!(!caps.has(Caps::CONDITIONAL_PUT), "服务器不支持条件写，探测却没认出来：mask={mask:#06b}");
    assert_eq!(caps.write_strategy(), WriteStrategy::S2, "适配器带的还是默认位图，探测结果没赶上本次会话");
    assert_eq!(caps.mask(), mask, "落库的位图与真正生效的位图必须一致");
    srv.clear_injection().await;

    a.app.create_note(&a.app.default_folder_id().unwrap(), doc("探测之后的一轮")).unwrap();
    let stats = a.app.sync_once().await.expect("第一轮要跑起来");
    assert!(stats.pushed >= 1, "本地新建要真的推上去：{stats:?}");
    // 请求日志是第二重证据：所有探测请求都要早于任何真实数据请求。
    // depth 探测打的是库根上的 PROPFIND，路径里不带 probe/，别把它漏判成"真实读写"。
    let log = srv.request_log();
    let is_probe = |r: &&notera_test_webdav::LoggedRequest| r.path.contains("/probe/") || r.method == "PROPFIND";
    let probe_last = log.iter().filter(is_probe).map(|r| r.seq).max().expect("探测请求要出现");
    let real_first = log.iter().filter(|r| !is_probe(r)).map(|r| r.seq).min().expect("真实读写要出现");
    let trace = log
        .iter()
        .map(|r| format!("{} {} -> {}", r.seq, r.method, r.path))
        .collect::<Vec<_>>()
        .join(" | ");
    assert!(probe_last < real_first, "探测穿插在真实读写之间，说明不是『先探后装』：{probe_last} vs {real_first}\n{trace}");

    // §5「每日一次」：同一天里的第二次入口不该再来一轮探测请求
    let probes_before = log.iter().filter(is_probe).count();
    a.app.remote_for_sync().await.expect("第二次装配");
    let probes_after = srv.request_log().iter().filter(is_probe).count();
    assert_eq!(probes_before, probes_after, "当天已探过就不该重复探测（每次启动 12+ 个请求）");
    // 探测对象用完要清掉：留在库里会污染清单并跟着同步到每台设备
    let leftover = srv
        .fs_dump()["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .filter(|e| e["path"].as_str().unwrap_or_default().contains("/probe/"))
        .count();
    assert_eq!(leftover, 0, "探测对象没清理");
    srv.stop().await;
}

/// 探测期间连接被掐断 → 退回保守默认 + 一条可见提示，库里的 cap_mask 保持"从未探测"。
/// 这一条守的是方向性风险：把"没探到"写成"不支持"，服务器就永久掉到 S3 盲写复验。
#[tokio::test]
async fn a_failed_probe_falls_back_to_the_conservative_default_and_says_so() {
    let srv = TestServer::start(Backend::Mem).await;
    let a = Device::boot("probe-fails", &srv.base_url());
    let acct = a.account_id();
    let rx = a.app.subscribe();
    srv.inject(Injection { drop_after_n: Some(0), ..Default::default() }).await;

    let remote = a.app.remote_for_sync().await.expect("探测失败不许挡住启动").expect("服务器在，就该给出适配器");
    assert_eq!(remote.caps(), Caps::conventional(), "探测没成功就必须用保守默认，不能编造结论");
    assert!(a.app.store().account_caps(&acct).unwrap().is_none(), "没探到就不该写 cap_mask");

    let mut saw_deferred = false;
    while let Ok(ev) = rx.try_recv() {
        if let BusEvent::Toast { message_key, level } = &ev {
            assert_eq!(level, "warn", "探测未完成是可恢复状态，不该报成错误");
            if message_key == "sync.probeDeferred" {
                saw_deferred = true;
            }
        }
    }
    assert!(saw_deferred, "探测失败必须让用户看得见，不能静默按默认跑");
    srv.clear_injection().await;
    srv.stop().await;
}

/// 一轮同步的端到端闭环：A 推 → 服务器落盘 → B 拉 → 标题/正文逐字一致。
#[tokio::test]
async fn one_round_carries_a_local_note_to_a_second_device() {
    let srv = TestServer::start(Backend::Mem).await;
    let url = srv.base_url();
    let a = Device::boot("push", &url);
    let text = "只有真服务器才算数的正文";
    a.app.create_note(&a.app.default_folder_id().unwrap(), doc(text)).unwrap();

    let stats = a.app.sync_once().await.expect("A 的一轮");
    assert_eq!(stats.outcome, notera_sync::RoundOutcome::Converged, "{stats:?}");
    assert!(stats.pushed >= 1);

    let b = Device::boot("pull", &url);
    let stats_b = b.app.sync_once().await.expect("B 的一轮");
    assert!(stats_b.pulled >= 1, "B 必须从服务器拉到东西：{stats_b:?}");
    let notes = b.app.store().list_notes(&NoteQuery::all()).unwrap();
    assert_eq!(notes.len(), 1, "B 拉下来的就该是 A 那一条");
    assert_eq!(notes[0].title, text, "跨设备内容必须逐字一致");
    // B 也各自探过一次能力（同一台服务器，结论应相同），并且没把 A 的库当成第二个库
    assert_eq!(b.app.store().account_caps(&b.account_id()).unwrap().map(Caps::from_mask).map(|c| c.write_strategy()), Some(WriteStrategy::S1));
    srv.stop().await;
}
