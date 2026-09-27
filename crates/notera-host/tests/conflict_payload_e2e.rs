//! P11（删除 vs 修改）的**两台真设备**断言：卡片要求用户二选一，另一版的正文必须真在他眼前。
//!
//! 补的是 CF-13 欠的那一半。存储层与 `preview_text` 的回退各自有单测（
//! `notera-store/tests/conflict_payload.rs`），但"引擎发出 `ConflictPayload` →
//! 宿主登记进冲突行"这条边只有编译期覆盖 —— 而它正是这条链上最容易悄悄断的一节：
//! 断了之后卡片还在、按钮还在、右栏却永远是一串哈希，界面看起来"一切都好"。
//!
//! 三条判据按老注释里的三个后果一一对上：
//!   1. 右栏读回来的是**对面那一版**的文字，不是本机这份（当年左右两栏是同一份内容）；
//!   2. 载荷真的落在冲突行上（`remote_wire` 非 NULL）；
//!   3. 引擎**没有**替用户选：B 本机正文一个字都没被改掉，笔记也还在正常列表里。
//!
//! 跑法：`cargo test -p notera-host --test conflict_payload_e2e`

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use notera_host::commands::AccountDraftCmd;
use notera_host::App;
use notera_test_webdav::{Backend, TestServer};
use serde_json::json;

const SECRET: &str = "sup3r-s3cr3t";
const REMOTE_TEXT: &str = "服务器那一版：报销单扫描件说明";
const LOCAL_TEXT: &str = "本机改过的一版：加了一行金额";
static SEQ: AtomicUsize = AtomicUsize::new(0);

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("notera-p11-{tag}-{}-{n}", std::process::id()));
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

struct Device {
    app: App,
    _dir: Tmp,
}

impl Device {
    fn boot(tag: &str, base_url: &str) -> Self {
        let dir = Tmp::new(tag);
        let app = App::boot(dir.path()).expect("核心启动");
        std::env::set_var("NOTERA_DEV_WEBDAV_SECRET", SECRET);
        let draft: AccountDraftCmd = serde_json::from_value(json!({
            "label": "p11", "baseUrl": base_url, "username": "notera-test",
        }))
        .unwrap();
        app.configure_account(draft).expect("配置账户");
        Self { app, _dir: dir }
    }

    /// 推到本机没有欠账为止（拉的一侧本来就干净，所以只看 pushed 侧的账）。
    async fn settle(&self, cap: usize) {
        for _ in 0..cap {
            self.app.sync_once().await.expect("一轮同步");
            let st = self.app.store().stats().unwrap();
            if st.dirty_notes == 0 && st.outbox_pending == 0 {
                return;
            }
        }
    }
}

fn doc(text: &str) -> serde_json::Value {
    json!({ "v": 1, "content": [{ "id": "blk000001", "type": "paragraph", "content": [{ "text": text }] }] })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_deleted_version_is_readable_on_the_device_that_kept_editing_it() {
    let dav = Tmp::new("dav");
    let srv = TestServer::start(Backend::Fs(dav.path().to_path_buf())).await;
    let url = srv.base_url();

    // A 建一条笔记并公告
    let a = Device::boot("a", &url);
    a.app.sync_once().await.expect("A 入伙");
    let folder = a.app.default_folder_id().unwrap();
    let created = a
        .app
        .create_note(&folder, doc(REMOTE_TEXT))
        .expect("A 建笔记");
    let id = notera_core::EntityId::parse(&created.id).expect("DTO 的 id 该能解析回来");
    a.settle(8).await;

    // B 入伙，把这一条拉下来（B 从此是"知道这一版"的设备）
    let b = Device::boot("b", &url);
    b.settle(8).await;
    let b_note = b
        .app
        .store()
        .get_note(&id)
        .expect("B 该读得到这条")
        .expect("B 库里必须有这条笔记");

    // A 删除并公告 → 服务器上是"软删、未 purge"的那一版
    a.app.store().delete_note(&id).expect("A 删除");
    a.settle(8).await;

    // B 在**不知道 A 删了**的情况下继续编辑本机这一条 → P11 的现场
    b.app
        .edit_note(&id, doc(LOCAL_TEXT), b_note.rev)
        .expect("B 编辑");

    // B 跑一轮：判成"删除 vs 修改"，引擎只登记、不采纳
    b.app.sync_once().await.expect("B 的一轮");

    let rows = b.app.store().open_conflicts().expect("open_conflicts");
    let row = rows
        .iter()
        .find(|r| r.id == id)
        .unwrap_or_else(|| panic!("B 该有一条 P11 未裁决冲突，实际：{rows:?}"));

    // ② 载荷真的落到了冲突行上（这条边断了就会在这里红）
    let wire = row.remote_wire.clone().unwrap_or_else(|| {
        panic!(
            "引擎取回的那一版没登记到冲突行上（remote_rev={}）：这条链断了，卡片右栏只能给哈希",
            row.remote_rev.get()
        )
    });
    assert!(
        wire.contains(REMOTE_TEXT),
        "登记的不是对面那一版的字节：{wire}"
    );

    // ① 面板拿到的正是"对面那一版"，而且不是本机这份的复制。
    //    走卡片的 remote_preview —— 绝不能拿 (noteId, remoteRev) 去查本机历史：
    //    rev 是各设备自己的编号，同号常是另一份内容（第一版就是这么错的）。
    let cards = b.app.open_conflicts().expect("卡片列表");
    let card = cards
        .iter()
        .find(|c| c.note_id == id.to_string())
        .unwrap_or_else(|| panic!("B 的卡片该在面板上：{cards:?}"));
    let remote_view = card.remote_preview.clone().unwrap_or_else(|| {
        panic!(
            "卡片没有对面那一版可读（remote_rev={}）—— 用户就在被要求凭哈希做决定",
            card.remote_rev
        )
    });
    assert!(
        remote_view.contains(REMOTE_TEXT),
        "右栏没显示被删那一版：{remote_view}"
    );
    assert!(
        !remote_view.contains(LOCAL_TEXT),
        "右栏拿本机内容冒充了对面那一版（左右两栏同一份就是当年那个 bug）：{remote_view}"
    );

    // ③ 引擎没替用户选：本机正文没被改掉，笔记也还在（P11 只登记不采纳）
    let still = b
        .app
        .store()
        .get_note(&id)
        .expect("读回")
        .expect("笔记必须还在");
    let local_text = notera_richtext::extract(
        &notera_richtext::parse_from_value(&still.doc.clone()).expect("本机 doc 合法"),
    )
    .plain_text;
    assert!(
        local_text.contains(LOCAL_TEXT),
        "本机那一版被引擎改掉了：{local_text}"
    );

    // 面板的左栏（本机那一版）也必须还读得到，两栏才真的并排得起来
    let local_view = b
        .app
        .preview_text(&id.to_string(), still.rev.get())
        .expect("本机这一版该读得回来");
    assert!(local_view.contains(LOCAL_TEXT), "左栏空了：{local_view}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_conflict_without_a_payload_still_stays_in_the_inbox() {
    // 取料失败不能把冲突一起弄没（§39 不许静默降级，§40 不许假装成功）。
    // 这里不制造网络故障，而是钉住更根本的一条：**冲突在册与载荷是两件事** ——
    // 载荷是 None 时，卡片仍在、内容仍是本机那一版。
    let dav = Tmp::new("dav2");
    let srv = TestServer::start(Backend::Fs(dav.path().to_path_buf())).await;
    let url = srv.base_url();
    let a = Device::boot("c", &url);
    a.app.sync_once().await.expect("入伙");
    let folder = a.app.default_folder_id().unwrap();
    let created = a.app.create_note(&folder, doc("原始那一版")).unwrap();
    let id = notera_core::EntityId::parse(&created.id).expect("DTO 的 id 该能解析回来");
    a.settle(8).await;
    let b = Device::boot("d", &url);
    b.settle(8).await;
    let head = b.app.store().get_note(&id).unwrap().unwrap();
    a.app.store().delete_note(&id).unwrap();
    a.settle(8).await;
    b.app.edit_note(&id, doc("我又改了"), head.rev).unwrap();

    // 手工登记一条没有载荷的冲突（等价于"这一轮没取回来"）：册子上必须在
    b.app
        .store()
        .record_conflict(&notera_store::ConflictRecord {
            account_id: notera_store::LOCAL_ACCOUNT_ID.into(),
            kind: notera_core::EntityKind::Note,
            id: id.clone(),
            base_rev: notera_core::Rev(1),
            local_rev: notera_core::Rev(3),
            remote_rev: notera_core::Rev(2),
            local_hash: "aaaa".into(),
            remote_hash: "bbbb".into(),
            auto_merged: false,
            copy_note_id: None,
        })
        .expect("登记冲突");
    let rows = b.app.store().open_conflicts().unwrap();
    assert!(
        rows.iter().any(|r| r.id == id && r.remote_wire.is_none()),
        "没有载荷的冲突该照旧在册并留 None：{rows:?}"
    );
    assert_eq!(
        b.app
            .open_conflicts()
            .unwrap()
            .into_iter()
            .find(|c| c.note_id == id.to_string())
            .expect("卡片在面板上")
            .remote_preview,
        None,
        "没取回来时卡片不许带任何「对面那一版」，界面才老实说没取到"
    );
}

/// §5.2「远端永久删除 vs 本机从未上传过的编辑」的两台真设备断言。
///
/// 协议确实选了"永久删除传播优先"（`plan.rs` 的 P12/P13），但那句话**只有在
/// 本机那一版先被留住的前提下**才成立。留不住的话，一台设备误按"彻底删除"就能
/// 吃掉另一台从未上传的编辑 —— 那是数据安全那一档的事，比同步正确性还靠前。
///
/// 判据照 §5.2 原文的三条：① 进收件箱（有一条未裁决冲突）；② 本机内容**先完整保留**；
/// ③ 传播不能覆盖未同步的本地写入（没留住又不报冲突 = 静默丢失）。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_remote_purge_never_eats_an_edit_that_never_left_the_device() {
    const PURGE_LOCAL: &str = "B 这台改了、一次都没上传过的那一版";
    let dav = Tmp::new("purge-dav");
    let srv = TestServer::start(Backend::Fs(dav.path().to_path_buf())).await;
    let url = srv.base_url();

    let a = Device::boot("purge-a", &url);
    a.app.sync_once().await.expect("A 入伙");
    let folder = a.app.default_folder_id().unwrap();
    let created = a
        .app
        .create_note(&folder, doc("A 建的、后来被彻底删除的那条"))
        .expect("A 建笔记");
    let id = notera_core::EntityId::parse(&created.id).expect("id");
    a.settle(8).await;

    let b = Device::boot("purge-b", &url);
    b.settle(8).await;
    let head = b.app.store().get_note(&id).unwrap().expect("B 拉到了这条");

    // B 改了但**一轮都不跑**（服务器上完全不知道有这一版）
    b.app
        .edit_note(&id, doc(PURGE_LOCAL), head.rev)
        .expect("B 编辑");

    // A 那边彻底删除（purge）并公告
    a.app.store().purge_note(&id).expect("A 永久删除");
    a.settle(8).await;

    // B 这一轮才撞上远端的 purged 墓碑
    b.app.sync_once().await.expect("B 的一轮");

    let text_of = |app: &App, ent: &notera_core::EntityId| -> String {
        match app.store().get_note(ent).ok().flatten() {
            Some(n) => {
                notera_richtext::extract(
                    &notera_richtext::parse_from_value(&n.doc).unwrap_or_default(),
                )
                .plain_text
            }
            None => String::new(),
        }
    };

    let cards = b.app.open_conflicts().expect("卡片列表");
    let card = cards.iter().find(|c| c.note_id == id.to_string());
    // ② 本机那一版还在不在：正文本身，或冲突卡片带来的那份副本
    let kept_live = text_of(&b.app, &id).contains(PURGE_LOCAL);
    let kept_copy = card
        .and_then(|c| c.copy_note_id.as_ref())
        .and_then(|s| notera_core::EntityId::parse(s).ok())
        .map(|cid| text_of(&b.app, &cid).contains(PURGE_LOCAL))
        .unwrap_or(false);

    assert!(
        card.is_some(),
        "①§5.2 要求这种处境进收件箱，B 的未裁决冲突里却没有它（卡片：{cards:?}）"
    );
    assert!(
        kept_live || kept_copy,
        "③静默丢失：远端的 purged 墓碑把 B 从未上传的那一版吃掉了 —— 正文没了，\
         卡片也没带副本（卡片 {:?}）。永久删除的传播不许覆盖未同步的本地写入。",
        card
    );
}

/// 用户在 P11 卡片上按"用服务器那一版替换"（对面是删除）之后，删除必须**真的发生并传播**。
///
/// 为什么单独一条：`Store::resolve_conflict` 只是把卡片改成 `state='resolved'`（记账），
/// 主机侧对 `"remote"` 这一支不做任何采纳动作 —— 于是"接受对面那一条删除"这个决定
/// 落地后：笔记还在本机正常列表里，而本机那个未同步的 head 下一轮带更高 rev 推上去，
/// **把对面已经确认的删除又覆盖回来**。用户按了按钮、面板说处理完了，实际什么都没发生，
/// 还顺手替别人撤销了删除（主指令禁止的"静默覆盖"正对着这条）。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn accepting_the_remote_delete_actually_deletes_and_propagates_it() {
    let dav = Tmp::new("acc-dav");
    let srv = TestServer::start(Backend::Fs(dav.path().to_path_buf())).await;
    let url = srv.base_url();

    let a = Device::boot("acc-a", &url);
    a.app.sync_once().await.expect("A 入伙");
    let folder = a.app.default_folder_id().unwrap();
    let created = a
        .app
        .create_note(&folder, doc(REMOTE_TEXT))
        .expect("A 建笔记");
    let id = notera_core::EntityId::parse(&created.id).expect("id");
    a.settle(8).await;

    let b = Device::boot("acc-b", &url);
    b.settle(8).await;
    let head = b.app.store().get_note(&id).unwrap().expect("B 有这条");

    a.app.store().delete_note(&id).expect("A 删除");
    a.settle(8).await;
    b.app
        .edit_note(&id, doc(LOCAL_TEXT), head.rev)
        .expect("B 改（不上行）");
    b.app.sync_once().await.expect("B 的一轮");

    let cards = b.app.open_conflicts().expect("卡片");
    let card = cards
        .iter()
        .find(|c| c.note_id == id.to_string())
        .unwrap_or_else(|| panic!("B 该有这条 P11 卡片：{cards:?}"));

    // 用户决定：接受对面那一版（= 接受这条删除）
    b.app
        .resolve_conflict(notera_host::commands::ResolveConflictCmd {
            id: card.id,
            action: "replaceWithRemote".into(),
        })
        .expect("resolve_conflict 应当接受这个动词");

    let after = b.app.store().get_note(&id).ok().flatten();
    assert!(
        after
            .as_ref()
            .map(|n| n.deleted_at.is_some())
            .unwrap_or(true),
        "按了\u{201c}用服务器那一版替换\u{201d}（对面是删除）之后，B 这条笔记还活着：\
         卡片被关掉就等于处理完了？deleted_at={:?}",
        after.as_ref().map(|n| n.deleted_at.clone())
    );

    // 而且不能反手替别人撤销删除：两边再各追一轮，服务器上必须仍是"已删除"
    b.settle(8).await;
    a.settle(8).await;
    let a_note = a.app.store().get_note(&id).ok().flatten();
    assert!(
        a_note
            .as_ref()
            .map(|n| n.deleted_at.is_some())
            .unwrap_or(true),
        "A 那台的删除被 B 的旧 head 覆盖回来了：反复活 —— 用户接受删除反而让笔记在两台都复活"
    );
}

// ——— 浏览器 lane 的留档现场 ———

fn boot_at(dir: &Path, base_url: &str) -> App {
    std::fs::create_dir_all(dir).expect("建数据目录");
    let app = App::boot(dir).expect("核心启动");
    std::env::set_var("NOTERA_DEV_WEBDAV_SECRET", SECRET);
    let draft: AccountDraftCmd = serde_json::from_value(json!({
        "label": "p11-lane", "baseUrl": base_url, "username": "notera-test",
    }))
    .unwrap();
    app.configure_account(draft).expect("配置账户");
    app
}

async fn settle_lane(app: &App, cap: usize) {
    for _ in 0..cap {
        app.sync_once().await.expect("一轮同步");
        let st = app.store().stats().unwrap();
        if st.dirty_notes == 0 && st.outbox_pending == 0 {
            return;
        }
    }
}

/// 把上面那条真 P11 现场留在盘上，交给**真浏览器**去读：
/// `scripts/verify-p11-panel.mjs` 拿 `device-b` 起桥，断言右栏渲染出来的就是
/// 对面那一版的文字。上面两个测试钉的是"卡片 DTO 里有"，管不到界面有没有真把它
/// 画出来 —— 那一格只有浏览器能给。
///
/// 为什么不在 lane 里现场同步：本机每 25 秒自动推一轮，"本机脏改"和"对面的删除
/// 公告"谁先到服务器是不确定的（这条边已由上面的真设备测试钉住，不需要重复赌）。
/// 渲染要的是确定现场，所以现场由这里产出，界面只负责看。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "留档夹具：由 scripts/verify-p11-panel.mjs 调用"]
async fn leave_a_p11_scene_on_disk_for_the_ui_lane() {
    let root = std::env::var("NOTERA_P11_SCENE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("D:/code/Notes/.logs/p11-lane"));
    let _ = std::fs::remove_dir_all(&root);
    let srv = TestServer::start(Backend::Fs(root.join("dav"))).await;
    let url = srv.base_url();

    let a = boot_at(&root.join("device-a"), &url);
    a.sync_once().await.expect("A 入伙");
    let folder = a.default_folder_id().expect("默认文件夹");
    let first = a
        .create_note(&folder, doc(REMOTE_TEXT))
        .expect("A 建第一条");
    let second = a
        .create_note(&folder, doc("第二条：对面那一版本轮没取回来"))
        .expect("A 建第二条");
    let id1 = notera_core::EntityId::parse(&first.id).expect("id");
    let id2 = notera_core::EntityId::parse(&second.id).expect("id");
    settle_lane(&a, 8).await;

    let b = boot_at(&root.join("device-b"), &url);
    settle_lane(&b, 8).await;
    let head1 = b.store().get_note(&id1).unwrap().expect("B 拉到了第一条");
    let head2 = b.store().get_note(&id2).unwrap().expect("B 拉到了第二条");

    // 第一条：走完整现场 —— 服务器上是删除公告，B 本机是未推送的修改。
    a.store().delete_note(&id1).expect("A 删第一条");
    settle_lane(&a, 8).await;
    b.edit_note(&id1, doc(LOCAL_TEXT), head1.rev)
        .expect("B 改第一条");
    b.sync_once().await.expect("B 的一轮");

    // 第二条：同样的处境，但 B 这一轮**没去取那一版**。这里用引擎登记冲突用的同一个
    // 存储入口（`record_conflict` 写册子、`ConflictPayload` 才往同一行上挂载荷），
    // 造出"在册但无载荷"这一格 —— 与上面那个测试同一口径。
    a.store().delete_note(&id2).expect("A 删第二条");
    settle_lane(&a, 8).await;
    b.edit_note(&id2, doc("第二条：本机又改了一次"), head2.rev)
        .expect("B 改第二条");
    let cur2 = b.store().get_note(&id2).unwrap().expect("B 的第二条还在");
    b.store()
        .record_conflict(&notera_store::ConflictRecord {
            account_id: notera_store::LOCAL_ACCOUNT_ID.into(),
            kind: notera_core::EntityKind::Note,
            id: id2.clone(),
            base_rev: head2.rev,
            local_rev: cur2.rev,
            remote_rev: notera_core::Rev(2),
            local_hash: "lane-local".into(),
            remote_hash: "lane-remote".into(),
            auto_merged: false,
            copy_note_id: None,
        })
        .expect("登记第二条的冲突");

    let cards = b.open_conflicts().expect("卡片列表");
    let c1 = cards
        .iter()
        .find(|c| c.note_id == id1.to_string())
        .unwrap_or_else(|| panic!("第一条该有卡片：{cards:?}"));
    let c2 = cards
        .iter()
        .find(|c| c.note_id == id2.to_string())
        .unwrap_or_else(|| panic!("第二条该有卡片：{cards:?}"));
    let remote1 = c1
        .remote_preview
        .clone()
        .unwrap_or_else(|| panic!("夹具自己就没造出带载荷的现场，lane 无从断言"));
    assert!(
        remote1.contains(REMOTE_TEXT) && !remote1.contains(LOCAL_TEXT),
        "夹具造的现场就不对：{remote1}"
    );
    assert!(
        c2.remote_preview.is_none(),
        "第二条不该带载荷：{:?}",
        c2.remote_preview
    );

    let expect = json!({
        "dirB": root.join("device-b").display().to_string(),
        "withPayload": {
            "conflictId": c1.id, "noteId": id1.to_string(),
            "localText": LOCAL_TEXT, "remoteText": remote1,
        },
        "noPayload": { "conflictId": c2.id, "noteId": id2.to_string() },
    });
    std::fs::write(root.join("expect.json"), expect.to_string()).expect("写 expect.json");
    println!("{expect}");
}
