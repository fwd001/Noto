//! §27「附件故障注入」里此前**一条都没注入过**的三种：本地附件缺失、远端文件损坏、
//! 下载被中途掐断。§8 的收尾句是硬要求：**正文必须尽量继续可用** —— 也就是说
//! 附件坏了不能把笔记一起拖死，而"坏"必须被认出来、最好还能自己修回来。
//!
//! 为什么单独一个文件而不是往 `attachment_resume.rs` 里塞：那一条讲的是"没下完时进度
//! 留在磁盘上"，这里讲的是"下完之后磁盘上没有了 / 下回来的字节是坏的"。判据不同，
//! 失败时想看的东西也不同。
//!
//! 跑法：`cargo test -p notera-host --test attachment_faults`

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
            std::env::temp_dir().join(format!("notera-fault-{tag}-{}-{n}", std::process::id()));
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
            "label": "故障注入", "baseUrl": url, "username": "notera-test",
        }))
        .unwrap();
        app.configure_account(draft).expect("配置账户");
    }
    app
}

/// A 建一条带图的笔记、把图和正文都落好账，**但还没跑附件轮**。返回 `(App, sha)`：
/// App 必须交回调用方，否则"上传前"这一半没有设备可用。
async fn seed_ready_to_upload(dir: &Path, url: &str, blob: &[u8]) -> (App, String) {
    let app = boot(dir, url);
    app.sync_once().await.expect("A 入伙");
    let folder = app.default_folder_id().unwrap();
    let note = app.create_note(&folder, doc("有一张图的笔记")).unwrap();
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
                { "id": "blk000001", "type": "paragraph", "content": [{ "text": "有一张图的笔记" }] },
                { "id": "blk000002", "type": "image", "attrs": {
                    "sha256": &sha, "ref": &sha, "role": "inline", "pending": false,
                    "size": blob.len(), "mediaType": "image/png", "name": "shot.png" } },
            ] }),
            head.rev,
        )
        .unwrap();
    app.sync_once().await.expect("A 推正文");
    (app, sha)
}

/// A 建一条带图的笔记并真的把图传上去，返回 sha。
async fn seed_source(dir: &Path, url: &str, blob: &[u8]) -> String {
    let (app, sha) = seed_ready_to_upload(dir, url, blob).await;
    let remote = app.remote_for_sync().await.unwrap().expect("A 有适配器");
    let round = app.run_attachment_round(&remote).await;
    assert_eq!(round.0, 1, "源设备要把这张图传上去：{round:?}");
    sha
}

/// B 拉到正文并把图下下来（一步到位的小附件）。
async fn drain_target(dir: &Path, url: &str, blob: &[u8], sha: &str) -> App {
    let b = boot(dir, url);
    b.sync_once().await.expect("B 拉到正文");
    let remote = b.remote_for_sync().await.unwrap().expect("B 有适配器");
    let round = b.run_attachment_round(&remote).await;
    assert_eq!(round.1, 1, "B 第一轮要把图下完：{round:?}");
    assert_eq!(
        blob_on_disk(&b, sha),
        blob.to_vec(),
        "前置不成立：本机没拿到那份字节"
    );
    b
}

/// 正文必须照旧可用：笔记读得到、图块还在、标题还是那句。
fn assert_body_still_usable(b: &App, sha: &str) {
    let notes = b
        .store()
        .list_notes(&notera_store::NoteQuery {
            folder: None,
            trash: false,
            limit: 50,
            offset: 0,
        })
        .expect("列表读得到");
    let row = notes
        .iter()
        .find(|n| n.title.contains("有一张图的笔记"))
        .expect("正文标题必须在列表里（附件坏了不许把笔记一起弄没）");
    let note = b
        .store()
        .get_note(&row.id)
        .unwrap()
        .expect("这条笔记要读得回来");
    let raw = serde_json::to_string(&note.doc).unwrap_or_default();
    assert!(
        raw.contains(sha),
        "正文里对这张图的引用被顺手清掉了（那不是'继续可用'，那是把问题藏起来）：{raw}"
    );
}

/// 盘上那份 blob 的字节（读不到就返回空）。
fn blob_on_disk(b: &App, sha: &str) -> Vec<u8> {
    std::fs::read(b.store().blob_path(sha)).unwrap_or_default()
}

/// 编辑器取图走的是命令面 `attachment_data`：坏/缺都必须回一个**具名**错误，
/// 而不是 panic、也不是回一段错字节让界面画出半张图。
fn ui_read(b: &App, sha: &str) -> serde_json::Value {
    notera_host::commands::dispatch(b, "attachment_data", json!({ "sha256": sha }))
        .unwrap_or_else(|e| json!({ "ok": false, "code": e.code }))
}

// ———————————————————————————————————————————————— 本地附件缺失

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lost_local_blob_is_repaired_and_never_strands_the_note() {
    let srv = TestServer::start(Backend::Mem).await;
    let url = srv.base_url();
    let blob: Vec<u8> = (0..9000).map(|i| (i % 253) as u8).collect();
    let a_dir = Tmp::new("lost-a");
    let sha = seed_source(a_dir.path(), &url, &blob).await;

    let b_dir = Tmp::new("lost-b");
    let b = drain_target(b_dir.path(), &url, &blob, &sha).await;
    assert_eq!(
        blob_on_disk(&b, &sha),
        blob,
        "前置：先确认本机真的有这份字节"
    );

    // 用户侧真实会发生的事：磁盘清理、杀毒隔离、误删 attachments 目录、换盘没搬完。
    std::fs::remove_file(b.store().blob_path(&sha)).expect("删掉本机 blob");
    assert!(!b.store().blob_path(&sha).exists());

    // ① 正文继续可用（§8 收尾句）。
    assert_body_still_usable(&b, &sha);
    // ② 编辑器读图必须拿到一个**具名**失败，而不是一段能画出来的错字节。
    let got = ui_read(&b, &sha);
    assert!(
        got.get("code").is_some() || got.get("data").and_then(|v| v.as_str()).is_none(),
        "本机已经没有这个文件，命令面却还是回了内容：{got}"
    );
    // ③ 下一轮同步要把它认成"缺失"并重下回来 —— 否则账上永远是 available，
    //    用户看到的是一张永远打不开的图，而且系统以为自己已经修好了。
    drop(b);
    let b2 = boot(b_dir.path(), &url);
    let remote = b2.remote_for_sync().await.unwrap().expect("有适配器");
    let round = b2.run_attachment_round(&remote).await;
    assert_eq!(
        round.1, 1,
        "本机 blob 丢了却没重下：这一条附件会永久坏掉（round={round:?}）"
    );
    assert_eq!(
        blob_on_disk(&b2, &sha),
        blob,
        "重新下回来的字节必须一字不差（哈希就是唯一的身份）"
    );
    let (local, _r) = b2.store().attachment_for_state(&sha);
    assert_eq!(
        local.as_str(),
        "available",
        "补回来之后账上才允许重新说 available"
    );
    srv.stop().await;
}

// ———————————————————————————————————————— 本地附件被截断（尺寸对不上）

/// 上一节救的是"文件不见了"，这一节救的是"文件还在、内容却短了"。
///
/// 分开写一条是因为修法不同：**必须先删掉那份坏文件再重下**。`ingest_blob` 见目标
/// 已存在就不覆盖（那是给"同一份字节被多处引用"省 IO 的），留着坏文件的后果是
/// 重下成功了、账也标回 available 了，盘上却还是那半截 —— 比不修更骗人。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_truncated_local_blob_is_replaced_not_reused() {
    let srv = TestServer::start(Backend::Mem).await;
    let url = srv.base_url();
    let blob: Vec<u8> = (0..9000).map(|i| (i % 229) as u8).collect();
    let a_dir = Tmp::new("trunc-a");
    let sha = seed_source(a_dir.path(), &url, &blob).await;
    let b_dir = Tmp::new("trunc-b");
    let b = drain_target(b_dir.path(), &url, &blob, &sha).await;

    let path = b.store().blob_path(&sha);
    std::fs::write(&path, &blob[..blob.len() / 2]).expect("把本机 blob 截断一半");
    assert_eq!(std::fs::metadata(&path).map(|m| m.len()).unwrap(), 4500);
    drop(b);

    let b2 = boot(b_dir.path(), &url);
    let remote = b2.remote_for_sync().await.unwrap().expect("有适配器");
    let round = b2.run_attachment_round(&remote).await;
    assert_eq!(
        round.1, 1,
        "半截的本地 blob 没被换掉：这一条会永远停在坏字节上（round={round:?}）"
    );
    assert_eq!(
        blob_on_disk(&b2, &sha),
        blob,
        "重下回来的必须是整份字节（短一半的那份不能继续用）"
    );
    assert!(
        !b2.store().blob_part_path(&sha).exists(),
        "补齐之后不该留下半截文件"
    );
    let (local, _r) = b2.store().attachment_for_state(&sha);
    assert_eq!(local.as_str(), "available");
    srv.stop().await;
}

// ———————————————————————————————————————————————— 远端文件损坏

/// 找出服务器目录里那份附件文件（真 TCP 服务器的 Fs 后端就是磁盘上的字节）。
fn find_remote_blob(dir: &Path, sha: &str, len: usize) -> PathBuf {
    let mut hits = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            let name = p
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_string();
            let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0) as usize;
            if size == len && (name.contains(sha) || name.contains(&sha[..8.min(name.len())])) {
                hits.push(p);
            }
        }
    }
    assert_eq!(
        hits.len(),
        1,
        "在服务器目录里该恰好找到一份 {len} 字节、名字带 sha 的附件，实际 {hits:?}"
    );
    hits.pop().unwrap()
}

/// 服务器权威快照（DUMP）里路径含 sha 的文件 —— TEST-PLAN 规定 DUMP 是唯一允许的
/// 服务端状态断言手段。用它而不只看磁盘：磁盘改了不等于服务器发的是那份字节
/// （Fs 后端平时服务内存表），也用它确认"半份上传根本没落成正式对象"。
fn served_blobs(srv: &TestServer, sha: &str) -> Vec<serde_json::Value> {
    let dump = srv.fs_dump();
    dump.get("entries")
        .and_then(|e| e.as_array())
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|e| {
            e.get("is_dir").and_then(|d| d.as_bool()) == Some(false)
                && e.get("path")
                    .and_then(|p| p.as_str())
                    .is_some_and(|p| p.contains(sha))
        })
        .collect()
}

/// 同上的"必须恰好一份"版本。
fn served_blob(srv: &TestServer, sha: &str) -> serde_json::Value {
    let entries = served_blobs(srv, sha);
    assert_eq!(
        entries.len(),
        1,
        "服务器上该恰好有一份含 {sha} 的对象（路径或数量不对就说明上传/落盘形态不是预期的）"
    );
    entries.into_iter().next().unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_corrupted_remote_blob_is_refused_and_the_note_stays_readable() {
    let dav = Tmp::new("corrupt-dav");
    let srv = TestServer::start(Backend::Fs(dav.path().to_path_buf())).await;
    let url = srv.base_url();
    let blob: Vec<u8> = (0..12000).map(|i| (i % 251) as u8).collect();
    let a_dir = Tmp::new("corrupt-a");
    let sha = seed_source(a_dir.path(), &url, &blob).await;

    // 服务器上的字节被换坏（磁盘故障、被别的客户端写错、中间人改了内容都会这样）。
    let victim = find_remote_blob(dav.path(), &sha, blob.len());
    let mut bad = blob.clone();
    for (i, byte) in bad.iter_mut().enumerate().take(4096).step_by(97) {
        *byte = byte.wrapping_add(3 + (i % 250) as u8);
    }
    assert_ne!(bad, blob, "前置不成立：这一改压根没改到内容");
    std::fs::write(&victim, &bad).expect("写坏服务器上的附件");
    assert_eq!(
        std::fs::metadata(&victim).map(|m| m.len()).unwrap_or(0) as usize,
        blob.len(),
        "要坏得隐蔽：长度对得上，只有内容不对"
    );
    // Fs 后端平时服务的是**内存表**：光改磁盘文件，服务器照旧发好字节（第一版就这么被骗过，
    // 于是这条测试测的是"客户端拿到了正确内容"，而不是它声称的"远端坏了"）。RESTART 才以磁盘为准重建。
    srv.restart().await;
    let served = served_blob(&srv, &sha);
    assert_eq!(
        served.get("bytes").and_then(|v| v.as_u64()),
        Some(blob.len() as u64),
        "DUMP 里那份附件的尺寸该还是原样：{served}"
    );
    assert_ne!(
        served.get("sha256").and_then(|v| v.as_str()).unwrap_or(""),
        format!("sha256:{sha}"),
        "前置不成立：服务器现在发的还是好字节，这一条测不到'远端损坏'：{served}"
    );

    let b_dir = Tmp::new("corrupt-b");
    let b = boot(b_dir.path(), &url);
    b.sync_once().await.expect("B 拉到正文");
    let remote = b.remote_for_sync().await.unwrap().expect("有适配器");
    let round = b.run_attachment_round(&remote).await;
    // ① 绝不把哈希对不上的字节当成"这份附件本机有了"。
    assert_eq!(
        round.1,
        0,
        "服务器给的是坏字节，却把这条算成了下载完成（round={round:?}）—— 内容寻址的唯一意义就是不许这样"
    );
    assert!(
        blob_on_disk(&b, &sha).is_empty(),
        "坏字节被落进了正式 blob 位置：下次读它的就是编辑器了"
    );
    let (local, _r) = b.store().attachment_for_state(&sha);
    assert_ne!(
        local.as_str(),
        "available",
        "坏字节被收下了：账上不该说这份附件可用"
    );
    // ② 正文继续可用；③ 界面读到的是具名失败而不是那张坏图。
    assert_body_still_usable(&b, &sha);
    let got = ui_read(&b, &sha);
    assert!(
        got.get("code").is_some() || got.get("data").and_then(|v| v.as_str()).is_none(),
        "坏附件竟然能被界面读出来：{got}"
    );
    srv.stop().await;
}

// ———————————————————————————————————————————————— 下载被中途掐断

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dropped_connection_mid_download_promotes_nothing() {
    let srv = TestServer::start(Backend::Mem).await;
    let url = srv.base_url();
    let blob: Vec<u8> = (0..200_000).map(|i| (i % 241) as u8).collect();
    let a_dir = Tmp::new("drop-a");
    let sha = seed_source(a_dir.path(), &url, &blob).await;

    // 正文要先正常落地，再把注入对准附件下载：`/_control/inject` 会把"已服务数据请求数"
    // 清零，所以 abort_after(0) = 从下一个数据请求起一律掐。第一版把它挂在 B 开机之前，
    // 结果 B 连清单都没拉到 —— 测的就不是"下载中断"，而是"网络不通"。
    let b_dir = Tmp::new("drop-b");
    let b = boot(b_dir.path(), &url);
    b.sync_once().await.expect("B 拉到正文");
    let remote = b.remote_for_sync().await.unwrap().expect("有适配器");
    assert_body_still_usable(&b, &sha);

    srv.inject(Injection::abort_after(0)).await;
    let round = b.run_attachment_round(&remote).await;
    srv.inject(Injection::none()).await;
    assert_eq!(round.1, 0, "被掐断的一轮不许算作下载完成：{round:?}");
    assert!(
        blob_on_disk(&b, &sha).is_empty(),
        "连接被掐却往正式 blob 位置落了字节：中途结果被当成了完整文件"
    );
    let (local, _r) = b.store().attachment_for_state(&sha);
    assert_ne!(
        local.as_str(),
        "available",
        "断线的一轮之后账上不许说这份附件可用"
    );
    assert_body_still_usable(&b, &sha);

    // 恢复之后要能真的补齐，而不是留下一条永远下不完的队列。
    drop(b);
    let b2 = boot(b_dir.path(), &url);
    let remote2 = b2.remote_for_sync().await.unwrap().expect("有适配器");
    let round2 = b2.run_attachment_round(&remote2).await;
    assert_eq!(round2.1, 1, "服务器恢复后这一条该补下来：{round2:?}");
    assert_eq!(blob_on_disk(&b2, &sha), blob, "补齐的字节必须一字不差");
    srv.stop().await;
}

// ———————————————————————————————————————————————— 上传被中途掐断

/// §27「上传中断」的真形态：服务器只读到半份 body 就连线断了。这一条此前**只写在
/// TEST-PLAN 的 FT-ATT-04 里，从来没有测试做过** —— 文档声称有，机器没证明过。
///
/// 要钉住三件事：① 半上传不许算成功（`up` 不涨）；② 服务器上不存在那个"半个正式对象"
/// （INV-09：没写完的东西不被引用，也不被落成引用得到的样子）；③ 账上不能因为"发过了 PUT"
/// 就说远端 present —— 那样下一台设备会永远等一份服务器上并不存在的字节。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_half_uploaded_attachment_lands_nothing_and_is_retried() {
    let srv = TestServer::start(Backend::Mem).await;
    let url = srv.base_url();
    let blob: Vec<u8> = (0..9000).map(|i| (i % 233) as u8).collect();
    let a_dir = Tmp::new("half-a");
    let (a, sha) = seed_ready_to_upload(a_dir.path(), &url, &blob).await;
    let remote = a.remote_for_sync().await.unwrap().expect("有适配器");

    srv.inject(Injection {
        truncate_upload_at: Some(400),
        ..Default::default()
    })
    .await;
    let round = a.run_attachment_round(&remote).await;
    srv.inject(Injection::none()).await;

    assert_eq!(round.0, 0, "半上传被当成了上传成功：{round:?}");
    assert!(
        served_blobs(&srv, &sha).is_empty(),
        "半份字节被服务器收成了正式对象（INV-09 破）：{round:?}"
    );
    let (_l, r) = a.store().attachment_for_state(&sha);
    assert_ne!(
        r.as_str(),
        "present",
        "没传成却在账上说服务器上有了：下一台设备会永远等一份不存在的东西"
    );
    assert_body_still_usable(&a, &sha);

    // 网恢复之后，失败的这一条要还能在下一轮补上（不是一次性消耗品），且最终恰好一份。
    let round2 = a.run_attachment_round(&remote).await;
    assert_eq!(round2.0, 1, "中断过一次之后再也传不上去：{round2:?}");
    let final_objects = served_blobs(&srv, &sha);
    assert_eq!(
        final_objects.len(),
        1,
        "重试之后服务器上该恰好一份：{final_objects:?}"
    );
    let (l2, r2) = a.store().attachment_for_state(&sha);
    assert_eq!(
        (l2.as_str(), r2.as_str()),
        ("available", "present"),
        "传成之后两半状态都要落对"
    );
    srv.stop().await;
}

// ———————————————————————————————————————————————— 附件端点超时（只有它超时）

/// §27 最后一条有实证缺口的注入：**只有附件端点不回应**。
///
/// 这条同时兜住两句一直没人证明的声称：
/// * `run_attachment_round` 的注释写着"任何失败都不向上抛：附件全失败时，文本同步必须
///   照常完成（TEST-PLAN 的『只拔附件端点』用例）" —— 此前那个用例不存在；
/// * §28 的保证句「网络问题永远不会让本地数据不可用」。
///
/// 为什么不能用现成的 `Injection::hang()`：那是**整台服务器**不应答，测到的是"网络不通"。
/// 真实形态常常是附件挂在另一个反代/存储桶上 —— 文本端点好好的，只有图取不回来。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hanging_attachment_endpoint_never_blocks_the_text_round() {
    let srv = TestServer::start(Backend::Mem).await;
    let url = srv.base_url();
    let blob: Vec<u8> = (0..20_000).map(|i| (i % 227) as u8).collect();
    let a_dir = Tmp::new("hang-a");
    let sha = seed_source(a_dir.path(), &url, &blob).await;

    let b_dir = Tmp::new("hang-b");
    let b = boot(b_dir.path(), &url);
    b.sync_once().await.expect("B 拉到正文");
    let remote = b.remote_for_sync().await.unwrap().expect("有适配器");
    // 上限从**产品自己的预算**取，不用我拍的数：这台机器实测一次挂死的附件轮 = 45.01 s，
    // 正好是 `notera_net::Timeouts::per_request`（重试与整体共用同一份预算，不是每试一次 45 s）。
    let budget = notera_net::Timeouts::default().per_request;

    srv.inject(Injection::hang_on(format!("GET *{sha}"))).await;
    // A 在附件挂着的时候推一条新笔记，B 的文本轮必须照常在预算内跑完 ——
    // 这两件事是**并发**发生的（常驻循环里附件与文本本来就是两个任务），所以用 join 而不是先后跑。
    let a = boot(a_dir.path(), &url);
    let folder = a.default_folder_id().unwrap();
    a.create_note(&folder, doc("附件挂着也要写得下去的第二条"))
        .expect("本机写入");
    a.sync_once().await.expect("A 推正文");

    let text_budget = budget / 4; // 文本轮本来只要几百毫秒，四分之一预算已经是极宽松的上界
    let (round, text) = tokio::join!(b.run_attachment_round(&remote), async {
        // 先确认真有一个请求挂在那儿（不然测的是"什么都没发生"）
        let mut stuck = false;
        for _ in 0..50 {
            stuck = srv
                .request_log()
                .iter()
                .any(|q| q.path.contains(&sha) && q.method == "GET" && q.status == 0);
            if stuck {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        assert!(stuck, "前置不成立：注入压根没打中那个附件请求");
        let started = std::time::Instant::now();
        let stats = tokio::time::timeout(text_budget * 3, b.sync_once())
            .await
            .expect("附件端点挂死把文本同步一起拖住了（§13 的队列隔离破了）");
        (started.elapsed(), stats)
    });
    let (text_took, text) = text;

    assert!(
        text.is_ok(),
        "文本轮本身报了错（附件挂死不许外溢成文本轮的失败）：{text:?}"
    );
    assert!(
        text_took < text_budget,
        "文本轮等了 {text_took:?}（预算 {text_budget:?}）—— 它被那个挂死的附件请求拖住了，\
         §28 的「网络问题永远不会让本地数据不可用」与 §13 的队列隔离都不成立"
    );
    let titles: Vec<String> = b
        .store()
        .list_notes(&notera_store::NoteQuery {
            folder: None,
            trash: false,
            limit: 50,
            offset: 0,
        })
        .expect("列表读得到")
        .iter()
        .map(|n| n.title.clone())
        .collect();
    assert!(
        titles.iter().any(|t| t.contains("第二条")),
        "附件挂着，B 就再也收不到新笔记了：{titles:?}"
    );

    assert_eq!(round.1, 0, "挂着的端点却报下载完成：{round:?}");
    let (local, _r) = b.store().attachment_for_state(&sha);
    assert_ne!(local.as_str(), "available", "没拿到字节就说本机有了");
    assert!(
        blob_on_disk(&b, &sha).is_empty(),
        "超时的一轮往正式 blob 位置落了字节"
    );
    assert!(
        !b.store().blob_part_path(&sha).exists(),
        "超时的一轮不许留下 .part（半路没有进度就别装作有）"
    );
    assert!(
        round.2 >= 1,
        "超时没被算进失败数 —— 状态页上看不出这一轮白跑了：{round:?}"
    );

    // 端点恢复之后，这一条要还能补回来：超时是"这一次没取到"，不是"这条坏了"。
    srv.inject(Injection::none()).await;
    let recovered = b.run_attachment_round(&remote).await;
    assert_eq!(
        recovered.1, 1,
        "端点恢复后这一条补不回来（超时把它变成永久坏掉）：{recovered:?}"
    );
    assert_eq!(blob_on_disk(&b, &sha), blob, "补回来的字节必须一字不差");
    srv.stop().await;
}

/// §27「文件不存在」：正文引用着某个 sha，服务器上那份对象却没了（用户在网页端手删、
/// 网盘侧回收站清掉、只同步了一半的镜像）。
///
/// 这里最值钱的是**最后一段**：客户端必须把"没有"当成一条**结论**记下来（`absent`），
/// 而不是一份每轮重问一次的悬案。`attachment_downloads()` 的注释一直声称"404 就此收手，
/// 不会变成每轮重复的空转" —— 那条声称此前没有任何测试撑着。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_missing_remote_object_is_marked_absent_and_stops_being_retried() {
    let srv = TestServer::start(Backend::Mem).await;
    let url = srv.base_url();
    let blob: Vec<u8> = (0..7000).map(|i| (i % 239) as u8).collect();
    let a_dir = Tmp::new("404-a");
    let sha = seed_source(a_dir.path(), &url, &blob).await;

    let b_dir = Tmp::new("404-b");
    let b = boot(b_dir.path(), &url);
    b.sync_once().await.expect("B 拉到正文");
    let remote = b.remote_for_sync().await.unwrap().expect("有适配器");
    assert_eq!(
        b.store().attachment_downloads(4).unwrap().len(),
        1,
        "前置：这一条要在下载队列里"
    );

    srv.inject(Injection::status(format!("GET *{sha}"), 404))
        .await;
    let round = b.run_attachment_round(&remote).await;
    srv.inject(Injection::none()).await;

    assert_eq!(round.1, 0, "服务器说没有，却算成了下载完成：{round:?}");
    let (local, remote_state) = b.store().attachment_for_state(&sha);
    assert_ne!(local.as_str(), "available", "没有字节凭什么说本机有了");
    assert_eq!(
        remote_state.as_str(),
        "absent",
        "远端明确答 404 就要把'没有'记成结论：{remote_state}"
    );
    assert!(
        !b.store().blob_part_path(&sha).exists(),
        "404 之后不许留下 .part"
    );
    assert_body_still_usable(&b, &sha);

    // 再走完整的两轮（文本 + 附件）：收手的意思是**不再为它发请求**，也不是只收手一轮。
    for _ in 0..2 {
        b.sync_once().await.expect("文本轮照常");
        b.run_attachment_round(&remote).await;
    }
    srv.clear_log().await;
    let again = b.run_attachment_round(&remote).await;
    let hits: Vec<String> = srv
        .request_log()
        .iter()
        .filter(|r| r.path.contains(&sha))
        .map(|r| format!("{} {} -> {}", r.seq, r.method, r.status))
        .collect();
    assert!(
        hits.is_empty(),
        "为一份服务器说'没有'的附件每轮空转：请求 {hits:?}，轮次 {again:?}"
    );
    srv.stop().await;
}

// ———————————————————————————————————————————————— 服务器返回 412

/// §27「服务器返回 412」打在附件路径上。这一条要分的不是"412 该不该当成功"，而是
/// **"当成功之前有没有去看一眼服务器"**。
///
/// MOVE 收到 405/412 时，`put_attachment` 原本直接 `return Ok(())` —— 而紧跟在它后面
/// 那次复读校验就被跳过了。善意解释（并发下别人先传了同一份内容）常常是对的，
/// 但它是一种**解释**，不是证据：412 也会由网关/代理凭空造出来，那时目标其实不存在。
/// 表现是账上多出一个 `remote_state=present`，第二台设备从此永远等一份服务器上没有的字节。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_move_that_really_landed_counts_as_a_success() {
    let srv = TestServer::start(Backend::Mem).await;
    let url = srv.base_url();
    let blob: Vec<u8> = (0..15_000).map(|i| (i % 241) as u8).collect();
    let a_dir = Tmp::new("412-ok-a");
    let (a, sha) = seed_ready_to_upload(a_dir.path(), &url, &blob).await;
    let remote = a.remote_for_sync().await.unwrap().expect("有适配器");

    // `post:` = 副作用先做完再回这个状态 —— 这就是"对面那个设备先落成了盘，我们这次 MOVE 被拒"。
    srv.inject(Injection::partial_write("MOVE *", 412)).await;
    let round = a.run_attachment_round(&remote).await;
    srv.inject(Injection::none()).await;

    assert_eq!(
        round.0, 1,
        "服务器上有这一份、只是 MOVE 被拒，就该当成功：{round:?}"
    );
    let served = served_blobs(&srv, &sha);
    assert_eq!(
        served.len(),
        1,
        "并发解释成立时服务器上该恰好一份：{served:?}"
    );
    assert_eq!(
        served[0]
            .get("sha256")
            .and_then(|v| v.as_str())
            .unwrap_or(""),
        format!("sha256:{sha}"),
        "落盘的那份内容必须就是我们要传的字节"
    );
    let (l, r) = a.store().attachment_for_state(&sha);
    assert_eq!(
        (l.as_str(), r.as_str()),
        ("available", "present"),
        "验过才能记 present"
    );
    srv.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_move_that_landed_nothing_is_not_a_success() {
    let srv = TestServer::start(Backend::Mem).await;
    let url = srv.base_url();
    let blob: Vec<u8> = (0..15_000).map(|i| (i % 239) as u8).collect();
    let a_dir = Tmp::new("412-lie-a");
    let (a, sha) = seed_ready_to_upload(a_dir.path(), &url, &blob).await;
    let remote = a.remote_for_sync().await.unwrap().expect("有适配器");

    // 裸 412：拒绝，而且**什么都没落成**（网关凭空造一个前置失败就是这形态）。
    srv.inject(Injection::status("MOVE *", 412)).await;
    let round = a.run_attachment_round(&remote).await;
    srv.inject(Injection::none()).await;

    assert_eq!(
        round.0, 0,
        "被拒且服务器上根本没有，却报上传成功：{round:?}"
    );
    assert!(
        served_blobs(&srv, &sha).is_empty(),
        "这一轮没落成对象，账上更不许说服务器有了"
    );
    let (_l, r) = a.store().attachment_for_state(&sha);
    assert_ne!(
        r.as_str(),
        "present",
        "凭一个 412 就记 present —— 第二台设备会永远等一份不存在的东西"
    );
    assert_body_still_usable(&a, &sha);

    // 412 是"这一次没成"，不是"这条坏了"：下一轮要能真的传上去。
    let round2 = a.run_attachment_round(&remote).await;
    assert_eq!(round2.0, 1, "被 412 拒过一次之后就再也传不上去：{round2:?}");
    assert_eq!(served_blobs(&srv, &sha).len(), 1, "最终恰好一份");
    assert_eq!(a.store().attachment_for_state(&sha).1.as_str(), "present");
    srv.stop().await;
}
