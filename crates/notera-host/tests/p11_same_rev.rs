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
//! 而"裁决之后账要结清"**早上还红过** —— 那条 rev 撞车的账停在 `dirty/outbox` 上推不出去，
//! 当时记为缺口 **G17**、它的下游（同名副本一篇一篇累积）记为 **G18**。
//! **0.0.36 起这两条都是真断言**（`ADR-0021` D1+D2 加上按实新增的计划层 P19：已裁决过的同一份
//! 分歧不再重算成冲突），本文件里保留的现场打印是给下次红的时候指路用的。
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

/// 走到"P20 卡片挂在删除那一台（A）的收件箱上"这一步的公共夹具。
///
/// 形状：A 删除、B 编辑，两侧撞在同一个 rev 上；B 按「保留两份」→ 抬号公告成功（G17 已修），
/// 对面那一版以 B 的副本落到 A；补若干轮之后 A 那台必须被问一句（P20）。
/// 返回 `(A, B, 笔记 id, A 的卡片 id, 两个临时目录)` —— 临时目录要让调用方持有，否则 Drop 就把库删了。
async fn a_revival_card(tag: &str) -> (App, App, EntityId, i64, (Tmp, Tmp)) {
    let srv = TestServer::start(Backend::Mem).await;
    let url = srv.base_url();
    let (a, da) = boot(&format!("{tag}-a"), &url);
    let (b, db) = boot(&format!("{tag}-b"), &url);
    a.sync_once().await.expect("A 入伙");
    b.sync_once().await.expect("B 入伙");
    let folder = a.default_folder_id().unwrap();
    let created = a.create_note(&folder, doc(BASE)).expect("建笔记");
    let id = EntityId::parse(&created.id).expect("id");
    settle(&a, 6).await;
    settle(&b, 6).await;
    let b_rev = b.store().get_note(&id).unwrap().unwrap().rev;
    a.store().delete_note(&id).expect("A 删除");
    b.edit_note(&id, doc(EDITED), b_rev).expect("B 编辑");
    settle(&a, 8).await;
    settle(&b, 8).await;
    settle(&a, 8).await;
    settle(&b, 8).await;
    let card = b
        .open_conflicts()
        .expect("B 的卡片")
        .into_iter()
        .find(|c| c.note_id == id.to_string())
        .expect("B 该有一张 P11 卡片");
    b.resolve_conflict(notera_host::commands::ResolveConflictCmd {
        id: card.id,
        action: "keepBoth".into(),
    })
    .expect("B：保留两份");
    settle(&b, 10).await;
    settle(&a, 10).await;
    for _ in 0..8 {
        a.sync_once().await.expect("A 补一轮");
        b.sync_once().await.expect("B 补一轮");
    }
    let a_card = a
        .open_conflicts()
        .expect("A 的卡片")
        .into_iter()
        .find(|c| c.note_id == id.to_string())
        .expect("A（删除那一台）该被 P20 问一句");
    // 卡片必须**自己带着对面那一版**：右栏读的是 `remote_preview`（载荷来自迁移 0008）。
    // 引擎以前只为 `UpdateUpdate` 取载荷，P20 的卡片于是只能是"没取回来"，
    // 而用户按「用服务器那一版」就没有东西可以采纳（判据见 M97）。
    assert!(
        a_card
            .remote_preview
            .as_deref()
            .unwrap_or_default()
            .contains(EDITED),
        "P20 的卡片没把对面那一版带下来，右栏只能是空的：remote_preview={:?}",
        a_card.remote_preview
    );
    let a_card = a_card.id;
    (a, b, id, a_card, (da, db))
}

/// 若干轮，让"裁决之后卡片会不会又算出来"这件事问到底。
async fn rounds(a: &App, b: &App, n: usize) {
    for _ in 0..n {
        a.sync_once().await.expect("A 一轮");
        b.sync_once().await.expect("B 一轮");
    }
}

fn cards_for(app: &App, id: &EntityId) -> usize {
    app.open_conflicts()
        .expect("收件箱")
        .into_iter()
        .filter(|c| c.note_id == id.to_string())
        .count()
}

/// A 按「用我这一版」= 维持删除：删除必须真传播出去，卡片不许回来，账要结清。
/// 不抬号重发的形状是"本机账上干净、服务器停在对面那一版"，于是卡片每轮重算（G17 的老形状）。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_keeping_the_delete_after_a_revival_card_propagates_the_delete_and_stops_asking() {
    let (a, b, id, card, _keep) = a_revival_card("keepdelete").await;
    a.resolve_conflict(notera_host::commands::ResolveConflictCmd {
        id: card,
        action: "replaceWithLocal".into(),
    })
    .expect("A：维持删除");
    rounds(&a, &b, 6).await;
    let mut problems = Vec::new();
    if a.store()
        .get_note(&id)
        .ok()
        .flatten()
        .and_then(|n| n.deleted_at)
        .is_none()
    {
        problems.push("A 维持删除之后，那条笔记在 A 上不是删除态了".to_string());
    }
    let st = a.store().stats().expect("A 的统计");
    if st.dirty_notes != 0 || st.outbox_pending != 0 {
        problems.push(format!(
            "A 重发删除之后账没结清：dirty={} outbox={}",
            st.dirty_notes, st.outbox_pending
        ));
    }
    if cards_for(&a, &id) != 0 {
        problems.push(format!("A 答过的卡片又回来了：{} 张", cards_for(&a, &id)));
    }
    println!("A 维持删除之后 B 的现场：{}", rows_of(&b).join(" ｜ "));
    // 对面那台：主行接受这次删除，但 B 那一版**不许丢**（它以 B 自己的副本活着）
    if trash_with(&b, EDITED) == 0 {
        problems.push("A 重发的删除没能在 B 上落地（B 的主行还活在正常列表）".to_string());
    }
    if live_with(&b, EDITED) == 0 {
        problems.push("删除传播到 B 之后，B 那一版在 B 上不见了（副本也没保住）".to_string());
    }
    assert!(problems.is_empty(), "{}", problems.join(" ｜ "));
}

/// A 按「用服务器那一版替换」= 接受对面那一版：这条笔记回到 A 的正常列表，而且是**对面那一版**
/// 的正文（不是 A 被删之前的旧正文）。通用分支在这一格是一颗空按钮，所以这条判据是给死按钮用的。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_taking_the_peer_version_after_a_revival_card_returns_the_newest_text() {
    let (a, b, id, card, _keep) = a_revival_card("takepeer").await;
    a.resolve_conflict(notera_host::commands::ResolveConflictCmd {
        id: card,
        action: "replaceWithRemote".into(),
    })
    .expect("A：用服务器那一版");
    rounds(&a, &b, 6).await;
    let note = a.store().get_note(&id).ok().flatten();
    let mut problems = Vec::new();
    match note {
        None => problems.push("A 采纳对面那一版之后，这条笔记在 A 上读不到".to_string()),
        Some(n) => {
            if n.deleted_at.is_some() {
                problems.push(format!("A 采纳了对面那一版，却还是删除态：rev={}", n.rev));
            }
            if !n.title.contains(EDITED) {
                problems.push(format!(
                    "A 采纳回来的不是对面那一版，而是本机被删的旧正文：「{}」",
                    n.title
                ));
            }
        }
    }
    if cards_for(&a, &id) != 0 {
        problems.push("A 答过的卡片又回来了".to_string());
    }
    if live_with(&b, EDITED) == 0 {
        problems.push("A 采纳对面那一版，反而把 B 那一版弄丢了".to_string());
    }
    assert!(problems.is_empty(), "{}", problems.join(" ｜ "));
    println!("A 采纳之后 A 的现场：{}", rows_of(&a).join(" ｜ "));
}

/// A 按「保留两份」= 删除保持 + 对面那一版另存一篇。这一格要的是"两份"字面成立：
/// 只关卡片、或只把本机那条放回正常列表，都不叫两份。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_keeping_both_after_a_revival_card_keeps_the_delete_and_a_copy_of_the_peer_version() {
    let (a, b, id, card, _keep) = a_revival_card("keepboth2").await;
    a.resolve_conflict(notera_host::commands::ResolveConflictCmd {
        id: card,
        action: "keepBoth".into(),
    })
    .expect("A：保留两份");
    rounds(&a, &b, 6).await;
    let mut problems = Vec::new();
    if a.store()
        .get_note(&id)
        .ok()
        .flatten()
        .and_then(|n| n.deleted_at)
        .is_none()
    {
        problems
            .push("A 按『保留两份』之后，原来那条不再是删除态（那是复活，不是两份）".to_string());
    }
    // "两份"要字面成立：本机这一支保持删除，对面那一版另存一篇**带着对面正文**的副本。
    // 只数"正常列表里有没有带 EDITED 的"是不够的 —— B 那台按保留两份时留下的副本早就同步过来了，
    // 数它永远 ≥1（第一版就这么判过，变异 M96 抓不到）。要数就数 A 自己这一支。
    let kept: Vec<_> = a
        .store()
        .list_notes(&NoteQuery::all())
        .unwrap()
        .into_iter()
        .filter(|r| r.title.starts_with(BASE) && r.title.contains("（本地副本）"))
        .collect();
    if kept.len() != 1 {
        problems.push(format!(
            "A 按『保留两份』之后，本机这一支该留下一份副本，实际 {} 份",
            kept.len()
        ));
    }
    if let Some(row) = kept.first() {
        let body = a
            .store()
            .get_note(&row.id)
            .ok()
            .flatten()
            .map(|n| n.plain_text)
            .unwrap_or_default();
        if !body.contains(EDITED) {
            problems.push(format!("A 的那份副本里装的不是对面那一版：「{body}」"));
        }
    }
    if cards_for(&a, &id) != 0 {
        problems.push("A 答过的卡片又回来了".to_string());
    }
    assert!(problems.is_empty(), "{}", problems.join(" ｜ "));
    println!("A 保留两份之后 A 的现场：{}", rows_of(&a).join(" ｜ "));
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
    // ③ 裁决之后账必须结清。这一组今天**是绿的**（0.0.35 的 D1 + ADR-0021 D2 之后）：
    //    抬号让 B 那一版重新可发布，分叉一消失，卡片也就没有下一轮可重算（G17 的两半同源）。
    //    三样收在**一次**断言里：逐条 assert 的话第一条 panic 就把进程带走，
    //    一次跑只见得到一个，而这几样是同一条链条上的。
    let mut problems = Vec::new();
    for (who, d) in [("B", &b), ("A", &a)] {
        let st = d.store().stats().unwrap();
        if st.dirty_notes != 0 || st.outbox_pending != 0 {
            problems.push(format!(
                "G17 裁决之后 {who} 仍追不平：dirty={} outbox={} —— 这才是「永远待同步」",
                st.dirty_notes, st.outbox_pending
            ));
        }
    }
    let cards = b.open_conflicts().expect("再读卡片").len();
    if cards != 0 {
        problems.push(format!("G17 裁决过的卡片又回到收件箱：{cards} 张"));
    }
    let copies = b
        .store()
        .list_notes(&NoteQuery::all())
        .unwrap()
        .into_iter()
        .filter(|r| r.title.ends_with("（本地副本）"))
        .count();
    if copies > 1 {
        problems.push(format!(
            "G18 同名副本累积到 {copies} 篇（§6 只该有一篇保底）"
        ));
    }
    assert!(problems.is_empty(), "{}", problems.join(" ｜ "));
    // `settle` 在"两台都干净"时会提前退出，而**已确认删除的那一行本来就是干净的** ——
    // 于是 A 只跑了一轮：那一轮的计划用的还是上一轮的远端视图（rev 2 的删除），
    // 它根本没机会看见 B 抬号后的 rev 3。真实调度器 25 秒一轮，不会停在这里，
    // 所以这里显式再补几轮，把"询问卡片该不该出现"这件事问到底。
    for _ in 0..8 {
        a.sync_once().await.expect("A 补一轮");
        b.sync_once().await.expect("B 补一轮");
    }
    // P20：删除那一台既**不许被静默改写**，也**不许悄悄错过那一版** —— 它得有一张问它的卡片。
    // 挡而不问 = "对面那次改动我永远不知道"，是同一种静默换了个方向。空轮快路径（清单没变 + seq
    // 已应用）以前正因为看不见这些干净的墓碑行而直接跳过规划，这一条断言就是钉住"它必须进规划"。
    let a_cards = a.open_conflicts().expect("A 的卡片");
    assert!(
        a_cards.iter().any(|c| c.note_id == id.to_string()),
        "A（删除那一台）没收到 P20 的询问卡片：收件箱是 {a_cards:?}"
    );
    // 而这张卡片**不许**顺手给 A 造一篇副本。§6 那份"进收件箱同一刻保底"针对的是"本机有一份
    // 还没公告出去的编辑"，A 这一侧的本地版本是**删除** —— 照同一套代码造副本，等于用户删掉的
    // 笔记靠同步回到正常列表（主指令明令禁止的那条）。修复前实测形状就是 A 多出一篇
    // 「共同的第 7 版：报销单说明（本地副本）」（变异 M89 摘掉守卫即复现）。
    assert_eq!(
        live_with(&a, &format!("{BASE}（本地副本）")),
        0,
        "P20 的卡片在删除那一台造了副本 = 已确认的删除被复活成正常笔记"
    );
    println!("P20 之后 A 的现场：{}", rows_of(&a).join(" ｜ "));
    assert!(
        a.store()
            .get_note(&id)
            .ok()
            .flatten()
            .and_then(|n| n.deleted_at)
            .is_some(),
        "A 那台的删除状态被改写了 —— §5.1「绝不静默二选一」管两侧，不只发起编辑那一侧"
    );
    assert!(
        live_with(&a, EDITED) >= 1 && live_with(&b, EDITED) >= 1,
        "裁决之后 B 那一版在两台上必须都还看得见：A={} B={}",
        live_with(&a, EDITED),
        live_with(&b, EDITED)
    );
}
