//! 导入源：读字节 → 三道闸门 → 解码 → 类型探测。
//!
//! 设计要点：**扩展名只用于"往哪个方向解析"，永远不用于"要不要拒绝这个文件"**。
//! `.txt` 里装着 Markdown 就按 Markdown 解析，`.md` 里装着散文就按散文解析，
//! 没有扩展名的文件照样能进（TEST-PLAN FT-IO-06）。真正会拒的只有三件事：
//! 太大、是二进制、解不出文本。

use crate::error::ImportError;
use notera_core::ContentHash;
use std::path::{Path, PathBuf};

/// 单文件上限（8 MiB）。超过即拒，不截断。
pub const MAX_SOURCE_BYTES: u64 = 8 * 1024 * 1024;

/// 二进制嗅探窗口：前 8 KiB 内出现 NUL 即判二进制。
pub const BINARY_SNIFF_BYTES: usize = 8 * 1024;

/// 解析方向。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceKind {
    /// Markdown（块级 + 行内语法都尝试识别）。
    Markdown,
    /// 纯文本（只按空行分块，不做任何行内解释，`*` 就是 `*`）。
    PlainText,
}

impl SourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SourceKind::Markdown => "markdown",
            SourceKind::PlainText => "text",
        }
    }
}

/// 判定依据（报告用：让用户知道我们为什么这么解析它）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KindSource {
    /// 扩展名直接说明。
    Extension,
    /// 扩展名不说或说得不像，由内容嗅探决定。
    Sniffed,
}

/// 一个已经读进内存、过了闸门、已解码的导入源。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportSource {
    /// 来源路径；由 `from_bytes` 构造时为 None。
    pub path: Option<PathBuf>,
    /// 报告用名字（有路径 = 文件名，无路径 = `<bytes:N>`）。
    pub label: String,
    pub kind: SourceKind,
    pub kind_source: KindSource,
    /// 解码后的文本：BOM 已剥、行尾**未**规范化（换行处理是解析器的事）。
    pub text: String,
    /// 原始字节的 sha256（区分"同一文件两次"与"两个内容相同的文件"）。
    pub source_hash: ContentHash,
    pub byte_len: u64,
    /// 是否带过 BOM（报告用，不算内容）。
    pub had_bom: bool,
}

impl ImportSource {
    /// 读盘 + 闸门 + 解码。
    pub fn read(path: &Path) -> Result<Self, ImportError> {
        let label = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let size = std::fs::metadata(path).map_err(|e| ImportError::Io {
            path: label.clone(),
            source: e,
        })?;
        if !size.is_file() {
            return Err(ImportError::Io {
                path: label,
                source: std::io::Error::new(std::io::ErrorKind::InvalidInput, "不是普通文件"),
            });
        }
        if size.len() > MAX_SOURCE_BYTES {
            return Err(ImportError::TooLarge {
                path: label,
                size: size.len(),
                limit: MAX_SOURCE_BYTES,
            });
        }
        let bytes = std::fs::read(path).map_err(|e| ImportError::Io {
            path: label.clone(),
            source: e,
        })?;
        Self::from_bytes(Some(path), &bytes)
    }

    /// 已经拿到字节时的同一套闸门（`from_bytes` 也用于测试与未来的剪贴板导入）。
    pub fn from_bytes(path: Option<&Path>, bytes: &[u8]) -> Result<Self, ImportError> {
        let label = path
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| format!("<bytes:{}>", bytes.len()));
        let display = label.clone();
        // 闸门顺序即契约：体积 → 二进制 → 编码。前一关不过就不必解后一关。
        if (bytes.len() as u64) > MAX_SOURCE_BYTES {
            return Err(ImportError::TooLarge {
                path: display,
                size: bytes.len() as u64,
                limit: MAX_SOURCE_BYTES,
            });
        }
        if !is_binary_exempt(bytes) {
            if let Some(offset) = bytes.iter().take(BINARY_SNIFF_BYTES).position(|&b| b == 0) {
                return Err(ImportError::Binary {
                    path: display,
                    offset,
                    window: BINARY_SNIFF_BYTES,
                });
            }
        }
        let (text, had_bom) = decode(&display, bytes)?;
        let (kind, kind_source) = detect(path, &text);
        Ok(ImportSource {
            path: path.map(|p| p.to_path_buf()),
            label,
            kind,
            kind_source,
            text,
            source_hash: ContentHash::of(bytes),
            byte_len: bytes.len() as u64,
            had_bom,
        })
    }

    /// 文件名去扩展名（标题优先级里的最后一档）。
    pub fn file_stem(&self) -> String {
        self.path
            .as_ref()
            .and_then(|p| p.file_stem())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.label.clone())
    }
}

/// UTF-16 BOM 是"我确实有 NUL 但我是文本"的唯一合法证明，故二进制嗅探对它让路。
/// 没有 BOM 的 UTF-16 与真二进制不可区分，一律不猜：带 NUL 的按二进制拒；
/// 恰好没 NUL 又恰好是合法 UTF-8 的（纯 CJK 常这样）按 UTF-8 收下，内容会是乱码 ——
/// 想稳就带 BOM 或另存为 UTF-8。猜测编码的风险比收乱码大得多。
fn is_binary_exempt(bytes: &[u8]) -> bool {
    matches!(
        bytes.first().copied().zip(bytes.get(1).copied()),
        Some((0xFF, 0xFE)) | Some((0xFE, 0xFF))
    )
}

/// 严格解码：UTF-8（可带 BOM）→ 带 BOM 的 UTF-16 → 失败。
fn decode(path: &str, bytes: &[u8]) -> Result<(String, bool), ImportError> {
    if let Some(utf16) = decode_utf16_bom(bytes) {
        return utf16.map(|s| (s, true)).map_err(|detail| ImportError::InvalidEncoding {
            path: path.to_string(),
            detail,
        });
    }
    let (body, bom) = match bytes.strip_prefix(&[0xEF, 0xBB, 0xBF][..]) {
        Some(rest) => (rest, true),
        None => (bytes, false),
    };
    match std::str::from_utf8(body) {
        Ok(s) => Ok((s.to_string(), bom)),
        Err(e) => Err(ImportError::InvalidEncoding {
            path: path.to_string(),
            detail: format!("UTF-8 在第 {} 字节处断裂；也不是带 BOM 的 UTF-16: {e}", e.valid_up_to()),
        }),
    }
}

/// `FF FE` = UTF-16LE，`FE FF` = UTF-16BE。仅 std（`char::decode_utf16` 处理代理对）。
fn decode_utf16_bom(bytes: &[u8]) -> Option<Result<String, String>> {
    let (le, body) = match bytes.first().copied().zip(bytes.get(1).copied()) {
        Some((0xFF, 0xFE)) => (true, &bytes[2..]),
        Some((0xFE, 0xFF)) => (false, &bytes[2..]),
        _ => return None,
    };
    if body.len() % 2 != 0 {
        return Some(Err(format!("UTF-16 负载长度不是偶数（{} 字节）", body.len())));
    }
    let units: Vec<u16> = body
        .chunks(2)
        .map(|c| {
            if le {
                u16::from_le_bytes([c[0], c[1]])
            } else {
                u16::from_be_bytes([c[0], c[1]])
            }
        })
        .collect();
    // 一个 BOM 都不该出现在负载中间：留着会在正文里显形为不可见字符，剥掉。
    let units: Vec<u16> = units.into_iter().filter(|u| *u != 0xFEFF).collect();
    match String::from_utf16(&units) {
        Ok(s) => Some(Ok(s)),
        Err(e) => Some(Err(format!("UTF-16 码元序列不合法: {e}"))),
    }
}

const MARKDOWN_EXTS: [&str; 7] = ["md", "markdown", "mdown", "mdwn", "mdtxt", "mkd", "mkdn"];

/// 探测解析方向。扩展名优先，嗅探兜底；两条路都只决定"怎么解析"，不决定"是否拒绝"。
fn detect(path: Option<&Path>, text: &str) -> (SourceKind, KindSource) {
    let ext = path
        .and_then(|p| p.extension())
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if MARKDOWN_EXTS.contains(&ext.as_str()) {
        return (SourceKind::Markdown, KindSource::Extension);
    }
    (sniff(text), KindSource::Sniffed)
}

/// 内容嗅探：出现任意一个"只有 Markdown 才会这么写"的块级标记，或两个行内标记，就算 Markdown。
fn sniff(text: &str) -> SourceKind {
    let mut inline_marks = 0u32;
    for raw in crate::markdown::split_lines(text) {
        let body = crate::markdown::indent_stripped(raw);
        if body.is_empty() {
            continue;
        }
        if crate::markdown::looks_like_markdown_line(body) {
            return SourceKind::Markdown;
        }
        if body.contains("**") || body.contains("__") || body.contains("~~") {
            inline_marks += 1;
        }
        if crate::markdown::has_link_like(body) {
            inline_marks += 1;
        }
        if inline_marks >= 2 {
            return SourceKind::Markdown;
        }
    }
    SourceKind::PlainText
}

// ============================================================ 单元测试 ===

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ImportError;
    use std::path::Path;

    fn src(name: &str, bytes: impl AsRef<[u8]>) -> Result<ImportSource, ImportError> {
        ImportSource::from_bytes(Some(Path::new(name)), bytes.as_ref())
    }

    #[test]
    fn oversize_is_the_first_gate() {
        let big = vec![b'a'; MAX_SOURCE_BYTES as usize + 1];
        match src("big.md", &big) {
            Err(ImportError::TooLarge { size, limit, .. }) => {
                assert_eq!(size, MAX_SOURCE_BYTES + 1);
                assert_eq!(limit, MAX_SOURCE_BYTES);
            }
            other => panic!("超限文件必须是 TooLarge，实得 {other:?}"),
        }
    }

    #[test]
    fn nul_in_the_sniff_window_is_binary() {
        let mut bytes = "# 标题\n".as_bytes().to_vec();
        bytes.extend([0u8, 1, 2, 3]);
        match src("note.md", &bytes) {
            Err(ImportError::Binary { offset, window, .. }) => {
                assert_eq!(offset, 9, "'# 标题\\n' 的 UTF-8 长度是 9 字节");
                assert_eq!(window, BINARY_SNIFF_BYTES);
            }
            other => panic!("NUL 必须是 Binary，实得 {other:?}"),
        }
    }

    #[test]
    fn invalid_utf8_is_not_guessed_away() {
        // GB18030 的"中文"：既不猜编码也不当二进制，报 InvalidEncoding 让用户转 UTF-8。
        let gbk = [0xD6u8, 0xD0, 0xCE, 0xC4];
        assert!(matches!(src("note.md", &gbk), Err(ImportError::InvalidEncoding { .. })));
        let broken = [0xFFu8, 0xBB, 0xBF, b'a'];
        assert!(matches!(src("note.md", &broken), Err(ImportError::InvalidEncoding { .. })));
    }

    #[test]
    fn utf8_bom_is_stripped_and_reported() {
        let s = src("note.md", "\u{feff}# 标题\n".as_bytes()).expect("UTF-8 BOM 必须能进");
        assert!(s.had_bom);
        assert_eq!(s.text, "# 标题\n");
        assert_eq!(s.kind, SourceKind::Markdown);
        assert_eq!(s.kind_source, KindSource::Extension);
    }

    #[test]
    fn utf16_with_bom_decodes_even_though_it_is_full_of_nul() {
        let mut le: Vec<u8> = vec![0xFF, 0xFE];
        le.extend("甲乙".encode_utf16().flat_map(|u| u.to_le_bytes()));
        let s = src("u16.txt", &le).expect("带 BOM 的 UTF-16LE 不是二进制");
        assert_eq!(s.text, "甲乙");
        assert!(s.had_bom);

        let mut be: Vec<u8> = vec![0xFE, 0xFF];
        be.extend("甲乙".encode_utf16().flat_map(|u| u.to_be_bytes()));
        assert_eq!(src("u16be.txt", &be).expect("UTF-16BE").text, "甲乙");

        // 没有 BOM 的 UTF-16 与真二进制不可区分 → 按二进制拒（宁可让用户转，也不猜）。
        // 用 ASCII：UTF-16LE 的每个 ASCII 字符都带一个 NUL 字节，正是被嗅探拦下的形状。
        let nobom: Vec<u8> = "AB".encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert!(matches!(src("noea.txt", &nobom), Err(ImportError::Binary { .. })));
        // 纯 CJK 的无 BOM UTF-16 可能一个 NUL 都没有，而字节又恰好是合法 UTF-8 ——
        // 那就只能按 UTF-8 收下（内容是乱码）。刻意不做"也许是 UTF-16?"的猜测：
        // 猜错会摧毁真文本文件。要稳，请带 BOM 或另存为 UTF-8。
        let cjk: Vec<u8> = "甲乙".encode_utf16().flat_map(u16::to_le_bytes).collect();
        let s = src("noea2.txt", &cjk).expect("不猜编码：字节合法就按 UTF-8 收");
        assert_eq!(s.text.chars().count(), 4);
        assert!(!s.text.contains('甲'), "无 BOM 的 UTF-16 不会被解成中文（设计内）");
    }

    #[test]
    fn markdown_inside_a_txt_file_is_sniffed_not_rejected() {
        let s = src("notes.txt", "# 标题\n\n- 项目\n").expect(".txt 也要能进");
        assert_eq!(s.kind, SourceKind::Markdown);
        assert_eq!(s.kind_source, KindSource::Sniffed);
    }

    #[test]
    fn plain_txt_stays_plain_so_stars_are_stars() {
        let s = src("notes.txt", "2 * 3 = 6\n这一行也没有 markdown 标记\n").expect("纯文本要能进");
        assert_eq!(s.kind, SourceKind::PlainText);
    }

    #[test]
    fn extensionless_and_unknown_extension_still_import() {
        let s = src("README", "# 项目说明\n").expect("没有扩展名不是拒绝的理由");
        assert_eq!(s.kind, SourceKind::Markdown);
        assert_eq!(s.file_stem(), "README");
        let other = src("notes.rst", "* 一条\n").expect("未知扩展名也照进");
        assert_eq!(other.kind, SourceKind::Markdown);
    }

    #[test]
    fn md_extension_does_not_need_sniffing() {
        let s = src("a.markdown", "just words **bold**\n").expect("markdown 扩展名");
        assert_eq!(s.kind, SourceKind::Markdown);
        assert_eq!(s.kind_source, KindSource::Extension);
    }

    #[test]
    fn zero_byte_source_reads_ok_and_hashes() {
        let s = src("empty.md", b"").expect("0 字节不是错误，是跳过");
        assert!(s.text.is_empty());
        assert_eq!(s.byte_len, 0);
        assert_eq!(s.source_hash, ContentHash::of(b""));
    }

    #[test]
    fn read_rejects_missing_files_with_typed_io_error() {
        let e = ImportSource::read(Path::new("D:/definitely-not-here/notera-importer-test.md"));
        assert!(matches!(e, Err(ImportError::Io { .. })), "缺文件必须是 Io，实得 {e:?}");
    }
}
