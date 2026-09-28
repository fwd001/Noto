//! §20 崩溃注入（TEST-PLAN L5）：进程在同步的每个提交点被**当场杀死**，
//! 重启之后必须仍然可恢复 —— 而且是机器证据，不是"理论上 WAL 会回滚"。
//!
//! 为什么要把子进程真的 spawn 出来：崩溃恢复的语义是"进程消失、什么都没收尾"。
//! 在同一进程里 panic + catch_unwind 测的是另一件事（析构会跑、WAL 不会被真正
//! 打断），那种测试全绿而产品在断电下坏掉是完全可能的。
//!
//! 两个方向都要钉住：
//! 1. **每个登记的注入点都必须真的在代码路径上** —— 子进程若在这一点没死，就是
//!    红（点名一个没人经过的"崩溃点"是自欺）。
//! 2. **崩完之后重启要能收敛** —— 用户写过的内容一条都不能少，也不能凭空多，
//!    outbox / dirty 最终归零，别拉一条"看起来同步好了其实丢了"的库。
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use notera_core::CRASH_EXIT_CODE;
use notera_host::commands::AccountDraftCmd;
use notera_host::App;
use notera_store::NoteQuery;
use notera_test_webdav::{Backend, TestServer};
use serde_json::json;

const SECRET: &str = "sup3r-s3cr3t";
/// 子进程模式开关：`<数据目录>|<base url>`。父进程不设这个，因此普通跑测试时
/// 那个"子进程测试"直接返回，不会伪装成一条通过的用例。
const CHILD: &str = "NOTERA_CRASH_CHILD";

static SEQ: AtomicUsize = AtomicUsize::new(0);

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("notera-crash-{tag}-{}-{n}", std::process::id()));
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

fn draft(base_url: &str) -> AccountDraftCmd {
    serde_json::from_value(json!({
        "label": "崩溃注入盘", "baseUrl": base_url, "username": "notera-test",
    }))
    .expect("最小账户草案")
}

fn boot(dir: &Path, base_url: &str) -> App {
    std::env::set_var("NOTERA_DEV_WEBDAV_SECRET", SECRET);
    let app = App::boot(dir).expect("核心启动");
    if app.current_account().unwrap().is_none() {
        app.configure_account(draft(base_url)).expect("配置账户");
    }
    app
}

/// 子进程本体：写一条本地笔记（带一个附件），推正文、传附件 —— 沿途任一点被
/// `NOTERA_CRASH_AT` 命中就直接消失（见 `notera_core::crash_point`）。
///
/// 附件必须真的带上：`sync_once` 只跑正文轮，附件走 `run_attachment_round`，
/// 队列为空时那两个注入点根本不会被经过（实测就是这样露出"点没接上"的）。
/// 崩溃矩阵负责的注入点 = 全表减去"要大库夹具才走得到"的那几个。
/// 那几个由 `tests/compaction_crash.rs` 覆盖；这里做减法而不是各写一份名单，
/// 是为了让"新加了注入点却没人覆盖"变成编译期/断言期就炸，而不是静静少测一格。
fn matrix_points() -> Vec<&'static str> {
    for extra in notera_core::CRASH_POINTS_NEED_LARGE_LIBRARY {
        assert!(
            notera_core::CRASH_POINTS.contains(extra),
            "大库名单里的 {extra} 不在 CRASH_POINTS 里"
        );
    }
    for extra in notera_core::CRASH_POINTS_NEED_SPECIAL_FIXTURE {
        assert!(
            notera_core::CRASH_POINTS.contains(extra),
            "专门夹具名单里的 {extra} 不在 CRASH_POINTS 里"
        );
    }
    notera_core::CRASH_POINTS
        .iter()
        .filter(|p| !notera_core::CRASH_POINTS_NEED_LARGE_LIBRARY.contains(p))
        .filter(|p| !notera_core::CRASH_POINTS_NEED_SPECIAL_FIXTURE.contains(p))
        .copied()
        .collect()
}

#[test]
fn child_writes_and_syncs() {
    let Ok(spec) = std::env::var(CHILD) else {
        return; // 不是子进程模式：这条测试什么都不做（父进程靠 spawn 显式启用）
    };
    let Some((dir, url)) = spec.split_once('|') else {
        return;
    };
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async move {
        let app = boot(Path::new(dir), url);
        let folder = app.default_folder_id().unwrap();
        // 标题带上注入点：这样"每一条都活下来、且只有一条"是可逐点判定的，
        // 而不是九轮写完只看得出"总数好像不对"。
        let point = std::env::var("NOTERA_CRASH_AT").unwrap_or_default();
        let text = format!("崩溃前写下的内容 {point}");
        let note = app.create_note(&folder, doc(&text)).expect("子进程写本地");
        let id = notera_core::EntityId::parse(&note.id).unwrap();
        // 每轮一份**唯一**字节：共用同一 blob 的话，第一轮传完之后队列就是空的，
        // `before/after_attachment_upload` 这两个点根本不会被经过（实测就是这样假绿）。
        let blob: Vec<u8> = format!("notera-crash-attachment-{point}").into_bytes();
        let sha = app
            .store()
            .attach_blob(&id, &blob, "image/png", Some("crash.png"), "blk000002")
            .unwrap()
            .sha256;
        // head 必须在 attach 之后再取：`attach_blob` 自己会推进 rev
        let head = app.store().get_note(&id).unwrap().unwrap();
        // 真实顺序（`stores/editor.ts::attachFile`）：引用住在 doc 里，落完 blob 才写正文
        app.store()
            .edit_note(
                &id,
                json!({ "v": 1, "content": [
                    { "id": "blk000001", "type": "paragraph", "content": [{ "text": text.clone() }] },
                    { "id": "blk000002", "type": "image", "attrs": {
                        "sha256": &sha, "ref": &sha, "role": "inline", "pending": false,
                        "size": blob.len(), "mediaType": "image/png", "name": "crash.png" } },
                ] }),
                head.rev,
            )
            .expect("子进程写带图的正文");
        let text = app.sync_once().await.expect("子进程的正文轮");
        if let Some(remote) = app.remote_for_sync().await.expect("装配远端") {
            let att = app.run_attachment_round(&remote).await;
            eprintln!("CHILD_ROUND text={text:?} attachment={att:?}");
        }
    });
}

/// 失败时把"到底哪一条挂着、挂在什么状态"打出来：不带这个的断言只能说明
/// "没收敛"，指不出是笔记脏着、还是 outbox 悬着、还是两者不是同一条。
fn stuck_report(app: &App) -> String {
    let acct = match app.current_account().ok().flatten() {
        Some(a) => a.id.to_string(),
        None => return "没有启用中的账户".to_string(),
    };
    let counts = format!(
        "outbox pending={} inflight={} failed={}",
        app.store()
            .outbox_len(&acct, &[notera_store::OpState::Pending])
            .unwrap_or(u32::MAX),
        app.store()
            .outbox_len(&acct, &[notera_store::OpState::Inflight])
            .unwrap_or(u32::MAX),
        app.store()
            .outbox_len(&acct, &[notera_store::OpState::Failed])
            .unwrap_or(u32::MAX),
    );
    let mut dirty = Vec::new();
    for r in app
        .store()
        .list_notes(&NoteQuery::all())
        .unwrap_or_default()
    {
        if let Ok(Some(n)) = app.store().get_note(&r.id) {
            if n.rev != n.sync_rev {
                dirty.push(format!("{} rev={} sync_rev={}", n.title, n.rev, n.sync_rev));
            }
        }
    }
    let mut rows = Vec::new();
    for o in app.store().outbox_take(&acct, 8).unwrap_or_default() {
        rows.push(format!(
            "{} rev={:?} sha={:?} op={:?}",
            o.kind,
            o.payload_rev,
            o.sha256.map(|s| s[..8].to_string()),
            o.op
        ));
    }
    format!("{counts}；脏笔记 {dirty:?}；待办行 {rows:?}")
}

fn spawn_crashed(point: &str, dir: &Path, url: &str) -> i32 {
    let out = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "child_writes_and_syncs",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD, format!("{}|{}", dir.display(), url))
        .env("NOTERA_CRASH_AT", point)
        .env("NOTERA_DEV_WEBDAV_SECRET", SECRET)
        .output()
        .expect("spawn 子进程");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    let code = out.status.code().unwrap_or(-1);
    // 死法也必须是"被注入杀死"：正常退出说明这一点根本没人经过；
    // panic / 其它非零码说明进程是被别的原因弄挂的 —— 那都不算崩溃注入的证据。
    assert_eq!(
        code, CRASH_EXIT_CODE,
        "注入点 {point} 没有让进程死在那里（退出码 {code}）\nstderr={stderr}"
    );
    code
}

/// `multi_thread` 是必须的：测试服务器跑在**本进程**里，而父进程要阻塞等子进程退出。
/// 单线程 runtime 下那一等就把服务器一起卡住，子进程只会拿到"读不到 protocol"。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_crash_point_is_on_the_real_path_and_the_library_recovers() {
    let srv = TestServer::start(Backend::Mem).await;
    let url = srv.base_url();

    let a_dir = Tmp::new("victim");
    let a = boot(a_dir.path(), &url);
    // §2：空库先入伙协商根，之后才允许有内容 —— 顺序反了就是 `root_mismatch`
    a.sync_once().await.expect("A 入伙");

    // 另一台设备先放点东西上去：子进程那一轮因此既有得推、也有得拉，
    // 崩溃点才落在真实的 push/apply 路径中间而不是空轮次里。
    let b_dir = Tmp::new("peer");
    {
        let b = boot(b_dir.path(), &url);
        b.sync_once().await.expect("B 入伙");
        let f = b.default_folder_id().unwrap();
        b.create_note(&f, doc("对面那台写的笔记")).unwrap();
        b.sync_once().await.expect("对面上去一轮");
    }

    // 基线：这台设备先干净地同步过一次，后面的崩溃才发生在"有已确认状态"之上
    {
        let f = a.default_folder_id().unwrap();
        a.create_note(&f, doc("基线笔记")).unwrap();
        a.sync_once().await.expect("基线轮");
    }

    let mut reached: Vec<String> = Vec::new();
    for point in matrix_points() {
        spawn_crashed(point, a_dir.path(), &url);
        reached.push((*point).to_string());

        // 重启 = 同一个目录再开一次，然后把这一轮跑完（正文 + 附件，和生产调度器一样）
        let stats = a.sync_once().await.expect("崩溃后的恢复轮");
        assert!(
            stats.outcome != notera_sync::RoundOutcome::Failed,
            "在 {point} 崩过一次，恢复轮直接失败：{stats:?}"
        );
        if let Some(remote) = a.remote_for_sync().await.expect("装配远端") {
            a.run_attachment_round(&remote).await;
        }
        let again = a.sync_once().await.expect("恢复后的第二轮");
        let pend = a.store().stats().unwrap().outbox_pending;
        if pend != 0 {
            // 只在失败路径上调 `stuck_report`：它会 take 走待办行（有副作用）
            panic!(
                "在 {point} 崩过一次之后，连着两轮同步仍留下 {pend} 条待办（第二轮 {again:?}）—— 这条改动永远不会结清\n{}",
                stuck_report(&a)
            );
        }
    }
    assert_eq!(reached.len(), matrix_points().len(), "有注入点没被跑到");

    // 收敛：正文轮 + 附件轮都要跑 —— 产品调度器就是这么排的，
    // 只跑正文轮就断言"结清了"是把附件那条腿当垃圾丢掉。
    for _ in 0..4 {
        a.sync_once().await.expect("A 收敛");
        if let Some(remote) = a.remote_for_sync().await.expect("装配远端") {
            a.run_attachment_round(&remote).await;
        }
    }
    let a_titles: Vec<String> = a
        .store()
        .list_notes(&NoteQuery::all())
        .unwrap()
        .into_iter()
        .map(|r| r.title)
        .collect();
    assert!(
        a_titles.iter().any(|t| t.contains("对面那台")),
        "别人的笔记没能拉下来：{a_titles:?}"
    );
    for point in matrix_points() {
        let mine = a_titles.iter().filter(|t| t.ends_with(point)).count();
        assert_eq!(
            mine, 1,
            "注入点 {point} 那轮写的笔记必须活下来且只有一条（0 = 被崩溃吃掉，>1 = 崩溃后被造出副本）：{a_titles:?}"
        );
    }

    let b = boot(b_dir.path(), &url);
    b.sync_once().await.expect("B 拉取");
    let mut b_titles: Vec<String> = b
        .store()
        .list_notes(&NoteQuery::all())
        .unwrap()
        .into_iter()
        .map(|r| r.title)
        .collect();
    b_titles.sort();
    let mut a_sorted = a_titles.clone();
    a_sorted.sort();
    assert_eq!(
        b_titles, a_sorted,
        "崩了九次之后两台设备必须逐条一致（多了=造副本，少了=丢数据）"
    );

    let st = a.store().stats().unwrap();
    assert_eq!(
        st.dirty_notes, 0,
        "崩溃若干次后本地还留着未公告的改动：{st:?}"
    );
    assert_eq!(
        st.outbox_pending, 0,
        "outbox 没结清，改动会永远悬着：{st:?}"
    );
    assert_eq!(
        st.attachments as usize,
        matrix_points().len() - 1,
        "每个注入点各一份唯一字节，只有崩在「写本地」那一轮还没走到附件：{st:?}"
    );
    srv.stop().await;
}
