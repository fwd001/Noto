//! §28「重试」那一格的**整轮**版本（前面那批在传输层：`notera-webdav/tests/retry_backoff.rs`、
//! `notera-net/tests/retry_idempotency.rs`）。这里问的是用户那一句：
//!
//! > 同步失败时应有明确提示，并支持重试。
//!
//! 拆开是三样得到货的判断，缺任何一样都会撒谎：
//!
//! * **挡不挡得住抖动**：一轮里 `protocol.json` 连坏两次，用户看到的是成还是败？这条量出来的
//!   是 shipped 政策的**真实预算**（1 次挡得住、2 次挡不住 = 缺口 G34，见第一个测试的注释）。
//!   只有传输层门禁证明不了这件事 —— 那里我自己 `with_retry` 给了预算，而产品那条边从没覆盖过。
//! * **一直坏要坏得看得见**：⇒ 这一轮**必须**红（`sync_refused` + `sync.protocol_unreadable`
//!   "稍后重试"那句），待办**不许**被吞，本机**照样**能写，撤掉故障后一次重试真把两条都推上去。
//! * **错误码不许串门**：瞬时故障报的是 `sync_refused` 而**不是** `sync_auth_failed`。
//!   这是 G33 那次修复的另一侧 —— 那次把凭据问题分了出来，这里钉住"服务器不回话不许被顺手
//!   改成凭据话术"：用户对着"不认这组凭据"会去翻口令，而这里该做的真的只是稍后重试。
//!
//! 实测读数是**整批 0.49 s**（不是我动手前以为的"按 §7 的 2 s 退避要十几秒"）——
//! 差在这产品的出口压根没走 §7 那套政策，这条文件就是把这件事量出来记在案上。
//!
//! 跑法：`cargo test -p notera-host --test retry_in_round`

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use notera_host::commands::{AccountDraftCmd, SyncStatusDto};
use notera_host::App;
use notera_test_webdav::{Backend, Injection, TestServer};
use serde_json::json;

const SECRET: &str = "sup3r-s3cr3t";
const PROTO: &str = "/.notes/protocol.json";
static SEQ: AtomicUsize = AtomicUsize::new(0);

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("notera-retry-{tag}-{}-{n}", std::process::id()));
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

fn doc(text: &str) -> serde_json::Value {
    json!({ "v": 1, "content": [{ "id": "blk000001", "type": "paragraph", "content": [{ "text": text }] }] })
}

fn boot(dir: &Path, url: &str) -> App {
    std::env::set_var("NOTERA_DEV_WEBDAV_SECRET", SECRET);
    let app = App::boot(dir).expect("核心启动");
    if app.current_account().unwrap().is_none() {
        let draft: AccountDraftCmd = serde_json::from_value(json!({
            "label": "重试这一轮", "baseUrl": url, "username": "notera-test",
        }))
        .unwrap();
        app.configure_account(draft).expect("配置账户");
    }
    app
}

fn status(app: &App) -> SyncStatusDto {
    app.sync_status().expect("状态读得到")
}

/// 源站日志里打到 (method, path) 且状态等于 `status` 的条数。
fn hits(srv: &TestServer, method: &str, path: &str, status: u16) -> usize {
    srv.request_log()
        .iter()
        .filter(|r| r.method == method && r.path == path && r.status == status)
        .count()
}

/// 这一轮里 `protocol.json` 的读被注入成 503：**坏一次挡得住，坏两次挡不住** ——
/// 而这正是缺口 **G34** 的测量值，不是一句"重试可用"的绿灯。
///
/// `WebDavRemote::new` 把重试政策硬编成 `RetryPolicy::deterministic(40, 1)`（40 ms、预算 1、
/// 无抖动），而 `docs/PROXY.md` §7 写的是 `2s × 1.85^n × (1±0.2)`、预算 3。`with_retry` 是留给
/// 调用方覆盖用的，产品里**没人调**（全仓唯一的调用点就是 `notera-webdav/tests/retry_backoff.rs`
/// 自己）。所以"真实抖动"这一格现在的答案是：**一次能扛，两次就整轮失败**，用户看到的是
/// "稍后重试"，而规范说这一步本来该被退避吸收掉。
///
/// 这两半都是**特征化断言**（pin 住现状），G34 修好之后第二半必须翻向 —— 那时这条会红，
/// 红得正是时候：它会把"预算从 1 变 3"这件事连同 §7 的措辞一起拽出来重验。
/// 为什么不在这里直接改产品：那一改会把"一轮失败"的耗时从 40 ms 变成十几秒，涉及**所有**
/// 离线/死服务器测试与界面反馈时延，属 §9 那类要先量的取舍，不在本批盲动。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_shipped_retry_budget_absorbs_one_blip_and_no_more() {
    let srv = TestServer::start(Backend::Mem).await;
    let dir = Tmp::new("one");
    let app = boot(dir.path(), &srv.base_url());
    app.sync_once()
        .await
        .expect("前置：第一轮要先成，protocol.json 得在服务器上");
    let folder = app.default_folder_id().unwrap();
    let note = app
        .create_note(&folder, doc("抖一次也该同步上去"))
        .expect("本机建笔记");

    // 半程①：只坏一次 ⇒ 预算 1 够用 ⇒ 这一轮必须成，且用户看不到失败。
    srv.inject(Injection {
        status_for: vec![(format!("GET {PROTO}#1"), 503)],
        ..Default::default()
    })
    .await;
    app.sync_once()
        .await
        .expect("一次 503 抖动必须被重试吸收（预算 1 的下限）");
    assert_eq!(
        hits(&srv, "GET", PROTO, 503),
        1,
        "注入没打到这一轮的关键读上：那下面那句『挡住了』没有依据"
    );
    let st = status(&app);
    assert_eq!(st.badge, "synced", "一轮成功却留着别的徽章：{}", st.badge);
    assert_eq!(st.pending_ops, 0, "同步成功了却还有待办");
    let dump = srv.dump_prefix("/.notes/records").to_string();
    assert!(
        dump.contains(note.id.as_str()),
        "重试说成了，服务器上却没有这条记录：{dump}"
    );

    // 半程②：连坏两次 ⇒ 按 shipped 的预算 1 就追不上了 ⇒ 整轮失败（G34 的症状本身）。
    let dir2 = Tmp::new("two");
    let app2 = boot(dir2.path(), &srv.base_url());
    app2.sync_once().await.expect("前置：这一台也要先配好");
    srv.clear_injection().await;
    app2.sync_once().await.expect("前置：撤掉注入后先成一轮");
    let folder2 = app2.default_folder_id().unwrap();
    app2.create_note(&folder2, doc("抖两次就掉下去了"))
        .expect("本机建笔记");
    let before503 = hits(&srv, "GET", PROTO, 503);
    srv.inject(Injection {
        status_for: vec![(format!("GET {PROTO}#2"), 503)],
        ..Default::default()
    })
    .await;
    let err = app2
        .sync_once()
        .await
        .expect_err("连两次 503：shipped 预算是 1，这一轮按现状必须失败（G34）");
    assert_eq!(
        err.code, "sync_refused",
        "G34 的症状报成了别的码：{}",
        err.code
    );
    // 台账是累加的（半程①已经留下过一条 503），所以这里读**增量**。
    assert_eq!(
        hits(&srv, "GET", PROTO, 503) - before503,
        2,
        "这一轮只挨了 {} 次 503：注入没完整打到读上，上面那句预算判据不成立",
        hits(&srv, "GET", PROTO, 503) - before503
    );
    srv.stop().await;
}

/// 方向二：读一直坏 ⇒ 这一轮必须**看得见地**失败，而且失败只落在网络侧。
///
/// 四样东西一起钉：错误码与文案键（是"稍后重试"而不是"不认凭据"）、待办不被吞、
/// 本机照常能写、修好之后一次重试把两条都推上去。少任何一样，验收里那句
/// "失败时应有明确提示，并支持重试" 都只是话。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_round_that_stays_broken_fails_visibly_and_keeps_the_backlog() {
    let srv = TestServer::start(Backend::Mem).await;
    let dir = Tmp::new("broken");
    let app = boot(dir.path(), &srv.base_url());
    app.sync_once().await.expect("前置：第一轮要先成");
    let folder = app.default_folder_id().unwrap();
    let n1 = app
        .create_note(&folder, doc("坏着的时候也要留得住"))
        .expect("本机建笔记");

    srv.inject(Injection {
        status_for: vec![(format!("GET {PROTO}#9"), 503)],
        ..Default::default()
    })
    .await;

    let err = app
        .sync_once()
        .await
        .expect_err("protocol.json 一直在 503，这一轮不该报成");
    assert_eq!(
        err.code, "sync_refused",
        "瞬时故障被报成了 {}：G33 那次分出的是**凭据**问题，这里是服务器不回话，\
         文案该是『稍后重试』而不是『不认这组凭据』",
        err.code
    );
    let reason = err
        .detail
        .as_ref()
        .and_then(|d| d.get("reason"))
        .and_then(|v| v.as_str())
        .unwrap_or("<没有 reason>");
    assert_eq!(
        reason, "sync.protocol_unreadable",
        "载荷里那句可读的话不对：{reason}"
    );

    let st = status(&app);
    assert!(
        st.pending_ops >= 1,
        "一次失败的同步把待办吞掉了（那才是真丢数据的前置）：{}",
        st.pending_ops
    );

    // 保证句的那一半：网络坏了不许让本机不可用。
    let n2 = app
        .create_note(&folder, doc("修好之前还要写得动"))
        .expect("同步失败之后本机照样能写");

    // 重试腿：把注入撤掉，下一轮要真把两条都推上去。
    srv.clear_injection().await;
    app.sync_once()
        .await
        .expect("撤掉注入之后的重试应当成功（这是『支持重试』的那一半）");
    let dump = srv.dump_prefix("/.notes/records").to_string();
    for id in [n1.id.as_str(), n2.id.as_str()] {
        assert!(dump.contains(id), "重试成功了，服务器上却没有 {id}：{dump}");
    }
    assert_eq!(status(&app).pending_ops, 0, "重试成功了却还有待办");
    srv.stop().await;
}
