//! 文件夹语义 + 外键闸门（ADR-0006：删除文件夹不级联删笔记）。
mod common;

use common::*;
use notera_core::EntityKind;
use notera_store::{NoteQuery, StoreError};

#[test]
fn delete_folder_moves_children_up_and_never_cascades_notes() {
    let fx = Fix::new();
    let store = fx.open();
    let root = default_folder(&store);

    let parent = store.create_folder(Some(&root), "项目").unwrap();
    let child = store.create_folder(Some(&parent.id), "子项目").unwrap();
    let n1 = create(&store, &child.id, "子文件夹里的笔记 A");
    let n2 = create(&store, &child.id, "子文件夹里的笔记 B");
    let n3 = create(&store, &parent.id, "父文件夹里的笔记 C");
    let notes_before = store.list_notes(&NoteQuery::all()).unwrap().len();
    let tomb_before = store.stats().unwrap().tombstones;

    store.delete_folder(&child.id).unwrap();

    // 笔记总数不变，且仍然全部可见（ADR-0006 的验收点）
    assert_eq!(
        store.list_notes(&NoteQuery::all()).unwrap().len(),
        notes_before,
        "笔记数必须不变"
    );
    assert_eq!(store.stats().unwrap().notes_trash, 0, "一条都不该进回收站");
    for id in [&n1.id, &n2.id] {
        let n = store.get_note(id).unwrap().expect("笔记必须还在");
        assert_eq!(
            n.folder_id, root,
            "笔记移入默认本（DATA-MODEL §9：子文件夹上移、笔记进默认本）"
        );
        assert!(n.deleted_at.is_none());
    }
    assert_eq!(
        store.get_note(&n3.id).unwrap().unwrap().folder_id,
        parent.id,
        "未被删的文件夹里的笔记原地不动"
    );

    // 只新增一条墓碑（文件夹自己的）
    let st = store.stats().unwrap();
    assert_eq!(st.tombstones, tomb_before + 1, "只能有文件夹本身那一条墓碑");
    assert_eq!(st.tombstones_purged, 0, "软删文件夹不写 purged");
    let t = store
        .get_tombstone(EntityKind::Folder, &child.id)
        .unwrap()
        .expect("文件夹墓碑");
    assert_eq!(t.title_snap.as_deref(), Some("子项目"));
    assert!(!t.purged);

    // 文件夹行进回收站，子内容不跟着消失
    assert!(store
        .get_folder(&child.id)
        .unwrap()
        .unwrap()
        .deleted_at
        .is_some());
    assert!(store
        .get_folder(&parent.id)
        .unwrap()
        .unwrap()
        .deleted_at
        .is_none());
    assert!(store.verify().is_empty(), "{:?}", store.verify());
}

#[test]
fn delete_folder_moves_child_folders_up_one_level() {
    let fx = Fix::new();
    let store = fx.open();
    let root = default_folder(&store);
    let a = store.create_folder(Some(&root), "A").unwrap();
    let b = store.create_folder(Some(&a.id), "B").unwrap();
    let c = store.create_folder(Some(&b.id), "C").unwrap();
    let notes_in_c = create(&store, &c.id, "深层笔记");

    store.delete_folder(&b.id).unwrap();
    let c_now = store.get_folder(&c.id).unwrap().unwrap();
    assert_eq!(c_now.parent_id.as_ref(), Some(&a.id), "C 上移一级到 A 之下");
    assert_eq!(
        store.get_note(&notes_in_c.id).unwrap().unwrap().folder_id,
        c.id,
        "笔记留在自己的文件夹里"
    );
    // B 的子内容被上移之后，删除 B 不再受外键 RESTRICT 阻塞
    assert!(store
        .get_folder(&b.id)
        .unwrap()
        .unwrap()
        .deleted_at
        .is_some());
    assert!(store.verify().is_empty());
}

#[test]
fn default_folder_is_protected() {
    let fx = Fix::new();
    let store = fx.open();
    let root = default_folder(&store);
    assert!(matches!(
        store.delete_folder(&root),
        Err(StoreError::Constraint(_))
    ));
    assert!(matches!(
        store.rename_folder(&root, "改名"),
        Err(StoreError::Constraint(_))
    ));
    assert!(matches!(
        store.move_folder(&root, None),
        Err(StoreError::Constraint(_))
    ));
    assert_eq!(
        store.get_folder(&root).unwrap().unwrap().name,
        notera_store::DEFAULT_FOLDER_NAME
    );
}

#[test]
fn move_folder_rejects_cycles_including_grandparent_loop() {
    let fx = Fix::new();
    let store = fx.open();
    let root = default_folder(&store);
    let a = store.create_folder(Some(&root), "A").unwrap();
    let b = store.create_folder(Some(&a.id), "B").unwrap();
    let c = store.create_folder(Some(&b.id), "C").unwrap();

    // A → C（C 是 A 的孙）＝成环
    let e = store.move_folder(&a.id, Some(&c.id)).unwrap_err();
    assert!(
        matches!(e, StoreError::Constraint(_)),
        "成环必须被拒，实际 {e:?}"
    );
    assert_eq!(
        store.get_folder(&a.id).unwrap().unwrap().parent_id.as_ref(),
        Some(&root),
        "被拒后树形不变"
    );
    // 自己成为自己的父
    assert!(matches!(
        store.move_folder(&b.id, Some(&b.id)),
        Err(StoreError::Constraint(_))
    ));
    // 直接子节点回环
    assert!(matches!(
        store.move_folder(&a.id, Some(&b.id)),
        Err(StoreError::Constraint(_))
    ));
    // 合法移动：C 移到根下
    let moved = store.move_folder(&c.id, None).unwrap();
    assert_eq!(moved.parent_id, None);
    assert!(moved.rev > c.rev, "移动是同步内容 → rev 必须推进（I2）");
    // 移到不存在的父 → NotFound
    assert!(matches!(
        store.move_folder(&c.id, Some(&missing_id())),
        Err(StoreError::NotFound {
            kind: EntityKind::Folder,
            ..
        })
    ));
    assert!(store.verify().is_empty());
}

#[test]
fn foreign_keys_are_enforced_and_delete_never_cascades_notes() {
    // 硬性要求 9：外键连接级生效；`ON DELETE RESTRICT` 让"删文件夹"不可能连带吞掉笔记
    let fx = Fix::new();
    let store = fx.open();
    let root = default_folder(&store);
    let f = store.create_folder(Some(&root), "有内容的本").unwrap();
    let n = create(&store, &f.id, "被保护的笔记");
    let notes_before = store.stats().unwrap().notes;

    let conn = rusqlite::Connection::open(fx.db_file()).unwrap();
    let fk_on: i64 = conn
        .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
        .unwrap();
    assert_eq!(fk_on, 1, "该连接必须处于 FK ON 状态才谈得上级联语义");

    let e = conn
        .execute("DELETE FROM folders WHERE id = ?1", [f.id.to_string()])
        .unwrap_err();
    assert!(
        matches!(&e, rusqlite::Error::SqliteFailure(_, Some(msg)) if msg.contains("FOREIGN KEY")),
        "删有子节点的文件夹必须被 FK 拦住，实际 {e}"
    );
    assert_eq!(store.stats().unwrap().notes, notes_before);
    assert!(store.get_note(&n.id).unwrap().is_some());

    // 子文件夹同样受保护（folders.parent_id 自引用 RESTRICT）
    let child = store.create_folder(Some(&f.id), "子").unwrap();
    assert!(conn
        .execute("DELETE FROM folders WHERE id = ?1", [f.id.to_string()])
        .is_err());
    // 把笔记移走、子文件夹上移之后，才允许走 Store 的正常删除路径
    store.delete_folder(&f.id).unwrap();
    assert!(store
        .get_folder(&child.id)
        .unwrap()
        .unwrap()
        .deleted_at
        .is_none());
    assert_eq!(
        store.stats().unwrap().notes,
        notes_before,
        "整条路径零笔记损失"
    );
    assert!(store.verify().is_empty());
}

#[test]
fn notes_cannot_point_at_a_missing_folder_and_folders_are_uuid_shaped() {
    let fx = Fix::new();
    let store = fx.open();
    let e = store
        .create_note(&missing_id(), doc_text("悬空"))
        .unwrap_err();
    assert!(
        matches!(
            e,
            StoreError::NotFound {
                kind: EntityKind::Folder,
                ..
            }
        ),
        "实际 {e:?}"
    );
    assert!(matches!(
        store.create_folder(Some(&missing_id()), "孤儿"),
        Err(StoreError::NotFound { .. })
    ));
    assert!(
        matches!(
            store.create_folder(None, "   "),
            Err(StoreError::Constraint(_))
        ),
        "空名必须被拒"
    );
    assert!(store.verify().is_empty());
}

#[test]
fn folder_rename_and_move_bump_rev_via_next_rev_only() {
    let fx = Fix::new();
    let store = fx.open();
    let _root = default_folder(&store);
    let a = store.create_folder(None, "A").unwrap();
    assert_eq!(a.rev, notera_core::Rev(1));
    assert_eq!(a.sync_rev, notera_core::Rev(0), "新实体从未确认一致");
    let b = store.rename_folder(&a.id, "A 改名").unwrap();
    assert_eq!(b.rev, notera_core::Rev(2));
    assert_eq!(b.name, "A 改名");
    // 远端头部更高时，本地推进必须取 max（I2 的 Lamport 语义，禁止手写 +1）
    store
        .set_remote_rev(
            EntityKind::Folder,
            &b.id,
            notera_core::Rev(41),
            "aabbccddeeff",
        )
        .unwrap();
    let c = store.rename_folder(&b.id, "再改名").unwrap();
    assert_eq!(c.rev, notera_core::Rev(42), "rev 必须是 max(本地,远端)+1");
    assert_eq!(c.remote_rev, notera_core::Rev(41));
    assert!(store.verify().is_empty());
}

/// 库里**已经**有环（0.0.32 之前那一版从远端那一支写进去的）时，读侧不许卡死、也不许报错给界面。
///
/// 这条钉的是 `descendant_ids` 那条递归 CTE 用 UNION 而不是 UNION ALL：UNION ALL 不按 id
/// 去重，环上永不收敛 —— 用户下一次移动文件夹（内部就是查后代）就是**无限等待，而且没有任何报错**。
/// 实测过：把那一句改回 UNION ALL，这条查询在本机跑满 60 s 都没返回（不是报错，是卡住）。
/// 写入侧现在拒绝造环，这一条管的是"修之前已经坏掉的那些库"：它们不许变成"打不开、动不了"，
/// 只许位置分叉、可修。
///
/// 环是手搓的（直接改表），因为产品写入路径现在已经会拒绝 —— 要模拟的正是旧版本留下的现场。
/// 查询放进**线程 + 超时**里收：如果在测试自己这一线程里调，卡死的实现会把整个测试进程一起挂住，
/// 那就不是一个会红的判据（判据必须能红 —— 见 TEST-PLAN 的门禁纪律与变异自证）。
#[test]
fn a_folder_cycle_already_on_disk_never_eats_the_subtree_query() {
    let fx = Fix::new();
    let store = std::sync::Arc::new(fx.open());
    let root = default_folder(&store);
    let a = store.create_folder(Some(&root), "甲").unwrap();
    let b = store.create_folder(Some(&a.id), "乙").unwrap();

    let conn = rusqlite::Connection::open(fx.db_file()).unwrap();
    let wrote = conn
        .execute(
            "UPDATE folders SET parent_id = ?1 WHERE id = ?2",
            rusqlite::params![b.id.as_str(), a.id.as_str()],
        )
        .unwrap();
    assert_eq!(wrote, 1, "甲的父要真的改成乙（此时 甲→乙→甲 成环）");
    drop(conn);

    let (tx, rx) = std::sync::mpsc::channel();
    let s = store.clone();
    let a_id = a.id.clone();
    std::thread::spawn(move || {
        let _ = tx.send(s.folder_subtree(std::slice::from_ref(&a_id)));
    });
    let started = std::time::Instant::now();
    let got = match rx.recv_timeout(std::time::Duration::from_secs(5)) {
        Ok(res) => res,
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            panic!("子树查询那支线程崩了 —— 判据没跑到")
        }
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => panic!(
            "子树查询在有环的库上 5 s 都没返回 —— 这就是界面上的'点了没反应'。\
             这条 CTE 必须按 id 去重（UNION），不许写回 UNION ALL"
        ),
    };
    let cost = started.elapsed();
    let set = got.unwrap_or_else(|e| {
        panic!("有环时子树查询把错直接抛给上层（界面就是'移动不了文件夹'）：{e}")
    });
    assert!(
        cost.as_millis() < 5_000,
        "子树查询花了 {} ms，预算 5 s",
        cost.as_millis()
    );
    // 成员算得出来：甲乙互指，两个都该被算到且不被吞掉（集合天然去重）。
    assert!(
        set.contains(a.id.as_str()) && set.contains(b.id.as_str()),
        "环上的两个成员都要被算到：{set:?}"
    );
    assert!(
        !set.contains(root.as_str()),
        "从甲出发不该把祖先默认本也算进来：{set:?}"
    );
}
