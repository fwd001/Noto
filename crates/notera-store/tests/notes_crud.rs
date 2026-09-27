//! 笔记全链路 + 派生列 + 崩溃/重启一致性（TEST-PLAN 功能矩阵 "笔记/创建…永久删除"）。
mod common;

use common::*;
use notera_store::{NoteQuery, StoreError, SUPPORTED_SCHEMA_VERSION};
use serde_json::json;

#[test]
fn full_chain_create_edit_get_delete_restore_purge() {
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);

    let note = create(&store, &folder, "第一篇");
    assert_eq!(note.rev, notera_core::Rev(1), "新笔记的 rev 必须是 1");
    assert_eq!(note.sync_rev, notera_core::Rev(0), "从未推送过");
    assert_eq!(note.title, "第一篇", "title 由 doc 派生");
    assert!(store.get_note(&note.id).unwrap().is_some());

    let edited = store
        .edit_note(&note.id, doc_text("第二版内容"), note.rev)
        .unwrap();
    assert_eq!(
        edited.rev,
        notera_core::Rev(2),
        "内容变化必须推进 rev（I2）"
    );
    assert_eq!(edited.title, "第二版内容");
    assert_ne!(edited.content_hash, note.content_hash);

    // 删除 → 回收站
    store.delete_note(&note.id).unwrap();
    let trashed = store.get_note(&note.id).unwrap().expect("软删后行仍在");
    assert!(trashed.deleted_at.is_some());
    assert_eq!(
        trashed.rev,
        notera_core::Rev(3),
        "删除也是内容变化（ADR-0006）"
    );
    assert!(
        store.list_notes(&NoteQuery::all()).unwrap().is_empty(),
        "活动列表不含回收站"
    );
    assert_eq!(store.list_notes(&NoteQuery::trash()).unwrap().len(), 1);

    // 恢复
    store.restore_note(&note.id).unwrap();
    let restored = store.get_note(&note.id).unwrap().unwrap();
    assert!(restored.deleted_at.is_none());
    assert_eq!(restored.rev, notera_core::Rev(4));
    assert_eq!(store.list_notes(&NoteQuery::all()).unwrap().len(), 1);

    // 永久删除
    store.purge_note(&note.id).unwrap();
    assert!(
        store.get_note(&note.id).unwrap().is_none(),
        "notes 行必须消失"
    );
    let tomb = store
        .get_tombstone(notera_core::EntityKind::Note, &note.id)
        .unwrap();
    let tomb = tomb.expect("tombstone 必须存在且不自动 GC（I3）");
    assert!(tomb.purged, "purge 必须写 purged=1");
    assert!(
        tomb.rev >= notera_core::Rev(4),
        "墓碑 rev 不得低于最后已知 rev"
    );
    assert_eq!(
        tomb.title_snap.as_deref(),
        Some("第二版内容"),
        "墓碑要能向用户解释曾经是什么"
    );
    assert_eq!(store.list_notes(&NoteQuery::trash()).unwrap().len(), 0);
}

#[test]
fn stale_expected_rev_returns_staleedit_and_changes_nothing() {
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);
    let note = create(&store, &folder, "原文");

    let before = store.get_note(&note.id).unwrap().unwrap();
    let err = store
        .edit_note(&note.id, doc_text("覆盖内容"), notera_core::Rev(99))
        .expect_err("expected_rev 不符必须失败");
    assert!(is_stale(&err), "必须是 StoreError::StaleEdit，实际 {err:?}");

    let after = store.get_note(&note.id).unwrap().unwrap();
    assert_eq!(after, before, "StaleEdit 之后数据必须逐字段不变");
    assert_eq!(after.title, "原文", "不能被静默覆盖");
    // 连 revision 历史都不许多出一条
    assert!(store
        .revision_doc(&note.id, notera_core::Rev(2))
        .unwrap()
        .is_none());
    // 正确的 expected_rev 仍然可用
    assert!(store
        .edit_note(&note.id, doc_text("覆盖内容"), after.rev)
        .is_ok());
}

#[test]
fn editing_a_trashed_note_is_refused() {
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);
    let note = create(&store, &folder, "删前");
    store.delete_note(&note.id).unwrap();
    let e = store
        .edit_note(&note.id, doc_text("偷改"), note.rev)
        .unwrap_err();
    assert!(
        matches!(e, StoreError::Constraint(_)),
        "回收站里的笔记不可编辑，实际 {e:?}"
    );
    assert_eq!(store.get_note(&note.id).unwrap().unwrap().title, "删前");
}

#[test]
fn restart_keeps_data_identical_and_does_not_reapply_migrations() {
    let fx = Fix::new();
    let folder;
    let note_id;
    {
        let store = fx.open();
        assert_eq!(
            store.migration_report().applied,
            (1..=SUPPORTED_SCHEMA_VERSION).collect::<Vec<_>>(),
            "空库必须一路迁到最新"
        );
        assert_eq!(
            store.stats().unwrap().user_version,
            SUPPORTED_SCHEMA_VERSION
        );
        folder = default_folder(&store);
        let n = create(&store, &folder, "重启后还在");
        note_id = n.id.clone();
        assert!(fx.db_file().exists());
        assert!(fx.dir.join("attachments").is_dir());
    }
    let store2 = fx.reopen();
    assert!(
        store2.migration_report().applied.is_empty(),
        "重开不得重复应用迁移（user_version 幂等）"
    );
    assert_eq!(store2.migration_report().to, SUPPORTED_SCHEMA_VERSION);
    let n = store2
        .get_note(&note_id)
        .unwrap()
        .expect("重启后笔记必须在");
    assert_eq!(n.title, "重启后还在");
    assert_eq!(n.plain_text, "重启后还在");
    assert_eq!(n.rev, notera_core::Rev(1));
    assert_eq!(store2.device_id(), fx.device);
    assert_eq!(
        default_folder(&store2),
        folder,
        "默认本 id 必须跨重启稳定（I1）"
    );
    // 列表与搜索在重启后仍然可用 = notes_fts 与权威表一起持久化
    assert_eq!(store2.list_notes(&NoteQuery::all()).unwrap().len(), 1);
    let hits = store2
        .search(&notera_store::SearchQuery::new("重启后"))
        .unwrap();
    assert_eq!(hits.len(), 1, "FTS 索引必须随库持久化");
    // 派生索引脱钩时的自愈：重建 generation 递增
    let gen_before = store2.stats().unwrap().search_generation;
    store2.rebuild_search().unwrap();
    assert_eq!(store2.stats().unwrap().search_generation, gen_before + 1);
    assert_eq!(
        store2
            .search(&notera_store::SearchQuery::new("重启后"))
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn derived_columns_follow_the_doc_in_the_same_transaction() {
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);
    let note = store
        .create_note(
            &folder,
            doc_heading(
                "同步设计",
                "两字中文词同步需要走 LIKE 路径，三字以上走 FTS。",
            ),
        )
        .unwrap();
    assert_eq!(note.title, "同步设计", "title = 第一个 heading");
    assert!(note.plain_text.contains("FTS"));
    assert!(!note.summary.is_empty(), "summary 是列表预览，必须有内容");
    assert_eq!(
        note.char_count,
        note.plain_text
            .chars()
            .filter(|c| !c.is_whitespace())
            .count() as u32,
        "char_count = 非空白码点数"
    );
    assert_eq!(note.block_count, 2);
    assert!(!note.has_attachment);

    // 改 doc → 派生列全变，且列表投影同步
    let edited = store
        .edit_note(
            &note.id,
            doc_heading("改后的标题", "完全不同的一段正文内容"),
            note.rev,
        )
        .unwrap();
    assert_eq!(edited.title, "改后的标题");
    assert_eq!(edited.plain_text, "改后的标题\n完全不同的一段正文内容");
    assert_ne!(edited.char_count, note.char_count);
    let row = store.list_notes(&NoteQuery::all()).unwrap().remove(0);
    assert_eq!(row.title, "改后的标题", "列表读的是派生列，必须同步");
    assert!(
        store.verify().is_empty(),
        "派生列改动后自检必须仍为空：{:?}",
        store.verify()
    );

    // 派生列不可绕过：verify 能抓到漂移（模拟崩溃/外部改库造成的脱钩）
    {
        let conn = rusqlite::Connection::open(fx.db_file()).unwrap();
        conn.execute(
            "UPDATE notes SET title = '被人手改过' WHERE id = ?1",
            [&note.id.to_string()],
        )
        .unwrap();
        conn.execute("INSERT INTO notes_fts(notes_fts) VALUES('rebuild')", [])
            .unwrap();
    }
    let v = store.verify();
    assert!(
        v.iter()
            .any(|x| x.id == "I5" && x.detail.contains("派生列")),
        "绕过写入口改派生列必须被 verify 抓到，实际 {v:?}"
    );
    store.rebuild_search().unwrap();
    let conn = rusqlite::Connection::open(fx.db_file()).unwrap();
    let title: String = conn
        .query_row(
            "SELECT title FROM notes WHERE id = ?1",
            [&note.id.to_string()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(title, "被人手改过", "rebuild_search 只重建索引，不改权威列");
}

#[test]
fn illegal_doc_never_reaches_authoritative_tables() {
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);
    let e = store
        .create_note(&folder, json!({"v": 1, "content": "not-a-list"}))
        .unwrap_err();
    assert!(is_rejection(&e), "I6：非法文档必须被拒绝，实际 {e:?}");
    assert_eq!(store.stats().unwrap().notes, 0, "拒绝后不得留下任何行");
    // 版本超前 → 只读，禁止写回（I7）
    let e2 = store
        .create_note(&folder, json!({"v": 9999, "content": []}))
        .unwrap_err();
    assert!(matches!(e2, StoreError::DocTooNew { .. }), "实际 {e2:?}");
    assert_eq!(store.stats().unwrap().notes, 0);
    assert!(store.verify().is_empty());
}

#[test]
fn edit_with_identical_doc_does_not_bump_rev() {
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);
    let doc = doc_text("同样内容重复保存");
    let a = store.create_note(&folder, doc.clone()).unwrap();
    let b = store.edit_note(&a.id, doc.clone(), a.rev).unwrap();
    assert_eq!(a.rev, b.rev, "内容未变 → 不产生新 rev");
    assert_eq!(a.content_hash, b.content_hash);
    assert_eq!(
        store.stats().unwrap().revisions,
        1,
        "也不该多出 revision 行"
    );
}

#[test]
fn revision_history_keeps_every_rev_and_base_retrievable() {
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);
    let n1 = store.create_note(&folder, doc_text("第一版")).unwrap();
    let n2 = store.edit_note(&n1.id, doc_text("第二版"), n1.rev).unwrap();
    let n3 = store.set_note_pinned(&n1.id, true).unwrap();
    assert_eq!(
        (n1.rev, n2.rev, n3.rev),
        (
            notera_core::Rev(1),
            notera_core::Rev(2),
            notera_core::Rev(3)
        )
    );
    for (rev, want) in [(1u64, "第一版"), (2, "第二版"), (3, "第二版")] {
        let doc = store
            .revision_doc(&n1.id, notera_core::Rev(rev))
            .unwrap()
            .unwrap_or_else(|| panic!("rev {rev} 的历史必须在"));
        let text = doc["content"][0]["content"][0]["text"]
            .as_str()
            .unwrap_or_default();
        assert_eq!(text, want, "rev {rev} 的历史内容");
    }
    // base（= sync_rev）可取：三方合并的前提（DATA-MODEL §4.3）
    store
        .mark_synced(
            notera_core::EntityKind::Note,
            &n1.id,
            n3.rev,
            &n3.content_hash,
        )
        .unwrap();
    let base = store
        .revision_doc(&n1.id, n3.rev)
        .unwrap()
        .expect("base 必须可取");
    assert_eq!(base, store.get_note(&n1.id).unwrap().unwrap().doc);
    assert!(store.verify().is_empty());
}

#[test]
fn list_projection_is_ordered_paginated_and_has_no_doc_field() {
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);
    for i in 0..5 {
        create(&store, &folder, &format!("笔记 {i}"));
    }
    let all = store.list_notes(&NoteQuery::all()).unwrap();
    assert_eq!(all.len(), 5);
    // NoteListRow 结构上没有 doc / plain_text 字段（编译期即保证列表不读正文）
    assert!(all[0].summary.len() < 200);
    let page = store
        .list_notes(&NoteQuery {
            folder: Some(folder.clone()),
            trash: false,
            limit: 2,
            offset: 0,
        })
        .unwrap();
    assert_eq!(page.len(), 2);
    let page2 = store
        .list_notes(&NoteQuery {
            limit: 2,
            offset: 2,
            ..NoteQuery::all()
        })
        .unwrap();
    assert_eq!(page2.len(), 2);
    assert_ne!(page[0].id, page2[0].id, "offset 必须真的翻页");
    let pinned = store.set_note_pinned(&all[4].id, true).unwrap();
    let ordered = store.list_notes(&NoteQuery::all()).unwrap();
    assert_eq!(ordered[0].id, pinned.id, "置顶项排最前");
    assert!(ordered[0].dirty, "rev != sync_rev → 列表位 dirty");
}

#[test]
fn tombstoned_note_is_never_treated_as_new_upload() {
    // 硬性要求 6：purge 之后不得被当成"新增"再上传（设备 B 复活路径的存储侧闸门）
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);
    let note = create(&store, &folder, "将被永久删除");
    store.purge_note(&note.id).unwrap();

    let dirty = store
        .dirty_entities(notera_store::LOCAL_ACCOUNT_ID)
        .unwrap();
    assert!(
        !dirty.iter().any(|d| d.id == note.id
            && matches!(
                d.why,
                notera_store::DirtyWhy::NeverPushed | notera_store::DirtyWhy::Edited
            )),
        "purged 笔记绝不能作为新增/编辑上行：{dirty:?}"
    );
    assert!(
        dirty
            .iter()
            .any(|d| d.id == note.id && d.why == notera_store::DirtyWhy::Purged),
        "永久删除本身必须可传播：{dirty:?}"
    );
    // outbox 里不得再有该笔记的 upsert 待办，只允许 purge
    let ops = store
        .outbox_take(notera_store::LOCAL_ACCOUNT_ID, 50)
        .unwrap();
    for op in ops.iter().filter(|o| o.entity_key == note.id.to_string()) {
        assert_eq!(op.op, notera_store::OpKind::Purge, "残留待办：{op:?}");
    }
    assert!(store.verify().is_empty(), "{:?}", store.verify());
    // 远端把这条记录再推回来 → 拒绝，且笔记仍不存在
    let env = note_envelope(&note.id, 99, &doc_text("复活尝试"), Some(&folder));
    let e = store
        .apply_remote(&[notera_store::ApplyOp::UpsertNote { env }])
        .unwrap_err();
    assert!(is_rejection(&e), "实际 {e:?}");
    assert!(store.get_note(&note.id).unwrap().is_none());
    assert!(store.verify().is_empty());
    // purge 幂等
    store.purge_note(&note.id).unwrap();
}

#[test]
fn verify_is_empty_on_a_fresh_store_and_reports_nothing_after_noop() {
    let fx = Fix::new();
    let store = fx.open();
    assert!(
        store.verify().is_empty(),
        "空库自检必须为空：{:?}",
        store.verify()
    );
    assert!(
        store.startup_violations().is_empty(),
        "启动自检必须为空：{:?}",
        store.startup_violations()
    );
    let folder = default_folder(&store);
    let n = create(&store, &folder, "什么也没改");
    store.set_note_pinned(&n.id, false).unwrap(); // no-op：不该产生 rev 抖动
    assert_eq!(
        store.get_note(&n.id).unwrap().unwrap().rev,
        notera_core::Rev(1)
    );
    assert!(store.verify().is_empty());
}
