//! 缺口 G38 选了 B 之后的那一格：**没有系统凭据库的平台上，口令只活在这次进程里**。
//!
//! 为什么必须单独一个文件、并且要一条测试缝：这一格的真相只在 `available()==false` 时成立，
//! 而本机是 Windows（真凭据库在）。`credentials.rs` 里那两条非 Windows 分支在 CI 的 macos
//! job 上到底跑不跑还不一定（那个 job 只出包），所以**光靠平台分支写着，等于没人验**。
//! 这里用 `credential_store::force_session_only()` 把 OS 后端当"这个平台没有"，于是退路
//! 在本机被真跑一遍。那条缝只在**本测试二进制**里翻（cargo 每个集成测试文件一个进程），
//! 不会跟 `credentials.rs`、`proxy_account_407.rs` 抢全局态。
//!
//! 三条最容易说谎的地方，判据都打在边界上而不是自述：
//! 1. **"存进去了"** —— 不看返回值，看 DTO 那三位（`hasCredential` / `credentialLive` /
//!    `credentialPersistent`）是不是把"引用挂着""这一轮拿得到""重启后还在"分开说了；
//! 2. **"这一轮真用上了"** —— 本文件**故意不设** `NOTERA_DEV_WEBDAV_SECRET`：debug 构建里
//!    `secret_for` 取不到凭据时会退回那个环境变量（`proxy_account_407.rs` 就是靠它喂的）。
//!    不设它，"这一轮成了"才只能来自会话表；再用"清掉口令 ⇒ 同一账户同一目录立刻不成"做差分，
//!    把"其实一直靠环境变量"这条路堵死。这正是本项目踩过三次的那个形状（实现齐全、单测全绿、
//!    没人调用它）。
//! 3. **"绝不落盘"** —— 不猜 SQLite 的列，直接把数据目录里**每一个文件**按字节扫一遍找口令。
//!
//! 跑法：`cargo test -p notera-host --test credential_session`
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use notera_host::commands::{self, AccountDraftCmd, AccountDto, SyncStatusDto};
use notera_host::{credential_store, App};
use notera_test_webdav::{Backend, HttpForwardProxy, TestServer};
use serde_json::json;

const WD_PASS: &str = "wd-会话态-口令-9f3c";
const PUSER: &str = "pu";
const PPASS: &str = "pp-会话态-代理口令-2b7a";

static SEQ: AtomicUsize = AtomicUsize::new(0);

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("notera-csess-{tag}-{}-{n}", std::process::id()));
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

/// 每个测试进来先做两件事：把 OS 后端当没有；把那条开发用环境变量**撤掉**。
/// 少了第二件，`secret_for` 会在会话表读空时静默退回环境变量，于是"这一轮成了"什么也没证明。
fn in_a_world_without_an_os_store() {
    credential_store::force_session_only();
    std::env::remove_var("NOTERA_DEV_WEBDAV_SECRET");
    assert!(
        !credential_store::available(),
        "那条缝没翻成功 —— 这一整个文件就都在测真凭据库，而不是它要测的退路"
    );
    assert!(
        std::env::var("NOTERA_DEV_WEBDAV_SECRET").is_err(),
        "环境变量还在：退路读空时会退回它，下面的「这一轮成了」就成了假的"
    );
}

/// 账户草稿；`proxy_port` 给 `Some` 时带上"要认证的 HTTP 代理"那三样。
fn draft(
    id: &str,
    url: &str,
    password: Option<&str>,
    proxy_port: Option<u16>,
    proxy_pass: Option<&str>,
) -> serde_json::Value {
    let mut obj = json!({
        "label": "会话态账户",
        "baseUrl": url,
        "username": "notera-test",
        "id": id,
    });
    if let Some(p) = password {
        obj["password"] = json!(p);
    }
    if let Some(port) = proxy_port {
        obj["proxyMode"] = json!("http");
        obj["proxyHost"] = json!("127.0.0.1");
        obj["proxyPort"] = json!(port);
        obj["proxyUsername"] = json!(PUSER);
        if let Some(p) = proxy_pass {
            obj["proxyPassword"] = json!(p);
        }
    }
    obj
}

fn configure(app: &App, cmd: serde_json::Value) -> AccountDto {
    let cmd: AccountDraftCmd = serde_json::from_value(cmd).expect("草稿形态合法");
    app.configure_account(cmd).expect("配置账户")
}

fn status(app: &App) -> SyncStatusDto {
    app.sync_status().expect("状态读得到")
}

/// 本机写一条笔记，返回它的 id（后面拿它去源站快照里找）。
fn write_note(app: &App, text: &'static str, block_id: &'static str) -> String {
    let folder = app.default_folder_id().expect("有默认文件夹");
    let note = app
        .create_note(
            &folder,
            json!({ "v": 1, "content": [{ "id": block_id, "type": "paragraph", "content": [{ "text": text }] }] }),
        )
        .expect("本机建笔记");
    note.id.as_str().to_string()
}

/// 数据目录里**每一个文件**都按字节找一遍这些串。
///
/// 为什么不是"查 SQLite 的某一列"：这一格的结论是"口令不落盘"，而落盘的方式不止一条
/// （配置 JSON、WAL、临时/备份文件、日志）。按文件扫不需要认识 schema，也就不会被我自己的
/// 误解放过 —— 哪天多写了一个配置文件，这里照样红。
fn secrets_on_disk(dir: &Path, needles: &[&str]) -> Vec<String> {
    let mut hits = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            for needle in needles {
                if bytes.windows(needle.len()).any(|w| w == needle.as_bytes()) {
                    hits.push(format!(
                        "{} 里出现了「{needle}」",
                        path.strip_prefix(dir).unwrap_or(&path).display()
                    ));
                }
            }
        }
    }
    hits
}

/// 主腿：**没有系统凭据库 ≠ 配不出同步**。保存要成、这一轮要真走得到、重启要说人话。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_password_saves_and_carries_the_round_with_no_os_store() {
    in_a_world_without_an_os_store();
    let srv = TestServer::start(Backend::Mem).await;
    let dir = Tmp::new("round");
    let app = App::boot(dir.path()).expect("核心启动");

    let url = srv.base_url();
    let acct = configure(&app, draft("", &url, Some(WD_PASS), None, None));
    assert!(
        acct.has_credential,
        "存进会话表了却还报「没存过」—— 界面上的「口令已设置」会说谎"
    );
    assert!(
        acct.credential_live,
        "这一轮明明拿得到口令，DTO 却说拿不到（徽标会停在「需要凭据」）"
    );
    assert!(
        !acct.credential_persistent,
        "会话表里的口令不算持久 —— 这一位要是 true，界面上那句「退出后不用重填」就是假话"
    );

    let note = write_note(&app, "会话态写下的笔记", "blk000001");
    app.sync_once()
        .await
        .expect("没有系统凭据库也要同步得动：这一轮的口令只在会话表里");
    let dump = srv.dump_prefix("/.notes/records").to_string();
    assert!(
        dump.contains(&note),
        "账户说同步成了，服务器上却没有这条笔记（{note}）：{dump}"
    );
    assert_eq!(
        status(&app).pending_ops,
        0,
        "这一轮之后还有待办，却在说已同步"
    );

    // —— 模拟"进程重启"：配置里那条引用还在，会话表里已经没有东西了。
    credential_store::remove(&credential_store::webdav_target(&acct.id)).expect("清掉这一条");
    let after_restart = app.current_account().unwrap().expect("账户还在");
    assert!(
        after_restart.has_credential,
        "引用还挂着就仍然算「配过」—— 这一位不许跟着口令一起灭，否则用户以为账户没了"
    );
    assert!(
        !after_restart.credential_live,
        "口令已经没了却还报「拿得到」：界面会继续显示「已保存」，而每次同步都失败"
    );

    let note2 = write_note(&app, "重启之后写的笔记", "blk000002");
    let err = app
        .sync_once()
        .await
        .expect_err("重启之后拿不到口令，这一轮不许装作成功");
    assert_eq!(
        err.code, "sync_needs_credentials",
        "报的是 `{}`：`no_account` 那句「还没有配置同步服务器」在这一格是假话（账户明明配好了）",
        err.code
    );
    let st = status(&app);
    assert_eq!(
        st.message_key.as_deref(),
        Some("sync.needsCredentials"),
        "徽标那一行没指到「需要凭据」，用户看不出该做什么"
    );
    assert!(
        st.pending_ops >= 1,
        "没同步成的那一条被吞掉了（那才是真丢数据的前置）：{}",
        st.pending_ops
    );
    let dump = srv.dump_prefix("/.notes/records").to_string();
    assert!(
        !dump.contains(&note2),
        "这一轮明明该失败，`{note2}` 却出现在源站快照里 —— 上面那次失败没真发生"
    );

    // 补上口令 ⇒ 旧账要一起结清（§2「失败时明确提示并支持重试」在这一格的样子）。
    configure(&app, draft(&acct.id, &url, Some(WD_PASS), None, None));
    app.sync_once().await.expect("重填口令之后这一轮应当走得通");
    let dump = srv.dump_prefix("/.notes/records").to_string();
    assert!(
        dump.contains(&note) && dump.contains(&note2),
        "重填之后源站上少了东西：note={note} note2={note2} 快照={dump}"
    );
    assert_eq!(status(&app).pending_ops, 0, "重试成功了却还有待办");
    srv.stop().await;
}

/// 退路的原语那一半：会话表存的到底是什么、存到哪儿算完。
///
/// `credentials.rs` 里同形状的那两条只在**非 Windows** 走到（本机 Windows 走的是真凭据库），
/// 这里由那条缝保证同一段断言在 Windows 上也真跑过。
#[test]
fn the_session_backend_stores_exactly_what_it_was_given() {
    in_a_world_without_an_os_store();
    let target = credential_store::webdav_target("sess-src");
    let proxy = credential_store::proxy_target("sess-src");
    credential_store::put(&target, "用户-Ⅰ", WD_PASS).expect("会话表存得进去");
    credential_store::put(&proxy, PUSER, PPASS).expect("代理口令同样进会话表");
    assert_eq!(
        credential_store::get(&target).unwrap(),
        Some(("用户-Ⅰ".to_string(), WD_PASS.to_string())),
        "用户名与口令都要原样读回来 —— 只回口令会把「用户名」悄悄换成别的"
    );
    assert!(credential_store::live(&target));
    assert!(
        !credential_store::persistent(&target),
        "会话表里的永远不算持久 —— 界面上那句「退出后要重填」照这一位说"
    );
    // 分目标不许串：换服务器/换代理不该借到别人的口令。
    assert_eq!(
        credential_store::get(&credential_store::webdav_target("sess-other")).unwrap(),
        None,
        "另一条目标读到了这一条的口令"
    );
    assert_eq!(
        credential_store::get(&credential_store::proxy_target("sess-other")).unwrap(),
        None,
        "webdav 与 proxy 两个前缀必须各归各的"
    );
    // 覆盖 = 重设口令那个形状：第二次说了算。
    credential_store::put(&target, "用户-Ⅰ", "换过的口令").expect("替换");
    assert_eq!(
        credential_store::get(&target).unwrap().unwrap().1,
        "换过的口令"
    );
    credential_store::remove(&target).expect("删掉");
    assert_eq!(
        credential_store::get(&target).unwrap(),
        None,
        "删完要真的没有"
    );
    credential_store::remove(&target).expect("再删一次必须幂等");
    assert_eq!(
        credential_store::get(&proxy).unwrap().unwrap().1,
        PPASS,
        "删一条不许连带清掉别人的那条"
    );
    credential_store::remove(&proxy).expect("收尾");
}

/// 保证句：**口令绝不落盘**。SQLite、配置 JSON、WAL、临时文件、日志一起扫。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_session_password_never_reaches_the_disk() {
    in_a_world_without_an_os_store();
    let srv = TestServer::start(Backend::Mem).await;
    let proxy = HttpForwardProxy::start_requiring(PUSER, PPASS)
        .await
        .expect("起带 407 策略的代理");
    let url = srv.base_url();
    let dir = Tmp::new("nodisk");

    let app = App::boot(dir.path()).expect("核心启动");
    configure(
        &app,
        draft("", &url, Some(WD_PASS), Some(proxy.port()), Some(PPASS)),
    );
    write_note(&app, "带着两个口令的笔记", "blk000003");
    app.sync_once()
        .await
        .expect("两个口令都只在会话表里，这一轮走得通");

    let hits = secrets_on_disk(dir.path(), &[WD_PASS, PPASS]);
    assert!(
        hits.is_empty(),
        "口令落到盘上了（选 B 就意味着「只活在这次进程里」，落盘就是毁约）：{hits:?}"
    );
    // 反向自检：这个扫描器真看得见东西吗？拿笔记正文当针扎一次 ——
    // 连正文都扫不到说明它其实什么都没读，上面那句"没落盘"就没有依据（先验探针再下结论）。
    let canary = secrets_on_disk(dir.path(), &["带着两个口令的笔记"]);
    assert!(
        !canary.is_empty(),
        "扫描器连本机笔记正文都读不到 ⇒ 工装坏了，这条门禁的绿灯不算数"
    );
    proxy.shutdown().await;
    srv.stop().await;
}

/// 代理那一半：会话表里的**代理**口令真被供应商收到，而且只有用它才成得了。
/// 判据打在代理自己的计数上（它没拿到正确凭据就一律 407、一个字节都不转）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_proxy_password_held_only_in_this_session_reaches_the_proxy() {
    in_a_world_without_an_os_store();
    let srv = TestServer::start(Backend::Mem).await;
    let proxy = HttpForwardProxy::start_requiring(PUSER, PPASS)
        .await
        .expect("起带 407 策略的代理");
    let url = srv.base_url();
    let dir = Tmp::new("proxy");
    let app = App::boot(dir.path()).expect("核心启动");

    let acct = configure(
        &app,
        draft("", &url, Some(WD_PASS), Some(proxy.port()), Some(PPASS)),
    );
    let note = write_note(&app, "会话里的代理口令写下的笔记", "blk000004");

    app.sync_once()
        .await
        .expect("代理口令只在会话表里，这一轮也要走得通");
    assert!(
        proxy.forwarded() >= 1,
        "这一轮成了而代理转发数是 0 ⇒ 同步绕过了配置里的代理"
    );
    assert_eq!(
        proxy.auth_rejects(),
        0,
        "口令是对的却被拒过 ⇒ 这次成功是蒙的"
    );
    let dump = srv.dump_prefix("/.notes/records").to_string();
    assert!(dump.contains(&note), "源站没有这条笔记：{dump}");

    // 差分：只清代理那一条（服务器口令还在），下一轮必须**在代理那一层**断，
    // 且一条都不许到源站 —— 悄悄退回直连是最坏的形状（用户以为代理还在起作用）。
    let served_before = srv.request_log().len();
    credential_store::remove(&credential_store::proxy_target(&acct.id)).expect("清掉代理那一条");
    let err = app
        .sync_once()
        .await
        .expect_err("代理口令没了，这一轮必须失败");
    assert_eq!(
        err.code, "proxy_credential_missing",
        "报的是 `{}`：这一轮看着不像「配置的代理少了凭据」，而是别的东西",
        err.code
    );
    assert_eq!(
        srv.request_log().len(),
        served_before,
        "代理凭据缺失的那一轮仍然打到了源站 ⇒ 产品悄悄绕过代理直连了"
    );
    proxy.shutdown().await;
    srv.stop().await;
}

/// 上限那条规则是**后端无关**的：会话表也不许收超长口令，且拒绝时什么都不改。
/// 走产品入口（`configure_account`）而不是只调原语 —— 界面上那句提示来自这条边的错误码。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_over_long_password_is_refused_through_the_product_path_without_storing() {
    in_a_world_without_an_os_store();
    let srv = TestServer::start(Backend::Mem).await;
    let dir = Tmp::new("toolong");
    let app = App::boot(dir.path()).expect("核心启动");

    let exactly_max = "ａ".repeat(credential_store::MAX_UNITS); // 全角：一个字符 = 一个 UTF-16 单元
    let ok = configure(
        &app,
        draft("", &srv.base_url(), Some(&exactly_max), None, None),
    );
    assert!(
        ok.has_credential && ok.credential_live,
        "正好在上限内必须存得进去"
    );

    let one_over = "ａ".repeat(credential_store::MAX_UNITS + 1);
    let cmd: AccountDraftCmd =
        serde_json::from_value(draft(&ok.id, &srv.base_url(), Some(&one_over), None, None))
            .expect("草稿合法");
    let err = app
        .configure_account(cmd)
        .expect_err("超长口令必须当场拒绝，不许悄悄截断存一半");
    assert_eq!(
        err.code, "credential_too_long",
        "报的是 `{}`：超长口令如果被截断，用户拿到的是一个「配好了但 401」的账户",
        err.code
    );
    // 上一次成功那份不能被这次失败改掉："拒绝 = 全部不改"。
    let back = app.current_account().unwrap().expect("账户还在");
    assert!(back.has_credential, "一次被拒绝的保存把配置里的引用改没了");
    assert!(
        credential_store::live(&credential_store::webdav_target(&back.id)),
        "被拒绝的那次保存把原来还能用的口令一起带走了"
    );
    srv.stop().await;
}

/// 只改标签、没重填口令：会话里那一条必须还在，这一轮照样走得通。
/// `credentials.rs` 里同形状那条依赖真凭据库（非 Windows 直接 return），这一条补上退路那一支。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reconfiguring_without_a_password_keeps_the_session_secret() {
    in_a_world_without_an_os_store();
    let srv = TestServer::start(Backend::Mem).await;
    let dir = Tmp::new("keep");
    let app = App::boot(dir.path()).expect("核心启动");
    let url = srv.base_url();
    let first = configure(&app, draft("", &url, Some(WD_PASS), None, None));
    assert!(first.credential_live);

    let mut obj = draft(&first.id, &url, None, None, None);
    obj["label"] = json!("只是改个名字");
    let second = configure(&app, obj);
    assert_eq!(second.label, "只是改个名字", "标签没改成功");
    assert!(
        second.credential_live,
        "没重填口令就等于把口令删了 —— 用户下一次同步会撞「需要凭据」"
    );
    write_note(&app, "改过名字之后还要同步得动", "blk000005");
    app.sync_once().await.expect("只改标签之后这一轮照样走得通");
    assert_eq!(status(&app).pending_ops, 0, "改个标签把待办弄丢了");
    srv.stop().await;
}

/// 跨语言契约：界面上那三句话读的是 `credentialLive` / `credentialPersistent` 两个**键名**。
///
/// 判据打在真序列化输出上而不是 TS 类型上：`AccountDto` 是 `#[serde(rename_all = "camelCase")]`，
/// 哪天有人把它换成 `snake_case`、或者把字段改名，TS 那边只会把它当 `undefined`，
/// 于是 `credentialVolatile` 永远为 false —— 界面安静地退回到"看着像已保存"，编译期和运行期都不报错。
/// 顺手把"口令本体绝不出现在载荷里"钉在同一份输出上：那才是这个 DTO 存在的理由。
#[test]
fn the_two_flags_reach_the_wire_under_their_camel_case_names_and_no_secret_leaks() {
    in_a_world_without_an_os_store();
    let dir = Tmp::new("wire");
    let app = App::boot(dir.path()).expect("核心启动");
    let saved = commands::dispatch(
        &app,
        "configure_account",
        draft(
            "",
            "http://127.0.0.1:5005/.notes",
            Some(WD_PASS),
            None,
            None,
        ),
    )
    .expect("会话表里存口令应当成功");
    for key in ["hasCredential", "credentialLive", "credentialPersistent"] {
        assert!(
            saved.get(key).is_some(),
            "`configure_account` 的载荷里没有 `{key}`：界面读它会拿到 undefined，那三句提示全都不出现"
        );
    }
    assert_eq!(saved["hasCredential"], json!(true));
    assert_eq!(
        saved["credentialLive"],
        json!(true),
        "键名对上了但值是 false —— 界面会显示「请重填」而口令其实就在会话表里"
    );
    assert_eq!(
        saved["credentialPersistent"],
        json!(false),
        "会话表里的口令被说成持久 —— 界面上那句「退出后不用重填」就成了假话"
    );
    // 载荷不许带出口令本体（`account` 那条是界面每次进设置页都会读的那一份）。
    let read_back = commands::dispatch(&app, "account", json!({})).expect("账户读得回来");
    let blob = format!("{saved} {read_back}");
    assert!(
        !blob.contains(WD_PASS),
        "下发给界面的账户载荷里出现了口令本体：{}",
        blob.chars().take(240).collect::<String>()
    );
    assert!(
        !blob.contains("keychain:"),
        "配置内部的那个引用前缀被下发到了界面"
    );
}
