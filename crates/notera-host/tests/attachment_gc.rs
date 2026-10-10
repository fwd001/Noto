//! §8 的「blob 物理回收」（GC）**安全的那一半**：不删，先隔离；隔离区内待够宽限期，
//! 且**始终**没有任何笔记引用它，才真的让字节离开磁盘。
//!
//! 为什么一半一半地做（用户 2026-09-28 的决定第 2 条）：GC 是唯一一段"代码主动销毁用户
//! 字节"的逻辑，判据错一条就是不可恢复的数据丢失。所以这一批把销毁拆成两步，中间留一段
//! **可撤销**的现场：
//! * 隔离 = `<attachments>/<2hex>/<sha>` 挪进 `<data>/attachments-quarantine/<2hex>/<sha>`，
//!   账上写 `local_state='missing'` + `deleted_at=now`。字节一个都没少，只是换了地方。
//! * 真删 = 只有 `deleted_at` 早于调用方给的宽限期界、且引用仍为 0 的行才动手。
//!
//! 还有一条不那么显然的准入条件：**只回收服务器已经确认有副本（`remote_state='present'`）
//! 的行**。没传上去过的字节是本机独家的一份，隔离它会顺手把它从上传队列里摘掉
//! （两个队列都带 `deleted_at IS NULL`），那就是"为了省磁盘把独家副本判死"。数据安全排在
//! 性能前面，所以这一类今天不收。
//!
//! 跑法：`cargo test -p notera-host --test attachment_gc`

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
        let dir = std::env::temp_dir().join(format!("notera-gc-{tag}-{}-{n}", std::process::id()));
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
            "label": "GC 注入", "baseUrl": url, "username": "notera-test",
        }))
        .unwrap();
        app.configure_account(draft).expect("配置账户");
    }
    app
}

/// A 建一条带图的笔记并把图真的传上去，返回 `(App, sha)` —— App 要交回调用方，
/// 因为「永久删除这条笔记」这一步必须在同一台设备上做。
async fn seed_uploaded(dir: &Path, url: &str, blob: &[u8]) -> (App, String) {
    let app = boot(dir, url);
    app.sync_once().await.expect("A 入伙");
    let folder = app.default_folder_id().unwrap();
    let note = app.create_note(&folder, doc("GC 目标笔记")).unwrap();
    let id = notera_core::EntityId::parse(&note.id).unwrap();
    let sha = app
        .store()
        .attach_blob(&id, blob, "image/png", Some("shot.png"), "blk000002")
        .unwrap()
        .sha256;
    let head = app.store().get_note(&id).unwrap().unwrap();
    app.store()
        .edit_note(
            &id,
            json!({ "v": 1, "content": [
                { "id": "blk000001", "type": "paragraph", "content": [{ "text": "GC 目标笔记" }] },
                { "id": "blk000002", "type": "image", "attrs": {
                    "sha256": &sha, "ref": &sha, "role": "inline", "pending": false,
                    "size": blob.len(), "mediaType": "image/png", "name": "shot.png" } },
            ] }),
            head.rev,
        )
        .unwrap();
    app.sync_once().await.expect("A 推正文");
    let remote = app.remote_for_sync().await.unwrap().expect("A 有适配器");
    let round = app.run_attachment_round(&remote).await;
    assert_eq!(round.0, 1, "前置不成立：源设备要把这张图传上去：{round:?}");
    (app, sha)
}

/// B 拉到正文并把图下下来。
async fn drain_target(dir: &Path, url: &str, blob: &[u8], sha: &str) -> App {
    let b = boot(dir, url);
    b.sync_once().await.expect("B 拉到正文");
    let remote = b.remote_for_sync().await.unwrap().expect("B 有适配器");
    let round = b.run_attachment_round(&remote).await;
    assert_eq!(round.1, 1, "B 第一轮要把图下完：{round:?}");
    assert_eq!(
        official_bytes(&b, sha),
        blob.to_vec(),
        "前置不成立：本机没拿到那份字节"
    );
    b
}

/// 正式位置上的字节（读不到回空）。
fn official_bytes(app: &App, sha: &str) -> Vec<u8> {
    std::fs::read(app.store().blob_path(sha)).unwrap_or_default()
}

/// 隔离区里的字节（读不到回空）—— 这条是"挪开而不是销毁"的唯一物证。
fn quarantine_bytes(app: &App, sha: &str) -> Vec<u8> {
    std::fs::read(app.store().quarantine_path(sha)).unwrap_or_default()
}

/// 账上这一行的隔离判据：`local_state` + `deleted_at`。
fn ledger(app: &App, sha: &str) -> (String, Option<String>) {
    app.store()
        .attachment_quarantine_state(sha)
        .expect("这一行该还在账上")
}

/// 打到这个 sha 上的 GET 条数（下载请求的路径就是内容寻址名）。
/// 「撤销期内重新引用要零网络」这条只能靠它变成可数的证据。
fn gets_for_sha(srv: &TestServer, sha: &str) -> usize {
    srv.request_log()
        .iter()
        .filter(|r| r.method == "GET" && r.path.contains(sha))
        .count()
}

/// 编辑器取图那条路：坏/缺必须是具名错误，不能是一段能画出来的错字节。
fn ui_read_code(app: &App, sha: &str) -> (Option<String>, Option<usize>) {
    match notera_host::commands::dispatch(app, "attachment_data", json!({ "sha256": sha })) {
        Ok(v) => (
            v.get("code").and_then(|x| x.as_str()).map(str::to_string),
            v.get("bytesBase64").and_then(|x| x.as_str()).map(str::len),
        ),
        Err(e) => (Some(e.code), None),
    }
}

fn call_recovery(app: &App, name: &str, sha: &str) -> Option<String> {
    match notera_host::commands::dispatch(app, name, json!({ "sha256": sha })) {
        Ok(_) => None,
        Err(e) => Some(e.code),
    }
}

/// 宽限期的两把尺子：由调用方给，测试就不依赖墙上时钟（也不会因为机器时间跳而偶发红）。
/// `deleted_at < cutoff` 才动手，所以"全都过期"取未来、"都没过期"取远古。
const ALL_PAST: &str = "9999-01-01T00:00:00.000Z";
const ALL_FUTURE: &str = "0000-01-01T00:00:00.000Z";

// ———————————————————————————————————————— FT-ATT-29：隔离，而不是删

/// 「永久删除一条带图的笔记」之后，那份字节今天会永久留在 `<attachments>/` 里 ——
/// 没有任何一行代码会把它拿走（§48 的 G1）。这一条钉住新增的回收第一步，并把它**不该**
/// 做的三件事一起钉住：
/// * 还在回收站里的笔记**仍算引用** → 不收（用户随时可能还原）。
/// * 收 = 挪进隔离区 + 账上写 `missing ∧ deleted_at` → **字节还在盘上**，不是销毁。
/// * 收完之后两个队列与磁盘体检都不该再看见它（三条口径都带 `deleted_at IS NULL`），
///   否则它会一边被 GC 认领、一边被别的机器重下或重传。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unreferenced_blob_is_quarantined_not_deleted() {
    let srv = TestServer::start(Backend::Mem).await;
    let url = srv.base_url();
    let blob: Vec<u8> = (0..7_000).map(|i| (i % 97) as u8).collect();
    let a_dir = Tmp::new("29-a");
    let (a, sha) = seed_uploaded(a_dir.path(), &url, &blob).await;

    // 先只**软删**：笔记进回收站，行还在 → 引用还在 → GC 一步都不许动。
    let id = a
        .store()
        .list_notes(&notera_store::NoteQuery {
            folder: None,
            trash: false,
            limit: 50,
            offset: 0,
        })
        .unwrap()
        .into_iter()
        .find(|n| n.title.contains("GC 目标笔记"))
        .map(|n| n.id.clone())
        .expect("列表里该有那条笔记（软删之前）");
    a.store().delete_note(&id).expect("移进回收站");
    assert_eq!(
        a.reclaim_unreferenced_blobs(50),
        0,
        "回收站里的笔记仍算引用，这一份一条都不许收"
    );
    assert_eq!(
        official_bytes(&a, &sha),
        blob,
        "判据没成立却把正式位置上的字节动了"
    );

    // 还原回来再永久删除：`note_attachments` 被 CASCADE 带走，引用才真的归零。
    a.store().restore_note(&id).expect("从回收站还原");
    a.store().purge_note(&id).expect("永久删除");
    assert_eq!(
        a.store().attachment_refs(&sha).unwrap(),
        0,
        "前置：永久删除要把链接一起带走，否则 GC 的判据根本没有零引用的样本"
    );

    assert_eq!(
        a.reclaim_unreferenced_blobs(50),
        1,
        "零引用的那一条要收进隔离区"
    );
    assert_eq!(
        quarantine_bytes(&a, &sha),
        blob,
        "隔离=挪开，字节必须完整还在盘上"
    );
    assert_eq!(
        official_bytes(&a, &sha),
        Vec::<u8>::new(),
        "正式位置要腾出来"
    );
    assert_eq!(ledger(&a, &sha).0, "missing", "账上要写「本机没有」");
    assert!(
        ledger(&a, &sha).1.is_some(),
        "隔离要落下 `deleted_at`，宽限期才有得算（没有它，撤销期与真删都无从判）"
    );

    // 三条口径都得不再看见它，否则这一行会同时被 GC 和别的后台循环各管一半：
    // * 下载队列：`local='missing' ∧ remote='present'` 正好是它的取活条件，唯一拦住它的
    //   就是 `deleted_at IS NULL` —— 拦不住的话刚挪走的字节当轮就被拉回来（可数的 GET）。
    // * GC 自己的候选集：搬走的行必须立刻离开，否则每轮重复搬同一批 = 上界形同虚设。
    assert!(
        !b_downloads(&a, &sha),
        "已隔离的行还在下载队列的取活范围里 = 刚挪进隔离区的字节会被当轮重新拉回来"
    );
    assert!(
        !a.store()
            .gc_quarantine_candidates(500)
            .unwrap()
            .contains(&sha),
        "已隔离的行还留在回收候选集里 = 下一轮会再认领它一次"
    );
    let gets_before = gets_for_sha(&srv, &sha);
    let remote = a.remote_for_sync().await.unwrap().expect("A 有适配器");
    let round = a.run_attachment_round(&remote).await;
    assert_eq!(
        gets_for_sha(&srv, &sha),
        gets_before,
        "GC 之后又跑了一轮附件轮，却为此前隔离的那一份发了请求：{round:?}"
    );
    assert_eq!(
        quarantine_bytes(&a, &sha),
        blob,
        "附件轮把隔离区里那份字节动了 = 两个后台循环在抢同一份内容"
    );
}

/// 这一行现在有没有在下载队列里（`attachment_downloads` 的口径由库说了算，这里只问结果）。
fn b_downloads(app: &App, sha: &str) -> bool {
    app.store()
        .attachment_downloads(50)
        .unwrap()
        .into_iter()
        .any(|j| j.sha256 == sha)
}

// ———————————————————————————————————— FT-ATT-29s：每轮量有界

/// 与磁盘体检同一类欠账：一轮搬完全部的话，用户刚清空回收站（几百上千条零引用）时
/// 常驻循环那一格会连着一大段写锁与 IO 不放，而用户那次保存正排在后面（§48 G4 的理由
/// 在 GC 这条路上同样成立）。所以每轮有上界，**并且剩下的下一轮接着搬**（不饿死）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_reclaim_quarantines_at_most_its_cap_per_round_and_picks_the_rest_next() {
    let srv = TestServer::start(Backend::Mem).await;
    let url = srv.base_url();
    let a_dir = Tmp::new("29s-a");
    let a = boot(a_dir.path(), &url);
    a.sync_once().await.expect("入伙");
    let folder = a.default_folder_id().unwrap();

    // 三条各自独立的 blob，全部传上去过（`present` 才收），然后把三条笔记都永久删除 →
    // 三个零引用候选。造不出候选集就谈不上"上界"。
    let mut shas = Vec::new();
    for i in 0..3usize {
        let blob: Vec<u8> = (0..600).map(|k| ((k * 7 + i * 31) % 251) as u8).collect();
        let note = a.create_note(&folder, doc("GC 上界样本")).unwrap();
        let id = notera_core::EntityId::parse(&note.id).unwrap();
        let sha = a
            .store()
            .attach_blob(&id, &blob, "image/png", Some("shot.png"), "blk000002")
            .unwrap()
            .sha256;
        let head = a.store().get_note(&id).unwrap().unwrap();
        a.store()
            .edit_note(
                &id,
                json!({ "v": 1, "content": [
                    { "id": "blk000001", "type": "paragraph", "content": [{ "text": "GC 上界样本" }] },
                    { "id": "blk000002", "type": "image", "attrs": {
                        "sha256": &sha, "ref": &sha, "role": "inline", "pending": false,
                        "size": blob.len(), "mediaType": "image/png", "name": "shot.png" } },
                ] }),
                head.rev,
            )
            .unwrap();
        shas.push((id, sha, blob));
    }
    a.sync_once().await.expect("三条正文都推上去");
    let remote = a.remote_for_sync().await.unwrap().expect("A 有适配器");
    let round = a.run_attachment_round(&remote).await;
    assert_eq!(
        round.0, 3,
        "前置：三份字节都要传上去（present 才允许收）：{round:?}"
    );
    for (id, _, _) in &shas {
        a.store().purge_note(id).expect("永久删除样本笔记");
    }
    assert_eq!(
        a.store().gc_quarantine_candidates(500).unwrap().len(),
        3,
        "前置：候选集里要正好三条"
    );

    // 上界为 0 时一条都不许动。
    assert_eq!(
        a.reclaim_unreferenced_blobs(0),
        0,
        "cap=0 还搬 = 每轮量有界这条根本没生效"
    );
    assert_eq!(a.store().gc_quarantine_candidates(500).unwrap().len(), 3);

    // 一轮一条，且被搬走的那条**离开候选集**（下一轮自然浮出剩下的 —— 上界不是"重复搬同一批"）。
    let blob_of = |sha: &str| -> Vec<u8> {
        shas.iter()
            .find(|(_, s, _)| s == sha)
            .map(|(_, _, b)| b.clone())
            .expect("候选集里出现了不属于样本的 sha")
            .to_vec()
    };
    let mut seen: Vec<String> = Vec::new();
    for expect_left in [2, 1, 0] {
        let before = a.store().gc_quarantine_candidates(500).unwrap();
        assert_eq!(a.reclaim_unreferenced_blobs(1), 1, "cap=1 要恰好搬一条");
        let left = a.store().gc_quarantine_candidates(500).unwrap();
        assert_eq!(
            left.len(),
            expect_left,
            "这一轮之后候选集该剩 {expect_left} 条：{left:?}"
        );

        // 这一轮**新**离开候选集的那一条：多于一条就是没设上界，零条就是它没被账上认领。
        let taken: Vec<String> = before.into_iter().filter(|s| !left.contains(s)).collect();
        assert_eq!(taken.len(), 1, "这一轮只该认领一条：{taken:?}");
        for sha in taken {
            assert!(
                !seen.contains(&sha),
                "同一条被两轮各认领一次 = 候选集没有立刻把它排除，上界形同虚设"
            );
            assert_eq!(
                quarantine_bytes(&a, &sha),
                blob_of(&sha),
                "轮到它就该在隔离区里"
            );
            seen.push(sha);
        }
        // 还没轮到它的那些：字节一个都不许动（还躺在正式位置）。
        for sha in &left {
            assert_eq!(
                official_bytes(&a, sha),
                blob_of(sha),
                "还没轮到的那条不许动"
            );
        }
    }
    assert_eq!(
        seen.len(),
        3,
        "三条都要在三轮里各被认领一次，一条不多一条不少"
    );
}

// ————————————————————————————————— FT-ATT-30：撤销期内回来，零网络

/// 隔离期的价值全在这一条：用户撤销删除（或另一台设备把同一张图重新塞进正文）之后，
/// 这份字节**已经在本地隔离区里**，就不该再去服务器要一遍。
///
/// 为什么值得单独钉：跳过一次网络拉取必须有**读侧的持久来源**，否则"跳过"是猜的。这里的
/// 来源就是隔离区那个文件本身，而且挪回来之前先复算一次 sha256 —— 内容与名字对得上才算命中。
/// 断言打在请求计数上（零条 GET），不是打在"看起来好了"上。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reference_returning_inside_the_grace_period_comes_back_without_a_single_request() {
    let srv = TestServer::start(Backend::Mem).await;
    let url = srv.base_url();
    let blob: Vec<u8> = (0..9_000).map(|i| (i % 71) as u8).collect();
    let a_dir = Tmp::new("30-a");
    let (a, sha) = seed_uploaded(a_dir.path(), &url, &blob).await;

    let b_dir = Tmp::new("30-b");
    let b = drain_target(b_dir.path(), &url, &blob, &sha).await;
    assert!(gets_for_sha(&srv, &sha) >= 1, "前置：B 下载过一次");

    // B 上这条笔记被永久删除 → 本机引用归零 → GC 把它挪进隔离区。
    let bid = b
        .store()
        .list_notes(&notera_store::NoteQuery {
            folder: None,
            trash: false,
            limit: 50,
            offset: 0,
        })
        .unwrap()
        .into_iter()
        .map(|n| n.id)
        .next()
        .expect("B 收到了那条带图笔记");
    b.store().purge_note(&bid).expect("B 永久删除");
    assert_eq!(b.reclaim_unreferenced_blobs(50), 1, "前置：先隔离");
    assert_eq!(official_bytes(&b, &sha), Vec::<u8>::new());

    // A 把同一份字节再引用一次（新建一条笔记贴同一张图 = 内容寻址同一个 sha），推上去。
    let folder = a.default_folder_id().unwrap();
    let note2 = a
        .create_note(&folder, doc("第二张引用同一份字节的笔记"))
        .unwrap();
    let id2 = notera_core::EntityId::parse(&note2.id).unwrap();
    a.store()
        .attach_blob(&id2, &blob, "image/png", Some("shot.png"), "blk000002")
        .expect("A 再引用同一份字节");
    let head = a.store().get_note(&id2).unwrap().unwrap();
    a.store()
        .edit_note(
            &id2,
            json!({ "v": 1, "content": [
                { "id": "blk000001", "type": "paragraph", "content": [{ "text": "第二张引用同一份字节的笔记" }] },
                { "id": "blk000002", "type": "image", "attrs": {
                    "sha256": &sha, "ref": &sha, "role": "inline", "pending": false,
                    "size": blob.len(), "mediaType": "image/png", "name": "shot.png" } },
            ] }),
            head.rev,
        )
        .unwrap();
    a.sync_once().await.expect("A 推第二条正文");

    // B 拉到第二条正文：登记这一步会把 `deleted_at` 清回 NULL，行重新进入下载队列。
    let before = gets_for_sha(&srv, &sha);
    b.sync_once().await.expect("B 拉到第二条正文");
    let remote = b.remote_for_sync().await.unwrap().expect("B 有适配器");
    let round = b.run_attachment_round(&remote).await;
    assert_eq!(round.1, 1, "B 这一轮要按「本机补回一份」记账：{round:?}");

    assert_eq!(
        gets_for_sha(&srv, &sha),
        before,
        "隔离区里就有这一份字节，却还是去服务器要了一遍（跳过的那一次没有读侧来源）"
    );
    assert_eq!(
        official_bytes(&b, &sha),
        blob,
        "字节要回到正式位置，且内容一个字节都没变"
    );
    assert_eq!(
        quarantine_bytes(&b, &sha),
        Vec::<u8>::new(),
        "挪回来之后隔离区不该再留一份，否则同一份字节占两处"
    );
    assert_eq!(
        ledger(&b, &sha).0,
        "available",
        "账上还写着本机没有 = 界面继续画占位"
    );
    assert!(
        ledger(&b, &sha).1.is_none(),
        "`deleted_at` 必须一起清掉：留着它，两个队列永远看不见这一行"
    );
    let (code, len) = ui_read_code(&b, &sha);
    assert_eq!(code, None, "重新引用的那张图要能再画出来：{code:?}");
    assert!(len.unwrap_or(0) > 0, "界面拿到的是字节而不是空");
}

// ————————————————————————————————— FT-ATT-31：宽限期之后才销毁

/// 真删这一步的判据必须**全部**可核对：宽限期由调用方给（不靠测试猜时钟）、引用为 0、
/// 而且删的是**行 + 隔离区那份字节**两样 —— 少删一样就是磁盘上的永久垃圾。
///
/// 反向的那半同样重要：宽限期内一条都不许销毁（那是撤销的全部窗口），以及**还有引用的行
/// 永远销毁不了**（`note_attachments.sha256` 上是 `ON DELETE RESTRICT`，判据又先问一遍引用）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_quarantined_blob_is_destroyed_only_after_the_grace_period_and_never_while_referenced() {
    let srv = TestServer::start(Backend::Mem).await;
    let url = srv.base_url();
    let blob: Vec<u8> = (0..8_000).map(|i| (i % 113) as u8).collect();
    let a_dir = Tmp::new("31-a");
    let (a, sha) = seed_uploaded(a_dir.path(), &url, &blob).await;
    // 引用归零才谈得上回收：把那条笔记永久删除（链接被 CASCADE 带走）。
    let nid = a
        .store()
        .list_notes(&notera_store::NoteQuery {
            folder: None,
            trash: false,
            limit: 50,
            offset: 0,
        })
        .unwrap()
        .into_iter()
        .find(|n| n.title.contains("GC 目标笔记"))
        .map(|n| n.id.clone())
        .expect("前置：那条带图笔记还在列表里");
    a.store().purge_note(&nid).expect("永久删除");
    assert_eq!(a.reclaim_unreferenced_blobs(50), 1, "前置：先隔离");
    // 销毁之前要向服务器再确认一次（FT-ATT-36），所以这一步要有个真适配器
    let remote = a.remote_for_sync().await.unwrap().expect("A 有适配器");

    // ① 宽限期还没到：一条都不许销毁。
    assert_eq!(
        a.purge_verified_blobs(&a.confirm_still_remote(&remote, ALL_FUTURE, 50).await),
        0,
        "宽限期内就销毁 = 撤销窗口根本不存在"
    );
    assert_eq!(quarantine_bytes(&a, &sha), blob, "宽限期内字节必须原样留着");
    assert!(
        !a.store()
            .gc_ready_to_purge(ALL_FUTURE, 50)
            .unwrap()
            .contains(&sha),
        "未到期的行不该进销毁清单"
    );
    // 「已回收」账（缺口 G102）：隔离不是回收 —— 字节只是挪进了隔离区，账上必须还是空的。
    assert_eq!(
        a.store().reclaimed_totals().unwrap(),
        notera_store::ReclaimedTotals { files: 0, bytes: 0 },
        "进隔离区就记成已回收 = 把「还没发生」说成「已经发生」"
    );

    // ② 到期：行与隔离区那份字节一起消失，正式位置本来就空着。
    assert_eq!(
        a.purge_verified_blobs(&a.confirm_still_remote(&remote, ALL_PAST, 50).await),
        1,
        "到期又零引用的那一条要真的销毁掉"
    );
    assert_eq!(
        quarantine_bytes(&a, &sha),
        Vec::<u8>::new(),
        "隔离区那份字节要删掉"
    );
    assert_eq!(
        a.store().attachment_for_state(&sha),
        ("absent".to_string(), "absent".to_string()),
        "字节都销毁了，账上却还留着这一行 = 下一台设备会等一份并不存在的东西"
    );
    // 销毁这一批要落进「已回收」账：1 份、字节数按账上的 `size`（= blob 长度）。
    let landed = a.store().reclaimed_totals().unwrap();
    assert_eq!(
        (landed.files, landed.bytes),
        (1, blob.len() as i64),
        "销毁了却没落账 = 界面上那句「已回收」永远说不出来"
    );

    // ③ 还有引用的行，即使带着 `deleted_at` 也销毁不了 —— 判据先问引用，FK 是机器兜底。
    let folder = a.default_folder_id().unwrap();
    let note = a
        .create_note(&folder, doc("还在用另一份字节的笔记"))
        .unwrap();
    let id = notera_core::EntityId::parse(&note.id).unwrap();
    let blob2: Vec<u8> = (0..1_200).map(|i| (i % 29) as u8).collect();
    let live = a
        .store()
        .attach_blob(&id, &blob2, "image/png", Some("live.png"), "blk000002")
        .unwrap()
        .sha256;
    assert_eq!(
        a.reclaim_unreferenced_blobs(50),
        0,
        "还在被引用的那份一条都不许收"
    );
    assert_eq!(
        a.purge_verified_blobs(&a.confirm_still_remote(&remote, ALL_PAST, 50).await),
        0,
        "没有已隔离的行时销毁清单必须是空的"
    );
    assert_eq!(official_bytes(&a, &live), blob2, "被引用的字节一个都不许动");
    assert!(
        !a.store()
            .gc_ready_to_purge(ALL_PAST, 50)
            .unwrap()
            .contains(&live),
        "有引用的行进销毁清单 = 判据根本没问引用计数"
    );
    // 「已回收」账要跟着**真删**走，不跟着"想删"走：被拒的批次不许在本账上留痕，
    // 已销毁的那一份再跑一遍也不许重复记账。
    assert_eq!(
        a.store().reclaimed_totals().unwrap(),
        landed,
        "没删成的批次落账 = 账比硬盘多"
    );
    assert_eq!(
        a.purge_verified_blobs(std::slice::from_ref(&sha)),
        0,
        "同一份再销毁一次删不到东西"
    );
    assert_eq!(
        a.store().reclaimed_totals().unwrap(),
        landed,
        "重跑重复记账 = 把一份 8000 字节说成两份"
    );
}

// —————————————（2026-10-10 增补，缺口 G102）：销毁落的那本账，「已回收」看得见

/// §6-9 第 4 项数据「回收字节数」读作"已经回收了多少"时的那本账（缺口 G102 的修法）：
/// 账要跟着**真删**走 —— **撤销隔离**（宽限期里把那一份取回来）不是回收，一个数都不许动。
///
/// FT-ATT-31 已经把"被拒的批次不许落账 / 重跑不许重复记账"钉住；这一条补的是负向的
/// 另一个入口：`release_attachment_quarantine`（用户点"取回来"那条路）。两个入口都不是销毁，
/// 两个都不许把"还没发生"写成"已经发生"。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn releasing_a_quarantine_never_lands_in_the_reclaim_ledger() {
    let srv = TestServer::start(Backend::Mem).await;
    let url = srv.base_url();
    let blob: Vec<u8> = (0..2_100).map(|i| (i % 37) as u8).collect();
    let a_dir = Tmp::new("g102-a");
    let (a, sha) = seed_uploaded(a_dir.path(), &url, &blob).await;
    let nid = a
        .store()
        .list_notes(&notera_store::NoteQuery {
            folder: None,
            trash: false,
            limit: 50,
            offset: 0,
        })
        .unwrap()
        .into_iter()
        .find(|n| n.title.contains("GC 目标笔记"))
        .map(|n| n.id.clone())
        .expect("前置：那条带图笔记还在列表里");
    a.store().purge_note(&nid).expect("永久删除");
    assert_eq!(a.reclaim_unreferenced_blobs(50), 1, "前置：先隔离");

    // 撤回来：字节还在隔离区里等着（或被下载那一轮挪回来），一个字节都没被回收。
    a.store().release_attachment_quarantine(&sha).unwrap();
    assert_eq!(
        a.store().reclaimed_totals().unwrap(),
        notera_store::ReclaimedTotals { files: 0, bytes: 0 },
        "撤销隔离进了这本账 = 用户明明把那份取回来了，界面却说已经回收了它"
    );
    assert_eq!(
        quarantine_bytes(&a, &sha),
        blob,
        "撤销之后字节必须还在（那份还要能再取回来）"
    );
}

// ———————————————————————————————— FT-ATT-32：手动动作也要认隔离区

/// 「重试取回」这颗按钮今天只认账上的 `remote_state`：它把结论撤掉、重新入队，等下一轮去
/// 下载。可对一台**刚把它隔离起来**的设备，那一行是 `deleted_at` 有值的，两个队列都不会挑它
/// —— 于是用户点了按钮，什么都没发生，而按钮说好"重试"。
///
/// 这一步把顺序摆正：先按读侧的持久来源（隔离区那份字节，复算哈希）本地补回，补不回来
/// 才去动远端结论。断言依旧打在请求计数上。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_user_retry_recovers_a_quarantined_attachment_locally_before_asking_the_server() {
    let srv = TestServer::start(Backend::Mem).await;
    let url = srv.base_url();
    let blob: Vec<u8> = (0..6_500).map(|i| (i % 53) as u8).collect();
    let a_dir = Tmp::new("32-a");
    let (_a_src, sha) = seed_uploaded(a_dir.path(), &url, &blob).await;
    let b_dir = Tmp::new("32-b");
    let b = drain_target(b_dir.path(), &url, &blob, &sha).await;
    assert!(gets_for_sha(&srv, &sha) >= 1, "前置：B 下载过一次");

    // B 永久删除笔记 → GC 隔离 → 图在这一刻本机还能"就近"补回来，但队列看不见它。
    let bid = b
        .store()
        .list_notes(&notera_store::NoteQuery {
            folder: None,
            trash: false,
            limit: 50,
            offset: 0,
        })
        .unwrap()
        .into_iter()
        .map(|n| n.id)
        .next()
        .expect("B 收到了那条带图笔记");
    b.store().purge_note(&bid).expect("B 永久删除");
    assert_eq!(b.reclaim_unreferenced_blobs(50), 1, "前置：先隔离");
    let before = gets_for_sha(&srv, &sha);

    let code = call_recovery(&b, "attachment_retry", &sha);
    assert_eq!(code, None, "对已隔离的附件点「重试取回」不该被拒：{code:?}");
    assert_eq!(
        official_bytes(&b, &sha),
        blob,
        "「重试取回」对已隔离的这一步必须真把字节放回正式位置"
    );
    assert_eq!(
        gets_for_sha(&srv, &sha),
        before,
        "本机隔离区里就有这一份，却还是去服务器要了一遍"
    );
    assert_eq!(ledger(&b, &sha).0, "available");
    assert!(ledger(&b, &sha).1.is_none(), "撤销期结束要一起清掉隔离标记");
}

// ————————————————————— FT-ATT-33：隔离区那份也坏了的时候，按钮要说真话

/// 「重试取回」在隔离区里那份**已经不可用**（位腐、或被磁盘清理连目录一起删了）时的行为。
///
/// 这条是被上一条逼出来的：那一版只验了"隔离区里有好字节"。坏字节那条路当时会走到
/// "撤远端结论 + 入队"，而这一行带着 GC 的隔离标记，两个队列与体检都看不见它（三条口径都带
/// `deleted_at IS NULL`）—— 于是用户点了按钮、一次请求都不会发、那条待办永远关不掉。
/// 这不是"少一次优化"，是**一颗说好会重试的按钮静默失效**。
///
/// 判据因此打在两件事上：撤销标记之后**真的有请求发出去**，而且回来的那份内容对得上；
/// 顺带钉住"坏掉的隔离副本被好副本替换后不留第二份"。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_corrupt_quarantine_copy_does_not_silently_defeat_the_retry_button() {
    let srv = TestServer::start(Backend::Mem).await;
    let url = srv.base_url();
    let blob: Vec<u8> = (0..5_200).map(|i| (i % 61) as u8).collect();
    let a_dir = Tmp::new("33-a");
    let (_a_src, sha) = seed_uploaded(a_dir.path(), &url, &blob).await;
    let b_dir = Tmp::new("33-b");
    let b = drain_target(b_dir.path(), &url, &blob, &sha).await;
    let bid = b
        .store()
        .list_notes(&notera_store::NoteQuery {
            folder: None,
            trash: false,
            limit: 50,
            offset: 0,
        })
        .unwrap()
        .into_iter()
        .map(|n| n.id)
        .next()
        .expect("B 收到了那条带图笔记");
    b.store().purge_note(&bid).expect("B 永久删除");
    assert_eq!(b.reclaim_unreferenced_blobs(50), 1, "前置：先隔离");

    // 磁盘位腐：**同长度**改坏那份隔离副本。同长度这一句是刻意的 —— 磁盘体检只判"在不在 +
    // 长度对不对"，同长度的坏字节它不动（DATA-MODEL §8 那条边界），所以这条路必须自己站得住。
    let qpath = b.store().quarantine_path(&sha);
    let mut rotted = blob.clone();
    for byte in rotted.iter_mut() {
        *byte = !*byte;
    }
    assert_eq!(rotted.len(), blob.len(), "前置：改坏要维持同长度");
    std::fs::write(&qpath, &rotted).expect("写坏隔离区那份");
    assert_eq!(
        b.reclaim_unreferenced_blobs(50),
        0,
        "已隔离的行不该再被认领一次"
    );

    let before = gets_for_sha(&srv, &sha);
    let code = call_recovery(&b, "attachment_retry", &sha);
    assert_eq!(
        code, None,
        "对隔离副本已坏的行点「重试取回」不该被拒：{code:?}"
    );
    assert!(
        ledger(&b, &sha).1.is_none(),
        "用户要求取回 = 授权结束宽限期；标记还留着，这一行就永远排不进下载队列"
    );

    let remote = b.remote_for_sync().await.unwrap().expect("B 有适配器");
    let round = b.run_attachment_round(&remote).await;
    assert!(
        gets_for_sha(&srv, &sha) > before,
        "点了重试却一次请求都没发（那颗按钮是静默失效的）：{round:?}"
    );
    assert_eq!(round.1, 1, "这一轮要把那份字节补回来：{round:?}");
    assert_eq!(
        official_bytes(&b, &sha),
        blob,
        "从服务器回来的那份内容要对得上 sha"
    );
    assert_eq!(
        quarantine_bytes(&b, &sha),
        Vec::<u8>::new(),
        "坏掉的隔离副本该被落好的那份替换掉，而不是留成第二份没人认领的字节"
    );
    let (ui_code, len) = ui_read_code(&b, &sha);
    assert_eq!(ui_code, None, "那张图要能再画出来：{ui_code:?}");
    assert!(len.unwrap_or(0) > 0);
}

// ———————————————— FT-ATT-34：崩在"挪走了"与"写上账"之间（§20 的 L5 崩溃注入）

/// 子进程模式的开关。崩溃注入靠的是"用同一个测试二进制再起一个进程，让它真的死在那里"
/// （`notera_core::crash_point` 是 `process::exit`，不是 panic —— 要的就是不跑析构、不收尾）。
const GC_CHILD: &str = "NOTERA_GC_CRASH_CHILD";

/// 子进程：在父进程已经准备好的那个目录上，只跑回收那一步。
/// `NOTERA_CRASH_AT=after_quarantine_move` 命中时它会在"字节已挪进隔离区、账还没写"那一瞬消失。
#[test]
fn child_reclaims_once() {
    let Ok(spec) = std::env::var(GC_CHILD) else {
        return; // 不是子进程模式：这条什么都不做（父进程靠 spawn 显式启用）
    };
    let Some((dir, url)) = spec.split_once('|') else {
        return;
    };
    let app = boot(Path::new(dir), url);
    let n = app.reclaim_unreferenced_blobs(50);
    // 能从这一行走出去就说明注入点压根没接上（没东西可挪，或者 crash_point 忘了放）
    eprintln!("CHILD_RECLAIM 走完了整次回收（这是错的）：认领 {n} 条");
}

/// GC 是本仓唯一一处"把用户的字节从正式位置搬走"的代码，而搬运是**两步**：先挪文件，后写账。
/// 断电落在这两步中间的那一格，形状是"账上写着 available，而正式位置空了"—— 也就是本文件
/// 开头那条"先挪后写"的顺序所选择的**残留**。这一条验的就是那个残留真的能自己收回来：
/// * 字节必须还在盘上（只是换了地方），绝不能因为"崩了一下"就少一份；
/// * 重启后一轮附件轮要把这一行纠正回来，而纠正的依据是既有的机器（磁盘体检把假账降级 →
///   下载那一轮**先看隔离区** → 本地挪回），**不是**新写的一条恢复路径；
/// * 补回来之后账回 `available`，再一轮 GC 才把它正常收进隔离区并写账 —— 也就是说系统会
///   **收敛到设计里的那个状态**，而不是在"补/收"之间来回摆；
/// * 全程为该 sha 发出去的请求数不变（本机就有那份字节）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_crash_between_the_move_and_the_ledger_heals_itself_without_one_request() {
    use std::process::Command;

    let srv = TestServer::start(Backend::Mem).await;
    let url = srv.base_url();
    let blob: Vec<u8> = (0..4_800).map(|i| (i % 37) as u8).collect();
    let dir = Tmp::new("34-a");
    let (a, sha) = seed_uploaded(dir.path(), &url, &blob).await;
    let nid = a
        .store()
        .list_notes(&notera_store::NoteQuery {
            folder: None,
            trash: false,
            limit: 50,
            offset: 0,
        })
        .unwrap()
        .into_iter()
        .find(|n| n.title.contains("GC 目标笔记"))
        .map(|n| n.id.clone())
        .expect("前置：那条带图笔记在");
    a.store().purge_note(&nid).expect("永久删除，让引用归零");
    assert!(
        a.store()
            .gc_quarantine_candidates(500)
            .unwrap()
            .contains(&sha),
        "前置：这一行要真是 GC 的候选，否则子进程根本走不到注入点"
    );
    assert_eq!(official_bytes(&a, &sha), blob, "前置：字节还在正式位置");
    // 让出这个库：子进程要在同一个目录上开它自己的核心
    drop(a);

    let out = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "child_reclaims_once",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(GC_CHILD, format!("{}|{}", dir.path().display(), url))
        .env("NOTERA_CRASH_AT", "after_quarantine_move")
        .env("NOTERA_DEV_WEBDAV_SECRET", SECRET)
        .output()
        .expect("起子进程");
    assert_eq!(
        out.status.code(),
        Some(notera_core::CRASH_EXIT_CODE),
        "注入点没有让进程死在那里（退出码 {:?}）—— 那这一格测的就不是崩溃：stderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );

    // 崩完的形状：字节在隔离区、正式位置空、而账上还写着 available —— 就是那个窗口本身
    let a = boot(dir.path(), &url);
    assert_eq!(
        quarantine_bytes(&a, &sha),
        blob,
        "崩在挪与写之间：那份字节必须整份还在盘上，只是在隔离区"
    );
    assert_eq!(
        official_bytes(&a, &sha),
        Vec::<u8>::new(),
        "正式位置该是空的（文件确实被挪走了，不是复制）"
    );
    assert_eq!(
        ledger(&a, &sha).0,
        "available",
        "账还没写：这一格就是那条假账，判据要打在它上面"
    );
    assert!(
        ledger(&a, &sha).1.is_none(),
        "隔离标记也还没落（写账那一步没跑到）"
    );

    let before = gets_for_sha(&srv, &sha);
    let remote = a.remote_for_sync().await.unwrap().expect("A 有适配器");
    let round = a.run_attachment_round(&remote).await;
    assert!(
        round.1 >= 1,
        "重启后的第一轮要把这一行按「本机补回一份」记账：{round:?}"
    );
    assert_eq!(official_bytes(&a, &sha), blob, "字节要自己回到正式位置");
    assert_eq!(
        quarantine_bytes(&a, &sha),
        Vec::<u8>::new(),
        "挪回来了就不该在隔离区留第二份"
    );
    assert_eq!(ledger(&a, &sha).0, "available", "假账纠正回来了");
    assert!(ledger(&a, &sha).1.is_none());
    assert_eq!(
        gets_for_sha(&srv, &sha),
        before,
        "本机隔离区里就有这一份，自愈却去服务器要了一遍：{round:?}"
    );

    // 收敛，而不是来回摆：再跑一次回收，它该正常认领并**把账写上**（这次没有断电）
    assert_eq!(
        a.reclaim_unreferenced_blobs(50),
        1,
        "补回来之后这一行仍是零引用，下一轮 GC 该把它收进隔离区并写账"
    );
    assert_eq!(ledger(&a, &sha).0, "missing", "写上了账：设计里的那个状态");
    assert!(ledger(&a, &sha).1.is_some(), "隔离标记这次落了");
    assert_eq!(
        a.reclaim_unreferenced_blobs(50),
        0,
        "已认领过的行不该被再认领一次（否则就是补/收来回摆）"
    );
}

// ——————————————— FT-ATT-35：链接表低估了引用，GC 就会销毁还在画的图

/// **这一条打的不是"GC 的判据写错了"，而是"引用计数从哪来"本来就不可信。**
///
/// `note_attachments` 只有两个写者：`attach_blob`（挂载）与外来笔记的登记（apply 那条路）。
/// 本机的 `create_note` / `edit_note` **从不派生链接** —— 于是"正文里明明引用着这张图，链接表里
/// 却没有对应行"是可达状态，而这条路径生产里每天都在走：冲突副本就是拿**服务器那一版的正文**直接
/// `create_note`（`App` 的 AdoptConflict 分支），那份正文里带着图片块；`swap_conflict_sides` 同理。
///
/// 后果是 GC 落地之后才成立的：零引用 = "这份字节可以回收"，于是当唯一保护着它的链接随着原件被
/// 永久删除（CASCADE 带走），这份字节会在 30 天后被销毁，而副本还在画它。
/// 修的是"引用不许被低估"：本机的 create/edit 也按正文派生链接 —— 宁可多算引用（少收点磁盘），
/// 不可少算（少算就是丢数据）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_local_note_that_references_an_image_in_its_doc_protects_its_bytes() {
    let srv = TestServer::start(Backend::Mem).await;
    let url = srv.base_url();
    let blob: Vec<u8> = (0..5_600).map(|i| (i % 41) as u8).collect();
    let a_dir = Tmp::new("35-a");
    let (a, sha) = seed_uploaded(a_dir.path(), &url, &blob).await;
    let folder = a.default_folder_id().unwrap();

    // 生产里每天都在走的一步：拿「另一个版本」的正文新建一条笔记（冲突副本就是这么造的）。
    // 它的正文引用着同一张图，可没有人替它写链接表。
    let copy = a
        .create_note(
            &folder,
            json!({ "v": 1, "content": [
                { "id": "blk000001", "type": "paragraph", "content": [{ "text": "冲突副本的正文" }] },
                { "id": "blk000002", "type": "image", "attrs": {
                    "sha256": &sha, "ref": &sha, "role": "inline", "pending": false,
                    "size": blob.len(), "mediaType": "image/png", "name": "shot.png" } },
            ] }),
        )
        .expect("冲突副本的创建（真走 create_note）");
    let original = a
        .store()
        .list_notes(&notera_store::NoteQuery {
            folder: None,
            trash: false,
            limit: 50,
            offset: 0,
        })
        .unwrap()
        .into_iter()
        .find(|n| n.title.contains("GC 目标笔记"))
        .map(|n| n.id.clone())
        .expect("前置：原件还在");

    // 第二条**编辑**路径：先把一张图搬进一条原本没有附件的笔记的正文里（编辑器里复制图片块、
    // 冲突采纳"保留本机"改写正文都是这一形）—— 这一步不调 `attach_blob`，因为字节本来就在盘上，
    // 于是链接表只能由正文派生。少了这一步，`edit_note` 那条路依然是"正文引用着、账上没登记"。
    let doc_with_image = json!({ "v": 1, "content": [
        { "id": "blk000001", "type": "paragraph", "content": [{ "text": "把图搬进来的那条" }] },
        { "id": "blk900002", "type": "image", "attrs": {
            "sha256": &sha, "ref": &sha, "role": "inline", "pending": false,
            "size": blob.len(), "mediaType": "image/png", "name": "shot.png" } },
    ] });
    let third = a
        .create_note(
            &folder,
            json!({ "v": 1, "content": [
                { "id": "blk900001", "type": "paragraph", "content": [{ "text": "空的一条" }] },
            ] }),
        )
        .expect("前置：建一条不带图的笔记");
    let third_id = notera_core::EntityId::parse(&third.id).unwrap();
    let head = a.store().get_note(&third_id).unwrap().unwrap();
    a.store()
        .edit_note(&third_id, doc_with_image, head.rev)
        .expect("把图片块搬进正文（真走 edit_note）");

    // 引用计数必须把副本与"搬进来"的那条都算进来 —— 这一句就是「少算 = 丢数据」那个洞的正身。
    assert_eq!(
        a.store().attachment_refs(&sha).unwrap(),
        3,
        "三条笔记的正文都引用着这份字节，链接表却认不全：GC 的判据建立在一个被低估的数上"
    );

    // 永久删掉原件：从此只有副本与搬进来的那条在引用它。
    a.store().purge_note(&original).expect("永久删除原件");
    assert_eq!(
        a.store().attachment_refs(&sha).unwrap(),
        2,
        "另外两条链接必须还在，否则这份字节就成了「没人引用」的孤儿"
    );
    assert_eq!(
        a.reclaim_unreferenced_blobs(50),
        0,
        "还在被副本引用的那份字节，一条都不许收"
    );
    assert_eq!(official_bytes(&a, &sha), blob, "被引用的字节一个都不许动");
    let (code, len) = ui_read_code(&a, &sha);
    assert_eq!(code, None, "副本打开那张图要拿得到字节：{code:?}");
    assert!(len.unwrap_or(0) > 0);
    let copy_id = notera_core::EntityId::parse(&copy.id).unwrap();
    let copy_note = a
        .store()
        .get_note(&copy_id)
        .unwrap()
        .expect("前置：副本确实建起来了（不能靠「根本没建出来」混过上面那几条断言）");
    assert!(
        copy_note.has_attachment,
        "副本自己得知道它带着附件（派生列），否则这条测试的前提是空的"
    );
}

/// 打到这个 sha 上的 HEAD 条数（`confirm_still_remote` 那一次"你还在不在"就问的是这个）。
/// 为什么单独有一个计数：销毁前的第二次确认是一个**动作**，不是一个说法。没有它，
/// "我们先问了服务器"这句注释在代码被改回去之后照样能留着，而没人会看见。
fn heads_for_sha(srv: &TestServer, sha: &str) -> usize {
    srv.request_log()
        .iter()
        .filter(|r| r.method == "HEAD" && r.path.contains(sha))
        .count()
}

/// §8 GC 第二步的凭据：账上那个 `present` 是**隔离那一刻**写下的，而隔离到销毁之间隔着整个
/// 宽限期。销毁是本仓唯一不可逆的那一步，所以它只能建立在**当下问到的事实**上（独立审查第 3 条）。
///
/// 三种回答分工不同，这一条把它们各钉一遍：
/// * **问不到**（这里是 503）= 不下结论 —— 字节不动，`remote_state` 也不写。
///   与下面那个 404 的区别是 §27 早就划好的：404 是关于远端事实的结论，5xx 不是。
/// * **服务器说没有**（404）= 本机这份成了仅存的一份 → 不销毁，并把这一格记成 `absent`。
/// * **记下来之后不再每轮重问**：`gc_ready_to_purge` 只收 `present`，所以判过结论的行离开候选集
///   （§27/§28 不许 20 秒一轮去敲同一扇门）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nothing_is_destroyed_until_the_server_confirms_it_still_has_the_bytes() {
    let srv = TestServer::start(Backend::Mem).await;
    let url = srv.base_url();
    let blob: Vec<u8> = (0..6_400).map(|i| (i % 71) as u8).collect();
    let a_dir = Tmp::new("36-a");
    let (a, sha) = seed_uploaded(a_dir.path(), &url, &blob).await;
    let nid = a
        .store()
        .list_notes(&notera_store::NoteQuery {
            folder: None,
            trash: false,
            limit: 50,
            offset: 0,
        })
        .unwrap()
        .into_iter()
        .find(|n| n.title.contains("GC 目标笔记"))
        .map(|n| n.id.clone())
        .expect("前置：那条带图笔记还在列表里");
    a.store().purge_note(&nid).expect("永久删除");
    assert_eq!(a.reclaim_unreferenced_blobs(50), 1, "前置：先隔离");
    let remote = a.remote_for_sync().await.unwrap().expect("A 有适配器");

    // ① 问不到：503 不是结论，所以账与字节都不许动
    srv.inject(notera_test_webdav::Injection::status(
        format!("HEAD *{sha}"),
        503,
    ))
    .await;
    let asked = heads_for_sha(&srv, &sha);
    assert_eq!(
        a.confirm_still_remote(&remote, ALL_PAST, 50).await,
        Vec::<String>::new(),
        "问不到的时候，销毁清单必须是空的"
    );
    assert!(
        heads_for_sha(&srv, &sha) > asked,
        "第二次确认根本没发出去 = 销毁还建立在隔离那一刻的旧结论上（问之前 {asked} 条，问之后 {} 条）",
        heads_for_sha(&srv, &sha)
    );
    assert_eq!(
        a.store().attachment_for_state(&sha).1,
        "present",
        "一次 503 不许被当成远端事实：把 present 改写掉就是拿猜代替问"
    );
    assert_eq!(
        quarantine_bytes(&a, &sha),
        blob,
        "没问出结论就不许动一个字节"
    );

    // ② 服务器说没有：这一格本机这份成了仅存的一份 → 不销毁，并把结论记下来
    srv.inject(notera_test_webdav::Injection::status(
        format!("HEAD *{sha}"),
        404,
    ))
    .await;
    assert_eq!(
        a.confirm_still_remote(&remote, ALL_PAST, 50).await,
        Vec::<String>::new(),
        "服务器已经没有副本了还销毁 = 把最后一份字节删掉"
    );
    assert_eq!(
        a.store().attachment_for_state(&sha).1,
        "absent",
        "404 是关于远端事实的结论，要记到账上（否则下一轮重问，而那颗「重新上传本机这份」看不见这一格）"
    );
    assert_eq!(
        quarantine_bytes(&a, &sha),
        blob,
        "记成 absent 的那一份必须还在隔离区：它是这张图现在唯一的地方"
    );

    // ③ 记过结论就不再每轮重问（§27/§28 那句"不许空转"在这一格的具体形态）
    let asked = heads_for_sha(&srv, &sha);
    assert_eq!(
        a.confirm_still_remote(&remote, ALL_PAST, 50).await,
        Vec::<String>::new(),
        "已判 absent 的行不该再进销毁清单"
    );
    assert_eq!(
        heads_for_sha(&srv, &sha),
        asked,
        "同一扇门每 20 秒敲一次 = 空转；结论要落在账上，让候选集自己把它排除掉"
    );

    // ④ 服务器确认还在：这才轮得到销毁，而且行与隔离区那份字节一起消失
    srv.inject(notera_test_webdav::Injection::none()).await;
    a.store()
        .set_attachment_states(&sha, None, Some("present"))
        .expect("把账恢复到「隔离完成、服务器有副本」那一格");
    let verified = a.confirm_still_remote(&remote, ALL_PAST, 50).await;
    assert_eq!(verified, vec![sha.clone()], "服务器说有 = 这一条该被放行");
    assert_eq!(a.purge_verified_blobs(&verified), 1, "放行之后要真销毁");
    assert_eq!(
        a.store().attachment_for_state(&sha),
        ("absent".to_string(), "absent".to_string()),
        "销毁之后账上不该再留着这一行"
    );
    assert_eq!(quarantine_bytes(&a, &sha), Vec::<u8>::new());
}

/// 独立审查第 5 条：同一份数据目录被两个进程同时打开时，GC 会不会互相踩。
///
/// **这条推翻了我自己写在 §48 里的一句前提** —— 那里原本写着"同一目录上的第二个进程今天
/// 会被锁挡住，所以这条不构成风险"。实情是 `pool.rs` 只有 `PRAGMA busy_timeout=5000`，
/// **没有任何单实例锁**：第二个 store 打得开。既然挡不住，那句免责就得换成一份实测：
/// 两个 store 抢同一批候选时，字节的去向要唯一、账只标一次、销毁只数到一次。
///
/// 形状是一个进程里开两个 `App` 指向同一个目录：它们各有自己的连接与自己那次候选查询
/// （竞态的两半都在），共享的正是跨进程时也共享的那两样 —— 同一份 SQLite 与同一份磁盘。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_stores_on_one_data_dir_never_move_or_destroy_the_same_bytes_twice() {
    let srv = TestServer::start(Backend::Mem).await;
    let url = srv.base_url();
    let blob: Vec<u8> = (0..5_200).map(|i| (i % 61) as u8).collect();
    let dir = Tmp::new("37-shared");
    let (a, sha) = seed_uploaded(dir.path(), &url, &blob).await;
    // 第二个"进程"：同一个目录，另一套连接与另一份候选查询
    let b = boot(dir.path(), &url);
    let nid = a
        .store()
        .list_notes(&notera_store::NoteQuery {
            folder: None,
            trash: false,
            limit: 50,
            offset: 0,
        })
        .unwrap()
        .into_iter()
        .find(|n| n.title.contains("GC 目标笔记"))
        .map(|n| n.id.clone())
        .expect("前置：那条带图笔记还在");
    a.store().purge_note(&nid).expect("永久删除");

    // 两边都先看到同一条候选（这一句是这条门禁的前提：它们抢的是同一批东西）
    assert_eq!(
        a.store().gc_quarantine_candidates(50).unwrap(),
        vec![sha.clone()]
    );
    assert_eq!(
        b.store().gc_quarantine_candidates(50).unwrap(),
        vec![sha.clone()]
    );

    // ① 回收：只能有一家把这一条算成自己认领的
    let moved = a.reclaim_unreferenced_blobs(50) + b.reclaim_unreferenced_blobs(50);
    assert_eq!(
        moved, 1,
        "同一份字节被两个 store 各自认领了一次 = 隔离那一步没有互斥"
    );
    assert_eq!(official_bytes(&a, &sha), Vec::<u8>::new(), "正式位置该空着");
    assert_eq!(
        quarantine_bytes(&b, &sha),
        blob,
        "隔离区里必须恰好一份、且内容还是那 5200 个字节"
    );

    // ② 销毁：两边都拿着同一份到期清单动手，只许数到一次
    let verified_a = {
        let remote = a.remote_for_sync().await.unwrap().expect("A 有适配器");
        a.confirm_still_remote(&remote, ALL_PAST, 50).await
    };
    let verified_b = {
        let remote = b.remote_for_sync().await.unwrap().expect("B 有适配器");
        b.confirm_still_remote(&remote, ALL_PAST, 50).await
    };
    assert_eq!(verified_a, vec![sha.clone()], "前置：A 那一边问到了还在");
    assert_eq!(verified_b, vec![sha.clone()], "前置：B 也拿到了同一份清单");
    let destroyed = a.purge_verified_blobs(&verified_a) + b.purge_verified_blobs(&verified_b);
    assert_eq!(
        destroyed, 1,
        "同一行被销毁两次 = 第二步也没有互斥（第二次的字节删除会踩到别人刚腾开的位置）"
    );
    assert_eq!(
        a.store().attachment_for_state(&sha),
        ("absent".to_string(), "absent".to_string()),
        "两个 store 看同一份账，销毁之后都得没有这一行"
    );
    assert_eq!(
        b.store().attachment_for_state(&sha),
        ("absent".to_string(), "absent".to_string())
    );
    assert_eq!(
        quarantine_bytes(&b, &sha),
        Vec::<u8>::new(),
        "隔离区不能留下第二份"
    );
}

// ===========================================================================
// 真两进程（FT-ATT-39）：把"跨进程竞争"从推理换成现场
// ===========================================================================

const GC_RACE_CHILD: &str = "NOTERA_GC_RACE_CHILD";

/// 等一个栅栏文件出现。**不是时间判据** —— 它只用来把两边对齐到同一瞬，量的是"有没有发生"，
/// 到点就写一条 `*_timeout` 然后收摊（父进程那侧的红会带上这个原因）。
/// 看到 `stop` 也算到点：父进程一旦 panic 就不会再放行，子进程不能留在那儿把测试二进制
/// 占着（下一轮构建会被自己的链接步骤挡掉，"Permission denied"，看着像门禁红了）。
fn race_wait(flags: &Path, name: &str) -> bool {
    let stop = flags.join("stop");
    let wanted = flags.join(name);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    while std::time::Instant::now() < deadline {
        if wanted.exists() {
            return true;
        }
        if stop.exists() {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    false
}

/// 父进程用的栅栏：没等到就先把 `stop` 立起来再收摊，保证两边一起退场。
fn race_barrier(flags: &Path, name: &str) -> bool {
    let ok = race_wait(flags, name);
    if !ok {
        let _ = std::fs::write(flags.join("stop"), name);
    }
    ok
}

fn race_read(flags: &Path, name: &str) -> String {
    std::fs::read_to_string(flags.join(name))
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn race_count(flags: &Path, name: &str) -> usize {
    // 读不到就把数打成"多到一定违反等式"的那个值：这条门禁的红必须是"抢了两次"那一形，
    // 不能因为栅栏没落而假绿。
    race_read(flags, name).parse().unwrap_or(usize::MAX)
}

/// 子进程模式：`NOTERA_GC_RACE_CHILD=<数据目录>|<webdav url>|<sha>|<栅栏目录>`。
///
/// 两边都**先把手里的清单取出来**，再等一个文件出现才动手 —— 于是"两边都握着同一份还没被
/// 认领的清单"这一格是被造出来的，不是靠运气撞上的。
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn gc_race_child_process() {
    let Ok(spec) = std::env::var(GC_RACE_CHILD) else {
        return; // 不是子进程模式：这条什么都不做（父进程靠 spawn 显式启用）
    };
    let parts: Vec<&str> = spec.split('|').collect();
    if parts.len() != 4 {
        return;
    }
    let dir = Path::new(parts[0]);
    let url = parts[1];
    let sha = parts[2].to_string();
    let flags = Path::new(parts[3]);
    let app = boot(dir, url);

    // ① 认领：取清单 → 栅栏 → 回收
    let candidates = app.store().gc_quarantine_candidates(50).unwrap_or_default();
    let listed = candidates.contains(&sha);
    let _ = std::fs::write(flags.join("child_listed"), if listed { "1" } else { "0" });
    if !race_barrier(flags, "go1") {
        let _ = std::fs::write(flags.join("child_timeout"), "go1");
        return;
    }
    let claimed = app.reclaim_unreferenced_blobs(50);
    let _ = std::fs::write(flags.join("child_claimed"), claimed.to_string());
    // 等父进程也跑完认领那一步，再去做销毁前的复查。这一格不是可有可无的：认领只有一家住账，
    // 而**这一行是另一家写的** —— 抢在它提交之前读"到期清单"会读到空，于是这个子进程拿不到
    // 清单，销毁那一步的现场根本没造出来（实测就是这样红过：`账上可准入/服务器确认在 = 1/0`，
    // 而同一瞬直接再问一次服务器答 `Ok(true)`，说明字节与账都没错，错的是我读的时机）。
    if !race_barrier(flags, "both1") {
        let _ = std::fs::write(flags.join("child_timeout"), "both1");
        return;
    }

    // ② 销毁：拿到"服务器说还在"的那份清单 → 栅栏 → 动手
    let remote = match app.remote_for_sync().await {
        Ok(Some(r)) => r,
        Ok(None) => {
            let _ = std::fs::write(flags.join("child_error"), "没有适配器");
            return;
        }
        Err(e) => {
            let _ = std::fs::write(flags.join("child_error"), format!("{e:?}"));
            return;
        }
    };
    let verified = app.confirm_still_remote(&remote, ALL_PAST, 50).await;
    // 诊断用的两个数分开落盘：`ready` 是"账上认为到期且可准入"的份数，`verified` 是
    // "服务器真的确认还在"的份数。只落一个数的话，红的时候分不清是账还是服务器。
    let ready = app
        .store()
        .gc_ready_to_purge(ALL_PAST, 50)
        .unwrap_or_default()
        .len();
    let confirmed = verified.len();
    let _ = std::fs::write(flags.join("child_ready"), format!("{ready}/{confirmed}"));
    if verified.is_empty() {
        // 探针：这一问服务器到底答了什么。`Ok(false)` 是"它说没有"（产品侧据此记 `absent`），
        // `Err` 是"这一问根本没问成"（产品侧据此**不下结论**）。两者的门禁含义不一样，
        // 红的时候必须分得开，不然会拿着夹具的账去改产品的判据。
        let probe = remote.has_attachment(&sha).await;
        let _ = std::fs::write(flags.join("child_probe"), format!("{probe:?}"));
    }
    let _ = std::fs::write(flags.join("child_verified"), confirmed.to_string());
    if !race_barrier(flags, "go2") {
        let _ = std::fs::write(flags.join("child_timeout"), "go2");
        return;
    }
    let destroyed = app.purge_verified_blobs(&verified);
    let _ = std::fs::write(flags.join("child_destroyed"), destroyed.to_string());

    // ③ 父进程也动完手之后，子进程再读一次账：两边各自读同一份 SQLite，结论必须一样
    if race_barrier(flags, "go3") {
        let state = app.store().attachment_for_state(&sha);
        let _ = std::fs::write(
            flags.join("child_state"),
            format!("{}|{}", state.0, state.1),
        );
    }
}

/// GC 是本仓唯一一段"两个东西可以同时销毁用户字节"的代码，而之前那条门禁（FT-ATT-37）测的是
/// **同一个进程里**的两个 store —— 它们共享地址空间，抢的只是数据库连接与磁盘。
///
/// 这一条换成真的两个 OS 进程，因为"第二个进程会被锁挡住"那个前提在这批被推翻了：这个项目
/// **没有单实例锁**（`store/pool.rs` 只有 `PRAGMA busy_timeout`），"用户把应用开了两次"在桌面上
/// 是一个能发生的形状，不是假想敌。形状与 FT-ATT-37 一样是两步，但每一步都在两个进程里各跑一遍：
/// * 认领只能数到一次，且隔离区里恰好一份、内容一字不差（两个进程同时 `rename` 同一份字节，
///   输的那一边必须"本轮不收这一条"，而不是把别人的现场再搬一次）；
/// * 拿着同一份到期清单动手，销毁也只能数到一次；
/// * 事后两边各读各的账，都得到"这一行没了"。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_os_processes_on_one_data_dir_claim_and_destroy_the_bytes_exactly_once() {
    use std::process::{Command, Stdio};

    let srv = TestServer::start(Backend::Mem).await;
    let url = srv.base_url();
    let blob: Vec<u8> = (0..5_400).map(|i| (i % 71) as u8).collect();
    let dir = Tmp::new("39-xproc");
    let (a, sha) = seed_uploaded(dir.path(), &url, &blob).await;
    let nid = a
        .store()
        .list_notes(&notera_store::NoteQuery {
            folder: None,
            trash: false,
            limit: 50,
            offset: 0,
        })
        .unwrap()
        .into_iter()
        .find(|n| n.title.contains("GC 目标笔记"))
        .map(|n| n.id.clone())
        .expect("前置：那条带图笔记还在");
    a.store().purge_note(&nid).expect("永久删除，让引用归零");

    // 再补 24 份"零引用而服务器已有副本"的字节：只用公开接口造（`attach_blob` 真往盘上写 →
    // 标 `present` → 永久删掉那条笔记），一次网络都不发。
    // 为什么是 25 份而不是 1 份：这个窗口要靠两边在**同一批 sha 上反复重叠**才抓得住 ——
    // 单份的形状是运气（修前实测 8 次红 2 次），二十几份是每次都撞。
    let folder = notera_core::EntityId::parse(notera_store::DEFAULT_FOLDER_ID).unwrap();
    let mut samples: Vec<(String, Vec<u8>)> = Vec::new();
    for i in 0..24 {
        let note = a
            .store()
            .create_note(&folder, doc(&format!("GC 重叠样本 {i}")))
            .unwrap();
        // 每份内容都不同 ⇒ 不同的 sha（按 7 步进地旋转 0..250 这一段：251 是质数，
        // 位移 7·i 在 i<24 内互不相同，所以没有两份样本会撞进同一个 sha）。
        let bytes: Vec<u8> = (0..1_024).map(|k| ((k + i * 7) % 251) as u8).collect();
        let sha = a
            .store()
            .attach_blob(&note.id, &bytes, "image/png", Some("s.png"), "blk000002")
            .unwrap()
            .sha256;
        a.store()
            .set_attachment_states(&sha, None, Some("present"))
            .unwrap();
        a.store().purge_note(&note.id).unwrap();
        samples.push((sha, bytes));
    }
    let unique: std::collections::HashSet<&String> = samples.iter().map(|(s, _)| s).collect();
    assert_eq!(
        unique.len(),
        samples.len(),
        "前置：{} 份样本必须有 {} 个不同的 sha，否则下面那句「认领总数 == 份数」根本不成立",
        samples.len(),
        samples.len()
    );
    let mut all_samples = samples;
    all_samples.push((sha.clone(), blob.clone()));
    let total = all_samples.len();

    let flags = dir.path().join("race-flags");
    std::fs::create_dir_all(&flags).unwrap();
    // 父进程不管从哪一步退场（包括 panic）都要把 `stop` 立起来：否则子进程会堵在栅栏上直到超时，
    // 那段时间它**正占着测试二进制**，下一次构建就报 `Permission denied` —— 看着像门禁红，其实是夹具。
    struct StopOnDrop(PathBuf);
    impl Drop for StopOnDrop {
        fn drop(&mut self) {
            let _ = std::fs::write(self.0.join("stop"), "parent-exited");
        }
    }
    let _stop = StopOnDrop(flags.clone());
    let child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "gc_race_child_process",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(
            GC_RACE_CHILD,
            format!(
                "{}|{}|{}|{}",
                dir.path().display(),
                url,
                sha,
                flags.display()
            ),
        )
        .env("NOTERA_DEV_WEBDAV_SECRET", SECRET)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("起第二个 OS 进程：这条门禁要的就是真跨进程");

    // ① 认领：两边都先握着同一份未认领的清单，再被同一个文件放行
    assert!(
        race_barrier(&flags, "child_listed"),
        "子进程没把候选清单写回来（启动或前置失败），原因写着：{}",
        race_read(&flags, "child_error")
    );
    assert_eq!(
        race_read(&flags, "child_listed"),
        "1",
        "前置：子进程那一侧的清单里必须有这一条，否则两边抢的根本不是同一批字节"
    );
    assert!(
        a.store()
            .gc_quarantine_candidates(50)
            .unwrap()
            .contains(&sha),
        "前置：父进程这一侧也还看到同一条（两边要在同一瞬各握一份）"
    );
    std::fs::write(flags.join("go1"), "1").unwrap();
    let claimed_here = a.reclaim_unreferenced_blobs(50);
    assert!(
        race_barrier(&flags, "child_claimed"),
        "子进程没跑完认领那一步，卡在：{}",
        race_read(&flags, "child_timeout")
    );
    let claimed_child = race_count(&flags, "child_claimed");
    // 两边的认领都跑完了（账上那一次写入无论出自谁家，此刻都已提交）—— 放行子进程去做销毁前的复查
    std::fs::write(flags.join("both1"), "1").unwrap();
    assert_eq!(
        claimed_here + claimed_child,
        total,
        "认领的总数必须正好等于候选的份数：多了就是同一行被两家各认领一次（父 {claimed_here} / 子 {claimed_child}，候选 {total} 份）"
    );
    // 两种相反的形状要分开数：还压在正式位置（两家都没搬走）与 两处都没有（**那份内容没了**）。
    let mut still_official: Vec<String> = Vec::new();
    let mut lost_bytes: Vec<String> = Vec::new();
    for (s, bytes) in &all_samples {
        if official_bytes(&a, s) != Vec::<u8>::new() {
            still_official.push(s.clone());
        }
        if quarantine_bytes(&a, s) != *bytes {
            lost_bytes.push(s.clone());
        }
    }
    assert!(
        still_official.is_empty(),
        "这些份还压在正式位置（两家都没把它们认领走）：{still_official:?}"
    );
    assert!(
        lost_bytes.is_empty(),
        "这些 sha 的字节既不在正式位置也不在隔离区 = 那份内容被搬走之后又被腾掉了：{lost_bytes:?}"
    );

    // ② 销毁：两边各自问到"服务器还在"，拿到同一份清单后同时动手
    let remote = a.remote_for_sync().await.unwrap().expect("父进程有适配器");
    let verified_here = a.confirm_still_remote(&remote, ALL_PAST, 50).await;
    assert_eq!(
        verified_here,
        vec![sha.clone()],
        "前置：父进程这一侧问到了「服务器还在」"
    );
    // 现场记一笔：父进程拿到清单之后，再问一次服务器对同一个 sha 的答复。子进程那侧若拿到过
    // `Ok(false)`，这一行就能分清"对象真的不在了"与"那一次问答错了"。
    let parent_probe = remote.has_attachment(&sha).await;
    assert!(
        race_barrier(&flags, "child_verified"),
        "子进程没跑完销毁前的复查，卡在：{}",
        race_read(&flags, "child_timeout")
    );
    assert_eq!(
        race_read(&flags, "child_verified"),
        "1",
        "前置：子进程也拿到了同一份销毁清单（它那边看到的「账上可准入/服务器确认在」= {}；它那一问的直接答复 = {}；父进程此刻再问一次 = {parent_probe:?}）",
        race_read(&flags, "child_ready"),
        race_read(&flags, "child_probe")
    );
    std::fs::write(flags.join("go2"), "1").unwrap();
    let destroyed_here = a.purge_verified_blobs(&verified_here);

    // ③ 放行子进程读账，然后收尸
    std::fs::write(flags.join("go3"), "1").unwrap();
    assert!(
        race_barrier(&flags, "child_state"),
        "子进程没读回账上的状态，卡在：{}",
        race_read(&flags, "child_timeout")
    );
    let out = child.wait_with_output().expect("等子进程收摊");
    assert!(
        out.status.success(),
        "子进程没活着走到最后：{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let destroyed_child = race_count(&flags, "child_destroyed");
    assert_eq!(
        destroyed_here + destroyed_child,
        1,
        "同一行被销毁两次 = 第二步在跨进程时也没有互斥（父 {destroyed_here} / 子 {destroyed_child}）"
    );
    assert_eq!(
        a.store().attachment_for_state(&sha),
        ("absent".to_string(), "absent".to_string()),
        "父进程这一侧：账上没这一行了"
    );
    assert_eq!(
        race_read(&flags, "child_state"),
        "absent|absent",
        "子进程那一侧读同一份账也必须没有这一行（两个进程各一份地址空间，结论要一样）"
    );
    assert_eq!(
        quarantine_bytes(&a, &sha),
        Vec::<u8>::new(),
        "隔离区不许留下第二份或残骸"
    );
}
