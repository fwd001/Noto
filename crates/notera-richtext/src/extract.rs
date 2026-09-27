//! 派生数据抽取（DATA-MODEL.md §7.1）：`title / plain_text / summary / char_count /
//! block_count / has_attachment`，外加 [`attachments`]（doc 引用的附件 sha 清单，§8）。
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
        i.marks
            .iter()
            .any(|m| matches!(m.kind, MarkKind::AttachmentRef) || m.attrs.contains_key("sha256"))
    })
}

/// 块上一个附件引用的**全部**已知信息（DATA-MODEL §8：收到一条记录 = 知道要哪些 blob）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockAttachment {
    pub block_id: String,
    /// 64 位小写十六进制，已核验。
    pub sha256: String,
    /// `inline` / `file`；块上没写就是 `None`，由存储层按媒体类型决定。
    pub role: Option<String>,
    /// 声明的字节数。`None` = 这份 doc 没告诉我们，不代表 0。
    pub size: Option<i64>,
    pub media_type: Option<String>,
    pub filename: Option<String>,
}

/// 导出 doc 引用的附件（按块顺序，同 sha 多次出现就多条，块 id 不同）。
///
/// 为什么需要它：附件字节是内容寻址、单独存放的，收到一条新笔记的人**只有 doc** ——
/// 不把里面的 sha 抄进 `attachments`，就没有下载任务、引用计数恒为 0（GC 会把还在用的
/// blob 删掉）、按文件夹导出的附件集合也是空的。实测踩过：第二台设备的图片永远停在占位。
///
/// 只认 `Image`/`Attachment` 块以及任何带 `sha256` 属性的块（前向兼容），且 **sha 必须形态
/// 合法**：`attachments.sha256` 主键上有 CHECK 约束，把畸形值塞进去会让整批写入回滚 ——
/// 那不是"这条附件不要了"，那是"这篇笔记同步不了"。所以畸形值在这里就被丢掉，
/// 由 [`extract`] 的 `has_attachment` 继续如实反映"doc 里出现过 sha256 这个键"。
pub fn attachments(doc: &Document) -> Vec<BlockAttachment> {
    doc.content
        .iter()
        .filter_map(|b| {
            let sha = b
                .attrs
                .get("sha256")
                .and_then(|v| v.as_str())
                .filter(|s| is_sha256_hex(s))?;
            let attr = |keys: &[&str]| {
                keys.iter()
                    .find_map(|k| b.attrs.get(*k).and_then(|v| v.as_str()))
                    .map(|s| s.to_string())
            };
            Some(BlockAttachment {
                block_id: b.id.clone(),
                sha256: sha.to_string(),
                role: attr(&["role"]).filter(|r| r == "inline" || r == "file"),
                size: b
                    .attrs
                    .get("size")
                    .and_then(|v| v.as_i64())
                    .filter(|n| *n >= 0),
                media_type: attr(&["mediaType", "media_type"]),
                filename: attr(&["name", "filename"]),
            })
        })
        .collect()
}

/// 64 位小写十六进制 —— 与 `attachments` 主键上的 CHECK 同一套判据。
fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
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
