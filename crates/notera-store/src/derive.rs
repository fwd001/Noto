//! richtext 桥：`doc` 的校验 / canonical / 哈希 / 派生列（DATA-MODEL §7.1、§10.3）。
//!
//! 写入顺序被 §10.3 钉死：`parse`（内含 normalize + validate）→ `canonical` → `sha256`。
//! 任一步失败即返回 `Err`，权威表一行都不写（I6）。
//! 落库的 `notes.doc` 就是 canonical 文本，因此 `content_hash == sha256(doc)` 恒可复核
//! —— [`crate::store::Store::verify`] 正是这样复查的。

use crate::error::StoreError;
use notera_core::ContentHash;

#[derive(Clone, Debug)]
pub(crate) struct Prepared {
    /// 存入 `notes.doc` 的 canonical JSON 文本。
    pub doc_json: String,
    pub doc_version: u16,
    pub content_hash: ContentHash,
    pub title: String,
    pub plain_text: String,
    pub summary: String,
    pub char_count: u32,
    pub block_count: u32,
    /// **仅**由 doc 推出的附件标记；与 `note_attachments` 链接取或后才落库（§7.1）。
    pub doc_has_attachment: bool,
    /// doc 里引用的附件（sha 已核验形态）。收到远端记录时据此登记 `attachments` /
    /// `note_attachments`，见 [`crate::apply`] 与 DATA-MODEL §8。
    pub attachments: Vec<notera_richtext::BlockAttachment>,
}

/// 从 JSON 值准备一次写入（`Store` 写入口与 `apply_remote` 共用同一条闸门）。
pub(crate) fn prepare(value: &serde_json::Value) -> Result<Prepared, StoreError> {
    // §10.4：doc.v 超前 → 只读闸门（写路径必须拒绝；读路径另用 parse_for_read）
    if let Some(v) = value.get("v").and_then(|x| x.as_u64()) {
        let v = v as u16;
        if !notera_richtext::supports(v) {
            return Err(StoreError::DocTooNew { doc: v, supported: notera_richtext::DOC_FORMAT });
        }
    }
    let doc = notera_richtext::parse_from_value(value).map_err(map_rich)?;
    finish(doc)
}

pub(crate) fn prepare_text(raw: &str) -> Result<Prepared, StoreError> {
    let doc = notera_richtext::parse(raw).map_err(map_rich)?;
    finish(doc)
}

fn map_rich(e: notera_richtext::RichError) -> StoreError {
    match e {
        notera_richtext::RichError::UnsupportedVersion(v) => {
            StoreError::DocTooNew { doc: v, supported: notera_richtext::DOC_FORMAT }
        }
        other => StoreError::InvalidDoc(other.to_string()),
    }
}

fn finish(doc: notera_richtext::Document) -> Result<Prepared, StoreError> {
    let version = doc.v;
    let canonical = notera_richtext::canonical(&doc);
    let content_hash = ContentHash::of(canonical.as_bytes());
    let ex = notera_richtext::extract(&doc);
    Ok(Prepared {
        doc_json: canonical,
        doc_version: version,
        content_hash,
        title: ex.title,
        plain_text: ex.plain_text,
        summary: ex.summary,
        char_count: ex.char_count,
        block_count: ex.block_count,
        doc_has_attachment: ex.has_attachment,
        attachments: notera_richtext::attachments(&doc),
    })
}

/// 复核外部声明的哈希是否等于 canonical 哈希（容忍大小写与 `sha256:` 前缀缺失）。
pub(crate) fn hash_matches(declared: &str, actual: &ContentHash) -> bool {
    let d = declared.trim().strip_prefix("sha256:").unwrap_or(declared.trim());
    let a = actual
        .as_str()
        .strip_prefix("sha256:")
        .unwrap_or(actual.as_str());
    d.eq_ignore_ascii_case(a)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepare_is_deterministic_and_derives_text() {
        let v = serde_json::json!({
            "v": notera_richtext::DOC_FORMAT,
            "content": [
                { "id": "block001", "type": "heading", "attrs": {"level": 1},
                  "content": [{ "text": "同步机制" }] },
                { "id": "block002", "type": "paragraph",
                  "content": [{ "text": "两字中文词同步需要走 LIKE 路径" }] }
            ]
        });
        let p = prepare(&v).expect("合法文档必须通过");
        assert_eq!(p.title, "同步机制");
        assert!(p.plain_text.contains("同步"));
        assert!(p.char_count > 0);
        assert_eq!(p.block_count, 2);
        assert!(p.content_hash.as_str().starts_with("sha256:"));
        assert_eq!(p.content_hash.as_str(), prepare(&v).unwrap().content_hash.as_str());
        assert!(hash_matches(p.content_hash.as_str(), &p.content_hash));
    }

    #[test]
    fn garbage_is_rejected_not_silently_accepted() {
        assert!(matches!(prepare_text("not json at all"), Err(StoreError::InvalidDoc(_))));
        assert!(matches!(
            prepare(&serde_json::json!({ "v": 9999, "content": [] })),
            Err(StoreError::DocTooNew { .. })
        ));
    }
}
