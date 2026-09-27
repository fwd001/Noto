//! 冲突卡片要能显示"服务器那一版"的正文（CONFLICT-RESOLUTION §5.1.1，迁移 0008）。
//!
//! 这里钉的是存储侧三条容易做错、又都不会报错的地方：
//! 1. 载荷只挂到**最新一条未裁决**冲突上（往已裁决的行上写会造出"处理完却被改动"，
//!    而且面板上根本没有那张卡了）；
//! 2. 读回来必须走 `open_conflicts` —— 也就是**界面真正用的那条路**，
//!    不是另开一个只有测试会调的 getter（那种 API 会假装自己被验过）；
//! 3. 账户之间不许串。
//!
//! 端到端那一半（引擎取料 → 宿主登记 → 卡片可读）在
//! `notera-host/tests/conflict_payload_e2e.rs`，两台真设备。

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

fn wire_of(store: &Store, id: &EntityId) -> Option<String> {
    store
        .open_conflicts()
        .expect("open_conflicts")
        .into_iter()
        .find(|r| &r.id == id)
        .and_then(|r| r.remote_wire)
}

#[test]
fn the_payload_lands_on_the_conflict_the_panel_actually_shows() {
    let store = store_in("happy");
    let id = note(&store);
    record(&store, notera_store::LOCAL_ACCOUNT_ID, &id, 9);
    let hit = store
        .conflict_attach_payload(
            notera_store::LOCAL_ACCOUNT_ID,
            EntityKind::Note,
            &id,
            &envelope(9, "对面那一版"),
        )
        .expect("挂载荷");
    assert_eq!(hit, 1, "该正好命中那一条未裁决冲突");
    let got = wire_of(&store, &id).expect("面板那条卡片该带得上载荷");
    assert!(
        got.contains("对面那一版"),
        "读回来的必须是被删那一版的原文：{got}"
    );
    assert!(got.contains("\"deleted_at\""), "要留得住删除事实：{got}");
}

#[test]
fn the_newest_open_row_is_the_one_that_gets_the_payload() {
    // 同一条笔记可能被登记过不止一次；卡片列表按时间倒序展示，用户看到的是最新那条。
    // 载荷挂到旧行上 = 挂到一张已经不显示的卡片上，等于没挂。
    let store = store_in("newest");
    let id = note(&store);
    record(&store, notera_store::LOCAL_ACCOUNT_ID, &id, 5);
    record(&store, notera_store::LOCAL_ACCOUNT_ID, &id, 9);
    store
        .conflict_attach_payload(
            notera_store::LOCAL_ACCOUNT_ID,
            EntityKind::Note,
            &id,
            &envelope(9, "最新那一版"),
        )
        .expect("挂载荷");
    let rows = store.open_conflicts().expect("open_conflicts");
    let with_payload: Vec<_> = rows
        .iter()
        .filter(|r| r.remote_wire.is_some())
        .map(|r| r.remote_rev.get())
        .collect();
    assert_eq!(
        with_payload,
        vec![9],
        "只该挂到 remote_rev=9 那条最新的行上：{rows:?}"
    );
}

#[test]
fn dismissed_conflicts_never_receive_or_expose_a_payload() {
    let store = store_in("dismissed");
    let id = note(&store);
    record(&store, notera_store::LOCAL_ACCOUNT_ID, &id, 9);
    let cid = store
        .open_conflicts()
        .expect("open")
        .into_iter()
        .next()
        .expect("刚登记的那条")
        .conflict_id;
    store
        .conflict_attach_payload(
            notera_store::LOCAL_ACCOUNT_ID,
            EntityKind::Note,
            &id,
            &envelope(9, "甲"),
        )
        .expect("挂载荷");
    store.dismiss_conflict(cid).expect("收卡");
    assert_eq!(wire_of(&store, &id), None, "收掉的卡片不该还往外递载荷");
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
    let none = store
        .conflict_attach_payload("acct-B", EntityKind::Note, &id, &envelope(9, "B 的那一版"))
        .expect("B 挂载荷");
    assert_eq!(none, 0, "账户之间不许串");
    let got = store
        .open_conflicts()
        .expect("open")
        .into_iter()
        .find(|r| r.account_id == "acct-A")
        .and_then(|r| r.remote_wire)
        .expect("A 的那一版该读得回来");
    assert!(got.contains("A 的那一版"), "{got}");
}

#[test]
fn no_payload_just_means_not_fetched_yet() {
    // 取料会失败（请求预算用尽、记录 404、网络断了）。这时卡片仍必须在，只是右栏
    // 退回说明"这一版没取回来" —— 所以载荷是 None 而冲突照旧在册。
    let store = store_in("none");
    let id = note(&store);
    record(&store, notera_store::LOCAL_ACCOUNT_ID, &id, 9);
    assert_eq!(wire_of(&store, &id), None);
    assert_eq!(
        store.open_conflicts().expect("未裁决冲突还在册").len(),
        1,
        "没取回来绝不能等于冲突消失"
    );
}
