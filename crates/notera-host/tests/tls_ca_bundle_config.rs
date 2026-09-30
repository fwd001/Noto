//! §6 那两档配置**到底进不进得来**：`ca_bundle` 的 PEM 与 `pin` 的指纹，
//! 从"设置页那条 `configure_account`"一路到"出口客户端按不按这一档建"。
//!
//! 为什么单独一个文件（这批最要紧的一段）：这两档在下拉里一直选得到，可是
//! * 界面上没有任何 PEM / 指纹输入口；
//! * `AccountDraftCmd` 连 `pinned_sha256` 这个字段都没有，而保存路径把它硬编成 `None`
//!   —— 于是「指纹锁定」这一档**从来就配不出来**；
//! * 账户 DTO 不回读「配过没配过」，重开表单看着像空的。
//!
//! 结果就是：`CaBundle` 是 PROXY.md §6 写的"内网自签主路径"，而它整条在应用里不可达；
//! 0.0.47 修的那条握手（缺口 G35）也因此对用户没有意义 —— 这正是本项目踩过四次的那个形状：
//! **实现齐全、单测全绿、没人调用它**。所以判据不许只打在"能构造出来"上，要打在
//! "那条调用边真的把这份 PEM 送到了出口层"上，而牙齿用源站自己的两本账给
//! （与 `notera-net/tests/tls_policies.rs` 同一套做法）：`accepted` 数连上来几条、
//! `handled` 数握手成并且真读到一条请求的有几条。
//!
//! 本机证据依赖系统凭据库（ADR-0020：Windows 真、其余按 BLOCKED 记），第一条因此带
//! `available()` 守卫；后面三条纯配置面，不需要凭据库。
//!
//! 跑法：`cargo test -p notera-host --test tls_ca_bundle_config`

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use notera_host::commands;
use notera_host::{credential_store, App};
use notera_test_webdav::TlsOrigin;
use serde_json::{json, Value};

static SEQ: AtomicUsize = AtomicUsize::new(0);

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("notera-cabundle-{tag}-{}-{n}", std::process::id()));
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

/// 走**命令面**存账户（不是直接调 `configure_account`）：这条边坏了的时候，设置页那颗
/// "保存"是唯一还能碰到它的地方，判据得打在那一条上 —— 包括它收不收 `caPem` 这个键名。
fn save(app: &App, mut obj: Value) -> Result<Value, String> {
    if obj["id"].is_null() {
        // 不给 id 会被当成"新账户"，而配置入口拒绝第二个启用账户（那是有意的）。
        // 所以编辑路径要把第一次保存回来的 id 带回去 —— 这一条本身也是要验的形状。
        obj["id"] = json!(current(app)["id"].as_str().unwrap_or(""));
    }
    commands::dispatch(app, "configure_account", obj).map_err(|e| {
        // `CmdError` 没有 Display（它按结构化码走），所以拒因要自己拼出来才能验"有没有指名是哪一格"。
        format!(
            "{} {}",
            e.code,
            e.detail.map(|d| d.to_string()).unwrap_or_default()
        )
    })
}

fn current(app: &App) -> Value {
    commands::dispatch(app, "account", json!({})).expect("账户读得回来")
}

/// ① **调用边**：设置页存进去的那份 PEM，真的决定了握不握得上手。
///
/// 判据是差分 + 源站的账，不看客户端自述：
/// * 先按 `strict` 配同一台端点 ⇒ 自签链必然握不上 ⇒ `handled == 0`（对照腿）；
/// * 换 `ca_bundle` + 这台端点自己的 CA ⇒ 唯一变的就是那份 PEM ⇒ `handled ≥ 1`。
///
/// 少了对照腿，"配了 CA 就连上"可能只是"这一档根本没在验证书"——那是最坏的那种坏法。
/// 这台源站不会说 WebDAV，所以两轮**都该失败**；判据问的是失败在哪一步。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_pem_saved_from_settings_is_what_opens_the_handshake() {
    if !credential_store::available() {
        eprintln!("系统凭据库不可用：这一格按 BLOCKED 记（ADR-0020），不算通过");
        return;
    }
    let srv = TlsOrigin::start().await.expect("起 TLS 源站");
    let url = format!("https://{}", srv.addr());
    let dir = Tmp::new("edge");
    let app = App::boot(dir.path()).expect("核心启动");

    save(
        &app,
        json!({"label":"strict 对照","baseUrl":url,"username":"u","password":"p",
               "rootPrefix":"/.notes","tlsPolicy":"strict"}),
    )
    .expect("strict 配置合法");
    let _ = app.sync_once().await;
    assert!(
        srv.accepted() >= 1,
        "strict 腿连一条 TCP 都没数到：这一轮根本没往那台端点发出去 ⇒ 后面的差分没有依据"
    );
    assert_eq!(
        srv.handled(),
        0,
        "自签链在 strict 档居然被服务了 {} 条请求 ⇒ 证书校验形同虚设",
        srv.handled()
    );

    let pem = srv.ca_pem().to_string();
    save(
        &app,
        json!({"label":"ca_bundle 正腿","baseUrl":url,"username":"u","password":"p",
               "rootPrefix":"/.notes","tlsPolicy":"ca_bundle","caPem":pem}),
    )
    .expect("ca_bundle 配置合法");
    let _ = app.sync_once().await;
    assert!(
        srv.handled() >= 1,
        "配了这台端点自己的 CA 而源站一条请求都没收到 ⇒ 设置里那格 PEM 没被送到出口层，\
         或者这一档又走回系统校验器（缺口 G35 的那条路线）。accepted={} handled={}",
        srv.accepted(),
        srv.handled()
    );
    let back = current(&app);
    assert_eq!(back["tlsPolicy"], "caBundle", "回读的档位不对：{back}");
    assert_eq!(
        back["hasCaPem"], true,
        "PEM 存下了却回读成「没配过」⇒ 界面会把这一格显示成空的，用户以为被吞了：{back}"
    );
}

/// ② `pin` 档的指纹以前根本进不来：命令里没有那个字段，保存路径又把它硬编成 `None`。
/// 判据就在回读上 —— 存进去的指纹要原样回得来（修之前这里是空表）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pin_fingerprints_survive_the_save_round_trip() {
    let dir = Tmp::new("pin");
    let app = App::boot(dir.path()).expect("核心启动");
    let pin_a = "a".repeat(64);
    let pin_b = "b".repeat(64);
    save(
        &app,
        json!({"label":"指纹锁定","baseUrl":"https://dav.internal:8443","username":"u",
               "password":"p","rootPrefix":"/.notes","tlsPolicy":"pin",
               "pinnedSha256":[pin_a.clone(), pin_b]}),
    )
    .expect("pin 配置合法");
    let back = current(&app);
    assert_eq!(back["tlsPolicy"], "pin", "档位没存住：{back}");
    let pins = back["pinnedSha256"]
        .as_array()
        .unwrap_or_else(|| panic!("pinnedSha256 没回读成数组：{back}"));
    assert_eq!(
        pins.len(),
        2,
        "指纹没存下来（修之前这里就是 0 —— 命令面压根没这个字段，等于这一档永远配不出来）"
    );
    assert!(
        pins.contains(&json!(pin_a)),
        "存进去的指纹与回读的不是同一批：{back}"
    );
}

/// ③ **改一次别的不许把根证书洗掉**：保存是整体替换配置的，所以"这一格留空 = 不改"必须成立，
/// 否则用户改个标签就把内网端点的信任锚清空 —— 下一次同步当场变成"证书不受信任"，
/// 而用户什么都没做错。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn editing_an_account_without_retyping_the_pem_keeps_the_stored_one() {
    let dir = Tmp::new("wipe");
    let app = App::boot(dir.path()).expect("核心启动");
    let pem = "-----BEGIN CERTIFICATE-----\nZm9v\n-----END CERTIFICATE-----".to_string();
    save(
        &app,
        json!({"label":"第一版","baseUrl":"https://dav.internal:8443","username":"u",
               "password":"p","rootPrefix":"/.notes","tlsPolicy":"ca_bundle","caPem":pem}),
    )
    .expect("第一次配置合法");
    assert_eq!(current(&app)["hasCaPem"], true, "PEM 一开始就没存住");

    // 第二次只改标签，**不带 caPem**（界面上那一格留空就是这个形状）。
    save(
        &app,
        json!({"label":"改过名字","baseUrl":"https://dav.internal:8443","username":"u",
               "rootPrefix":"/.notes","tlsPolicy":"ca_bundle"}),
    )
    .expect("第二次配置合法");
    let back = current(&app);
    assert_eq!(back["label"], "改过名字", "标签没改动：{back}");
    assert_eq!(back["hasCaPem"], true, "只改个标签就把根证书洗掉了：{back}");
}

/// ④ 选了 `ca_bundle` 却不给 PEM：**保存当场就要被拒**，而且不许留下半拉子配置。
///
/// `net_tls` 那条"没 PEM 就退回 strict"的兜底是安全的一侧，但它只能在**读**的时候兜；
/// 如果写这一步放过去了，界面上就会出现"档位显示 caBundle、实际按 strict 走"的分裂状态。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn choosing_ca_bundle_without_a_pem_is_refused_at_save_time() {
    let dir = Tmp::new("nopem");
    let app = App::boot(dir.path()).expect("核心启动");
    let err = save(
        &app,
        json!({"label":"空 CA","baseUrl":"https://dav.internal:8443","username":"u",
               "password":"p","rootPrefix":"/.notes","tlsPolicy":"ca_bundle"}),
    )
    .expect_err("选了 ca_bundle 又没给 PEM，保存必须被拒");
    // 只认核心实发的那个字段名（实测串：`字段 ca_pem 无效: 选择「自定义 CA」时必须提供 PEM`）。
    // 这里刻意**不写** `|| err.contains("CA")` 那种"或"半边 —— 它在这条消息里几乎恒真，
    // 而 arch-check 第 28 条就是把这种断言抓出来的（这次抓到的是我自己新写的那一条）。
    assert!(
        err.contains("ca_pem"),
        "拒因没指名是哪一格，用户对着这句话不知道该填什么：{err}"
    );
    assert!(
        current(&app).is_null(),
        "保存被拒了却留下一条账户（半拉子的 ca_bundle 配置）：{:?}",
        current(&app)
    );
}
