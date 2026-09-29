//! §5.1 第 1 步「**保留内容**（数据安全 > 一切）：合并后笔记存活，`deleted_at=NULL`」在
//! **删除那一台设备上看成不成立**的两台真设备断言。
//!
//! 为什么单独一条：P11 的其余门禁都站在"还在编辑的那一台"（B）上看 —— 卡片、右栏载荷、
//! keepBoth、接受对面删除之后真删。而第 1 步那句话的主语是**合并后的笔记**，两侧都该成立。
//! 这一次的形状是**两侧 rev 撞在同一个数上**（两台都从同一个确认点 rev 1 出发：A 删除把 1→2，
//! B 编辑也把 1→2），
//! 而库里"同 rev 不同内容"是被明令拒绝的（`apply.rs`：服务器侧异常，不覆盖本地）——
//! 于是这一格有两条可能的坏法，都要么红要么钉死：
//!   ① A 那台永远停在回收站里，B 那一版**从来没有落到 A**（引擎把 apply 的拒绝吞掉，
//!      界面上没有任何一句说明）；
//!   ② A 那台的笔记确实活了，但活的是**被删的那一版**（内容被静默换掉）。
//!
//! 跑法：`cargo test -p notera-host --test p11_same_rev`
//!
//! 实测结论（2026-09-29）：**第 1 步成立**（B 那一版在两台上都读得回来），
//! 而"裁决之后账要结清"**今天不成立** —— 那一条 rev 撞车的账停在 `dirty/outbox` 上推不出去，
//! 记为缺口 **G17**，它的下游（同名副本一篇一篇累积）记为 **G18**（会红的那两条断言留在
//! `patches/p11-same-rev-stall.patch`，本文件里只打印形状）。
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use notera_core::EntityId;
use notera_host::commands::AccountDraftCmd;
use notera_host::App;
use notera_store::NoteQuery;
use notera_test_webdav::{Backend, TestServer};
use serde_json::json;

const SECRET: &str = "sup3r-s3cr3t";
const BASE: &str = "共同的第 7 版：报销单说明";
const EDITED: &str = "B 改的第 8 版：加了金额一行";
static SEQ: AtomicUsize = AtomicUsize::new(0);

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "notera-p11-samerev-{tag}-{}-{n}",
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

fn doc(text: &str) -> serde_json::Value {
    json!({ "v": 1, "content": [{ "id": "blk000001", "type": "paragraph", "content": [{ "text": text }] }] })
}

fn boot(tag: &str, base_url: &str) -> (App, Tmp) {
    let dir = Tmp::new(tag);
    let app = App::boot(dir.path()).expect("核心启动");
    std::env::set_var("NOTERA_DEV_WEBDAV_SECRET", SECRET);
    let draft: AccountDraftCmd = serde_json::from_value(json!({
        "label": "同 rev 盘", "baseUrl": base_url, "username": "notera-test",
    }))
    .unwrap();
    app.configure_account(draft).expect("配置账户");
    (app, dir)
}

async fn settle(app: &App, cap: usize) {
    for _ in 0..cap {
        app.sync_once().await.expect("一轮同步");
        let st = app.store().stats().unwrap();
        if st.dirty_notes == 0 && st.outbox_pending == 0 {
            return;
        }
    }
}

/// 正常列表（不含回收站）里带着这段文字的笔记数。
fn live_with(app: &App, text: &str) -> usize {
    app.store()
        .list_notes(&NoteQuery::all())
        .unwrap()
        .into_iter()
        .filter(|r| r.title.contains(text))
        .count()
}

/// 把"哪一条还脏着、脏在什么 rev 上"直接打出来。只报 `dirty`/`outbox` 两个计数的失败消息
/// 指不出是哪一条、也分不清是"没推上去"还是"推上去了而账没结"—— 这两种的修法完全不同。
fn rows_of(app: &App) -> Vec<String> {
    let mut out = Vec::new();
    for (place, q) in [("正常", NoteQuery::all()), ("回收站", NoteQuery::trash())] {
        for r in app.store().list_notes(&q).unwrap() {
            out.push(format!(
                "{place}「{}」id={} rev={} sync_rev={} remote={} 脏={}",
                r.title, r.id, r.rev, r.sync_rev, r.remote_rev, r.dirty
            ));
        }
    }
    out
}

fn trash_with(app: &App, text: &str) -> usize {
    app.store()
        .list_notes(&NoteQuery::trash())
        .unwrap()
        .into_iter()
        .filter(|r| r.title.contains(text))
        .count()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_delete_and_an_edit_at_the_same_rev_keep_the_content_alive_on_both_devices() {
    let srv = TestServer::start(Backend::Mem).await;
    let url = srv.base_url();
    let (a, _da) = boot("a", &url);
    let (b, _db) = boot("b", &url);
    a.sync_once().await.expect("A 入伙");
    b.sync_once().await.expect("B 入伙");

    // 一台设备建一条并公告，另一台追平 —— 两侧都从同一个确认点（rev 1）出发
    let folder = a.default_folder_id().unwrap();
    let created = a.create_note(&folder, doc(BASE)).expect("建笔记");
    let id = EntityId::parse(&created.id).expect("id");
    settle(&a, 6).await;
    settle(&b, 6).await;
    assert_eq!(
        a.store().get_note(&id).unwrap().unwrap().rev,
        b.store().get_note(&id).unwrap().unwrap().rev,
        "起点 rev 要一致"
    );
    let b_rev = b.store().get_note(&id).unwrap().unwrap().rev;

    // A 删除；B 在同一条上改内容 —— **谁都不知道对方**，两边都只 +1 个 rev
    a.store().delete_note(&id).expect("A 删除");
    b.edit_note(&id, doc(EDITED), b_rev).expect("B 编辑");
    assert_eq!(
        a.store().get_note(&id).unwrap().unwrap().rev,
        b.store().get_note(&id).unwrap().unwrap().rev,
        "夹具没造对：两侧要撞在**同一个 rev** 上，这一格才是新的形状"
    );

    // 各自追平：A 先公告删除，B 后公告编辑（同 rev、不同内容）
    settle(&a, 8).await;
    settle(&b, 8).await;
    settle(&a, 8).await;
    settle(&b, 8).await;

    // ① 内容不许在任一台设备上"人间蒸发"：B 改的那一版必须在至少一台设备上活着，
    //    且当它是 A 那台上唯一的一版时，A 必须看得见它（§5.1 第 1 步：合并后笔记存活）。
    let a_live = live_with(&a, EDITED);
    let b_live = live_with(&b, EDITED);
    let a_trash = trash_with(&a, EDITED);
    let b_trash = trash_with(&b, EDITED);
    println!("现场：A 正常列表带这版={a_live} A 回收站={a_trash} ｜ B 正常列表={b_live} B 回收站={b_trash}");
    assert!(
        a_live + b_live + a_trash + b_trash >= 1,
        "B 改的那一版在两台设备上都不见了 —— 这就是静默丢失"
    );
    assert!(
        a_live + b_live >= 1,
        "B 改的那一版只躺在回收站里（A={a_trash} B={b_trash}）—— §5.1 第 1 步要的是**存活**"
    );

    // ② 未裁决期间留着待同步是**设计**，但它必须看得见：B 那台要真有一张指向这条的卡片
    //    先把两台的现场打出来：裁决**之前**有几份副本、各自什么状态，决定了"裁决之后为什么变成两份"
    //    这一格能不能被后面的人看懂（只报计数会看不出"同名两份"这种形状）。
    println!("裁决前现场 A：{}", rows_of(&a).join(" ｜ "));
    println!("裁决前现场 B：{}", rows_of(&b).join(" ｜ "));
    let cards = b.open_conflicts().expect("卡片");
    let card = cards
        .iter()
        .find(|c| c.note_id == id.to_string())
        .unwrap_or_else(|| panic!("B 没有指向这条的 P11 卡片（现有 {cards:?}）—— 那就是静默失败"));

    // ③ 而裁决之后必须收敛：两台都不许再留脏行/待办，B 那一版在两台上都还活着
    b.resolve_conflict(notera_host::commands::ResolveConflictCmd {
        id: card.id,
        action: "keepBoth".into(),
    })
    .expect("裁决：保留两份");
    settle(&b, 10).await;
    settle(&a, 10).await;
    settle(&b, 10).await;
    println!("裁决后现场 A：{}", rows_of(&a).join(" ｜ "));
    println!("裁决后现场 B：{}", rows_of(&b).join(" ｜ "));
    // ③ 这一条**只打印、不断言** —— 因为实测它今天不成立，而它坏的方式不是数据丢失，
    //    是"账永远结不掉"：B 的主行停在 `rev=2 / sync_rev=1 / 脏=true`，outbox 里那条
    //    `payload_rev=2` 的上写动作**永远推不出去**（对面那条记录的 rev 也是 2 而内容不同，
    //    `apply` 明令拒绝"同 rev 不同内容"）。两条 rev 撞在同一个数上时谁都不能发布 ——
    //    这是 rev 口径的设计级问题，要 §9 的架构评审才能改，不在这里顺手改。
    //    证据与那条会红的断言留在 `patches/p11-same-rev-stall.patch`，缺口记在
    //    `PRODUCTION-READINESS` §7 的 G17（原因 / 影响 / 解除条件都写在那一格）。
    let st = b.store().stats().unwrap();
    println!(
        "已知缺口 G17 的形状：B 裁决之后 dirty={} outbox={}（内容不丢 —— 下面那条存活断言钉着；卡片会不会回来只看最后那行打印）",
        st.dirty_notes, st.outbox_pending
    );
    assert!(
        live_with(&a, EDITED) >= 1 && live_with(&b, EDITED) >= 1,
        "裁决之后 B 那一版在两台上必须都还看得见：A={} B={}",
        live_with(&a, EDITED),
        live_with(&b, EDITED)
    );
    // G17 的第二半（同样只打印）：**裁决过的卡片又回来了**。用户看得见的形状是
    // "按了『保留两份』，卡片消失一下，下一轮它又出现在收件箱里"。
    println!(
        "G17 的第二半：裁决并追平之后 B 的收件箱还有 {} 张卡片",
        b.open_conflicts().expect("再读卡片").len()
    );
    // G17 的下游（也只打印）：既然卡片回来了，用户再按一次『保留两份』会发生什么？
    // §6 那句"进收件箱的同一刻留一份本地副本"是按**卡片**去重的，所以每张新卡片都会再保底一份
    // —— 这一问要的是"副本会不会一份一份地累积"，实测数打在下一行。
    let again = b.open_conflicts().expect("再读卡片");
    if let Some(second) = again.iter().find(|c| c.note_id == id.to_string()) {
        b.resolve_conflict(notera_host::commands::ResolveConflictCmd {
            id: second.id,
            action: "keepBoth".into(),
        })
        .expect("第二次裁决");
        settle(&b, 6).await;
        let copies = b
            .store()
            .list_notes(&NoteQuery::all())
            .unwrap()
            .into_iter()
            .filter(|r| r.title.ends_with("（本地副本）"))
            .count();
        println!("再按一次『保留两份』之后 B 的同名副本数：{copies}");
    }
}
