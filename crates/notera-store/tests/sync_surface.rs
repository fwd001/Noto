//! 同步协作面：脏集 / outbox / 确认点 / apply_remote 原子性 / 冲突收件箱 / 附件。
mod common;

use common::*;
use notera_core::{EntityId, EntityKind, Rev};
use notera_store::{
    ApplyOp, ConflictRecord, DirtyWhy, NoteQuery, OpKind, OpState, RemoteIndexEntry, SearchQuery,
    StoreError,
};

#[test]
fn dirty_entities_is_exactly_rev_ne_sync_rev() {
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);
    let acct = notera_store::LOCAL_ACCOUNT_ID;

    // 默认本自身是新实体 → 恰好一条脏（rev 1 != sync_rev 0）
    let d = store.dirty_entities(acct).unwrap();
    assert_eq!(d.len(), 1, "初始只有默认本待上传：{d:?}");
    store
        .mark_synced(EntityKind::Folder, &folder, store.get_folder(&folder).unwrap().unwrap().rev, "sha256:x")
        .unwrap();
    assert!(store.dirty_entities(acct).unwrap().is_empty(), "全部确认后脏集必须为空");

    let n = create(&store, &folder, "刚建");
    let d = store.dirty_entities(acct).unwrap();
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].id, n.id);
    assert_eq!(d[0].why, DirtyWhy::NeverPushed, "sync_rev=0 → 从未推送");
    assert_eq!(d[0].rev, n.rev);

    store.mark_synced(EntityKind::Note, &n.id, n.rev, &n.content_hash).unwrap();
    assert!(store.dirty_entities(acct).unwrap().is_empty(), "push 确认后必须干净");

    let edited = store.edit_note(&n.id, doc_text("改过"), n.rev).unwrap();
    let d = store.dirty_entities(acct).unwrap();
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].why, DirtyWhy::Edited);
    assert_eq!(d[0].sync_rev, Rev(1));

    store.delete_note(&n.id).unwrap();
    let d = store.dirty_entities(acct).unwrap();
    assert_eq!(d[0].why, DirtyWhy::Deleted, "软删在脏集里必须标 Deleted");
    store.restore_note(&n.id).unwrap();
    assert_eq!(store.dirty_entities(acct).unwrap()[0].why, DirtyWhy::Edited);

    // 反向保证：rev == sync_rev 时绝不出现
    let cur = store.get_note(&n.id).unwrap().unwrap();
    store.mark_synced(EntityKind::Note, &n.id, cur.rev, &cur.content_hash).unwrap();
    assert!(store.dirty_entities(acct).unwrap().is_empty());
    assert_eq!(store.get_note(&n.id).unwrap().unwrap().sync_hash.as_deref(), Some(cur.content_hash.as_str()));
    // 未知账户：实体脏集仍然可判（purg 确认信息按账户记账）
    assert!(store.dirty_entities("没有这个账户").unwrap().iter().all(|e| e.why != DirtyWhy::Purged));
    let _ = edited;
}

#[test]
fn every_write_enqueues_one_outbox_row_per_enabled_account_and_supersedes_old_rev() {
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);
    let acct = notera_store::LOCAL_ACCOUNT_ID;
    let n = create(&store, &folder, "第一版");

    // outbox 是"写入那一刻的队列"，不是历史重放：注册第二台服务器不倒灌旧待办。
    store.register_account("acct-b", "内网", "https://dav.internal/notes").unwrap();
    assert_eq!(store.outbox_len("acct-b", &[OpState::Pending]).unwrap(), 0, "注册不得凭空造出旧待办");

    // 新建一条笔记在 local 名下产生 **2** 行：默认本（开库时入队）+ 笔记。
    // 默认本必须被公告，否则另一台设备会自己再造一个默认本，笔记就分家了。
    assert_eq!(
        store.outbox_len(acct, &[OpState::Pending]).unwrap() as usize,
        2,
        "本地账户：默认本 + 笔记各一行"
    );

    // ---- 扇出：注册之后的一次本地写必须给**每个**账户各一行 ----
    // `dedupe_key` 若不含 account_id，第二个账户的 INSERT 会被 `ON CONFLICT DO UPDATE`
    // 吸收成第一账户的行 —— 表现为"本地编辑只同步到其中一台服务器"，且完全静默。
    let v1 = store.get_note(&n.id).unwrap().unwrap();
    let v2 = store.edit_note(&n.id, doc_text("第二版"), v1.rev).unwrap();
    let mut note_keys: Vec<String> = Vec::new();
    // local 还带着开库时入队的默认本那一行，所以取到 2 行；acct-b 只有笔记这 1 行。
    for (a, want) in [(acct, 2usize), ("acct-b", 1usize)] {
        let taken = store.outbox_take(a, 10).unwrap();
        assert_eq!(taken.len(), want, "{a} 取到的行数不对（笔记那一行塌成 0 = 静默漏同步一台服务器）：{taken:?}");
        let note_row = taken.iter().find(|t| t.kind == EntityKind::Note).expect("必须有笔记那一行");
        assert_eq!(note_row.payload_rev, Some(v2.rev));
        assert_eq!(note_row.op, OpKind::Upsert);
        assert_eq!(note_row.state, OpState::Inflight);
        assert_eq!(note_row.attempts, 1);
        assert!(
            note_row.dedupe_key.starts_with(&format!("{a}:")),
            "dedupe_key 必须以账户开头：{}",
            note_row.dedupe_key
        );
        note_keys.push(note_row.dedupe_key.clone());
        for op in &taken {
            store.outbox_state(op.id, OpState::Done, None, None).unwrap();
        }
    }
    assert_eq!(
        note_keys[0].split_once(':').map(|t| t.1),
        note_keys[1].split_once(':').map(|t| t.1),
        "去掉账户前缀后键体必须一致 —— 差异只允许出现在账户段"
    );
    assert_ne!(note_keys[0], note_keys[1], "两个账户的 dedupe_key 必须不同，否则唯一索引让它们互相吸收");
    assert_eq!(store.outbox_take(acct, 10).unwrap().len(), 0, "inflight 不得重复取");
    assert_eq!(store.outbox_len(acct, &[OpState::Done]).unwrap(), 2, "默认本 + 笔记都已确认");
    assert_eq!(store.outbox_len(acct, &[OpState::Pending]).unwrap(), 0);

    // ---- 再编辑：同实体更早的 pending 被 superseded（远端只需最终态）----
    let e1 = store.edit_note(&n.id, doc_text("第三版"), v2.rev).unwrap();
    let e2 = store.edit_note(&n.id, doc_text("第四版"), e1.rev).unwrap();
    assert_eq!(
        store.outbox_len(acct, &[OpState::Pending]).unwrap(),
        1,
        "只留本笔记最新的一行"
    );
    let pend = store.outbox_take(acct, 10).unwrap();
    assert_eq!(pend.len(), 1, "同一笔记的连续编辑只留最新一条 pending：{pend:?}");
    assert_eq!(pend[0].kind, EntityKind::Note);
    assert_eq!(pend[0].payload_rev, Some(e2.rev), "留下的必须是最新 rev，旧的应被 supersede");
    assert!(pend[0].dedupe_key.contains(&n.id.to_string()));
    assert_eq!(
        store.outbox_len(acct, &[OpState::Superseded]).unwrap(),
        2,
        "被超越的行留痕不删除：rev1（被 rev2 超越）+ rev3（被 rev4 超越）"
    );

    // 失败重试：退避未到期不得取件，到期后必须可重试
    store
        .outbox_state(pend[0].id, OpState::Failed, Some("offline"), Some("2099-01-01T00:00:00.000Z"))
        .unwrap();
    assert_eq!(store.outbox_take(acct, 10).unwrap().len(), 0, "退避未到期的 failed 不得取件");
    store
        .outbox_state(pend[0].id, OpState::Failed, Some("offline"), Some("2000-01-01T00:00:00.000Z"))
        .unwrap();
    assert_eq!(store.outbox_take(acct, 10).unwrap().len(), 1, "退避到期的 failed 必须可重试");

    // 禁用账户不得继续排队，否则它的 outbox 会无界增长（引擎永不消费它）
    store.set_account_enabled("acct-b", false).unwrap();
    let a_before = store
        .outbox_len("acct-b", &[OpState::Pending, OpState::Inflight, OpState::Failed])
        .unwrap();
    let _ = store.create_note(&folder, doc_text("禁用之后的写入")).unwrap();
    assert_eq!(
        store
            .outbox_len("acct-b", &[OpState::Pending, OpState::Inflight, OpState::Failed])
            .unwrap(),
        a_before,
        "已禁用账户不应再收到 outbox 行"
    );
    // 而本地哨兵账户（enabled=0）必须继续留痕：它是"本地写入不依赖网络"的记账
    assert!(
        store.outbox_len(acct, &[OpState::Pending]).unwrap() >= 1,
        "local 哨兵账户即使 enabled=0 也必须留痕"
    );
}

#[test]
fn outbox_state_rejects_unknown_id_and_take_respects_limit() {
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);
    for i in 0..4 {
        create(&store, &folder, &format!("批量 {i}"));
    }
    let acct = notera_store::LOCAL_ACCOUNT_ID;
    let two = store.outbox_take(acct, 2).unwrap();
    assert_eq!(two.len(), 2);
    assert_eq!(store.outbox_len(acct, &[OpState::Inflight]).unwrap(), 2);
    assert!(matches!(store.outbox_state(999999, OpState::Done, None, None), Err(StoreError::Constraint(_))));
}

#[test]
fn set_remote_rev_and_remote_index_replace_roundtrip() {
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);
    let n = create(&store, &folder, "等着被远端超越");
    store.set_remote_rev(EntityKind::Note, &n.id, Rev(9), "a1b2c3d4e5f6").unwrap();
    let cur = store.get_note(&n.id).unwrap().unwrap();
    assert_eq!(cur.remote_rev, Rev(9));
    assert_eq!(cur.rev, Rev(1), "观测远端不推进本地 rev");
    // 下一次本地写必须取 max（I2）
    let next = store.edit_note(&n.id, doc_text("本地继续写"), cur.rev).unwrap();
    assert_eq!(next.rev, Rev(10));
    assert!(matches!(
        store.set_remote_rev(EntityKind::Note, &missing_id(), Rev(1), "x"),
        Err(StoreError::NotFound { .. })
    ));

    let entries = vec![
        RemoteIndexEntry {
            kind: EntityKind::Note,
            id: n.id.clone(),
            rev: Rev(10),
            hash12: Some("deadbeefcafe".into()),
            size: Some(220),
            deleted: false,
            purged: false,
            seg: Some("seg-0000".into()),
            sha256: None,
        },
        RemoteIndexEntry {
            kind: EntityKind::Folder,
            id: folder.clone(),
            rev: Rev(3),
            hash12: None,
            size: None,
            deleted: false,
            purged: false,
            seg: None,
            sha256: None,
        },
    ];
    store.remote_index_replace(notera_store::LOCAL_ACCOUNT_ID, &entries).unwrap();
    let got = store.remote_index_lookup(notera_store::LOCAL_ACCOUNT_ID, EntityKind::Note, n.id.as_str()).unwrap();
    assert_eq!(got.map(|e| (e.rev, e.seg)), Some((Rev(10), Some("seg-0000".to_string()))));
    // 整表替换：旧条目消失
    store.remote_index_replace(notera_store::LOCAL_ACCOUNT_ID, &entries[1..]).unwrap();
    assert!(store.remote_index_lookup(notera_store::LOCAL_ACCOUNT_ID, EntityKind::Note, n.id.as_str()).unwrap().is_none());
    assert!(matches!(
        store.remote_index_replace("不存在", &entries),
        Err(StoreError::Constraint(_))
    ));
}

#[test]
fn apply_remote_writes_envelope_and_becomes_clean() {
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);
    store
        .mark_synced(EntityKind::Folder, &folder, store.get_folder(&folder).unwrap().unwrap().rev, "sha256:f")
        .unwrap();
    let remote_id = EntityId::new();
    let doc = doc_heading("来自另一台设备", "同步协议在两字中文下必须走 LIKE");
    let env = note_envelope(&remote_id, 7, &doc, Some(&folder));

    let rep = store.apply_remote(&[ApplyOp::UpsertNote { env: env.clone() }]).unwrap();
    assert_eq!((rep.applied, rep.notes_written), (1, 1));
    let n = store.get_note(&remote_id).unwrap().expect("远端笔记必须落地");
    assert_eq!(n.rev, Rev(7));
    assert_eq!(n.sync_rev, Rev(7), "pull 生效 → sync_rev ← rev（§4.2）");
    assert_eq!(n.remote_rev, Rev(7));
    assert_eq!(n.title, "来自另一台设备", "派生列必须由 store 重算（I5）");
    assert_eq!(n.content_hash, hash_of(&doc));
    assert!(store.dirty_entities(notera_store::LOCAL_ACCOUNT_ID).unwrap().is_empty(), "拉回的内容不该再上行");
    assert_eq!(store.search(&SearchQuery::new("另一台")).unwrap().len(), 1, "FTS 同事务更新");
    assert_eq!(store.revision_doc(&remote_id, Rev(7)).unwrap().unwrap(), n.doc);
    // 幂等重放
    let rep2 = store.apply_remote(&[ApplyOp::UpsertNote { env }]).unwrap();
    assert_eq!((rep2.applied, rep2.skipped), (0, 1));
    assert_eq!(store.get_note(&remote_id).unwrap().unwrap().rev, Rev(7));
    // 更新版本：rev 前进、base 仍可取（三方合并前提）
    let doc2 = doc_heading("来自另一台设备", "改过的正文内容");
    let rep3 = store.apply_remote(&[ApplyOp::UpsertNote { env: note_envelope(&remote_id, 9, &doc2, Some(&folder)) }]).unwrap();
    assert_eq!(rep3.applied, 1);
    let n2 = store.get_note(&remote_id).unwrap().unwrap();
    assert_eq!((n2.rev, n2.sync_rev), (Rev(9), Rev(9)));
    assert!(store.revision_doc(&remote_id, Rev(7)).unwrap().is_some(), "旧 revision 不能被覆盖");
    assert!(store.verify().is_empty(), "{:?}", store.verify());
}

#[test]
fn apply_remote_is_one_transaction_no_partial_write() {
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);
    let a = EntityId::new();
    let b = EntityId::new();
    let bad = EntityId::new();

    let ops = vec![
        ApplyOp::UpsertNote { env: note_envelope(&a, 2, &doc_text("第一批 A"), Some(&folder)) },
        ApplyOp::UpsertNote { env: note_envelope(&b, 2, &doc_text("第一批 B"), Some(&folder)) },
        // 第三条哈希对不上 → I6 必须整批回滚
        ApplyOp::UpsertNote {
            env: {
                let mut e = note_envelope(&bad, 2, &doc_text("坏数据"), Some(&folder));
                e["hash"] = serde_json::json!("sha256:0000000000000000000000000000000000000000000000000000000000000000");
                e
            },
        },
    ];
    let e = store.apply_remote(&ops).unwrap_err();
    assert!(is_rejection(&e), "实际 {e:?}");
    for id in [&a, &b, &bad] {
        assert!(store.get_note(id).unwrap().is_none(), "中途失败后不得有部分写入：{id}");
    }
    assert_eq!(store.stats().unwrap().notes, 0);
    assert_eq!(store.stats().unwrap().revisions, 0, "revision 也不能只写一半");
    assert_eq!(store.stats().unwrap().fts_rows, 0, "FTS 必须一起回滚");
    assert!(store.verify().is_empty(), "{:?}", store.verify());

    // 同一批里"前面合法、后面非法"的其它形状：rev 回退
    let doc = doc_text("先落地");
    let ok = ApplyOp::UpsertNote { env: note_envelope(&a, 5, &doc, Some(&folder)) };
    let stale = ApplyOp::UpsertNote { env: note_envelope(&a, 3, &doc_text("更旧的远端"), Some(&folder)) };
    assert!(store.apply_remote(&[ok, stale]).unwrap_err().is_rejection());
    assert!(store.get_note(&a).unwrap().is_none(), "回退批必须整体不生效");

    // 纯附件/加密载荷之类无法落地的形状，同样不得留下半条
    let mut encrypted = note_envelope(&b, 1, &doc, Some(&folder));
    encrypted["payload"] = serde_json::Value::Null;
    encrypted["ct"] = serde_json::json!({"nonce":"AA==","ct":"BB=="});
    encrypted["enc"] = serde_json::json!({"alg":"aes-256-gcm-siv"});
    assert!(store.apply_remote(&[ApplyOp::UpsertNote { env: encrypted }]).unwrap_err().is_rejection());
    assert!(store.get_note(&b).unwrap().is_none());
    assert_eq!(store.stats().unwrap().notes, 0);
}

#[test]
fn apply_folder_tombstone_and_purge_from_remote() {
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);
    store
        .mark_synced(EntityKind::Folder, &folder, store.get_folder(&folder).unwrap().unwrap().rev, "sha256:f")
        .unwrap();
    let f = store.create_folder(Some(&folder), "远端会删掉它").unwrap();
    store.mark_synced(EntityKind::Folder, &f.id, f.rev, "sha256:ff").unwrap();
    let n = create(&store, &f.id, "夹在里面的笔记");
    store.mark_synced(EntityKind::Note, &n.id, n.rev, &n.content_hash).unwrap();

    // 远端删除 → 本地软删 + 墓碑
    store.apply_remote(&[ApplyOp::Tombstone { kind: EntityKind::Folder, id: f.id.clone(), rev: Rev(5) }]).unwrap();
    let after = store.get_folder(&f.id).unwrap().unwrap();
    assert!(after.deleted_at.is_some());
    assert!(store.get_tombstone(EntityKind::Folder, &f.id).unwrap().unwrap().purged == false);
    assert!(store.get_note(&n.id).unwrap().is_some(), "远端删文件夹也不能级联删笔记");
    assert!(store.verify().is_empty(), "{:?}", store.verify());

    // 远端永久删除（干净实体）→ 行消失 + purged 墓碑
    store.apply_remote(&[ApplyOp::Purge { kind: EntityKind::Note, id: n.id.clone() }]).unwrap();
    assert!(store.get_note(&n.id).unwrap().is_none());
    let t = store.get_tombstone(EntityKind::Note, &n.id).unwrap().expect("墓碑");
    assert!(t.purged);
    // 墓碑被确认（remote index）后不再算脏
    let dirty = store.dirty_entities(notera_store::LOCAL_ACCOUNT_ID).unwrap();
    assert!(dirty.iter().any(|d| d.id == n.id && d.why == DirtyWhy::Purged));
    store.mark_synced(EntityKind::Note, &n.id, Rev(6), "sha256:0").unwrap();
    assert!(!store.dirty_entities(notera_store::LOCAL_ACCOUNT_ID).unwrap().iter().any(|d| d.id == n.id));
    // 脏实体上的远端永久删除 → 拒绝（C1/C4）
    let m = create(&store, &folder, "本地未推送的修改");
    let e = store.apply_remote(&[ApplyOp::Purge { kind: EntityKind::Note, id: m.id.clone() }]).unwrap_err();
    assert!(is_rejection(&e), "实际 {e:?}");
    assert!(store.get_note(&m.id).unwrap().is_some(), "用户内容不因远端永久删除而静默消失");
}

#[test]
fn mark_synced_refuses_to_lead_local_head_and_attachment_kind_is_explicit() {
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);
    let n = create(&store, &folder, "确认点不能领先");
    assert!(matches!(
        store.mark_synced(EntityKind::Note, &n.id, Rev(50), "sha256:y"),
        Err(StoreError::Rejected(_))
    ));
    assert_eq!(store.get_note(&n.id).unwrap().unwrap().sync_rev, Rev(0));
    assert!(matches!(
        store.mark_synced(EntityKind::Note, &missing_id(), Rev(1), "sha256:y"),
        Err(StoreError::NotFound { .. })
    ));
    // 附件以 sha256 寻址：契约的 EntityId 表达不了，必须走专用入口（见交付说明）
    assert!(matches!(
        store.mark_synced(EntityKind::Attachment, &n.id, Rev(1), "sha256:y"),
        Err(StoreError::Constraint(_))
    ));
}

#[test]
fn attachments_are_content_addressed_and_deduplicated() {
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);
    let n = create(&store, &folder, "带附件的笔记");
    let bytes = b"\x89PNG\r\n\x1a\n fake png payload".to_vec();
    let sha = notera_crypto::sha256_hex(&bytes);

    let a1 = store.attach_blob(&n.id, &bytes, "image/png", Some("a.png"), "blk000001").unwrap();
    assert_eq!(a1.sha256, sha);
    assert_eq!(a1.local_state, "available");
    assert_eq!(a1.remote_state, "unknown");
    assert_eq!(a1.role, "inline", "image/* → inline");
    let path = store.blob_path(&sha);
    assert_eq!(path, fx.dir.join("attachments").join(&sha[..2]).join(&sha), "落盘路径 = <2hex>/<sha>");
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    let parts: Vec<_> = std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.ends_with(".part"))
        .collect();
    assert!(parts.is_empty(), "临时文件必须被 rename 掉：{parts:?}");

    // 同一 blob 再挂一次：内容寻址去重，引用计数派生
    let n2 = create(&store, &folder, "第二个引用");
    let a2 = store.attach_blob(&n2.id, &bytes, "application/pdf", Some("a.pdf"), "blk000001").unwrap();
    assert_eq!(a2.sha256, sha);
    assert_eq!(a2.role, "file");
    assert_eq!(store.stats().unwrap().attachments, 1, "同一 sha256 只存一行");
    assert_eq!(store.attachment_refs(&sha).unwrap(), 2);

    let with = store.get_note(&n.id).unwrap().unwrap();
    assert!(with.has_attachment, "note_attachments 链接要反映到派生列（I5）");
    assert!(with.rev > n.rev, "派生列变化也要推进 rev");
    assert!(store.verify().is_empty(), "{:?}", store.verify());

    // 生命周期状态迁移（§8）
    store.set_attachment_states(&sha, None, Some("present")).unwrap();
    assert_eq!(store.stats().unwrap().attachment_bytes, bytes.len() as u64);
    assert!(matches!(store.set_attachment_states(&sha, Some("bogus"), None), Err(StoreError::Constraint(_))));
    store.enqueue_download(&sha).unwrap();
    let ups = store
        .outbox_take(notera_store::LOCAL_ACCOUNT_ID, 100)
        .unwrap()
        .into_iter()
        .filter(|o| o.kind == EntityKind::Attachment)
        .collect::<Vec<_>>();
    assert!(ups
        .iter()
        .any(|o| o.op == OpKind::Upload && o.sha256.as_deref() == Some(sha.as_str()) && o.entity_key == sha),
        "附件上行待办的键必须是 sha256");
    assert!(ups.iter().any(|o| o.op == OpKind::Download && o.entity_key == sha), "附件的键是 sha256");

    // 永久删除笔记：链接随 CASCADE 消失、blob 行仍在（回收交给引用计数）
    store.purge_note(&n.id).unwrap();
    assert_eq!(store.attachment_refs(&sha).unwrap(), 1);
    assert_eq!(store.stats().unwrap().attachments, 1);
    assert!(path.exists(), "还有 1 个引用 → blob 不能删（§8）");
    assert!(store.verify().is_empty(), "{:?}", store.verify());
}

#[test]
fn conflicts_are_recorded_and_openable_then_resolved() {
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);
    let n = create(&store, &folder, "冲突的本地侧");
    let id = store
        .record_conflict(&ConflictRecord {
            account_id: "acct-a".into(),
            kind: EntityKind::Note,
            id: n.id.clone(),
            base_rev: Rev(0),
            local_rev: n.rev,
            remote_rev: Rev(4),
            local_hash: n.content_hash.clone(),
            remote_hash: "sha256:".to_string() + &"b".repeat(64),
            auto_merged: false,
            copy_note_id: None,
        })
        .unwrap();
    assert!(id > 0);
    let open = store.open_conflicts().unwrap();
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].id, n.id);
    assert_eq!(open[0].state, notera_store::ConflictState::Open);
    assert!(!open[0].auto_merged);
    store.resolve_conflict(id, "kept_both").unwrap();
    assert!(store.open_conflicts().unwrap().is_empty(), "裁决后不再是 open");
    assert!(matches!(store.resolve_conflict(id, "乱写"), Err(StoreError::Constraint(_))));
    let _ = NoteQuery::all();
}

#[test]
fn probed_caps_survive_a_restart_and_null_means_never_probed() {
    // §5 的探测结果必须能存下来：否则每次启动都退回保守默认，
    // 一台真的支持条件写的服务器会被永久当成 S3 盲写。
    let fx = Fix::new();
    let store = fx.open();
    store.register_account("acct-c", "c", "https://dav.example/dav").unwrap();

    assert_eq!(store.account_caps("acct-c").unwrap(), None, "新登记的账户应当是『从未探测』，而不是『零能力』");
    store.set_account_caps("acct-c", 0b101).unwrap();
    assert_eq!(store.account_caps("acct-c").unwrap(), Some(0b101));

    // 0 是合法值（什么都不支持），必须与 NULL 区分开
    store.set_account_caps("acct-c", 0).unwrap();
    assert_eq!(store.account_caps("acct-c").unwrap(), Some(0), "全不支持被存成了从未探测");

    drop(store);
    let again = fx.open();
    assert_eq!(again.account_caps("acct-c").unwrap(), Some(0), "重启后探测结果必须还在");
    assert!(matches!(again.set_account_caps("nope", 1), Err(StoreError::Constraint(_))), "不存在的账户要报错");
}

#[test]
fn settle_locates_a_row_by_entity_and_rev_and_touches_nothing_else() {
    // 引擎结清待办时手里只有 (kind, id, rev)。按这三样定位必须精确：
    // 猜错行 = 把另一条还没上传的待办标成已完成，那是静默漏同步。
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);
    let acct = notera_store::LOCAL_ACCOUNT_ID;
    let n = create(&store, &folder, "第一版");
    // 此时 local 名下有两行：默认本 + 笔记
    assert_eq!(store.outbox_len(acct, &[OpState::Pending]).unwrap(), 2);

    assert!(
        store.outbox_settle(acct, EntityKind::Note, n.id.as_str(), n.rev.get() as i64, OpState::Done).unwrap(),
        "按实体 + rev 必须能定位到那一行"
    );
    assert_eq!(store.outbox_len(acct, &[OpState::Done]).unwrap(), 1, "只结清指定的那一行");
    assert_eq!(store.outbox_len(acct, &[OpState::Pending]).unwrap(), 1, "默认本那行不该被顺手标掉");

    // rev 对不上 = 找不到，如实返回 false（而不是"随便结一行"）
    assert!(!store.outbox_settle(acct, EntityKind::Note, n.id.as_str(), 999, OpState::Done).unwrap());
    // 账户也在键里：别的账户没有这一行
    assert!(!store.outbox_settle("acct-other", EntityKind::Note, n.id.as_str(), n.rev.get() as i64, OpState::Done).unwrap());
    // 已 done 的行不会被第二次结清改写状态
    assert!(!store.outbox_settle(acct, EntityKind::Note, n.id.as_str(), n.rev.get() as i64, OpState::Failed).unwrap());
    assert_eq!(store.outbox_len(acct, &[OpState::Done]).unwrap(), 1, "重复结清不许把 done 改成 failed");
    // kind 对不上同样不许命中：同 id/同 rev 也不会跨实体类型误结
    assert!(!store.outbox_settle(acct, EntityKind::Folder, n.id.as_str(), n.rev.get() as i64, OpState::Done).unwrap());
}
