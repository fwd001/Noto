//! 「清除一切数据」之后，库必须落在**刚装好**那一格，而不是"空到不能用"。
//!
//! 为什么单独一份：`erase_all_data` 是按表 `DELETE` 的，而"刚装好"的状态里
//! 有两样**引导期种下的行**（`Store::open` 的 bootstrap）：
//!   ① 默认本（`system_kind='default'`，固定 id）—— DATA-MODEL §9 说删文件夹不级联，
//!      笔记要移进默认本；没有它，"清除之后新建笔记"这一步连落点都没有。
//!   ② 本地哨兵账户（`LOCAL_ACCOUNT_ID`，`enabled=0`）—— I8「提交即入 outbox，不等网络」
//!      靠它留痕；没有它，`all_accounts()` 返回空，本地写入**静默不入队**，
//!      用户清完库接着记的笔记不会同步出去（而界面只说"请重启"）。
mod common;

use common::*;
use notera_store::{NoteQuery, OpState};

#[test]
fn erase_leaves_a_usable_library_not_an_unusable_one() {
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);
    create(&store, &folder, "清除之前的一条");
    assert_eq!(store.list_notes(&NoteQuery::all()).unwrap().len(), 1);

    let report = store.erase_all_data().unwrap();
    assert!(report.tables > 0, "报告要说清清了几张表");
    assert_eq!(
        store.list_notes(&NoteQuery::all()).unwrap().len(),
        0,
        "正文必须真的没了"
    );

    // ① 默认本还在（没有就是 panic：`必须存在默认本`）
    let fresh_folder = default_folder(&store);
    // ② 清除之后照样能记笔记，并且这一条**当场留痕**（I8 不依赖网络，也不依赖"重启之后"）
    let n = create(&store, &fresh_folder, "清除之后新建的一条");
    assert!(
        store
            .outbox_len(notera_store::LOCAL_ACCOUNT_ID, &[OpState::Pending])
            .unwrap()
            >= 1,
        "清除之后本地写入必须照样排进 outbox（哨兵账户被一起删了就是静默不同步）"
    );
    assert_eq!(n.title, "清除之后新建的一条");
}
