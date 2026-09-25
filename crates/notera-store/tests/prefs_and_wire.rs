//! 两个 host 需要的存储面：`settings` 偏好（DATA-MODEL §4.1）与记录 wire（§11 / SYNC-PROTOCOL §3）。
//!
//! wire 的验收方式是**往返**：这里构造的字节必须能被 `apply_remote` 的 I6 闸门接受，
//! 并在另一台设备上还原出同一 rev / 同一 content_hash 的实体 —— 这同时也是
//! "与 `tests/common::note_envelope` 同一形状"的证明（走的是同一个解析器）。
mod common;

use common::*;
use notera_core::{hash_json, EntityKind};
use notera_store::{ApplyOp, StoreError, LOCAL_ACCOUNT_ID};
use serde_json::Value;

fn wire_to_json(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).expect("wire 必须是合法 JSON")
}

// ------------------------------------------------------------------ 偏好 ---

#[test]
fn set_pref_upserts_and_roundtrips_latest_value() {
    let fx = Fix::new();
    let store = fx.open();

    store.set_pref("theme", &serde_json::json!("dark")).unwrap();
    store.set_pref("font_size", &serde_json::json!(15)).unwrap();
    let p = store.get_prefs().unwrap();
    assert_eq!(p["theme"], serde_json::json!("dark"));
    assert_eq!(p["font_size"], serde_json::json!(15));

    // 同键再写 = 覆盖。若 upsert 写成了第二条，SQLite 的表达式唯一索引会直接报约束错。
    store.set_pref("theme", &serde_json::json!("light")).unwrap();
    let p = store.get_prefs().unwrap();
    assert_eq!(p["theme"], serde_json::json!("light"), "必须取到最新值");
    assert_eq!(p["font_size"], serde_json::json!(15), "覆盖一个键不得动别的键");
    drop(store);
    let again = fx.open();
    assert_eq!(
        again.get_prefs().unwrap()["theme"],
        serde_json::json!("light"),
        "偏好必须落到 WAL 里的真实行，而不是内存缓存"
    );
}

#[test]
fn pref_keeps_json_shape_not_stringified() {
    let fx = Fix::new();
    let store = fx.open();
    let v = serde_json::json!({ "collapsed": [true, false], "opacity": 0.5, "extra": null });
    store.set_pref("sidebar", &v).unwrap();
    assert_eq!(store.get_prefs().unwrap()["sidebar"], v, "偏好按 JSON 结构回来（CHECK json_valid 的前提）");
}

#[test]
fn prefs_never_enqueue_outbox_work_and_empty_key_is_rejected() {
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);
    create(&store, &folder, "占位笔记");
    let pending = store.stats().unwrap().outbox_pending;
    let dirty = store.dirty_entities(LOCAL_ACCOUNT_ID).unwrap().len();

    store.set_pref("theme", &serde_json::json!("dark")).unwrap();
    assert_eq!(store.stats().unwrap().outbox_pending, pending, "改偏好不得产生上传待办（§6：UI 态永不上传）");
    assert_eq!(store.dirty_entities(LOCAL_ACCOUNT_ID).unwrap().len(), dirty, "偏好不得把任何实体变脏");

    let e = store.set_pref("   ", &serde_json::json!(1)).unwrap_err();
    assert!(matches!(e, StoreError::Constraint(_)), "空键必须是约束错误，实际 {e:?}");
    assert_eq!(store.get_prefs().unwrap().as_object().unwrap().len(), 1, "被拒的写入不得留下行");
}

// ------------------------------------------------------------------ wire ---

#[test]
fn note_wire_survives_apply_remote_on_another_device_with_same_rev_and_hash() {
    let a = Fix::new();
    let b = Fix::new();
    let (sa, sb) = (a.open(), b.open());
    let folder = default_folder(&sa);
    let note = create(&sa, &folder, "跨设备还原");

    // 笔记的父本在对端不存在 —— 先复刻文件夹（真实 bootstrap 的顺序，R2 的先实体后清单）。
    let fw = sa.folder_envelope_wire(&folder).unwrap().expect("默认本自身也是待上传实体");
    sb.apply_remote(&[ApplyOp::UpsertFolder { env: wire_to_json(&fw) }]).unwrap();

    let w = sa.note_envelope_wire(&note.id).unwrap().expect("刚写的笔记必须有 wire");
    let env = wire_to_json(&w);
    assert_eq!(env["kind"], "note", "§3：kind 必须与远端目录一致");
    assert_eq!(env["id"], note.id.as_str());
    assert_eq!(env["rev"], note.rev.get() as u64);
    assert_eq!(env["hash"], note.content_hash.as_str(), "hash 就是行上的 content_hash");
    assert_eq!(env["purged"], false);
    assert_eq!(env["ct"], Value::Null, "alg=none 时 payload 与 ct 恰好一个非空");
    assert!(env["payload"]["content"].is_array(), "payload 必须带正文");
    assert_eq!(env["payload"]["folder_id"], folder.as_str(), "§6：folder_id 属同步内容");

    let rep = sb.apply_remote(&[ApplyOp::UpsertNote { env }]).unwrap();
    assert_eq!(rep.applied, 1);
    let got = sb.get_note(&note.id).unwrap().expect("对端必须读到该笔记");
    assert_eq!(got.rev, note.rev, "rev 必须无损还原");
    assert_eq!(got.content_hash, note.content_hash, "content_hash 必须无损还原");
    assert_eq!(got.doc, note.doc);
    assert_eq!(got.folder_id, folder);
}

#[test]
fn note_wire_is_idempotent_on_the_store_it_came_from() {
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);
    let note = create(&store, &folder, "幂等回放");
    let w = store.note_envelope_wire(&note.id).unwrap().unwrap();
    let rep = store.apply_remote(&[ApplyOp::UpsertNote { env: wire_to_json(&w) }]).unwrap();
    assert_eq!((rep.applied, rep.skipped, rep.notes_written), (0, 1, 0), "同 rev 同 hash 必须幂等跳过");
    let after = store.get_note(&note.id).unwrap().unwrap();
    assert_eq!((after.rev, after.content_hash, after.doc), (note.rev, note.content_hash.clone(), note.doc));
    assert_eq!(after.sync_rev, note.sync_rev, "幂等回放不得推进任何 rev（I2）");
}

#[test]
fn folder_wire_hash_equals_canonical_payload_hash() {
    let fx = Fix::new();
    let store = fx.open();
    let f = store.create_folder(None, "工作").unwrap();
    let w = store.folder_envelope_wire(&f.id).unwrap().unwrap();
    let env = wire_to_json(&w);
    assert_eq!(env["kind"], "folder");
    // SYNC-PROTOCOL §3 约束表：hash == sha256(canonical(payload))
    assert_eq!(env["hash"].as_str().unwrap(), hash_json(&env["payload"]).as_str());
    assert_eq!(env["hash"], f.content_hash);

    let b = Fix::new();
    let sb = b.open();
    let rep = sb.apply_remote(&[ApplyOp::UpsertFolder { env }]).unwrap();
    assert_eq!(rep.folders_written, 1);
    let got = sb.get_folder(&f.id).unwrap().unwrap();
    assert_eq!((got.rev, got.content_hash, got.name), (f.rev, f.content_hash.clone(), f.name));
}

#[test]
fn deleted_note_wire_carries_deleted_at_and_purge_is_an_announcement() {
    let a = Fix::new();
    let sa = a.open();
    let folder = default_folder(&sa);
    let note = create(&sa, &folder, "会被永久删除");

    sa.delete_note(&note.id).unwrap();
    let deleted = wire_to_json(&sa.note_envelope_wire(&note.id).unwrap().unwrap());
    assert!(deleted["deleted_at"].is_string(), "软删是带 deleted_at 的 upsert（ADR-0006）");
    assert_eq!(deleted["purged"], false);
    assert!(deleted["payload"].is_object(), "软删记录仍带正文（可被恢复）");

    // 永久删除后行不存在：wire 变成墓碑公告（payload: null）
    sa.purge_note(&note.id).unwrap();
    let tomb = sa.get_tombstone(EntityKind::Note, &note.id).unwrap().expect("purged 墓碑必须留存");
    let ann = sa.note_envelope_wire(&note.id).unwrap().expect("永久删除必须可传播，否则是静默同步失败");
    let ann = wire_to_json(&ann);
    assert_eq!(ann["purged"], true);
    assert!(ann["payload"].is_null(), "§3：purged 记录 payload 为 null");
    assert_eq!(ann["rev"], tomb.rev.get() as u64);
    assert_eq!(ann["deleted_at"], tomb.deleted_at.as_str());

    // 对端应用公告后：读不到行，而且旧内容再 PUT 上来必须被拒
    let b = Fix::new();
    let sb = b.open();
    sb.apply_remote(&[ApplyOp::UpsertNote { env: ann }]).unwrap();
    assert!(sb.get_note(&note.id).unwrap().is_none());
    let e = sb.apply_remote(&[ApplyOp::UpsertNote { env: deleted }]).unwrap_err();
    assert!(matches!(e, StoreError::Rejected(_)), "复活已 purge 的笔记必须被拒，实际 {e:?}");
    assert!(sb.get_note(&note.id).unwrap().is_none(), "被拒的复活不得留下半条数据");
}

#[test]
fn wire_of_unknown_or_unsyncable_entity_is_none_not_garbage() {
    let fx = Fix::new();
    let store = fx.open();
    assert_eq!(store.note_envelope_wire(&missing_id()).unwrap(), None);
    assert_eq!(store.folder_envelope_wire(&missing_id()).unwrap(), None);
    // 只有 purged 墓碑才值得发公告；软删的文件夹没有墓碑行，但记录仍在 → 仍有 wire（带 deleted_at）
    let f = store.create_folder(None, "待删").unwrap();
    store.delete_folder(&f.id).unwrap();
    let w = store.folder_envelope_wire(&f.id).unwrap().expect("删除事实也要能上传");
    assert!(wire_to_json(&w)["deleted_at"].is_string());
}
