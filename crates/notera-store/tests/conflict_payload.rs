//! 冲突卡片要显示"服务器那一版"的正文（CONFLICT-RESOLUTION §5.1.1，迁移 0008）。
//!
//! 这里钉的是三条容易做错、又都不会报错的地方：
//! 1. 载荷只挂到**最新一条未裁决**冲突上（往已裁决的行上写会造出"处理完却被改动"）；
//! 2. 读的时候 **rev 必须对得上**（对面那一版是 rev=N，挂着的却是别的 rev 就是给错内容）；
//! 3. 读不越账户边界（同一条笔记在两个账户下各有一行时不能串）。

use notera_core::{DeviceId, EntityId, EntityKind, Rev};
use notera_store::{ConflictRecord, Store};
use std::path::PathBuf;

fn store_in(tag: &str) -> Store {
    let dir = std::env::temp_dir().join(format!(
        "notera-store-conflict-{}-{tag}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("临时目录");
    Store::open(&PathBuf::from(&dir).join("db"), DeviceId::new()).expect("Store::open")
}

fn default_folder(store: &Store) -> EntityId {
    store
        .list_folders()
        .expect("list_folders")
        .into_iter()
        .find(|f| f.system_kind.as_deref() == Some("default"))
        .expect("默认本必须存在")
        .id
}

/// 造一条真笔记（冲突行指的是它，`remote_wire` 才有地方挂）。
fn note(store: &Store) -> EntityId {
    let doc = serde_json::json!({
        "v": 1,
        "content": [{ "id": "aaaa1111", "type": "paragraph",
                      "content": [{ "text": "本机这一版" }] }],
    });
    store
        .create_note(&default_folder(store), doc)
        .expect("create_note")
        .id
}

fn record(store: &Store, account: &str, id: &EntityId, remote_rev: u64) {
    store
        .record_conflict(&ConflictRecord {
            account_id: account.into(),
            kind: EntityKind::Note,
            id: id.clone(),
            base_rev: Rev(7),
            local_rev: Rev(8),
            remote_rev: Rev(remote_rev),
            local_hash: "localhash".into(),
            remote_hash: "remotehash".into(),
            auto_merged: false,
            copy_note_id: None,
        })
        .expect("record_conflict");
}

/// 服务器信封的样子（引擎 `fetch_record` 拿回来的就是这份字节）。
fn envelope(rev: u64, text: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "protocol": 1, "kind": "n", "rev": rev, "hash": "remotehash",
        "updated_at": "2026-09-27T00:00:00Z", "deleted_at": "2026-09-26T00:00:00Z",
        "payload": { "v": 1, "content": [{ "id": "bbbb2222", "type": "paragraph",
                        "content": [{ "text": text }] }] },
    }))
    .unwrap()
}

#[test]
fn the_payload_lands_on_the_newest_open_conflict_and_reads_back() {
    let store = store_in("happy");
    let id = note(&store);
    record(&store, notera_store::LOCAL_ACCOUNT_ID, &id, 9);
    let wire = envelope(9, "对面那一版");

    let hit = store
        .conflict_attach_payload(notera_store::LOCAL_ACCOUNT_ID, EntityKind::Note, &id, &wire)
        .expect("挂载荷");
    assert_eq!(hit, 1, "该正好命中那一条未裁决冲突");

    let got = store
        .conflict_remote_wire(&id, 9)
        .expect("读载荷")
        .expect("rev 对得上，该读得回来");
    assert!(
        got.contains("对面那一版"),
        "读回来的必须是被删那一版的原文：{got}"
    );
    // 卡片拿这份原文之后自己算摘要（复用 preview_text 那条路），这里只保证字节没被改过形状
    assert!(got.contains("\"deleted_at\""), "要留得住删除事实：{got}");
}

#[test]
fn a_rev_that_is_not_the_one_on_the_row_reads_as_none() {
    // 对面那一版是 rev=9。若界面问 rev=3，宁可返回 None 让它照实说"没取回来"，
    // 也不能把 rev=9 的内容当成 rev=3 的答案递过去 —— 那是给用户看一份错的东西。
    let store = store_in("rev");
    let id = note(&store);
    record(&store, notera_store::LOCAL_ACCOUNT_ID, &id, 9);
    store
        .conflict_attach_payload(
            notera_store::LOCAL_ACCOUNT_ID,
            EntityKind::Note,
            &id,
            &envelope(9, "甲"),
        )
        .expect("挂载荷");
    assert_eq!(store.conflict_remote_wire(&id, 3).expect("读"), None);
    assert!(store.conflict_remote_wire(&id, 9).expect("读").is_some());
}

#[test]
fn resolved_conflicts_never_receive_or_expose_a_payload() {
    let store = store_in("resolved");
    let id = note(&store);
    let cid = record_id(&store, &id);
    store
        .conflict_attach_payload(
            notera_store::LOCAL_ACCOUNT_ID,
            EntityKind::Note,
            &id,
            &envelope(9, "甲"),
        )
        .expect("挂载荷");
    store.dismiss_conflict(cid).expect("收卡");
    // 裁决之后：既读不到（面板上已经没有这张卡片），也不许再被挂上（0 行）
    assert_eq!(store.conflict_remote_wire(&id, 9).expect("读"), None);
    let again = store
        .conflict_attach_payload(
            notera_store::LOCAL_ACCOUNT_ID,
            EntityKind::Note,
            &id,
            &envelope(9, "乙"),
        )
        .expect("再挂");
    assert_eq!(again, 0, "已裁决的冲突不该被补写载荷，也不该被重新打开");
}

#[test]
fn payloads_do_not_leak_across_accounts() {
    let store = store_in("acct");
    let id = note(&store);
    record(&store, "acct-A", &id, 9);
    store
        .conflict_attach_payload("acct-A", EntityKind::Note, &id, &envelope(9, "A 的那一版"))
        .expect("挂载荷");
    // 换账户查同一条笔记：没有它的未裁决冲突，就该什么都读不到
    let none = store
        .conflict_attach_payload("acct-B", EntityKind::Note, &id, &envelope(9, "B 的那一版"))
        .expect("B 挂载荷");
    assert_eq!(none, 0, "账户之间不许串");
    assert!(
        store.conflict_remote_wire(&id, 9).expect("读").is_some(),
        "A 的那一版该读得回来 —— 账户之间不许串"
    );
}

#[test]
fn no_payload_just_means_not_fetched_yet() {
    // 取料会失败（请求预算用尽、记录 404、网络断了）。这时卡片仍必须在，只是右栏
    // 退回哈希并说明没取到 —— 所以这里读出来是 None，而不是"冲突消失了"。
    let store = store_in("none");
    let id = note(&store);
    record(&store, notera_store::LOCAL_ACCOUNT_ID, &id, 9);
    assert_eq!(store.conflict_remote_wire(&id, 9).expect("读"), None);
    assert_eq!(store.open_conflicts().expect("未裁决冲突还在册").len(), 1);
}

fn record_id(store: &Store, id: &EntityId) -> i64 {
    record(store, notera_store::LOCAL_ACCOUNT_ID, id, 9);
    store
        .open_conflicts()
        .expect("open_conflicts")
        .into_iter()
        .next()
        .expect("刚登记的那条")
        .conflict_id
}
