//! 派生数据抽取（DATA-MODEL.md §7.1）：`title / plain_text / summary / char_count /
//! block_count / has_attachment`。
//!
//! 这些列**只读**，且必须与 `doc` 同事务写入（I5）—— 所以抽取函数只能在这里，
//! 不允许 UI 侧另算一份。
//!
//! 计数一律按 **Unicode 码点**（`chars().count()`），绝不按字节：中文按字节计数会把
//! `char_count` 放大 3 倍，预览截断也会切在半个 UTF-8 序列上。

use crate::model::{Block, BlockType, Document, MarkKind};

/// 从文档导出的派生列集合。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Extracted {
    pub title: String,
    pub plain_text: String,
    pub summary: String,
    pub char_count: u32,
    pub block_count: u32,
    pub has_attachment: bool,
}

/// `title` 截断长度（§7.1：200 字符）。
const TITLE_MAX: usize = 200;
/// `summary` 截断长度（§7.1：200 字符）。
const SUMMARY_MAX: usize = 200;

/// 单向导出：文档 → 派生列。纯函数，不失败。
pub fn extract(doc: &Document) -> Extracted {
    let mut plain = String::new();
    for (i, b) in doc.content.iter().enumerate() {
        if i > 0 {
            plain.push('\n');
        }
        plain.push_str(&b.plain_text());
    }

    // title：第一个 heading，否则第一个非空 text 节点。
    let title_idx = doc
        .content
        .iter()
        .position(|b| b.type_ == BlockType::Heading && !b.plain_text().trim().is_empty())
        .or_else(|| {
            doc.content
                .iter()
                .position(|b| !b.plain_text().trim().is_empty())
        });
    let title = title_idx
        .map(|i| truncate_chars(doc.content[i].plain_text().trim(), TITLE_MAX))
        .unwrap_or_default();

    // summary：plain_text **跳过标题**取前 200 字符（列表预览不该重复标题一遍）。
    let summary_src = match title_idx {
        Some(i) => {
            let mut s = String::new();
            for (j, b) in doc.content.iter().enumerate() {
                if j == i {
                    continue;
                }
                if j > 0 {
                    s.push('\n');
                }
                s.push_str(&b.plain_text());
            }
            s
        }
        None => plain.clone(),
    };
    let summary = collapse_ws(&truncate_chars(summary_src.trim(), SUMMARY_MAX));

    Extracted {
        // 排除空白后的码点数（§7.1）。
        char_count: u32::try_from(plain.chars().filter(|c| !c.is_whitespace()).count())
            .unwrap_or(u32::MAX),
        block_count: u32::try_from(doc.content.len()).unwrap_or(u32::MAX),
        has_attachment: doc.content.iter().any(block_has_attachment_ref),
        title,
        plain_text: plain,
        summary,
    }
}

/// 是否存在 attachment 引用：附件块、带 `sha256` 的块（图片/附件/前向兼容容器都算）、
/// 或 `attachmentRef` mark。
fn block_has_attachment_ref(b: &Block) -> bool {
    if matches!(b.type_, BlockType::Attachment) {
        return true;
    }
    if b.attrs.contains_key("sha256") {
        return true;
    }
    b.content.iter().any(|i| {
        i.marks.iter().any(|m| {
            matches!(m.kind, MarkKind::AttachmentRef) || m.attrs.contains_key("sha256")
        })
    })
}

fn truncate_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_ws = false;
    for c in s.chars() {
        if c.is_whitespace() {
            if !in_ws {
                out.push(' ');
                in_ws = true;
            }
        } else {
            in_ws = false;
            out.push(c);
        }
    }
    out
}
