//! Markdown / 纯文本 → 权威 Document（DATA-MODEL §10，块级扁平模型）。
//!
//! 手写解析器，不引入 `pulldown-cmark` 之类依赖（架构评审前禁止依赖漂移）。
//! 代价是"只支持子集"，收益是**无损规则可以逐条审计**。
//!
//! ## 无损规则（本 crate 的存在理由）
//! 解析器只允许**移动**字符，不允许**删除**字符：
//! * 看不懂的行内标记 → 原样留在段落文本里（`[a][b]`、`<div>`、`[^1]`、非标点的反斜杠…）；
//! * 没闭合的围栏 → 一路吃到 EOF，围栏之后的每一行都进代码块；
//! * 引用式链接（`[text][ref]` 与其定义行）→ 整串按字面文本保留，**不**做链接；
//! * 被吃掉的字符只有三类：块级语法标记（`#` `>` `-` `1.` ``` ``` ``` `---` `[ ]`）、
//!   成对包裹符号（`**` `__` `*` `_` `~~` `` ` `` `[]()`，其文字进文本、目标进 attrs）、
//!   行尾空白与 BOM。
//!
//! ## 有意的偏离（都进报告，不是 bug）
//! * **硬换行 = 块边界**：扁平 schema 没有 `hardBreak` 行内节点，所以一段里的每一行
//!   成为一个段落块，正文里因此不出现控制字符（编辑器侧不会被二次规范化）。
//!   唯一例外是 `codeBlock`：代码内部的换行是内容，整块存成一个文本节点。
//! * **URL 进 attrs 不进正文**：`[text](url)` 的 `url` 成为 `link{href}`，
//!   图片的 `src` 成为 image 块的 `attrs.src`。它们在 `extract().plain_text` 里不可见。
//! * **行内图片保持字面**：只有"整行只有一张图片"时才提升为 `image` 块；
//!   混在句子里的 `![alt](src)` 原样保留为文字（扁平模型没有行内块）。
//! * **制表符**：只参与缩进计算（4 列一个停靠位），从不被复制进正文；
//!   行首的 tab 属于缩进，其余位置的 tab 属于文字，原样保留。

use crate::source::SourceKind;
use notera_richtext::{Block, BlockType, Document, Inline, Mark, MarkKind, DOC_FORMAT};
use serde_json::Value;
use std::collections::BTreeMap;

/// 单行超过这个长度就放弃行内解释，整行按字面进段落（无损，只是不再生效）。
/// 递归的 emphasis 配对最坏情况是 O(n²)，这一条把 8 MiB 单行文件的成本封顶。
pub const MAX_INLINE_PARSE_CHARS: usize = 64 * 1024;

/// §10.2 `indent` 合法区间上限。
const MAX_INDENT: usize = 8;

/// 一个还没拿到 `id` 的块。
struct Draft {
    type_: BlockType,
    attrs: BTreeMap<String, Value>,
    content: Vec<Inline>,
}

impl Draft {
    fn new(type_: BlockType, content: Vec<Inline>) -> Self {
        Draft {
            type_,
            attrs: BTreeMap::new(),
            content,
        }
    }
    fn attr(mut self, key: &str, val: Value) -> Self {
        self.attrs.insert(key.to_string(), val);
        self
    }
}

/// 块 id 前缀由此生成，保证"同一输入 → 同一 id"（幂等导入的前提）。
pub fn id_prefix(source_hash_short: &str) -> String {
    format!("in-{source_hash_short}")
}

/// 主入口。`title_heading` 非空时在文档最前面插一个一级标题块
/// （front-matter 的 `title` 只有这样才落得进 `notes.title` —— 那一列是从 doc 派生的）。
pub fn to_document(
    text: &str,
    kind: SourceKind,
    prefix: &str,
    title_heading: Option<&str>,
) -> Document {
    // 解码阶段已剥过一次 BOM；这里再剥一次是纵深防御 —— 残留的 U+FEFF 会让首行
    // `# 标题` 不再是标题（首字符不是 `#`），而 richtext 的 normalize 随后会把它吃掉，
    // 于是"井号还在、标题没了"。
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut drafts = match kind {
        SourceKind::PlainText => plain_blocks(text),
        SourceKind::Markdown => markdown_blocks(text),
        // `.enex` 不进这条路径：一份 .enex 里是 N 条笔记，没有"这一份的文档"可言，
        // 由 `plan::plan_enex` 处理，而 `document_for` 也明确拒绝 Enex。真走到这里
        // 说明上面某条路由错了 —— 宁可炸出来，也不悄悄产出一条没有结构的笔记。
        SourceKind::Enex => unreachable!(".enex 不该进 Markdown 解析路径（走 plan_enex）"),
    };
    if let Some(t) = title_heading {
        drafts.insert(
            0,
            Draft::new(BlockType::Heading, inline_nodes(t, kind)).attr("level", Value::from(1u8)),
        );
    }
    Document {
        v: DOC_FORMAT,
        content: drafts
            .into_iter()
            .enumerate()
            .map(|(i, d)| Block {
                // 序号只在本文档内保证唯一；前缀是源文件内容哈希，跨文档也不撞。
                id: format!("{prefix}-{i:04}"),
                type_: d.type_,
                attrs: d.attrs,
                content: d.content,
            })
            .collect(),
    }
}

/// 纯文本：每个非空行一个段落，不做任何行内解释（`*` 就是 `*`）。
fn plain_blocks(text: &str) -> Vec<Draft> {
    split_lines(text)
        .into_iter()
        .map(indent_stripped)
        .filter(|b| !b.is_empty())
        .map(|b| Draft::new(BlockType::Paragraph, text_inline(b)))
        .collect()
}

// ============================================================ 块级解析 ===

fn markdown_blocks(text: &str) -> Vec<Draft> {
    let lines = split_lines(text);
    let mut out: Vec<Draft> = Vec::new();
    // 已打开列表的缩进列，用来算 `indent` 层级（扁平模型没有嵌套块）。
    let mut list_stack: Vec<usize> = Vec::new();
    let mut i = 0usize;

    while i < lines.len() {
        let raw = lines[i];
        let indent = indent_cols(raw);
        let body = indent_stripped(raw);
        if body.is_empty() {
            i += 1;
            continue;
        }

        if indent <= 3 {
            // ---- 围栏代码块（``` / ~~~ + info string）--------------------------------
            if let Some((fc, flen, info)) = open_fence(body) {
                let mut code: Vec<String> = Vec::new();
                i += 1;
                while i < lines.len() {
                    let b = indent_stripped(lines[i]);
                    if let Some((cc, clen)) = bare_fence(b) {
                        if cc == fc && clen >= flen {
                            i += 1;
                            break;
                        }
                    }
                    code.push(strip_columns(lines[i], indent).to_string());
                    i += 1;
                }
                out.push(
                    Draft::new(BlockType::CodeBlock, text_inline(&code.join("\n")))
                        .attr("lang", Value::String(info.to_string())),
                );
                list_stack.clear();
                continue;
            }

            // ---- ATX 标题 --------------------------------------------------------------
            if let Some((level, rest)) = atx_heading(body) {
                out.push(
                    Draft::new(BlockType::Heading, inline_nodes(rest, SourceKind::Markdown))
                        .attr("level", Value::from(level)),
                );
                i += 1;
                list_stack.clear();
                continue;
            }

            // ---- 分割线（先于列表：`- - -` 看着也像列表项）-----------------------------
            if thematic_break(body) {
                out.push(Draft::new(BlockType::HorizontalRule, Vec::new()));
                i += 1;
                list_stack.clear();
                continue;
            }

            // ---- 引用：连续 `>` 行，每行一个 blockquote 块，层数进 attrs ----------------
            if body.starts_with('>') {
                while i < lines.len() {
                    let b = indent_stripped(lines[i]);
                    if !b.starts_with('>') {
                        break;
                    }
                    let (depth, inner) = quote_markers(b);
                    if !inner.is_empty() {
                        out.push(
                            Draft::new(
                                BlockType::BlockQuote,
                                inline_nodes(inner, SourceKind::Markdown),
                            )
                            .attr("indent", Value::from(depth as u8)),
                        );
                    }
                    i += 1;
                }
                list_stack.clear();
                continue;
            }
        }

        // ---- 列表项（无序 / 有序 / 任务）----------------------------------------------
        if let Some((ordered, marker_cols, checked, text_start)) = list_marker(body) {
            let content_col = indent + marker_cols;
            let mut item = body[text_start..].trim().to_string();
            i += 1;
            // 续行：比内容列更靠右、且自己不像是新块的行，并进同一项（一个字都不丢）。
            while i < lines.len() {
                let b = indent_stripped(lines[i]);
                if b.is_empty() || starts_new_block(b) || indent_cols(lines[i]) < content_col {
                    break;
                }
                if !item.is_empty() {
                    item.push(' ');
                }
                item.push_str(b.trim_end());
                i += 1;
            }
            while list_stack.last().is_some_and(|&l| l >= indent) {
                list_stack.pop();
            }
            list_stack.push(indent);
            let level = (list_stack.len() - 1).min(MAX_INDENT) as u8;
            let inlines = inline_nodes(&item, SourceKind::Markdown);
            let d = match checked {
                // 任务清单的 `[x]` 进 attrs.checked，方括号本身算语法标记。
                Some(done) => Draft::new(BlockType::ChecklistItem, inlines)
                    .attr("checked", Value::Bool(done))
                    .attr("indent", Value::from(level)),
                None if ordered => Draft::new(BlockType::OrderedList, inlines)
                    .attr("indent", Value::from(level))
                    .attr("number", Value::from(i64::from(number_of(body)))),
                None => {
                    Draft::new(BlockType::BulletList, inlines).attr("indent", Value::from(level))
                }
            };
            out.push(d);
            continue;
        }

        // ---- 独占一行的图片 → image 块（其余位置的图片保持字面文本）--------------------
        if let Some((alt, src, title)) = sole_image(body) {
            let mut d = Draft::new(BlockType::Image, text_inline(&alt))
                .attr("src", Value::String(src))
                .attr("alt", Value::String(alt));
            if !title.is_empty() {
                d = d.attr("title", Value::String(title));
            }
            out.push(d);
            i += 1;
            list_stack.clear();
            continue;
        }

        // ---- 段落：每一行一个块（硬换行 = 块边界）-------------------------------------
        out.push(Draft::new(
            BlockType::Paragraph,
            inline_nodes(body.trim_end(), SourceKind::Markdown),
        ));
        i += 1;
        list_stack.clear();
    }
    out
}

// ------------------------------------------------------------ 行工具 ---

/// 按 `\n` / `\r\n` / `\r` 切行；尾部换行不产生空行。
pub(crate) fn split_lines(text: &str) -> Vec<&str> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let (mut start, mut i) = (0usize, 0usize);
    while i < b.len() {
        match b[i] {
            b'\n' => {
                out.push(&text[start..i]);
                i += 1;
                start = i;
            }
            b'\r' => {
                out.push(&text[start..i]);
                i += if b.get(i + 1) == Some(&b'\n') { 2 } else { 1 };
                start = i;
            }
            _ => i += 1,
        }
    }
    if start < b.len() {
        out.push(&text[start..]);
    }
    out
}

/// 行首缩进的列宽（tab 走到 4 列停靠位）。
pub(crate) fn indent_cols(line: &str) -> usize {
    let mut cols = 0usize;
    for c in line.chars() {
        match c {
            ' ' => cols += 1,
            '\t' => cols += 4 - (cols % 4),
            _ => break,
        }
    }
    cols
}

/// 去掉行首空白后的正文（块判定与嗅探都用它）。
pub(crate) fn indent_stripped(line: &str) -> &str {
    line.trim_start_matches([' ', '\t'])
}

/// 按**列数**剥行首空白，剥不动就停（tab 跨边界时不多吃列）。
fn strip_columns(line: &str, cols: usize) -> &str {
    let mut used = 0usize;
    let mut idx = 0usize;
    for c in line.chars() {
        let w = match c {
            ' ' => 1,
            '\t' => 4 - (used % 4),
            _ => break,
        };
        if used + w > cols {
            break;
        }
        used += w;
        idx += c.len_utf8();
    }
    &line[idx..]
}

// ---------------------------------------------------------- 块级判定 ---

/// 开围栏：``` 或 ~~~（≥3 个），后面是 info string。
fn open_fence(body: &str) -> Option<(char, usize, &str)> {
    let fc = body.chars().next()?;
    if fc != '`' && fc != '~' {
        return None;
    }
    let len = body.chars().take_while(|&c| c == fc).count();
    if len < 3 {
        return None;
    }
    let rest = &body[len..];
    if fc == '`' && rest.contains('`') {
        // CommonMark：info string 里不许出现反引号，否则这行不是围栏。
        return None;
    }
    Some((fc, len, rest.trim()))
}

/// 闭围栏：整行只有围栏字符（可夹尾部空白）。
fn bare_fence(body: &str) -> Option<(char, usize)> {
    let fc = body.chars().next()?;
    if fc != '`' && fc != '~' {
        return None;
    }
    let len = body.chars().take_while(|&c| c == fc).count();
    if len < 3 || !body[len..].trim().is_empty() {
        return None;
    }
    Some((fc, len))
}

/// ATX 标题：1..=6 个 `#` 后必须跟空白或行尾；尾部闭合的 `###` 关掉。
fn atx_heading(body: &str) -> Option<(u8, &str)> {
    let hashes = body.chars().take_while(|&c| c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = &body[hashes..];
    if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
        return None;
    }
    let mut text = rest.trim();
    if !text.is_empty() && text.chars().all(|c| c == '#') {
        text = ""; // `## ###` = 空标题
    } else {
        while text.ends_with('#') {
            let cut = text.trim_end_matches('#');
            if cut.is_empty() || cut.ends_with([' ', '\t']) {
                text = cut.trim_end();
            } else {
                break; // `# x###` 里的 ### 是正文
            }
        }
    }
    Some((hashes as u8, text))
}

/// 分割线：整行只有同一种 `-`/`*`/`_`（≥3 个，可夹空白）。
fn thematic_break(body: &str) -> bool {
    let t: Vec<char> = body.chars().filter(|c| !c.is_whitespace()).collect();
    t.len() >= 3
        && matches!(t.first(), Some('-' | '*' | '_'))
        && t.iter().all(|&c| Some(c) == t.first().copied())
}

/// 引用标记：连续 `>`（中间可夹一个空格）。返回层数与剥完之后的正文。
fn quote_markers(body: &str) -> (usize, &str) {
    let b = body.as_bytes();
    let mut depth = 0usize;
    let mut idx = 0usize;
    while idx < b.len() && b[idx] == b'>' {
        depth += 1;
        idx += 1;
        if b.get(idx) == Some(&b' ') {
            idx += 1;
        }
    }
    (depth.max(1), body[idx..].trim())
}

/// 列表标记。返回 (是否有序, 标记占的列宽, 任务勾选态, 正文起始字节偏移)。
fn list_marker(body: &str) -> Option<(bool, usize, Option<bool>, usize)> {
    let b = body.as_bytes();
    let (ordered, mut pos) = match *b.first()? {
        b'-' | b'*' | b'+' => (false, 1usize),
        d if d.is_ascii_digit() => {
            let mut j = 0usize;
            while j < b.len() && b[j].is_ascii_digit() && j < 9 {
                j += 1;
            }
            match b.get(j) {
                Some(b'.') | Some(b')') => (true, j + 1),
                _ => return None,
            }
        }
        _ => return None,
    };
    // 标记之后必须是空白（`*bold*` 不是列表项），或者整行就是空标记。
    match b.get(pos) {
        None => return Some((ordered, pos, None, pos)),
        Some(b' ') | Some(b'\t') => pos += 1,
        _ => return None,
    }
    // 任务清单：`- [ ]` / `- [x]`。model.rs 有 ChecklistItem + attrs.checked，用它。
    let mut checked = None;
    if let Some(inner) = body[pos..].strip_prefix('[') {
        if let Some(close) = inner.find(']') {
            let flag = &inner[..close];
            let done = match flag {
                "x" | "X" => Some(true),
                " " => Some(false),
                _ => None,
            };
            if let Some(d) = done {
                checked = Some(d);
                pos += close + 2; // 吃掉 "[x]"
                if body[pos..].starts_with(' ') {
                    pos += 1;
                }
            }
        }
    }
    Some((ordered, pos, checked, pos))
}

/// 有序项的字面序号（进 attrs.number；"3." 这个字面量本身是语法标记）。
fn number_of(body: &str) -> u32 {
    body.chars()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .unwrap_or(1)
}

/// 续行判定：看起来会自成一个块（含被当列表项）的行不并进前一项。
fn starts_new_block(body: &str) -> bool {
    open_fence(body).is_some()
        || atx_heading(body).is_some()
        || thematic_break(body)
        || body.starts_with('>')
        || list_marker(body).is_some()
}

/// 整行只有一张图片时返回 (alt, src, title)。
fn sole_image(body: &str) -> Option<(String, String, String)> {
    if !body.starts_with("![") {
        return None;
    }
    let chars: Vec<char> = body.chars().collect();
    let (ts, te, dest, title, next) = link_like(&chars, 1)?;
    if next != chars.len() {
        return None; // 图后面还有字：保持字面，不拆块
    }
    Some((chars[ts..te].iter().collect(), dest, title))
}

// ============================================================ 行内解析 ===

fn text_inline(text: &str) -> Vec<Inline> {
    if text.is_empty() {
        Vec::new()
    } else {
        vec![Inline {
            text: text.to_string(),
            marks: Vec::new(),
        }]
    }
}

/// 行内解析入口。超过 `MAX_INLINE_PARSE_CHARS` 的行退化为字面文本（无损）。
pub fn inline_nodes(src: &str, kind: SourceKind) -> Vec<Inline> {
    if src.is_empty() || kind == SourceKind::PlainText {
        return text_inline(src);
    }
    if src.chars().count() > MAX_INLINE_PARSE_CHARS {
        return text_inline(src);
    }
    let chars: Vec<char> = src.chars().collect();
    let mut out: Vec<Inline> = Vec::new();
    let mut buf = String::new();
    let mut i = 0usize;

    macro_rules! flush {
        ($buf:expr, $out:expr) => {
            if !$buf.is_empty() {
                $out.push(Inline {
                    text: std::mem::take($buf),
                    marks: Vec::new(),
                });
            }
        };
    }

    while i < chars.len() {
        let c = chars[i];

        // 反斜杠转义 ASCII 标点：吃掉反斜杠，保住被保护的字符。
        if c == '\\' {
            if let Some(&n) = chars.get(i + 1) {
                if n.is_ascii_punctuation() {
                    buf.push(n);
                    i += 2;
                    continue;
                }
            }
            buf.push('\\');
            i += 1;
            continue;
        }

        // 代码片段 `x` / `` `x` ``：找不到同长的一组反引号就原样留着。
        if c == '`' {
            let n = run_len(&chars, i, '`');
            if let Some(close) = find_code_close(&chars, i + n, n) {
                flush!(&mut buf, &mut out);
                let inner: String = chars[i + n..close].iter().collect();
                let inner = inner.strip_prefix(' ').unwrap_or(&inner);
                let inner = inner.strip_suffix(' ').unwrap_or(inner);
                out.push(Inline {
                    text: inner.to_string(),
                    marks: vec![mark(MarkKind::Code)],
                });
                i = close + n;
                continue;
            }
            for _ in 0..n {
                buf.push('`');
            }
            i += n;
            continue;
        }

        // 图片：行内一律原样保留（只有独占一行时才成为 image 块）。
        if c == '!' && chars.get(i + 1) == Some(&'[') {
            if let Some((_, _, _, _, next)) = link_like(&chars, i + 1) {
                buf.extend(chars[i..next].iter());
                i = next;
                continue;
            }
            buf.push('!');
            i += 1;
            continue;
        }

        // 链接 [text](url "title")
        if c == '[' {
            if let Some((ts, te, dest, title, next)) = link_like(&chars, i) {
                flush!(&mut buf, &mut out);
                let inner: String = chars[ts..te].iter().collect();
                let mut pieces = inline_nodes(&inner, SourceKind::Markdown);
                if pieces.is_empty() {
                    pieces.push(Inline {
                        text: String::new(),
                        marks: Vec::new(),
                    });
                }
                for p in pieces.iter_mut() {
                    p.marks.push(link_mark(&dest, &title));
                }
                out.extend(pieces);
                i = next;
                continue;
            }
        }

        // <http://…> / <mailto:…> 自动链接
        if c == '<' {
            if let Some((url, next)) = autolink(&chars, i) {
                flush!(&mut buf, &mut out);
                out.push(Inline {
                    text: url.clone(),
                    marks: vec![link_mark(&url, "")],
                });
                i = next;
                continue;
            }
        }

        // 裸 URL（GFM 风格自动链接）；尾部标点退回正文，一个字符不丢。
        if c == 'h' || c == 'H' {
            if let Some((url, next)) = bare_url(&chars, i) {
                flush!(&mut buf, &mut out);
                out.push(Inline {
                    text: url.clone(),
                    marks: vec![link_mark(&url, "")],
                });
                i = next;
                continue;
            }
        }

        // 强调与删除线：* / _ / ** / __ / ~~
        if matches!(c, '*' | '_' | '~') {
            let n = run_len(&chars, i, c);
            let before = if i == 0 { None } else { Some(chars[i - 1]) };
            let after = chars.get(i + n).copied();
            if c == '~' {
                if n >= 2 {
                    if let Some(close) = find_strike_close(&chars, i + n) {
                        flush!(&mut buf, &mut out);
                        let inner: String = chars[i + 2..close].iter().collect();
                        let mut pieces = inline_nodes(&inner, SourceKind::Markdown);
                        if pieces.is_empty() {
                            pieces.push(Inline {
                                text: String::new(),
                                marks: Vec::new(),
                            });
                        }
                        for p in pieces.iter_mut() {
                            p.marks.push(mark(MarkKind::Strike));
                        }
                        out.extend(pieces);
                        i = close + 2;
                        continue;
                    }
                }
            } else if can_emph_open(c, before, after) {
                if let Some((close, clen)) = find_em_close(&chars, i + n, c, n) {
                    let consume = emph_consume(n, clen);
                    flush!(&mut buf, &mut out);
                    let inner: String = chars[i + consume..close].iter().collect();
                    let mut pieces = inline_nodes(&inner, SourceKind::Markdown);
                    if pieces.is_empty() {
                        pieces.push(Inline {
                            text: String::new(),
                            marks: Vec::new(),
                        });
                    }
                    for p in pieces.iter_mut() {
                        p.marks.extend(emph_marks(consume));
                    }
                    out.extend(pieces);
                    i = close + consume;
                    continue;
                }
            }
            for _ in 0..n {
                buf.push(c);
            }
            i += n;
            continue;
        }

        buf.push(c);
        i += 1;
    }
    flush!(&mut buf, &mut out);
    out
}

fn mark(kind: MarkKind) -> Mark {
    Mark {
        kind,
        attrs: BTreeMap::new(),
    }
}

fn link_mark(href: &str, title: &str) -> Mark {
    let mut attrs = BTreeMap::new();
    attrs.insert("href".to_string(), Value::String(href.to_string()));
    if !title.is_empty() {
        attrs.insert("title".to_string(), Value::String(title.to_string()));
    }
    Mark {
        kind: MarkKind::Link,
        attrs,
    }
}

fn emph_consume(open_len: usize, close_len: usize) -> usize {
    if open_len >= 3 && close_len >= 3 {
        3
    } else if open_len >= 2 && close_len >= 2 {
        2
    } else {
        1
    }
}

/// 一次吃掉 3 个 = 粗+斜（`***x***`），2 个 = 粗，1 个 = 斜。
fn emph_marks(consume: usize) -> Vec<Mark> {
    match consume {
        0 | 1 => vec![mark(MarkKind::Italic)],
        2 => vec![mark(MarkKind::Bold)],
        _ => vec![mark(MarkKind::Italic), mark(MarkKind::Bold)],
    }
}

fn run_len(chars: &[char], from: usize, c: char) -> usize {
    chars[from..].iter().take_while(|&&x| x == c).count()
}

/// 标点判定要含常用中日韩标点，否则中文里的 `（重要）` 会被当成文字相邻而误判边界。
fn is_punct(c: char) -> bool {
    c.is_ascii_punctuation()
        || matches!(
            c,
            '“' | '”'
                | '‘'
                | '’'
                | '《'
                | '》'
                | '〈'
                | '〉'
                | '「'
                | '」'
                | '『'
                | '』'
                | '（'
                | '）'
                | '、'
                | '。'
                | '·'
                | '！'
                | '？'
                | '：'
                | '；'
                | '—'
                | '…'
                | '〜'
                | '～'
        )
}

fn left_flanking(before: Option<char>, after: Option<char>) -> bool {
    match after {
        None => false,
        Some(a) => {
            !a.is_whitespace()
                && (!is_punct(a) || before.is_none_or(|b| b.is_whitespace() || is_punct(b)))
        }
    }
}

fn right_flanking(before: Option<char>, after: Option<char>) -> bool {
    match before {
        None => false,
        Some(b) => {
            !b.is_whitespace()
                && (!is_punct(b) || after.is_none_or(|a| a.is_whitespace() || is_punct(a)))
        }
    }
}

/// `_` 的词内规则让 `snake_case_name` 保持字面（CommonMark 同源规则）。
fn can_emph_open(c: char, before: Option<char>, after: Option<char>) -> bool {
    if !left_flanking(before, after) {
        return false;
    }
    if c == '_' {
        return !right_flanking(before, after) || before.is_none_or(is_punct);
    }
    true
}

fn can_emph_close(c: char, before: Option<char>, after: Option<char>) -> bool {
    if !right_flanking(before, after) {
        return false;
    }
    if c == '_' {
        return !left_flanking(before, after) || after.is_none_or(is_punct);
    }
    true
}

/// 从 `start` 往后找第一个能关掉这个开符的闭符（跳过代码片段与转义字符）。
fn find_em_close(chars: &[char], start: usize, c: char, open_len: usize) -> Option<(usize, usize)> {
    let mut j = start;
    while j < chars.len() {
        if chars[j] == '`' {
            let n = run_len(chars, j, '`');
            j = find_code_close(chars, j + n, n).map_or(j + n, |close| close + n);
            continue;
        }
        if chars[j] == '\\' && j + 1 < chars.len() {
            j += 2;
            continue;
        }
        if chars[j] == c {
            let n = run_len(chars, j, c);
            let before = if j == 0 { None } else { Some(chars[j - 1]) };
            let after = chars.get(j + n).copied();
            let strength_ok = open_len < 2 || n >= 2;
            if j > start && can_emph_close(c, before, after) && strength_ok {
                return Some((j, n));
            }
            j += n;
            continue;
        }
        j += 1;
    }
    None
}

fn find_strike_close(chars: &[char], start: usize) -> Option<usize> {
    let mut j = start.max(1);
    while j + 1 < chars.len() {
        if chars[j] == '~' && chars[j + 1] == '~' {
            return Some(j);
        }
        j += 1;
    }
    None
}

fn find_code_close(chars: &[char], start: usize, n: usize) -> Option<usize> {
    let mut j = start;
    while j < chars.len() {
        if chars[j] == '`' {
            let run = run_len(chars, j, '`');
            if run == n {
                return Some(j);
            }
            j += run;
            continue;
        }
        j += 1;
    }
    None
}

/// `[text](dest "title")` → (正文区间, dest, title, 下一个待读位置)。
fn link_like(chars: &[char], open: usize) -> Option<(usize, usize, String, String, usize)> {
    if chars.get(open) != Some(&'[') {
        return None;
    }
    let mut depth = 0usize;
    let mut close = None;
    let mut j = open;
    while j < chars.len() {
        match chars[j] {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(j);
                    break;
                }
            }
            '\\' => j += 1,
            _ => {}
        }
        j += 1;
    }
    let close = close?;
    if chars.get(close + 1) != Some(&'(') {
        return None; // `[text][ref]` / `[ref]: url` → 整体按字面保留
    }
    let mut paren = 1usize;
    let mut k = close + 2;
    let mut end = None;
    while k < chars.len() {
        match chars[k] {
            '(' => paren += 1,
            ')' => {
                paren -= 1;
                if paren == 0 {
                    end = Some(k);
                    break;
                }
            }
            '\\' => k += 1,
            _ => {}
        }
        k += 1;
    }
    let end = end?;
    if chars[open + 1..close].contains(&'[') {
        return None;
    }
    let raw: String = chars[close + 2..end].iter().collect();
    let (dest, title) = split_dest_title(raw.trim());
    Some((open + 1, close, dest, title, end + 1))
}

fn split_dest_title(raw: &str) -> (String, String) {
    if let Some(rest) = raw.strip_prefix('<') {
        if let Some(pos) = rest.find('>') {
            return (
                rest[..pos].to_string(),
                strip_quotes(rest[pos + 1..].trim()),
            );
        }
    }
    match raw.split_once([' ', '\t', '\n']) {
        Some((d, t)) => (d.to_string(), strip_quotes(t.trim())),
        None => (raw.to_string(), String::new()),
    }
}

fn strip_quotes(s: &str) -> String {
    let s = s.trim();
    let (first, last) = match (s.chars().next(), s.chars().last()) {
        (Some(f), Some(l)) => (f, l),
        _ => return s.to_string(),
    };
    if first == last && matches!(first, '"' | '\'' | '“' | '”') && s.chars().count() >= 2 {
        return s[first.len_utf8()..s.len() - first.len_utf8()].to_string();
    }
    s.to_string()
}

/// `<scheme:...>` 自动链接（不吞空格，也不吞 HTML 标签）。
fn autolink(chars: &[char], open: usize) -> Option<(String, usize)> {
    let mut out = String::new();
    let mut j = open + 1;
    while j < chars.len() {
        let c = chars[j];
        if c == '>' {
            if out.contains("://") || out.starts_with("mailto:") {
                return Some((out, j + 1));
            }
            return None;
        }
        if c.is_whitespace() || c == '<' {
            return None;
        }
        out.push(c);
        j += 1;
    }
    None
}

/// 裸 URL：前一个字符不能是文字/URL 标点（否则是路径的一部分），尾部标点剥掉后
/// 由主循环当普通字符处理 —— 字符不会消失，只是不再属于 URL。
fn bare_url(chars: &[char], i: usize) -> Option<(String, usize)> {
    if i > 0 {
        let p = chars[i - 1];
        if p.is_alphanumeric()
            || matches!(p, ':' | '/' | '.' | '-' | '_' | '%' | '?' | '&' | '=' | '#')
        {
            return None;
        }
    }
    let mut j = i;
    while j < chars.len() {
        let c = chars[j];
        if c.is_whitespace()
            || matches!(
                c,
                '<' | '>' | '|' | '[' | ']' | '(' | ')' | '"' | '\'' | '`'
            )
        {
            break;
        }
        j += 1;
    }
    let mut k = j;
    while k > i && matches!(chars[k - 1], ',' | '.' | ';' | ':' | '!' | '?' | '-' | '_') {
        k -= 1;
    }
    let cand: String = chars[i..k].iter().collect();
    if !(cand.starts_with("http://") || cand.starts_with("https://")) {
        return None;
    }
    Some((cand, k))
}

// ------------------------------------------------------------ 嗅探辅助 ---

/// 只用于 `source::sniff`：这一行是否"只有 Markdown 才这么写"。
pub(crate) fn looks_like_markdown_line(body: &str) -> bool {
    open_fence(body).is_some()
        || atx_heading(body).is_some()
        || thematic_break(body)
        || body == ">"
        || body.starts_with("> ")
        || list_marker(body).is_some()
}

/// 只用于 `source::sniff`：像不像有链接/图片（不做真正的行内解析）。
pub(crate) fn has_link_like(body: &str) -> bool {
    if body.len() > 8 * 1024 {
        return false; // 巨型单行不必嗅探（无损：解析不了就是字面文本）
    }
    let chars: Vec<char> = body.chars().collect();
    chars
        .iter()
        .enumerate()
        .any(|(i, &c)| c == '[' && link_like(&chars, i).is_some())
}

// ============================================================ 单元测试 ===

#[cfg(test)]
mod tests {
    use super::*;
    use notera_richtext::{parse_from_value, MarkKind};
    use std::collections::HashSet;

    /// fixture 一律过真实闸门：`to_document` 的产物必须能被 `parse()` 零错误读回。
    pub(crate) fn md(src: &str) -> Document {
        let draft = to_document(src, SourceKind::Markdown, "in-test0000abcd", None);
        let v = serde_json::to_value(&draft).expect("文档必须可序列化");
        parse_from_value(&v).expect("Markdown 产物必须过 parse()")
    }

    pub(crate) fn txt(src: &str) -> Document {
        let draft = to_document(src, SourceKind::PlainText, "in-test0000abcd", None);
        let v = serde_json::to_value(&draft).expect("文档必须可序列化");
        parse_from_value(&v).expect("纯文本产物必须过 parse()")
    }

    fn types(d: &Document) -> Vec<BlockType> {
        d.content.iter().map(|b| b.type_.clone()).collect()
    }

    fn text_at(d: &Document, i: usize) -> String {
        d.content[i].plain_text()
    }

    fn kinds(d: &Document, i: usize, j: usize) -> Vec<MarkKind> {
        d.content[i].content[j]
            .marks
            .iter()
            .map(|m| m.kind.clone())
            .collect()
    }

    fn attr(d: &Document, i: usize, key: &str) -> Option<Value> {
        d.content[i].attrs.get(key).cloned()
    }

    #[test]
    fn atx_headings_one_to_six_with_levels() {
        let d = md("# 一级\n###### 六级\n####### 七个井号不是标题\n## 关掉 ##\n");
        assert_eq!(
            types(&d),
            vec![
                BlockType::Heading,
                BlockType::Heading,
                BlockType::Paragraph,
                BlockType::Heading
            ]
        );
        assert_eq!(attr(&d, 0, "level"), Some(Value::from(1u8)));
        assert_eq!(attr(&d, 1, "level"), Some(Value::from(6u8)));
        assert_eq!(text_at(&d, 2), "####### 七个井号不是标题");
        assert_eq!(text_at(&d, 3), "关掉");
    }

    #[test]
    fn paragraph_and_thematic_breaks() {
        let d = md("第一行\n第二行\n\n---\n\n* * *\n___\n");
        assert_eq!(
            types(&d),
            vec![
                BlockType::Paragraph,
                BlockType::Paragraph,
                BlockType::HorizontalRule,
                BlockType::HorizontalRule,
                BlockType::HorizontalRule,
            ]
        );
    }

    #[test]
    fn hard_break_becomes_block_boundary() {
        // 扁平 schema 没有 hardBreak 行内节点：行尾两个空格 = 换块，不吃字符。
        let d = md("甲  \n乙\\\n丙\n");
        assert_eq!(types(&d), vec![BlockType::Paragraph; 3]);
        assert_eq!(notera_richtext::extract(&d).plain_text, "甲\n乙\\\n丙");
        for b in &d.content {
            assert!(!b.plain_text().contains('\n'), "正文里不该出现换行符");
        }
    }

    #[test]
    fn fenced_code_info_string_and_internal_newlines() {
        let d = md("```rust\nfn a() {\n\tlet x = 1;\n}\n```\n~~~py\nprint(1)\n~~~\n");
        assert_eq!(types(&d), vec![BlockType::CodeBlock, BlockType::CodeBlock]);
        assert_eq!(attr(&d, 0, "lang"), Some(Value::from("rust")));
        assert_eq!(text_at(&d, 0), "fn a() {\n\tlet x = 1;\n}");
        assert_eq!(attr(&d, 1, "lang"), Some(Value::from("py")));
        assert_eq!(text_at(&d, 1), "print(1)");
    }

    #[test]
    fn unclosed_fence_eats_everything_to_eof() {
        let d = md("前面\n```\n没闭合\n还有 *星* 也不解释\n");
        assert_eq!(types(&d), vec![BlockType::Paragraph, BlockType::CodeBlock]);
        assert_eq!(text_at(&d, 1), "没闭合\n还有 *星* 也不解释");
    }

    #[test]
    fn blockquote_lines_carry_depth() {
        let d = md("> 一层\n> > 两层\n>\n> 引文里的 `代码`\n");
        assert_eq!(
            types(&d),
            vec![
                BlockType::BlockQuote,
                BlockType::BlockQuote,
                BlockType::BlockQuote
            ]
        );
        assert_eq!(attr(&d, 0, "indent"), Some(Value::from(1u8)));
        assert_eq!(attr(&d, 1, "indent"), Some(Value::from(2u8)));
        assert_eq!(text_at(&d, 2), "引文里的 代码");
        assert_eq!(kinds(&d, 2, 1), vec![MarkKind::Code]);
    }

    #[test]
    fn lists_are_flat_with_indent_and_number() {
        let d = md("- 甲\n  - 乙\n    - 丙\n- 丁\n\n3. 起步\n4. 第二\n");
        assert_eq!(
            types(&d),
            vec![
                BlockType::BulletList,
                BlockType::BulletList,
                BlockType::BulletList,
                BlockType::BulletList,
                BlockType::OrderedList,
                BlockType::OrderedList,
            ]
        );
        let indents: Vec<u64> = d
            .content
            .iter()
            .map(|b| b.attrs.get("indent").and_then(Value::as_u64).unwrap_or(99))
            .collect();
        assert_eq!(indents, vec![0, 1, 2, 0, 0, 0]);
        assert_eq!(attr(&d, 4, "number"), Some(Value::from(3i64)));
        assert_eq!(attr(&d, 5, "number"), Some(Value::from(4i64)));
    }

    #[test]
    fn nested_list_continuation_joins_the_item() {
        let d = md("- 第一项\n  续写的一行\n- 第二项\n  - 子项\n");
        assert_eq!(
            types(&d),
            vec![
                BlockType::BulletList,
                BlockType::BulletList,
                BlockType::BulletList
            ]
        );
        assert_eq!(text_at(&d, 0), "第一项 续写的一行");
        assert_eq!(attr(&d, 2, "indent"), Some(Value::from(1u8)));
    }

    #[test]
    fn task_items_use_checklist_with_checked_attr() {
        let d = md("- [ ] 未做\n- [x] 已做\n- [X] 大写也算\n");
        assert_eq!(types(&d), vec![BlockType::ChecklistItem; 3]);
        let checked: Vec<Option<bool>> = d
            .content
            .iter()
            .map(|b| b.attrs.get("checked").and_then(Value::as_bool))
            .collect();
        assert_eq!(checked, vec![Some(false), Some(true), Some(true)]);
        assert_eq!(text_at(&d, 0), "未做");
        assert_eq!(text_at(&d, 1), "已做");
    }

    #[test]
    fn emphasis_and_code_and_strike() {
        let d = md("**粗** 与 *斜* 与 `码` 与 ~~删~~ 与 ***粗斜***\n");
        assert_eq!(types(&d), vec![BlockType::Paragraph]);
        assert_eq!(kinds(&d, 0, 0), vec![MarkKind::Bold]);
        assert_eq!(kinds(&d, 0, 2), vec![MarkKind::Italic]);
        assert_eq!(kinds(&d, 0, 4), vec![MarkKind::Code]);
        assert_eq!(kinds(&d, 0, 6), vec![MarkKind::Strike]);
        let last = d.content[0].content.len() - 1;
        assert_eq!(
            d.content[0].content[last]
                .marks
                .iter()
                .map(|m| m.kind.clone())
                .collect::<Vec<_>>(),
            vec![MarkKind::Italic, MarkKind::Bold]
        );
        assert_eq!(text_at(&d, 0), "粗 与 斜 与 码 与 删 与 粗斜");
    }

    #[test]
    fn intraword_underscore_stays_literal_but_star_emphasises() {
        assert_eq!(text_at(&md("snake_case_name\n"), 0), "snake_case_name");
        assert_eq!(text_at(&md("un*be*lievable\n"), 0), "unbelievable");
        // 未闭合的 ** 一个字不丢，星号留在正文里
        assert_eq!(text_at(&md("开头 **没有闭合\n"), 0), "开头 **没有闭合");
    }

    #[test]
    fn links_and_autolinks_move_href_into_attrs() {
        let d = md("[文字](https://example.test/a \"标题\") 与 <https://auto.test/x> 与 https://bare.test/p\n");
        let link = &d.content[0].content[0];
        assert_eq!(link.text, "文字");
        assert_eq!(link.marks[0].kind, MarkKind::Link);
        assert_eq!(
            link.marks[0].attrs["href"],
            Value::from("https://example.test/a")
        );
        assert_eq!(link.marks[0].attrs["title"], Value::from("标题"));
        let plain = notera_richtext::extract(&d).plain_text;
        assert!(
            plain.contains("https://auto.test/x"),
            "角括号自动链接的文字要留在正文"
        );
        assert!(
            plain.contains("https://bare.test/p"),
            "裸 URL 的文字要留在正文"
        );
    }

    #[test]
    fn image_alone_becomes_block_and_inline_image_stays_text() {
        let d = md("![配图](images/a.png)\n前面 ![行内图](b.png) 后面\n");
        assert_eq!(types(&d), vec![BlockType::Image, BlockType::Paragraph]);
        assert_eq!(attr(&d, 0, "src"), Some(Value::from("images/a.png")));
        assert_eq!(attr(&d, 0, "alt"), Some(Value::from("配图")));
        assert_eq!(text_at(&d, 0), "配图");
        assert_eq!(text_at(&d, 1), "前面 ![行内图](b.png) 后面");
    }

    #[test]
    fn unknown_markup_survives_verbatim() {
        let src = "[文字][标]\n\n[标]: https://ref.test \"引用式\" \n<div class=\"x\">原样</div>\nSetext\n======\n脚注[^1]\n";
        let d = md(src);
        let plain = notera_richtext::extract(&d).plain_text;
        for token in [
            "文字",
            "[文字][标]",
            "[标]:",
            "https://ref.test",
            "原样",
            "Setext",
            "======",
            "脚注[^1]",
        ] {
            assert!(
                plain.contains(token),
                "{token} 必须原样留在正文里；实得：{plain}"
            );
        }
    }

    #[test]
    fn crlf_tabs_and_bom_are_handled() {
        let d = md("\u{feff}# 标题\r\n\r\n- 甲\r\n\t- 乙\r\n");
        assert_eq!(
            types(&d),
            vec![
                BlockType::Heading,
                BlockType::BulletList,
                BlockType::BulletList
            ]
        );
        assert_eq!(text_at(&d, 0), "标题");
        assert_eq!(attr(&d, 2, "indent"), Some(Value::from(1u8)));
    }

    #[test]
    fn plain_text_source_makes_no_marks() {
        let d = txt("# 这不是标题\n**也不是粗体** `也不是代码`\n");
        assert_eq!(types(&d), vec![BlockType::Paragraph, BlockType::Paragraph]);
        assert_eq!(text_at(&d, 0), "# 这不是标题");
        assert!(d
            .content
            .iter()
            .all(|b| b.content.iter().all(|i| i.marks.is_empty())));
    }

    #[test]
    fn block_ids_are_deterministic_and_unique() {
        let a = md("# 标题\n\n段落 *x*\n");
        let b = md("# 标题\n\n段落 *x*\n");
        assert_eq!(a, b, "同一输入必须逐字段相同（含 id）");
        let ids: Vec<&str> = a.content.iter().map(|x| x.id.as_str()).collect();
        let uniq: HashSet<&str> = ids.iter().copied().collect();
        assert_eq!(ids.len(), uniq.len(), "块 id 文档内必须唯一");
        for id in &ids {
            assert!(
                (8..=32).contains(&id.len()),
                "§10.2 要求 id 长度 8..32，实得 {id}"
            );
        }
    }

    #[test]
    fn emoji_and_cjk_boundaries_survive() {
        let d = md("家庭 👨‍👩‍👧 与 **重要** 与 （中文括号）里的 *强调*\n");
        let plain = notera_richtext::extract(&d).plain_text;
        assert!(plain.contains("👨‍👩‍👧"), "ZWJ 序列不许被拆：{plain}");
        assert!(plain.contains("重要"));
        assert!(plain.contains("（中文括号）里的"));
    }

    #[test]
    fn overlong_line_degrades_to_literal_text_instead_of_blowing_up() {
        // 成本封顶的代价是"不解释"，但一个字都不能丢。
        let line = format!("{}**尾**", "甲".repeat(MAX_INLINE_PARSE_CHARS + 1));
        let d = md(&line);
        assert_eq!(types(&d), vec![BlockType::Paragraph]);
        assert_eq!(text_at(&d, 0), line);
    }

    #[test]
    fn checkbox_without_space_and_empty_items_are_lossless() {
        let d = md("- [x]无空格\n- \n-\n* \n");
        assert_eq!(
            types(&d),
            vec![
                BlockType::ChecklistItem,
                BlockType::BulletList,
                BlockType::BulletList,
                BlockType::BulletList
            ]
        );
        assert_eq!(text_at(&d, 0), "无空格");
        assert_eq!(attr(&d, 0, "checked"), Some(Value::Bool(true)));
        assert!(
            d.content[1].content.is_empty(),
            "空列表项保留成空块，不消失"
        );
    }
}
