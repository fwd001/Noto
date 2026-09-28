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

    // ① 宽限期还没到：一条都不许销毁。
    assert_eq!(
        a.purge_released_blobs(ALL_FUTURE, 50),
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

    // ② 到期：行与隔离区那份字节一起消失，正式位置本来就空着。
    assert_eq!(
        a.purge_released_blobs(ALL_PAST, 50),
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
        a.purge_released_blobs(ALL_PAST, 50),
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
