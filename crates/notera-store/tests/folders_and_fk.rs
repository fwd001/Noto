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
    assert_eq!(store.list_notes(&NoteQuery::all()).unwrap().len(), notes_before, "笔记数必须不变");
    assert_eq!(store.stats().unwrap().notes_trash, 0, "一条都不该进回收站");
    for id in [&n1.id, &n2.id] {
        let n = store.get_note(id).unwrap().expect("笔记必须还在");
        assert_eq!(n.folder_id, root, "笔记移入默认本（DATA-MODEL §9：子文件夹上移、笔记进默认本）");
        assert!(n.deleted_at.is_none());
    }
    assert_eq!(store.get_note(&n3.id).unwrap().unwrap().folder_id, parent.id, "未被删的文件夹里的笔记原地不动");

    // 只新增一条墓碑（文件夹自己的）
    let st = store.stats().unwrap();
    assert_eq!(st.tombstones, tomb_before + 1, "只能有文件夹本身那一条墓碑");
    assert_eq!(st.tombstones_purged, 0, "软删文件夹不写 purged");
    let t = store.get_tombstone(EntityKind::Folder, &child.id).unwrap().expect("文件夹墓碑");
    assert_eq!(t.title_snap.as_deref(), Some("子项目"));
    assert_eq!(t.purged, false);

    // 文件夹行进回收站，子内容不跟着消失
    assert!(store.get_folder(&child.id).unwrap().unwrap().deleted_at.is_some());
    assert!(store.get_folder(&parent.id).unwrap().unwrap().deleted_at.is_none());
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
    assert_eq!(store.get_note(&notes_in_c.id).unwrap().unwrap().folder_id, c.id, "笔记留在自己的文件夹里");
    // B 的子内容被上移之后，删除 B 不再受外键 RESTRICT 阻塞
    assert!(store.get_folder(&b.id).unwrap().unwrap().deleted_at.is_some());
    assert!(store.verify().is_empty());
}

#[test]
fn default_folder_is_protected() {
    let fx = Fix::new();
    let store = fx.open();
    let root = default_folder(&store);
    assert!(matches!(store.delete_folder(&root), Err(StoreError::Constraint(_))));
    assert!(matches!(store.rename_folder(&root, "改名"), Err(StoreError::Constraint(_))));
    assert!(matches!(store.move_folder(&root, None), Err(StoreError::Constraint(_))));
    assert_eq!(store.get_folder(&root).unwrap().unwrap().name, notera_store::DEFAULT_FOLDER_NAME);
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
    assert!(matches!(e, StoreError::Constraint(_)), "成环必须被拒，实际 {e:?}");
    assert_eq!(store.get_folder(&a.id).unwrap().unwrap().parent_id.as_ref(), Some(&root), "被拒后树形不变");
    // 自己成为自己的父
    assert!(matches!(store.move_folder(&b.id, Some(&b.id)), Err(StoreError::Constraint(_))));
    // 直接子节点回环
    assert!(matches!(store.move_folder(&a.id, Some(&b.id)), Err(StoreError::Constraint(_))));
    // 合法移动：C 移到根下
    let moved = store.move_folder(&c.id, None).unwrap();
    assert_eq!(moved.parent_id, None);
    assert!(moved.rev > c.rev, "移动是同步内容 → rev 必须推进（I2）");
    // 移到不存在的父 → NotFound
    assert!(matches!(
        store.move_folder(&c.id, Some(&missing_id())),
        Err(StoreError::NotFound { kind: EntityKind::Folder, .. })
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
    let fk_on: i64 = conn.query_row("PRAGMA foreign_keys", [], |r| r.get(0)).unwrap();
    assert_eq!(fk_on, 1, "该连接必须处于 FK ON 状态才谈得上级联语义");

    let e = conn.execute("DELETE FROM folders WHERE id = ?1", [f.id.to_string()]).unwrap_err();
    assert!(
        matches!(&e, rusqlite::Error::SqliteFailure(_, Some(msg)) if msg.contains("FOREIGN KEY")),
        "删有子节点的文件夹必须被 FK 拦住，实际 {e}"
    );
    assert_eq!(store.stats().unwrap().notes, notes_before);
    assert!(store.get_note(&n.id).unwrap().is_some());

    // 子文件夹同样受保护（folders.parent_id 自引用 RESTRICT）
    let child = store.create_folder(Some(&f.id), "子").unwrap();
    assert!(conn.execute("DELETE FROM folders WHERE id = ?1", [f.id.to_string()]).is_err());
    // 把笔记移走、子文件夹上移之后，才允许走 Store 的正常删除路径
    store.delete_folder(&f.id).unwrap();
    assert!(store.get_folder(&child.id).unwrap().unwrap().deleted_at.is_none());
    assert_eq!(store.stats().unwrap().notes, notes_before, "整条路径零笔记损失");
    assert!(store.verify().is_empty());
}

#[test]
fn notes_cannot_point_at_a_missing_folder_and_folders_are_uuid_shaped() {
    let fx = Fix::new();
    let store = fx.open();
    let e = store.create_note(&missing_id(), doc_text("悬空")).unwrap_err();
    assert!(matches!(e, StoreError::NotFound { kind: EntityKind::Folder, .. }), "实际 {e:?}");
    assert!(matches!(store.create_folder(Some(&missing_id()), "孤儿"), Err(StoreError::NotFound { .. })));
    assert!(matches!(store.create_folder(None, "   "), Err(StoreError::Constraint(_))), "空名必须被拒");
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
    store.set_remote_rev(EntityKind::Folder, &b.id, notera_core::Rev(41), "aabbccddeeff").unwrap();
    let c = store.rename_folder(&b.id, "再改名").unwrap();
    assert_eq!(c.rev, notera_core::Rev(42), "rev 必须是 max(本地,远端)+1");
    assert_eq!(c.remote_rev, notera_core::Rev(41));
    assert!(store.verify().is_empty());
}
