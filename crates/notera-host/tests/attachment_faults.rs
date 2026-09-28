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
    seed_ready_to_upload_declaring(dir, url, blob, blob.len()).await
}

/// 同上，但**正文图片块上声明的 `size`** 由调用方给（可以与真实字节数不等）。
///
/// 要造"台账尺寸与盘不符"就得走这条路而不是直接改库：块属性是每台设备各自写上去的，
/// 而 `upsert_attachment_row` 的冲突规则是 `MAX(旧, 新)` —— 只许涨不许落，所以一个偏大
/// 的声明值会稳定留在台账里，直到磁盘体检把它纠正（或直接改到那列就是作弊）。
async fn seed_ready_to_upload_declaring(
    dir: &Path,
    url: &str,
    blob: &[u8],
    declared: usize,
) -> (App, String) {
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
                    "size": declared, "mediaType": "image/png", "name": "shot.png" } },
            ] }),
            head.rev,
        )
        .unwrap();
    app.sync_once().await.expect("A 推正文");
    (app, sha)
}

/// A 建一条带图的笔记并真的把图传上去，返回 sha。
async fn seed_source(dir: &Path, url: &str, blob: &[u8]) -> String {
    seed_source_declaring(dir, url, blob, blob.len()).await
}

/// 同上，但正文声明的尺寸 = `declared`。
async fn seed_source_declaring(dir: &Path, url: &str, blob: &[u8], declared: usize) -> String {
    let (app, sha) = seed_ready_to_upload_declaring(dir, url, blob, declared).await;
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
///
/// 返回的是**形状**（错误码 + 字节长度）而不是原始 JSON。这里踩过一次坑：
/// 原来写成 `got.get("code").is_some() || got.get("data")…is_none()`，而
/// `commands::dispatch` 根本没有 `{ok, data}` 那层包（`j()` 就是 `serde_json::to_value`）——
/// 于是右半边对任何成功响应恒为真、左半边对任何错误恒为真，整条断言**数学上不可能失败**，
/// 却一度被算进"§27 有几条实证"。判据要打在真 dispatch 的输出上，也不许用 `||` 兜自己。
struct UiShape {
    code: Option<String>,
    bytes_len: Option<usize>,
}

impl UiShape {
    /// 只打错误码与字节长度：一次失败把 12 KB 的 base64 刷进 CI 日志是噪音不是证据。
    fn describe(&self) -> String {
        format!("code={:?} bytes_len={:?}", self.code, self.bytes_len)
    }
}

fn ui_read(b: &App, sha: &str) -> UiShape {
    match notera_host::commands::dispatch(b, "attachment_data", json!({ "sha256": sha })) {
        Ok(v) => UiShape {
            code: v.get("code").and_then(|x| x.as_str()).map(str::to_string),
            bytes_len: v.get("bytesBase64").and_then(|x| x.as_str()).map(str::len),
        },
        Err(e) => UiShape {
            code: Some(e.code),
            bytes_len: None,
        },
    }
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
    assert_eq!(
        got.code.as_deref(),
        Some("attachment_missing"),
        "本机已经没有这个文件，命令面却没报具名的缺失：{}",
        got.describe()
    );
    assert!(
        got.bytes_len.is_none(),
        "报缺失的同时还把字节发出去了：{}",
        got.describe()
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
    assert_eq!(
        b2.store().attachment_for_state(&sha),
        ("available".into(), "present".into()),
        "补回来之后账上才允许重新说 available（远端态也不许被顺手改掉）"
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
    // 坏拷贝的清理边：只有"手上已有一份哈希对得上的替代"时才允许它消失。
    assert!(
        parked_copies(&b2, &sha).is_empty(),
        "补齐之后那份坏拷贝还留着 —— 清理那一半没人验过"
    );
    // 修好之后不能每轮重来一遍：体检的"哈希相符就不动"那一条判据就是为此而在。
    // （它不在体检队列里"消失"是设计如此 —— 每条 available ∧ present 的都是每轮的候选，
    //   真正的判据是：被 stat 过之后状态不许再被降级。）
    let remote2 = b2.remote_for_sync().await.unwrap().expect("有适配器");
    let again = b2.run_attachment_round(&remote2).await;
    assert_eq!(
        again,
        (0, 0, 0),
        "修好的附件每轮又被重下一次 —— 体检把正常文件当成了坏文件"
    );
    assert_eq!(
        b2.store().attachment_for_state(&sha).0.as_str(),
        "available",
        "再跑一轮之后状态被打回去了"
    );
    assert_eq!(
        b2.store().attachment_for_state(&sha),
        ("available".into(), "present".into()),
        "补齐之后两半状态都要落对"
    );
    srv.stop().await;
}

// ———————————————— 登记尺寸与盘不符：体检复算出来的事实不许丢掉

/// 台账里这条附件的登记尺寸。读的是**体检自己用的那条生产查询**
/// （`attachment_repair_candidates`），不为测试新开一个只有测试在读的口子。
fn ledger_size(app: &App, sha: &str) -> i64 {
    app.store()
        .attachment_repair_candidates(500)
        .expect("体检候选查询要读得回来")
        .into_iter()
        .find(|(s, _)| s == sha)
        .map(|(_, n)| n)
        .expect("这条附件该还留在体检候选里（available ∧ present）")
}

/// 服务器收到过几条打到这个 sha 上的 GET（下载请求的路径就是内容寻址名）。
/// 用它把"完好的一份字节没被重下一遍"变成可数的证据，而不是推理。
fn gets_for_sha(srv: &TestServer, sha: &str) -> usize {
    srv.request_log()
        .iter()
        .filter(|r| r.method == "GET" && r.path.contains(sha))
        .count()
}

/// `attachments.size` 是"各台设备报上来的最大值"，不是"盘上那份字节的长度"：块属性由
/// 客户端各自写，冲突规则又只许涨不许落，于是**账上 13096 / 盘上 9000** 这样一行是会
/// 出现的，而且登记那条路永远不会纠正它。
///
/// 唯一能纠正它的是磁盘体检 —— 因为它为了判"是不是同一份字节"已经把整份读进内存复算过
/// sha256，**哈希相符就意味着盘上这个长度就是真实尺寸**。修之前那条分支只记一句 debug
/// 就 `continue`，把刚算出来的事实原样丢掉，代价是这一行在此后每一轮附件轮（常驻循环
/// 20 s 一轮）里都被整份读+哈希一遍（单条上界 32 MiB），而那个偏大的错误尺寸还会继续
/// 排进上传预算（`attachment_jobs` 按 `size` 排序）。
///
/// 断言打在三处：尺寸被一次改对、快路径判据（`stat` 长度 == 登记值）真的成立、以及
/// 改对的代价没有碰那份字节。**没有**声称测过 IO 次数 —— 工装不计数，能数的只有
/// "有没有多出一次 GET"和"判据落在哪条 `if` 上"。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hash_verified_row_has_its_size_corrected_once_and_then_stops_being_work() {
    let srv = TestServer::start(Backend::Mem).await;
    let url = srv.base_url();
    let blob: Vec<u8> = (0..9000).map(|i| (i % 131) as u8).collect();
    let declared = blob.len() + 4096;

    let a_dir = Tmp::new("sizefix-a");
    let sha = seed_source_declaring(a_dir.path(), &url, &blob, declared).await;
    let b_dir = Tmp::new("sizefix-b");
    let b = drain_target(b_dir.path(), &url, &blob, &sha).await;

    // 前置：B 账上那个偏大的声明值是从正文里派生出来的，盘上却是整份真实字节。
    assert_eq!(
        ledger_size(&b, &sha),
        declared as i64,
        "前置不成立：正文声明的尺寸没进台账，这条测试就没在测它想测的那格"
    );
    assert_eq!(
        blob_on_disk(&b, &sha),
        blob,
        "前置：本机盘上是完好的一份字节"
    );

    drop(b);
    let b2 = boot(b_dir.path(), &url);
    let remote = b2.remote_for_sync().await.unwrap().expect("有适配器");
    let gets_before = gets_for_sha(&srv, &sha);
    let round = b2.run_attachment_round(&remote).await;
    assert_eq!(
        round,
        (0, 0, 0),
        "一份哈希对得上的好文件被体检当成活干了：{round:?}"
    );
    // ① 尺寸一次改对，改成的正是盘上长度。
    assert_eq!(
        ledger_size(&b2, &sha),
        blob.len() as i64,
        "体检复算过 sha256 却没回填真实尺寸 —— 这一行会每轮被整份重读重哈希"
    );
    // ② 快路径的判据要**真的**成立（`sweep_lost_local_blobs` 里那条
    //    `size > 0 && on_disk == size`），否则"下一轮只 stat"只是好听话。
    assert!(
        std::fs::metadata(b2.store().blob_path(&sha))
            .map(|m| m.len())
            .unwrap_or(0)
            == ledger_size(&b2, &sha) as u64
            && ledger_size(&b2, &sha) > 0,
        "登记值与盘上长度还是对不上：下一轮走的仍是慢路"
    );
    // ③ 改尺寸的代价不许碰到那份字节，也不许改状态。
    assert_eq!(blob_on_disk(&b2, &sha), blob, "回填登记尺寸顺手把文件改了");
    assert!(
        parked_copies(&b2, &sha).is_empty(),
        "一份哈希对得上的文件被挪成 .corrupt —— 那是在销毁好字节"
    );
    assert_eq!(
        b2.store().attachment_for_state(&sha),
        ("available".into(), "present".into()),
        "回填尺寸把两半状态之一带跑了"
    );
    assert_eq!(
        gets_for_sha(&srv, &sha),
        gets_before,
        "完好的一份字节被体检重下了一遍"
    );

    // ④ 稳定：再来一轮既不该有活，也不该把尺寸漂回去。
    let remote2 = b2.remote_for_sync().await.unwrap().expect("有适配器");
    let again = b2.run_attachment_round(&remote2).await;
    assert_eq!(
        again,
        (0, 0, 0),
        "纠正过尺寸的附件每轮又被当成活：体检没把它当已完成（round={again:?}）"
    );
    assert_eq!(
        ledger_size(&b2, &sha),
        blob.len() as i64,
        "第二轮登记尺寸又漂回偏大值"
    );
    srv.stop().await;
}

// ———————————————————————— 体检的每轮工作量有界（§48 缺口 G4）

/// G4 管的是这一种真实场景：**整个 `attachments/` 目录被搬走或删掉**（换盘没搬完、
/// 杀毒按目录隔离、误 `rm -r`）。此时体检一轮能攒出成百上千条候选，而修之前是
/// "候选不分页 + 每行一个写事务" —— 那么多次提交排队占住写锁，用户那次保存正排在锁后面。
/// 现在候选按 `cap` 分页，降级是**一次**批量写事务。
///
/// 工装能数什么、数不清什么，写在前面免得这条被当成量化了代价：
/// * **能数**"一轮降了几条、剩下的要下一轮才降"（`sweep_lost_local_blobs` 的返回就是条数），
///   以及"降级出来的行当轮就进了下载队列"（队列查询）。
/// * **数不清**"提交了几次" —— Store 不暴露事务计数，为这条去加一个只有测试在读的计数器
///   就是 §39 禁的那种东西。所以"一次批量写"那一半是**代码事实**：
///   `set_attachments_locally_missing` 里只有一个 `write_tx`；它该被钉住的行为（点名的全降、
///   没点名的不许动、远端态不被顺手改、返回数不虚报）结在
///   `notera-store/tests/attachment_queue.rs::a_batched_demotion_moves_exactly_the_listed_rows`。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_sweep_demotes_at_most_its_cap_per_round_and_picks_the_rest_next() {
    let srv = TestServer::start(Backend::Mem).await;
    let url = srv.base_url();
    let blobs: Vec<Vec<u8>> = (0..3)
        .map(|i| (0..600).map(|k| (k % (i + 7) as usize) as u8).collect())
        .collect();

    // A：三条笔记、三张图，正文都引用了，图都真传上去。
    let a_dir = Tmp::new("cap-a");
    let a = boot(a_dir.path(), &url);
    a.sync_once().await.expect("A 入伙");
    let folder = a.default_folder_id().unwrap();
    let mut shas = Vec::new();
    for blob in blobs.iter() {
        let note = a.create_note(&folder, doc("有一张图的笔记")).unwrap();
        let id = notera_core::EntityId::parse(&note.id).unwrap();
        let sha = a
            .store()
            .attach_blob(&id, blob, "image/png", Some("shot.png"), "blk000002")
            .unwrap()
            .sha256;
        let head = a.store().get_note(&id).unwrap().unwrap();
        a.store()
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
        shas.push(sha);
    }
    a.sync_once().await.expect("A 推正文");
    let remote = a.remote_for_sync().await.unwrap().expect("A 有适配器");
    let round = a.run_attachment_round(&remote).await;
    assert_eq!(round.0, 3, "A 该把三张图都传上去：{round:?}");
    drop(a);

    // B：一轮把三张图都下回来，然后**整个目录没了**。
    let b_dir = Tmp::new("cap-b");
    let b = boot(b_dir.path(), &url);
    b.sync_once().await.expect("B 拉到正文");
    let remote = b.remote_for_sync().await.unwrap().expect("有适配器");
    let round = b.run_attachment_round(&remote).await;
    assert_eq!(round.1, 3, "B 第一轮该把三张图都下完：{round:?}");
    for sha in &shas {
        assert!(!blob_on_disk(&b, sha).is_empty(), "前置：{sha} 没落到本机");
    }
    for sha in &shas {
        std::fs::remove_file(b.store().blob_path(sha)).expect("删掉本机 blob");
    }

    // ① cap=2 的一轮只许认两条。多降一条就是"候选不分页"还在起作用。
    let demoted = b.sweep_lost_local_blobs(2);
    assert_eq!(demoted, 2, "cap=2 却降了 {demoted} 条：每轮的上界没起作用");
    let now_missing: Vec<&String> = shas
        .iter()
        .filter(|s| b.store().attachment_for_state(s).0 == "missing")
        .collect();
    assert_eq!(
        now_missing.len(),
        2,
        "降级条数与账上的 `missing` 对不上（降了却没人知道）"
    );
    assert_eq!(
        b.store().attachment_downloads(10).unwrap().len(),
        2,
        "降下来的两条必须当轮就进下载队列"
    );
    // 另一条此刻**还是** `available` —— 它没被这轮看到，也就没被顺手改判
    let still = shas
        .iter()
        .find(|s| b.store().attachment_for_state(s).0 == "available")
        .expect("第三条这一轮该还没被动到");
    assert_eq!(
        b.store().attachment_for_state(still).1,
        "present",
        "没被核对过的行，远端态被体检顺手改了"
    );

    // ② 下一轮补齐第三条（降过的行离开候选集，所以不会饿死后面那些）。
    assert_eq!(b.sweep_lost_local_blobs(2), 1, "第二轮该只补上剩下那一条");

    // ③ 三条都补回来，然后体检必须收手 —— 不许把已经修好的行继续当活。
    let remote = b.remote_for_sync().await.unwrap().expect("有适配器");
    let round = b.run_attachment_round(&remote).await;
    assert_eq!(
        round.1, 3,
        "降级出来的三条要在同一轮都被下载队列看见：{round:?}"
    );
    for (sha, blob) in shas.iter().zip(&blobs) {
        assert_eq!(
            blob_on_disk(&b, sha),
            blob.clone(),
            "重下回来的字节必须与源逐字节相同：{sha}"
        );
    }
    assert_eq!(
        b.sweep_lost_local_blobs(2),
        0,
        "三份都补齐了体检还在降级 —— 循环没断"
    );
    srv.stop().await;
}

// ———————————————————————————— 读侧不许把错字节画出来

/// 磁盘体检有一道明确的边界：它不会每轮把整库重哈希，所以"**长度分毫不差、内容被改坏**"
/// 这一种它看不见。读侧必须补上这一格 —— 否则用户看到的是一张**画得出来的错图**：
/// `attachment_data` 按 sha 查文件，把内容对不上号的字节当正文交给界面，而界面又按 sha
/// 缓存整个会话。那条 sha 明明不是那份字节，屏幕上却没有任何地方说它坏了。
///
/// 现在的判据：读的时候算一次哈希，对不上就报 `attachment_corrupt`，界面留占位。
/// 附件不阻塞正文（INV 已有），而这一条把"错得看不见"变成"缺得看得见"。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_same_length_local_corruption_is_refused_by_the_reader_not_drawn() {
    let srv = TestServer::start(Backend::Mem).await;
    let url = srv.base_url();
    let blob: Vec<u8> = (0..9000).map(|i| (i % 199) as u8).collect();
    let a_dir = Tmp::new("readhash-a");
    let sha = seed_source(a_dir.path(), &url, &blob).await;
    let b_dir = Tmp::new("readhash-b");
    let b = drain_target(b_dir.path(), &url, &blob, &sha).await;
    let healthy = ui_read(&b, &sha);
    assert!(
        healthy.bytes_len.is_some() && healthy.code.is_none(),
        "前置不成立：好字节本来就读不出来，那后面那条断言就没有意义：{}",
        healthy.describe()
    );

    // 同长度原地改坏：stat 看不出来，体检因此也不会降级它。
    let path = b.store().blob_path(&sha);
    let mut bad = blob.clone();
    for (i, byte) in bad.iter_mut().take(2048).step_by(64).enumerate() {
        *byte = byte.wrapping_add((7 + i) as u8);
    }
    assert_eq!(bad.len(), blob.len(), "要坏得只有哈希能发现");
    assert_ne!(bad, blob);
    std::fs::write(&path, &bad).expect("原地改坏本机 blob");

    let got = ui_read(&b, &sha);
    assert_eq!(
        got.code.as_deref(),
        Some("attachment_corrupt"),
        "内容对不上这个 sha，界面却拿到了能画出来的字节：{}",
        got.describe()
    );
    assert!(
        got.bytes_len.is_none(),
        "报错的同时还把字节带出去了：{}",
        got.describe()
    );
    assert_body_still_usable(&b, &sha);
    srv.stop().await;
}

// ————————————————————————————— 坏字节在没有替代时不许被销毁

/// 体检发现本机字节坏了要重下 —— 但**服务器也可能给不出替代**（那份对象被人在网页端删了）。
/// 那时候正确做法不是删掉本机这份然后两手空空，而是把它挪开留着：
/// 它虽然哈希不符（不是正文引用的那份），却是这台机器上最后一份现场，
/// 用户或恢复工具还可能从里面捞出可用内容。数据安全在这一格上的含义就是"不销毁无可替代之物"。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn corrupt_local_bytes_are_parked_not_destroyed_when_the_server_has_nothing() {
    let srv = TestServer::start(Backend::Mem).await;
    let url = srv.base_url();
    let blob: Vec<u8> = (0..9000).map(|i| (i % 211) as u8).collect();
    let a_dir = Tmp::new("park-a");
    let sha = seed_source(a_dir.path(), &url, &blob).await;
    let b_dir = Tmp::new("park-b");
    let b = drain_target(b_dir.path(), &url, &blob, &sha).await;

    // 本机这份被截断（长度与账不符，体检因此才会去复算哈希）。
    let path = b.store().blob_path(&sha);
    let mangled: Vec<u8> = blob[..4500].to_vec();
    std::fs::write(&path, &mangled).expect("截断本机 blob");
    drop(b);

    // 服务器上那份已经没有了：重下注定拿不到替代。
    srv.inject(Injection::status(format!("GET *{sha}"), 404))
        .await;
    let b2 = boot(b_dir.path(), &url);
    b2.sync_once().await.expect("B 拉到正文");
    let remote = b2.remote_for_sync().await.unwrap().expect("有适配器");
    let round = b2.run_attachment_round(&remote).await;
    assert_eq!(round.1, 0, "服务器没有这份，不该报下载成功：{round:?}");

    // ① 正式位置要腾出来（否则 ingest_blob 见"已存在"就不写新的）
    assert!(!path.exists(), "坏文件还占着正式位置，重下永远写不进去");
    // ② 但那份字节必须还在盘上，一个字节都不许被改动
    let survivors = parked_copies(&b2, &sha);
    assert_eq!(
        survivors.len(),
        1,
        "没有替代的时候，体检把本机最后一份坏字节销毁了"
    );
    assert_eq!(
        survivors[0], mangled,
        "留下的那份被改过了 —— 挪开可以，动内容不行"
    );
    // ③ 账要说实话：本机没有、服务器也没有（不是"available"那种谎）
    let (local, remote_state) = b2.store().attachment_for_state(&sha);
    assert_eq!(
        (local.as_str(), remote_state.as_str()),
        ("missing", "absent"),
        "状态位没跟上这次挪开+404"
    );
    // ④ 正文照旧可用（§8 收尾句）
    assert_body_still_usable(&b2, &sha);
    srv.stop().await;
}

/// 磁盘上为某个 sha 留着的坏拷贝（`<sha>.corrupt*`）。正式位置不在其中。
fn parked_copies(app: &App, sha: &str) -> Vec<Vec<u8>> {
    let path = app.store().blob_path(sha);
    let Some(dir) = path.parent() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| {
            let n = e.file_name().to_string_lossy().to_string();
            n.starts_with(sha) && n.contains(".corrupt")
        })
        .filter_map(|e| std::fs::read(e.path()).ok())
        .collect()
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

/// 这一轮里服务器收到过几条 MOVE。412 那两条测试的规则打在 `MOVE *` 上，
/// 而 MOVE 的请求路径其实是 tmp 名（不含 sha），所以只能靠"这一轮就一条 MOVE"来保证
/// 那个状态码确实落在我们想拒的那次搬运上 —— 否则将来附件轮里多出别的 MOVE，
/// 这两条会静默地拒错对象还照样绿。
fn move_requests(srv: &TestServer) -> Vec<String> {
    srv.request_log()
        .iter()
        .filter(|r| r.method == "MOVE")
        .map(|r| format!("#{} {} -> {}", r.seq, r.path, r.status))
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
    // ②' 服务器给的就是坏字节 —— 那不该变成每 20 s 重下一遍的悬案。
    //     判据：第二轮针对这个 sha **零请求**，且状态停在诚实的那对上（本机没有 + 远端报错）。
    srv.clear_log().await;
    let second = b.run_attachment_round(&remote).await;
    let again: Vec<String> = srv
        .request_log()
        .iter()
        .filter(|q| q.path.contains(&sha))
        .map(|q| format!("{} {} -> {}", q.seq, q.method, q.status))
        .collect();
    assert!(
        again.is_empty(),
        "为一个已经证明内容不符的对象每轮重下：{again:?}（第二轮 {second:?}）"
    );
    assert_eq!(
        b.store().attachment_for_state(&sha),
        ("missing".into(), "error".into()),
        "坏远端的状态位没落对：内容不符是**结论**，不是待办"
    );
    // ③ 正文继续可用；④ 界面读到的是具名失败而不是那张坏图。
    assert_body_still_usable(&b, &sha);
    let got = ui_read(&b, &sha);
    // 坏字节被拦在门外之后本机就没有这个文件；界面那条读路径必须报**具名**失败，
    // 而不是给出一段能画出来的字节（那等于把坏图当正常图缓存整个会话）。
    assert_eq!(
        got.code.as_deref(),
        Some("attachment_missing"),
        "坏附件竟然能被界面读出来：{}",
        got.describe()
    );
    assert!(
        got.bytes_len.is_none(),
        "拒绝坏附件的同时还回了字节：{}",
        got.describe()
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
    // 精确状态对而不是 `assert_ne!`：`attachment_for_state` 对**根本不存在的行**也返回
    // ("absent","absent")，只判"不等于 available"的话，"整行被弄没了"这种回归也能蒙混过关。
    assert_eq!(
        b.store().attachment_for_state(&sha),
        ("missing".into(), "unknown".into()),
        "断线的一轮之后账上不许说这份附件可用，也不许顺手改掉远端态"
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

    let text_budget = (budget / 4).max(std::time::Duration::from_secs(10));
    // 两轮的耗时都从**同一个起点**量，这样"文本轮先跑完、附件轮还在里面"才是可证的并发，
    // 而不是"两个数字看起来都不大"。
    let t0 = std::time::Instant::now();
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
        (started.elapsed(), t0.elapsed(), stats)
    });
    let att_done = t0.elapsed();
    let (text_took, text_done, text) = text;

    // 附件轮必须在**产品自己的请求预算**内放手：不是被外层 timeout 救下来的。
    // 实测落点 45.0 s ≈ `Timeouts::per_request`；这里只要求它落在 [预算/2, 预算×2] 之内，
    // 这样把预算调小/调大都不用改测试，而"永不放手"（旧疑点）会直接红。
    assert!(
        att_done >= budget / 2 && att_done <= budget * 2,
        "挂死的附件请求没有在自己的 per_request（{budget:?}）之内收场：实测 {att_done:?}；         要么超时预算形同虚设，要么这一轮压根没被挂住"
    );
    // 并发凭据：文本轮结束的那一刻，附件轮还没返回（两者共用同一个起点 t0）。
    assert!(
        text_done < att_done,
        "文本轮结束时附件轮也已经结束 —— 两轮其实是先后跑的，§13 的隔离没被证明         （text_done={text_done:?} att_done={att_done:?}）"
    );
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
    srv.clear_log().await;
    srv.inject(Injection::partial_write("MOVE *", 412)).await;
    let round = a.run_attachment_round(&remote).await;
    srv.inject(Injection::none()).await;
    assert_eq!(
        move_requests(&srv).len(),
        1,
        "这一轮只该搬运那一个附件对象，否则 412 未必落在我们想指的那次 MOVE 上：{:?}",
        move_requests(&srv)
    );

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
    srv.clear_log().await;
    srv.inject(Injection::status("MOVE *", 412)).await;
    let round = a.run_attachment_round(&remote).await;
    srv.inject(Injection::none()).await;
    assert_eq!(
        move_requests(&srv).len(),
        1,
        "这一轮只该搬运那一个附件对象，否则 412 未必落在我们想指的那次 MOVE 上：{:?}",
        move_requests(&srv)
    );

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
// ————————————————— 上传侧的两种中断（§27 台账里最后两处"只有声称"）———
/// 这一轮里服务器收到过几条某个方法的请求（`-> 0` 表示没有响应 = 被掐）。
fn requests_for(srv: &TestServer, method: &str) -> Vec<String> {
    srv.request_log()
        .iter()
        .filter(|r| r.method == method)
        .map(|r| format!("#{} {} -> {}", r.seq, r.path, r.status))
        .collect()
}

/// §27 的「上传中断」此前**只有文档声称**：TEST-PLAN 的 FT-ATT-04 那行一直写着
/// `FAIL(abort,target=attachments/**)`，而全仓从没对附件上传打过一条 abort —— 半上传
/// （`truncate-upload`）那条 FT-ATT-15 打的是"服务器只读到半份 body"，不是"连接被掐"。
///
/// 切点选在**发布那一步**（MOVE），因为它比"body 读到一半"更阴：暂存对象已经整份写到
/// 服务器上了，客户端手里只剩一个"不知道成没成"的事实。内容寻址最怕的就是把"不知道"
/// 记成"有" —— 那会让对面设备永远去取一份服务器上并不存在的东西。
///
/// 坏的那一段咬住四件事：本轮不算成功、算一次失败、服务器上不许出现含该 sha 的正式对象、
/// **本机那份独家字节与 `local_state` 都不许被动到**（传输失败与本机内容无关）。
/// 恢复那一段要求重试真能落成，且最终**恰好一份**、内容就是要传的那份。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_severed_publish_step_rolls_the_upload_back_and_the_retry_lands_once() {
    let srv = TestServer::start(Backend::Mem).await;
    let url = srv.base_url();
    let blob: Vec<u8> = (0..15_000).map(|i| (i % 197) as u8).collect();
    let a_dir = Tmp::new("sevmove-a");
    let (a, sha) = seed_ready_to_upload(a_dir.path(), &url, &blob).await;
    let remote = a.remote_for_sync().await.unwrap().expect("有适配器");

    srv.clear_log().await;
    srv.inject(Injection::abort_on("MOVE *")).await;
    let round = a.run_attachment_round(&remote).await;
    srv.inject(Injection::none()).await;

    // 前置：本轮恰好一次 PUT（暂存整份写完了）+ 恰好一次 MOVE，且那条 MOVE 没收到响应。
    // 这两条都在**判产品之前**，否则"注入没打中"会被读成"产品没问题"。
    assert_eq!(
        requests_for(&srv, "PUT").len(),
        1,
        "附件上传该先整份写一份暂存，本轮 PUT 数不是 1：{:?}",
        requests_for(&srv, "PUT")
    );
    let moves = requests_for(&srv, "MOVE");
    assert_eq!(moves.len(), 1, "要掐的必须是那一次发布搬运：{moves:?}");
    assert!(
        moves[0].ends_with("-> 0"),
        "那次 MOVE 拿到了响应码，说明注入压根没打中，这条测的就不是「上传中断」：{moves:?}"
    );

    assert_eq!(round.0, 0, "发布被掐却报上传成功：{round:?}");
    assert_eq!(
        round.2, 1,
        "掐掉的这一次没被算进失败：账目对不上就等于没人知道它坏了（{round:?}）"
    );
    assert!(
        served_blobs(&srv, &sha).is_empty(),
        "上传没成却在服务器上落成了含该 sha 的正式对象：{:?}",
        served_blobs(&srv, &sha)
    );
    assert_eq!(
        blob_on_disk(&a, &sha),
        blob,
        "上传失败把本机那份字节弄没了 —— 那是这台机器上唯一的一份"
    );
    assert_eq!(
        a.store().attachment_for_state(&sha),
        ("available".into(), "unknown".into()),
        "「不知道服务器有没有」被写成了一侧的结论"
    );
    assert_body_still_usable(&a, &sha);

    // 恢复：下一轮真能传上去，最终恰好一份，内容就是要传的那份。
    let round2 = a.run_attachment_round(&remote).await;
    assert_eq!(round2.0, 1, "被掐一次之后就再也传不上去：{round2:?}");
    let obj = served_blob(&srv, &sha);
    assert_eq!(
        obj.get("sha256").and_then(|v| v.as_str()),
        Some(format!("sha256:{sha}").as_str()),
        "重试落成对象的内容不是我们要传的那份：{obj}"
    );
    assert_eq!(
        a.store().attachment_for_state(&sha),
        ("available".into(), "present".into()),
        "复读验过之后两半状态才允许同时落对"
    );
    srv.stop().await;
}

/// 上传那条路上收到 **404**（§27 台账原话："上传侧收到 404 的形态未注入"）。真实形态有
/// 好几种：网关把 MOVE 的目标判成"没有这个集合"、暂存对象被服务器侧的清理步骤先回收了、
/// 反代把未知动词转成了一个裸 404。
///
/// 关键是**这个 404 不是关于远端事实的结论**。下载侧那个 404 才是结论（服务器明确说没有
/// ⇒ `absent`，就此收手，FT-ATT-16）。上传侧这个 404 只说明"我们这次搬运没成"：把它读成
/// `present` 会让对面永远等一份不存在的东西，读成 `absent` 又是凭一次失败的请求下一个
/// 关于服务器存在性的判断。今天两条都不该发生 —— 远端态只能由**读回内容比对**（上传侧）
/// 或**下载那次的 404**（下载侧）来写，所以断言打在这对精确状态**保持不动**上：
/// `(available, unknown)`。顺带钉住"别把暂存留成垃圾"：那次 404 之后要发过 DELETE。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_not_found_answer_on_the_publish_step_is_not_read_as_a_conclusion() {
    let srv = TestServer::start(Backend::Mem).await;
    let url = srv.base_url();
    let blob: Vec<u8> = (0..15_000).map(|i| (i % 173) as u8).collect();
    let a_dir = Tmp::new("move404-a");
    let (a, sha) = seed_ready_to_upload(a_dir.path(), &url, &blob).await;
    let remote = a.remote_for_sync().await.unwrap().expect("有适配器");

    srv.clear_log().await;
    srv.inject(Injection::status("MOVE *", 404)).await;
    let round = a.run_attachment_round(&remote).await;
    srv.inject(Injection::none()).await;

    let moves = requests_for(&srv, "MOVE");
    assert_eq!(moves.len(), 1, "404 必须落在那一次发布搬运上：{moves:?}");
    assert!(
        moves[0].ends_with("-> 404"),
        "那条 MOVE 收到的不是 404，这条测的就不是「上传侧收到 404」：{moves:?}"
    );

    assert_eq!(round.0, 0, "服务器回了 404 却算成上传成功：{round:?}");
    assert_eq!(round.2, 1, "这次失败没被记账：{round:?}");
    assert!(
        served_blobs(&srv, &sha).is_empty(),
        "被 404 拒掉的一轮却落成了正式对象：{:?}",
        served_blobs(&srv, &sha)
    );
    assert_eq!(
        a.store().attachment_for_state(&sha),
        ("available".into(), "unknown".into()),
        "一次失败的 MOVE 被读成了关于远端的结论（present 与 absent 都不许）"
    );
    assert!(
        !requests_for(&srv, "DELETE").is_empty(),
        "搬完之后那份暂存没去清 —— 服务器上一堆 tmp 垃圾是 §11.3 明确不要的形态"
    );
    assert_body_still_usable(&a, &sha);

    // 404 是"这次没成"，不是"这条坏了"：撤掉注入后下一轮必须真能传上去。
    let round2 = a.run_attachment_round(&remote).await;
    assert_eq!(round2.0, 1, "被 404 拒过一次之后就再也传不上去：{round2:?}");
    assert_eq!(served_blobs(&srv, &sha).len(), 1, "最终恰好一份");
    assert_eq!(
        a.store().attachment_for_state(&sha),
        ("available".into(), "present".into()),
        "重试成功后两半状态没落对"
    );
    srv.stop().await;
}

/// 上传的**最后一步**（复读校验）被掐 —— 这一形是最容易漏的一格，因为它前面全都成功了：
/// 暂存整份写完、MOVE 也落成了正式对象，唯独"读回来对一次哈希"这一步连接被掐。
///
/// 为什么值得单独一条：这时服务器上**确实有**那份字节，所以"报成功"看着无害 —— 但客户端
/// 手里没有任何**证据**（内容没对过）。SYNC-PROTOCOL §13 那条规矩就是这个形状：
/// **present 必须出自一次读回的内容比对**。当场记 present 的话，一份被中间设备改过的对象
/// 会被这台设备当成"我已经确认过远端有了"，而它下面那一轮、对面那一台都会信这个数。
///
/// 恢复那一段顺便把 §13 承认的那个**例外**跑成门禁：第二轮 `has_attachment` 的 HEAD 命中
/// 就直接算过 —— 那个 present 是**借来的**（没有内容比对）。它今天可接受的唯一理由是消费侧
/// 下载时会复验（FT-ATT-13 已证那条路会拦下坏字节并记 `error`）；这条测试把"借来的 present"
/// 与"最终恰好一份对象"钉住，同时把 G5 的成因留在文档里（服务器那份若坏了，本机不会再传）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_severed_read_back_after_a_landed_upload_records_no_present() {
    let srv = TestServer::start(Backend::Mem).await;
    let url = srv.base_url();
    let blob: Vec<u8> = (0..13_000).map(|i| (i % 211) as u8).collect();
    let a_dir = Tmp::new("sevread-a");
    let (a, sha) = seed_ready_to_upload(a_dir.path(), &url, &blob).await;
    let remote = a.remote_for_sync().await.unwrap().expect("有适配器");

    srv.clear_log().await;
    srv.inject(Injection::abort_on(format!("GET *{sha}"))).await;
    let round = a.run_attachment_round(&remote).await;
    srv.inject(Injection::none()).await;

    // 前置：暂存与发布都做完了，只有复读被掐 —— PUT 一条 + MOVE 一条（带回 2xx）+ 一条无响应的 GET。
    let moves = requests_for(&srv, "MOVE");
    assert_eq!(moves.len(), 1, "要断的是那一次发布之后的复读：{moves:?}");
    assert!(
        !moves[0].ends_with("-> 0"),
        "MOVE 本身被掐了，这条测的就不是「复读被掐」：{moves:?}"
    );
    let gets: Vec<String> = srv
        .request_log()
        .iter()
        .filter(|r| r.method == "GET" && r.path.contains(&sha))
        .map(|r| format!("#{} {} -> {}", r.seq, r.path, r.status))
        .collect();
    assert!(
        gets.iter().any(|g| g.ends_with("-> 0")),
        "针对这个 sha 的 GET 没被掐断，注入没打中：{gets:?}"
    );

    assert_eq!(
        round.0, 0,
        "没读到回包却把这次上传算成了成功（账要等验过才能落对）：{round:?}"
    );
    assert_eq!(round.2, 1, "这次失败没被记账：{round:?}");
    assert_eq!(
        a.store().attachment_for_state(&sha),
        ("available".into(), "unknown".into()),
        "内容没比对过就记 present —— 对面设备会拿这个数当「服务器上有，而且就是这份字节」"
    );
    // 服务器上那份字节**确实落成了**（MOVE 成功了），这正是这一形微妙的地方。
    let landed = served_blobs(&srv, &sha);
    assert_eq!(
        landed.len(),
        1,
        "前置：MOVE 该已经把对象落成，才有「复读被掐」这一形：{landed:?}"
    );
    assert_body_still_usable(&a, &sha);

    // 恢复：第二轮走 §13 那个 HEAD 例外 —— 不许重复落第二份对象。
    let round2 = a.run_attachment_round(&remote).await;
    assert_eq!(round2.0, 1, "复读被掐过一次之后就再也落不成：{round2:?}");
    assert_eq!(
        served_blobs(&srv, &sha).len(),
        1,
        "重试把同一份内容落成了第二个对象：{:?}",
        served_blobs(&srv, &sha)
    );
    assert_eq!(
        served_blob(&srv, &sha)
            .get("sha256")
            .and_then(|v| v.as_str()),
        Some(format!("sha256:{sha}").as_str()),
        "服务器上那份内容不是我们要传的字节"
    );
    assert_eq!(
        a.store().attachment_for_state(&sha).1,
        "present",
        "HEAD 命中那条例外今天仍然当成功用（它的凭据是消费侧复验，见 §13 与本条上面的注释）"
    );
    srv.stop().await;
}
