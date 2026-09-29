//! CONFLICT-RESOLUTION §1.2 那一格「两侧最终内容相同 → 收敛（`rev := R.rev`）」的机器证据。
//!
//! 为什么单独一条：这是"看起来最不该出事"的那一格 —— 内容都一样，凭什么算冲突？而它同时是
//! **最容易被写成永久待同步**的一格：规划层（`plan.rs` 的 P7）遇到"两边都改过而内容哈希相同"
//! 时给的是 `NoOp`，也就是**既不推也不拉**。如果那一行本机的 `dirty` 就此留在原地，
//! 用户看到的是"待同步永远不掉"（§18 要的正是这句话诚实）。
//!
//! 形状要造得准：两侧**都改过**（都相对 `sync_rev` 变脏）、**最终内容逐字节相同**、
//! 但 **rev 不同** —— A 连改两次（rev 3），B 直接改成同一份（rev 2）。
//!
//! 跑法：`cargo test -p notera-host --test same_content_convergence`
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use notera_host::commands::AccountDraftCmd;
use notera_host::App;
use notera_store::NoteQuery;
use notera_test_webdav::{Backend, TestServer};
use serde_json::json;

const SECRET: &str = "sup3r-s3cr3t";
static SEQ: AtomicUsize = AtomicUsize::new(0);

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "notera-samecontent-{tag}-{}-{n}",
            std::process::id()
        ));
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
            "label": "同内容盘", "baseUrl": base_url, "username": "notera-test",
        }))
        .unwrap();
        app.configure_account(draft).expect("配置账户");
        Self { app, _dir: dir }
    }
    /// 同一份块 id + 同一段文字 —— 内容哈希要真的相等，这条测试才成立。
    fn doc(text: &str) -> serde_json::Value {
        json!({ "v": 1, "content": [{ "id": "blk000001", "type": "paragraph", "content": [{ "text": text }] }] })
    }
    fn note(&self, id: &notera_core::EntityId) -> notera_store::Note {
        self.app
            .store()
            .get_note(id)
            .unwrap()
            .unwrap_or_else(|| panic!("这台设备上读不到那条笔记"))
    }
    fn plain(&self, id: &notera_core::EntityId) -> String {
        self.note(id).plain_text
    }
}

/// 失败时把"到底哪一条挂着、挂在什么状态"打出来：只说"没收敛"指不出是
/// 笔记脏着、还是 outbox 悬着、还是两者不是同一条。
fn stuck(d: &Device) -> String {
    let acct = match d.app.current_account().ok().flatten() {
        Some(a) => a.id.to_string(),
        None => return "没有启用中的账户".to_string(),
    };
    let s = d.app.store();
    [
        notera_store::OpState::Pending,
        notera_store::OpState::Inflight,
        notera_store::OpState::Failed,
        notera_store::OpState::Superseded,
        notera_store::OpState::Done,
    ]
    .into_iter()
    .map(|st| {
        format!(
            "{:?}={}",
            st,
            s.outbox_len(&acct, std::slice::from_ref(&st))
                .unwrap_or(u32::MAX)
        )
    })
    .collect::<Vec<_>>()
    .join(" ")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn identical_final_content_on_both_sides_converges_and_leaves_nothing_pending() {
    let srv = TestServer::start(Backend::Mem).await;
    let url = srv.base_url();
    let a = Device::boot("sc-a", &url);
    let b = Device::boot("sc-b", &url);
    a.app.sync_once().await.expect("A 入伙");
    b.app.sync_once().await.expect("B 入伙");

    let folder = a.app.default_folder_id().unwrap();
    let n = a
        .app
        .store()
        .create_note(&folder, Device::doc("第一版"))
        .expect("A 建笔记");
    a.app.sync_once().await.expect("A 公告");
    for _ in 0..3 {
        b.app.sync_once().await.expect("B 追平");
    }
    // 起点必须一致：两边都在这条上"确认过一致"，否则后面 P7 根本不成立
    assert_eq!(a.note(&n.id).rev, b.note(&n.id).rev, "起点 rev 要一致");
    assert_eq!(
        a.note(&n.id).sync_rev,
        b.note(&n.id).sync_rev,
        "起点的 sync_rev 要一致"
    );

    // A 连改两次（rev +2），B 直接改成**同一份**（rev +1）：内容相同、rev 不同、两侧都脏。
    a.app
        .store()
        .edit_note(&n.id, Device::doc("中间一版"), a.note(&n.id).rev)
        .expect("A 第一次改");
    a.app
        .store()
        .edit_note(&n.id, Device::doc("最终同一版"), a.note(&n.id).rev)
        .expect("A 第二次改");
    b.app
        .store()
        .edit_note(&n.id, Device::doc("最终同一版"), b.note(&n.id).rev)
        .expect("B 一次改成同一版");
    assert_eq!(
        a.note(&n.id).content_hash,
        b.note(&n.id).content_hash,
        "夹具没造对：两侧最终内容哈希必须相同"
    );
    assert_ne!(
        a.note(&n.id).rev,
        b.note(&n.id).rev,
        "夹具没造对：两侧 rev 必须不同（这才是要测那一格）"
    );

    a.app.sync_once().await.expect("A 先追平");
    for _ in 0..5 {
        a.app.sync_once().await.expect("A 再一轮");
        b.app.sync_once().await.expect("B 再一轮");
    }

    // ① 内容不许丢：两边都还是那一版正文
    assert!(
        a.plain(&n.id).contains("最终同一版") && b.plain(&n.id).contains("最终同一版"),
        "同内容收敛那一格把正文弄丢了：A={} B={}",
        a.plain(&n.id),
        b.plain(&n.id)
    );

    // ② 这一格真正承诺的事：**不许留下永久待同步**。两边都必须"没有脏行、没有待发操作"，
    //    且本机那一行的 rev 与 sync_rev 要合上（`rev := R.rev` 那句的机器形态）。
    for (who, d) in [("A", &a), ("B", &b)] {
        let st = d.app.store().stats().unwrap();
        assert_eq!(
            (st.dirty_notes, st.outbox_pending),
            (0, 0),
            "{who} 在同内容收敛之后仍留着待同步：dirty={} outbox={}｜outbox 明细：{} \
             —— 用户看得见这句话（§18）",
            st.dirty_notes,
            st.outbox_pending,
            stuck(d)
        );
        let row = d.note(&n.id);
        assert_eq!(
            row.rev, row.sync_rev,
            "{who} 那条笔记的 rev 与 sync_rev 没合上：rev={} sync_rev={}",
            row.rev, row.sync_rev
        );
    }

    // ③ 再各跑两轮不许有新动作（空轮不产生写入）：这一格一旦收敛就该彻底停下来。
    let before = a.app.store().list_notes(&NoteQuery::all()).unwrap().len();
    a.app.sync_once().await.expect("A 空轮");
    b.app.sync_once().await.expect("B 空轮");
    assert_eq!(before, 1, "一条笔记就该只有一条");
}
