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
