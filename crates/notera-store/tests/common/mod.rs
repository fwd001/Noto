//! 集成测试公共夹具：临时目录 + Store 打开 + 文档/信封构造。
//!
//! 只用真实 SQLite 文件（`tempfile` 目录），不 mock：崩溃/重启类断言必须有真磁盘。
#![allow(dead_code)]
use notera_core::{ContentHash, DeviceId, EntityId, Rev};
use notera_store::{Note, Store, StoreError};
use serde_json::{json, Value};
use std::path::PathBuf;

/// 一个临时库目录（Drop 时清理）。测试里必须持有它，否则目录被删。
pub struct Fix {
    pub dir: PathBuf,
    pub device: DeviceId,
    keep: tempfile::TempDir,
}

impl Fix {
    pub fn new() -> Self {
        let keep = tempfile::tempdir().expect("临时目录");
        let dir = keep.path().to_path_buf();
        Self {
            dir,
            device: DeviceId::new(),
            keep,
        }
    }

    pub fn open(&self) -> Store {
        Store::open(&self.dir, self.device.clone()).expect("open Store")
    }

    pub fn reopen(&self) -> Store {
        self.open()
    }

    pub fn db_file(&self) -> PathBuf {
        self.dir.join("notera.sqlite")
    }
}

/// 块 id：8..32 字符（DATA-MODEL §10.2），文档内唯一。
pub fn blk(i: usize) -> String {
    format!("blk{i:06}")
}

/// 单段落文档。
pub fn doc_text(text: &str) -> Value {
    json!({
        "v": 1,
        "content": [{ "id": blk(1), "type": "paragraph", "content": [{ "text": text }] }]
    })
}

/// 标题 + 正文（title 由第一个 heading 派生）。
pub fn doc_heading(title: &str, body: &str) -> Value {
    json!({
        "v": 1,
        "content": [
            { "id": blk(1), "type": "heading", "attrs": { "level": 1 }, "content": [{ "text": title }] },
            { "id": blk(2), "type": "paragraph", "content": [{ "text": body }] }
        ]
    })
}

/// 默认本之外的文件夹 id 由调用方创建；这里给一个"不存在"的 id 用于负例。
pub fn missing_id() -> EntityId {
    EntityId::parse("00000000-0000-0000-0000-000000000001").expect("固定 UUID")
}

/// 当前 doc 的权威哈希（= Store 内部算法：richtext canonical → sha256）。
pub fn hash_of(doc: &Value) -> String {
    let parsed = notera_richtext::parse_from_value(doc).expect("测试文档必须合法");
    ContentHash::of(notera_richtext::canonical(&parsed).as_bytes())
        .as_str()
        .to_string()
}

/// 造一条 `enc.alg=none` 的记录信封（DATA-MODEL §11 / SYNC-PROTOCOL §3）。
pub fn note_envelope(id: &EntityId, rev: u64, doc: &Value, folder: Option<&EntityId>) -> Value {
    let mut payload = doc.clone();
    if let Some(f) = folder {
        payload["folder_id"] = json!(f.as_str());
    }
    json!({
        "protocol": 1, "kind": "note", "id": id.as_str(), "rev": rev,
        "hash": hash_of(doc), "updated_at": "2026-09-25T10:00:00.000Z",
        "device": "01920000-0000-7000-8000-000000000000",
        "deleted_at": null, "purged": false,
        "enc": { "alg": "none", "hash_alg": "sha256" },
        "payload": payload, "ct": null
    })
}

pub fn folder_envelope(id: &EntityId, rev: u64, name: &str, parent: Option<&EntityId>) -> Value {
    json!({
        "protocol": 1, "kind": "folder", "id": id.as_str(), "rev": rev,
        "hash": format!("sha256:{}", "0".repeat(64)),
        "updated_at": "2026-09-25T10:00:00.000Z",
        "deleted_at": null, "purged": false,
        "enc": { "alg": "none", "hash_alg": "sha256" },
        "payload": { "name": name, "parent_id": parent.map(|p| p.as_str().to_string()), "color": null, "sort_order": 0 },
        "ct": null
    })
}

pub fn purged_envelope(id: &EntityId, rev: u64) -> Value {
    json!({
        "protocol": 1, "kind": "note", "id": id.as_str(), "rev": rev,
        "hash": format!("sha256:{}", "0".repeat(64)), "deleted_at": "2026-09-25T10:00:00.000Z",
        "purged": true, "enc": { "alg": "none" }, "payload": null, "ct": null
    })
}

/// 便捷写：在默认本里建一条笔记。
pub fn create(store: &Store, folder: &EntityId, text: &str) -> Note {
    store
        .create_note(folder, doc_text(text))
        .expect("create_note")
}

/// 默认本 id：`list_folders` 里 `system_kind = Some("default")` 的那一个。
pub fn default_folder(store: &Store) -> EntityId {
    store
        .list_folders()
        .expect("list_folders")
        .into_iter()
        .find(|f| f.system_kind.as_deref() == Some("default"))
        .expect("必须存在默认本")
        .id
}

pub fn is_stale(e: &StoreError) -> bool {
    matches!(e, StoreError::StaleEdit(_))
}

pub fn is_rejection(e: &StoreError) -> bool {
    matches!(e, StoreError::Rejected(_) | StoreError::InvalidDoc(_))
}

pub const R: u64 = 1; // Rev 的原始值别名，便于写 expected_rev

pub fn rev(v: u64) -> Rev {
    Rev(v)
}
