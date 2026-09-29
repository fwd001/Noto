//! `.enex`（Evernote 导出）→ 结构化笔记。
//!
//! 规范来源：Evernote ENEX/ENML —— `.enex` 根 `<en-export>`，每条 `<note>` 里
//! `<title>` / `<content>` / 若干字段 / 若干 `<resource>`，而 `<content>` 装的是
//! **一整段 CDATA 包起来的 XHTML 子集（ENML）**，附件以 base64 内嵌在 `<resource><data>`。
//! 依赖 `quick-xml` / `base64` 都已在 workspace 单一版本源里（用户 2026-09-27 批准为此动依赖图）。
//!
//! ## 两遍解析（这不是洁癖，是格式决定的）
//! 第一遍读信封（`enex_envelope`）拿到 title / 那段 CDATA / 字段 / 资源字节；
//! 第二遍（`enml`）把 CDATA 里那段 XML 当 XML 读。一遍做不到：CDATA 里的 `<div>`
//! 对 XML 词法来说是**字符**，不是标签。
//!
//! ## 判据与 Markdown 侧同一条：看得懂的搬进来，看不懂的点名
//! 未映射的 ENML 标签进 [`EnexNote::unknown_enml_tags`]，本库 schema 表达不了的 note 级
//! 字段进 [`EnexNote::unknown_fields`]，配对不上的引用进 [`EnexNote::dangling_media_hashes`]。
//! **没有"不认识就丢掉"这条路**（§39 禁止静默降级）。
//!
//! ### 三处设计内的偏离（都有测试钉着）
//! 1. **表格不映射成 `table` 族**：本 crate 的 Markdown 侧同样不把 GFM 表格当结构（按字面进正文，
//!    见 lib.rs 的"无损"那节），而表格族的成组约束属于权威 schema 的判定，导入器不去猜。
//!    ENML 里一行表格变成一条段落（单元格以 ` | ` 连接），`table`/`tbody`/`tr`/`td`/`th` 记名。
//! 2. **`<tag>`（笔记标签）与 `created`/`updated`/`author`/`source-url` 不入库**：本库没有标签列，
//!    笔记时间戳一律由 Store 在写入时盖（DATA-MODEL §5.1）。伪造时间戳会污染排序与三方合并
//!    的锚点，所以选择**报出来**并留在这里 —— 要落地得先按 §51 定产品语义。
//! 3. **`<en-media hash>` 是 MD5**（Evernote 的历史包袱），而本库按 **sha256** 内容寻址。
//!    这里自己算 sha256、拿 md5 只做"正文引用 → resource"的配对，`md5` 原值进块 attrs。

use crate::error::ImportError;
use base64::Engine as _;
use notera_richtext::{Block, BlockType, Document, Inline, Mark, MarkKind};
use quick_xml::events::Event;
use quick_xml::Reader;
use std::collections::BTreeMap;

/// 一个 `<resource>` 解出来的字节与元数据。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnexResource {
    /// 我们自己算的 sha256（64 位小写十六进制）—— 本库的内容寻址键。
    pub sha256: String,
    /// Evernote 给的 MD5（通常 32 位大写十六进制），用来配对正文里的 `<en-media>`。
    pub md5: Option<String>,
    pub mime: String,
    pub filename: Option<String>,
    pub data: Vec<u8>,
}

impl EnexResource {
    fn is_inline_image(&self) -> bool {
        self.mime.starts_with("image/")
    }

    /// 落进 doc 的块 attrs。存储层就是按这些键从 doc 里派生
    /// `attachments` / `note_attachments`（`notera_richtext::extract::attachments`）。
    fn block_attrs(&self) -> BTreeMap<String, serde_json::Value> {
        let mut attrs = BTreeMap::new();
        attrs.insert(
            "sha256".into(),
            serde_json::Value::String(self.sha256.clone()),
        );
        attrs.insert(
            "role".into(),
            serde_json::Value::String(if self.is_inline_image() {
                "inline".into()
            } else {
                "file".into()
            }),
        );
        attrs.insert(
            "size".into(),
            serde_json::Value::Number(self.data.len().into()),
        );
        attrs.insert(
            "mediaType".into(),
            serde_json::Value::String(self.mime.clone()),
        );
        if let Some(name) = &self.filename {
            attrs.insert("name".into(), serde_json::Value::String(name.clone()));
        }
        if let Some(md5) = &self.md5 {
            attrs.insert("md5".into(), serde_json::Value::String(md5.clone()));
        }
        attrs
    }
}

/// 一条 `<note>` 的解析结果。
#[derive(Clone, Debug, Default)]
pub struct EnexNote {
    pub title: String,
    /// 正文 + 附件块（附件字节由调用方写盘：`Store::restore_blob`）。
    pub doc: Document,
    pub resources: Vec<EnexResource>,
    /// 本库表达不了的 note 级字段，**点名**给出（`tag` / `created` / `note-attr` …）。
    pub unknown_fields: Vec<String>,
    /// ENML 里没被映射成块/mark 的标签名（去重、按出现顺序）。
    pub unknown_enml_tags: Vec<String>,
    /// 正文引用了、但这份文件里没有对应资源的 `<en-media hash>`。
    pub dangling_media_hashes: Vec<String>,
    /// 解不出字节的资源（非 base64 编码、坏 base64、0 字节），带原因。
    pub undecodable_resources: Vec<String>,
}

/// 一份 `.enex` 的全部结果。
#[derive(Clone, Debug, Default)]
pub struct EnexFile {
    pub notes: Vec<EnexNote>,
    /// `<en-export>` 上除 `export-date` / `version` 之外的属性名。
    pub unknown_export_fields: Vec<String>,
}

// ================================================================ 第一遍 ===

/// 信封里的一条 note（正文还是原始 CDATA 字符串）。
#[derive(Clone, Debug, Default)]
struct RawNote {
    title: String,
    content: String,
    unknown_fields: Vec<String>,
    resources: Vec<EnexResource>,
    undecodable: Vec<String>,
}

#[derive(Clone, Debug, Default)]
struct RawResource {
    mime: String,
    filename: Option<String>,
    md5: Option<String>,
    encoding: String,
    data: String,
}

/// note 级认识的字段；`title` / `content` 在别处处理，其余一律进 `unknown_fields`。
/// 这份清单只用来区分"我们确实看懂了的字段"与"没见过但也不能当没有的字段" ——
/// 后者一律记名，不静默吞（§39）。
const NOTE_FIELDS: &[&str] = &[
    "title",
    "content",
    "created",
    "updated",
    "tag",
    "author",
    "source-url",
    "task-completed",
    "task-expiry",
    "task-flag",
    "task-done",
    "reminder-time",
    "reminder-time-timezone",
    "reminder-user-id",
    "reminder-user-time",
    "reminder-done",
    "share-date",
    "owner-id",
    "color",
    "conflict-md5",
    "conflict-order",
    "attributes",
    "attribute",
    "note-attributes",
    "note-attribute",
    "resource",
    "expand",
    "sticky",
    "chat-fragment-guid",
    "chat-nickname",
    "shared",
    "limit-reached",
    "content-class",
    "largest-resource-encryption",
];

/// 标签名。ENEX 不带命名空间（Evernote 的 DTD 里没有前缀），所以整名即局部名。
fn local(e: &quick_xml::name::QName) -> String {
    e.as_ref().to_string()
}

fn attrs_of(e: &quick_xml::events::BytesStart) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for a in e.attributes().flatten() {
        let k = a.key.as_ref().to_string();
        // `normalized_value` 比旧那版 `unescape_value` 多做一件事：按 XML 的 AVNormalize
        // 把属性值里的字面空白（制表/换行）折成空格。这一格只读 `encoding` 与 `hash`
        // 这类单词值，所以这条差异没有受害者 —— 但它是一处**真实的语义差**，写下来免得
        // 以后有人拿"升级没改行为"这句话去解释别的属性。
        let v = a
            .normalized_value(quick_xml::XmlVersion::default())
            .map(|c| c.into_owned())
            .unwrap_or_default();
        out.insert(k, v);
    }
    out
}

fn make_reader<'a>(text: &'a str) -> Reader<&'a [u8]> {
    let mut r = Reader::from_str(text);
    // CDATA 要能拿回原文（第二遍的输入就是第一遍的 CDATA 内容）
    r.config_mut().check_end_names = true;
    // 没分号的裸 `&`（`Tom & Jerry` 这种手写/第三方导出）按原文放行。
    // 默认是整篇 IllFormed 报错 —— 那等于"一个字符让 500 条笔记一条都导不进来"。
    // 摘掉这一行会红在 `a_lone_ampersand_survives_without_failing_the_import`。
    r.config_mut().allow_dangling_amp = true;
    r
}

/// 普通文本节点。0.42 起解析器**不再**顺手反转义，引用是另一个事件（见 `ref_text`），
/// 这里只做行尾归一。
fn text_of(e: &quick_xml::events::BytesText) -> String {
    e.xml10_content().into_owned()
}

/// 引用事件（`&amp;` / `&#65;`）→ 该进文本的那几个字符。
///
/// 为什么必须自己填这一格：换解析库版本之前，`&amp;` 是解析器在 Text 里就解好的；
/// 现在它单独成一个事件，不接的话"粗 &amp; 斜"会变成"粗 斜"——**静默少字符**，
/// 而不是任何报错。认不出的引用（ENML 里常见的 `&nbsp;`）原样留着：0.37 那版
/// 是把整段文本变成空串，那是更坏的静默丢失。
fn ref_text(r: &quick_xml::events::BytesRef) -> String {
    let raw = format!("&{};", r.xml10_content());
    quick_xml::escape::unescape(&raw)
        .map(|c| c.into_owned())
        .unwrap_or(raw)
}

/// 解析一份 `.enex`。`id_prefix` 参与块 id 生成：同一份字节两次导入必须得到同一批 id
/// （幂等判定与 diff 都依赖这点）。
pub fn parse_enex(text: &str, id_prefix: &str) -> Result<EnexFile, ImportError> {
    let raws = read_envelope(text)?;
    let mut out = EnexFile::default();
    for (i, raw) in raws.iter().enumerate() {
        let mut note = EnexNote {
            title: raw.title.clone(),
            resources: raw.resources.clone(),
            unknown_fields: raw.unknown_fields.clone(),
            undecodable_resources: raw.undecodable.clone(),
            ..Default::default()
        };
        // 一条 note 一套 id：前缀带上序号，避免两条 note 的块 id 撞车
        let prefix = format!("{id_prefix}n{i:03}");
        let parsed = parse_enml(&raw.content, &prefix)?;
        let tags = parsed.unknown_tags.clone();
        let (doc, dangling) = parsed.document_for(&note.resources);
        note.doc = doc;
        note.unknown_enml_tags = tags;
        note.dangling_media_hashes = dangling;
        out.notes.push(note);
    }
    Ok(out)
}

fn read_envelope(text: &str) -> Result<Vec<RawNote>, ImportError> {
    let mut reader = make_reader(text);
    let mut notes: Vec<RawNote> = Vec::new();
    let mut cur: Option<RawNote> = None;
    let mut res: Option<RawResource> = None;
    // 当前文本节点对应到哪个槽位
    let mut slot = Slot::None;

    loop {
        match reader.read_event() {
            Err(e) => {
                return Err(ImportError::InvalidDoc(format!(
                    ".enex 不是合法的 XML：{e}"
                )))
            }
            Ok(Event::Eof) => break,
            Ok(Event::Start(e)) => {
                let name = local(&e.name());
                match name.as_str() {
                    "note" => {
                        cur = Some(RawNote::default());
                        slot = Slot::None;
                    }
                    "title" => slot = Slot::Title,
                    "content" => slot = Slot::Content,
                    "resource" if cur.is_some() => {
                        res = Some(RawResource::default());
                        slot = Slot::None;
                    }
                    "data" | "data-encoding" | "mime" | "md5" | "file-name" | "width"
                    | "height" | "duration" | "recognition" => {
                        if res.is_some() {
                            // Evernote 的编码写在 `<data encoding="base64">` 的**属性**上，
                            // 不是子元素 —— 只读 `<data-encoding>` 会把所有资源当成"未知编码"。
                            if name == "data" {
                                if let Some(enc) = attrs_of(&e).get("encoding").cloned() {
                                    if let Some(r) = &mut res {
                                        r.encoding = enc;
                                    }
                                }
                            }
                            slot = Slot::Res(name.clone());
                        }
                    }
                    "resource-attributes" | "alternate-data" | "data-headers" => {
                        if res.is_some() {
                            slot = Slot::None;
                        }
                    }
                    other => {
                        // 认得但本库表达不了的（`tag`/时间戳/`author`…），和根本没见过的，
                        // 都要记名；其余容器型标签（resource / attributes 这类）不必吵。
                        const UNMAPPABLE: &[&str] =
                            &["tag", "created", "updated", "author", "source-url"];
                        if res.is_none() {
                            if !NOTE_FIELDS.contains(&other) || UNMAPPABLE.contains(&other) {
                                if let Some(n) = &mut cur {
                                    push_unique(&mut n.unknown_fields, other.to_string());
                                }
                            }
                            slot = Slot::Ignored;
                        }
                    }
                }
            }
            Ok(Event::Empty(e)) if res.is_none() => {
                let name = local(&e.name());
                if !NOTE_FIELDS.contains(&name.as_str()) {
                    if let Some(n) = &mut cur {
                        push_unique(&mut n.unknown_fields, name);
                    }
                }
            }
            Ok(Event::Text(t)) => {
                // 普通文本：引用已经被拆成单独的 GeneralRef 事件（见 `ref_text`）
                let raw = text_of(&t);
                route_text(&slot, &mut cur, &mut res, &raw);
            }
            Ok(Event::GeneralRef(r)) => {
                let raw = ref_text(&r);
                route_text(&slot, &mut cur, &mut res, &raw);
            }
            Ok(Event::CData(t)) => {
                // CDATA：正文那一段就是靠它装着的（`<![CDATA[<en-note>…</en-note>]]>`）。
                // 这个变体不接就等于把整个正文丢掉 —— 第一版就是这么"导出 0 块还不报错"。
                let raw = t.into_inner().into_owned();
                route_text(&slot, &mut cur, &mut res, &raw);
            }
            Ok(Event::End(e)) => {
                let name = local(&e.name());
                match name.as_str() {
                    "note" => {
                        if let Some(mut n) = cur.take() {
                            n.title = n.title.trim().to_string();
                            notes.push(n);
                        }
                        slot = Slot::None;
                    }
                    "title" | "content" => slot = Slot::None,
                    "resource" => {
                        if let (Some(n), Some(r)) = (&mut cur, res.take()) {
                            finish_resource(r, n);
                        }
                        slot = Slot::None;
                    }
                    "data" | "data-encoding" | "mime" | "md5" | "file-name" | "width"
                    | "height" | "duration" | "recognition" => slot = Slot::None,
                    _ => {}
                }
            }
            Ok(_) => {}
        }
    }
    // 半截的 <note> 绝不能"默默少一条"：用户看到的会是"导入成功，0 条新增"，
    // 而那正是他最需要知道"文件坏了"的时刻。
    if cur.is_some() {
        return Err(ImportError::InvalidDoc(
            ".enex 在 <note> 里就结束了（文件被截断）—— 拒绝而不是少导一条".into(),
        ));
    }
    Ok(notes)
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Slot {
    None,
    Ignored,
    Title,
    Content,
    Res(String),
}

/// 把一个文本/CDATA 节点送到该去的地方。信封里只认四个槽，别的都当噪声。
fn route_text(slot: &Slot, cur: &mut Option<RawNote>, res: &mut Option<RawResource>, raw: &str) {
    match slot {
        Slot::Content => {
            if let Some(n) = cur {
                n.content.push_str(raw);
            }
        }
        Slot::Title => {
            if let Some(n) = cur {
                n.title.push_str(raw);
            }
        }
        Slot::Res(k) => {
            if let Some(r) = res {
                match k.as_str() {
                    "data" => r.data.push_str(raw),
                    "data-encoding" => r.encoding.push_str(raw),
                    "mime" => r.mime.push_str(raw),
                    "md5" => r.md5.get_or_insert_with(String::new).push_str(raw),
                    "file-name" => r.filename.get_or_insert_with(String::new).push_str(raw),
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

fn finish_resource(r: RawResource, n: &mut RawNote) {
    let label = r
        .filename
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| {
            if r.mime.trim().is_empty() {
                "<无文件名>".into()
            } else {
                r.mime.clone()
            }
        });
    let encoding = if r.encoding.trim().is_empty() {
        "base64".to_string()
    } else {
        r.encoding.trim().to_lowercase()
    };
    if encoding != "base64" {
        n.undecodable.push(format!(
            "{label}（data-encoding={encoding}，本版本只解 base64）"
        ));
        return;
    }
    let compact: String = r.data.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = match base64::engine::general_purpose::STANDARD.decode(compact.as_bytes()) {
        Ok(b) => b,
        Err(e) => {
            n.undecodable.push(format!("{label}（base64 解不开：{e}）"));
            return;
        }
    };
    if bytes.is_empty() {
        n.undecodable
            .push(format!("{label}（解出来 0 字节，本库不挂空 blob）"));
        return;
    }
    let mime = if r.mime.trim().is_empty() {
        "application/octet-stream".to_string()
    } else {
        r.mime.trim().to_string()
    };
    n.resources.push(EnexResource {
        sha256: notera_crypto::sha256_hex(&bytes),
        md5: r
            .md5
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        mime,
        filename: r
            .filename
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        data: bytes,
    });
}

// ================================================================ 第二遍 ===

/// 第二遍的产物；块 id 已经排好，附件块由 `document_for` 在拿到资源表之后补上
/// （因为 `<en-media>` 可能出现在 `<resource>` 之前，必须两趟）。
struct EnmlParse {
    blocks: Vec<Block>,
    unknown_tags: Vec<String>,
    dangling: Vec<String>,
    media_refs: Vec<String>,
    next_id: u32,
    prefix: String,
}

impl EnmlParse {
    /// 把资源落成块：正文引用过的按引用顺序落，没被引用的（Evernote 允许"只挂不嵌"）
    /// 追加在文末 —— 一个都不许丢。
    fn document_for(mut self, resources: &[EnexResource]) -> (Document, Vec<String>) {
        let mut used = vec![false; resources.len()];
        let mut resolved: Vec<usize> = Vec::new();
        for hash in &self.media_refs {
            let needle = hash.trim().to_uppercase();
            let hit = resources
                .iter()
                .position(|r| r.md5.as_deref().is_some_and(|m| m.to_uppercase() == needle));
            match hit {
                Some(i) => {
                    used[i] = true;
                    resolved.push(i);
                }
                None => push_unique(&mut self.dangling, hash.clone()),
            }
        }
        let mut ids: Vec<usize> = (0..resources.len()).filter(|i| !used[*i]).collect();
        resolved.append(&mut ids);
        let blocks = &mut self.blocks;
        let prefix = self.prefix.clone();
        let seq = &mut self.next_id;
        for i in resolved {
            let r = &resources[i];
            let type_ = if r.is_inline_image() {
                BlockType::Image
            } else {
                BlockType::Attachment
            };
            *seq += 1;
            blocks.push(Block {
                id: format!("{prefix}{seq:08x}"),
                type_,
                attrs: r.block_attrs(),
                content: Vec::new(),
            });
        }
        let dangling = std::mem::take(&mut self.dangling);
        (
            Document {
                v: 1,
                content: std::mem::take(&mut self.blocks),
            },
            dangling,
        )
    }

    fn next_block_id(&mut self) -> String {
        self.next_id += 1;
        format!("{}{:08x}", self.prefix, self.next_id)
    }
}

fn parse_enml(text: &str, prefix: &str) -> Result<EnmlParse, ImportError> {
    let mut out = EnmlParse {
        blocks: Vec::new(),
        unknown_tags: Vec::new(),
        dangling: Vec::new(),
        media_refs: Vec::new(),
        next_id: 0,
        prefix: prefix.to_string(),
    };
    if text.trim().is_empty() {
        return Ok(out);
    }
    let mut reader = make_reader(text);
    let mut runs: Vec<Inline> = Vec::new();
    let mut marks: Vec<Mark> = Vec::new();
    let mut type_ = BlockType::Paragraph;
    let mut attrs: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    let mut list_stack: Vec<bool> = Vec::new(); // true = 有序
    let mut cell_index: Option<usize> = None; // 在一行里的第几个单元格

    loop {
        match reader.read_event() {
            Err(e) => {
                return Err(ImportError::InvalidDoc(format!(
                    "note 正文不是合法的 ENML：{e}"
                )))
            }
            Ok(Event::Eof) => break,
            Ok(Event::Start(e)) => {
                let name = local(&e.name());
                let a = attrs_of(&e);
                match name.as_str() {
                    "en-note" => {}
                    "ul" | "ol" => list_stack.push(name == "ol"),
                    "li" => {
                        flush(&mut out, &mut runs, &mut marks, &mut type_, &mut attrs);
                        type_ = if list_stack.last().copied().unwrap_or(false) {
                            BlockType::OrderedList
                        } else {
                            BlockType::BulletList
                        };
                        attrs = style_attrs(&a);
                        cell_index = None;
                    }
                    "tr" => {
                        flush(&mut out, &mut runs, &mut marks, &mut type_, &mut attrs);
                        type_ = BlockType::Paragraph;
                        attrs = style_attrs(&a);
                        cell_index = Some(0);
                        push_unique(&mut out.unknown_tags, "table".to_string());
                    }
                    "td" | "th" => {
                        if cell_index.is_some() {
                            if cell_index == Some(0) {
                                cell_index = Some(1);
                            } else {
                                push(&mut runs, " | ", &marks);
                                if let Some(c) = cell_index.as_mut() {
                                    *c += 1;
                                }
                            }
                        }
                        push_unique(&mut out.unknown_tags, name.clone());
                    }
                    "table" | "tbody" | "thead" | "tfoot" => {
                        push_unique(&mut out.unknown_tags, name.clone());
                    }
                    "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                        flush(&mut out, &mut runs, &mut marks, &mut type_, &mut attrs);
                        let level: u8 = name[1..].parse().unwrap_or(1);
                        type_ = BlockType::Heading;
                        attrs = style_attrs(&a);
                        attrs.insert("level".into(), serde_json::Value::Number(level.into()));
                    }
                    "pre" => {
                        flush(&mut out, &mut runs, &mut marks, &mut type_, &mut attrs);
                        type_ = BlockType::CodeBlock;
                        attrs = style_attrs(&a);
                    }
                    "blockquote" => {
                        flush(&mut out, &mut runs, &mut marks, &mut type_, &mut attrs);
                        type_ = BlockType::BlockQuote;
                        attrs = style_attrs(&a);
                    }
                    "div" | "p" => {
                        flush(&mut out, &mut runs, &mut marks, &mut type_, &mut attrs);
                        type_ = BlockType::Paragraph;
                        attrs = style_attrs(&a);
                    }
                    "br" => push(&mut runs, "\n", &marks),
                    "en-media" | "en-resource" => {
                        let h = a
                            .get("hash")
                            .cloned()
                            .or_else(|| a.get("data-md5").cloned())
                            .unwrap_or_default();
                        out.media_refs.push(h);
                    }
                    "hr" => {
                        flush(&mut out, &mut runs, &mut marks, &mut type_, &mut attrs);
                        let id = out.next_block_id();
                        out.blocks.push(Block {
                            id,
                            type_: BlockType::HorizontalRule,
                            attrs: BTreeMap::new(),
                            content: Vec::new(),
                        });
                    }
                    other => {
                        if let Some(m) = mark_for(other, &a) {
                            marks.push(m);
                        } else {
                            push_unique(&mut out.unknown_tags, other.to_string());
                        }
                    }
                }
            }
            Ok(Event::Empty(e)) => {
                let name = local(&e.name());
                let a = attrs_of(&e);
                match name.as_str() {
                    "br" => push(&mut runs, "\n", &marks),
                    "hr" => {
                        flush(&mut out, &mut runs, &mut marks, &mut type_, &mut attrs);
                        let id = out.next_block_id();
                        out.blocks.push(Block {
                            id,
                            type_: BlockType::HorizontalRule,
                            attrs: BTreeMap::new(),
                            content: Vec::new(),
                        });
                    }
                    "en-media" | "en-resource" => {
                        let h = a
                            .get("hash")
                            .cloned()
                            .or_else(|| a.get("data-md5").cloned())
                            .unwrap_or_default();
                        out.media_refs.push(h);
                    }
                    other => {
                        push_unique(&mut out.unknown_tags, other.to_string());
                    }
                }
            }
            Ok(Event::Text(t)) => push(&mut runs, &text_of(&t), &marks),
            Ok(Event::GeneralRef(r)) => push(&mut runs, &ref_text(&r), &marks),
            Ok(Event::CData(t)) => push(&mut runs, &t.into_inner(), &marks),
            Ok(Event::End(e)) => {
                let name = local(&e.name());
                match name.as_str() {
                    "ul" | "ol" => {
                        list_stack.pop();
                    }
                    "li" | "div" | "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "pre"
                    | "blockquote" | "tr" => {
                        flush(&mut out, &mut runs, &mut marks, &mut type_, &mut attrs);
                        type_ = BlockType::Paragraph;
                        attrs = BTreeMap::new();
                        if name == "tr" {
                            cell_index = None;
                        }
                    }
                    "a" | "b" | "i" | "u" | "s" | "em" | "strong" | "ins" | "del" | "code"
                    | "span" | "font" | "en-hyperlink" => {
                        if mark_for(&name, &BTreeMap::new()).is_some()
                            || name == "span"
                            || name == "font"
                        {
                            marks.pop();
                        }
                    }
                    "en-note" => {}
                    _ => {}
                }
            }
            Ok(_) => {}
        }
    }
    flush(&mut out, &mut runs, &mut marks, &mut type_, &mut attrs);
    Ok(out)
}

fn push(runs: &mut Vec<Inline>, raw: &str, marks: &[Mark]) {
    if raw.is_empty() {
        return;
    }
    runs.push(Inline {
        text: raw.to_string(),
        marks: marks.to_vec(),
    });
}

/// 收尾当前块：纯空白块整体丢掉（ENML 里那全是排版噪声），非空块按**整段**裁头尾空白
/// —— 每个 run 单独 trim 会把 "粗" 与 "体" 之间的空格吃掉，所以只在两端动。
fn flush(
    out: &mut EnmlParse,
    runs: &mut Vec<Inline>,
    marks: &mut Vec<Mark>,
    type_: &mut BlockType,
    attrs: &mut BTreeMap<String, serde_json::Value>,
) {
    if runs.is_empty() && attrs.is_empty() {
        marks.clear();
        return;
    }
    let content = std::mem::take(runs);
    marks.clear();
    let joined: String = content.iter().map(|r| r.text.as_str()).collect();
    if joined.trim().is_empty() {
        // 纯空白块整体丢掉（ENML 里那全是排版噪声），块 attrs 也跟着走
        attrs.clear();
        return;
    }
    let id = out.next_block_id();
    out.blocks.push(Block {
        id,
        type_: type_.clone(),
        attrs: std::mem::take(attrs),
        content: trim_ends(content),
    });
}

fn trim_ends(mut runs: Vec<Inline>) -> Vec<Inline> {
    if let Some(first) = runs.first_mut() {
        first.text = first.text.trim_start().to_string();
    }
    if let Some(last) = runs.last_mut() {
        last.text = last.text.trim_end().to_string();
    }
    runs.retain(|r| !r.text.is_empty());
    runs
}

fn mark_for(name: &str, attrs: &BTreeMap<String, String>) -> Option<Mark> {
    let mut kind = match name {
        "b" | "strong" => Some(MarkKind::Bold),
        "i" | "em" => Some(MarkKind::Italic),
        "u" | "ins" => Some(MarkKind::Underline),
        "s" | "strike" | "del" => Some(MarkKind::Strike),
        "code" | "kbd" | "samp" | "tt" => Some(MarkKind::Code),
        "a" | "en-hyperlink" => Some(MarkKind::Link),
        "span" | "font" => None,
        _ => return None,
    };
    // Evernote 大量用 <span style="font-weight:bold"> 而不是 <b>：不看懂 style，
    // 导入回来的笔记样式全是死的。
    if kind.is_none() {
        let style = attrs
            .get("style")
            .map(|s| s.to_lowercase().replace(' ', ""))
            .unwrap_or_default();
        kind = if style.contains("font-weight:bold") || style.contains("font-weight:700") {
            Some(MarkKind::Bold)
        } else if style.contains("font-style:italic") {
            Some(MarkKind::Italic)
        } else if style.contains("text-decoration:line-through") {
            Some(MarkKind::Strike)
        } else if style.contains("text-decoration:underline") {
            Some(MarkKind::Underline)
        } else if style.contains("background-color:") {
            Some(MarkKind::Highlight)
        } else {
            let weight = attrs.get("weight").map(|w| w.as_str());
            let face = attrs.get("face").map(|f| f.to_lowercase());
            if weight == Some("bold") {
                Some(MarkKind::Bold)
            } else if face.as_deref() == Some("italic") {
                Some(MarkKind::Italic)
            } else {
                None
            }
        };
    }
    let mut mark = Mark {
        kind: kind?,
        attrs: BTreeMap::new(),
    };
    if mark.kind == MarkKind::Link {
        if let Some(href) = attrs.get("href") {
            mark.attrs
                .insert("href".into(), serde_json::Value::String(href.clone()));
        }
    }
    Some(mark)
}

/// 块级 attrs：把不理解的样式按 `unknown:style:*` 前向兼容容器留着（I7：未知键必须写回）。
fn style_attrs(attrs: &BTreeMap<String, String>) -> BTreeMap<String, serde_json::Value> {
    let mut out = BTreeMap::new();
    let style = match attrs.get("style") {
        Some(s) => s.clone(),
        None => return out,
    };
    let lower = style.to_lowercase();
    for key in ["text-align", "font-family", "font-size", "color"] {
        if let Some(at) = lower.find(key) {
            let val = style[at..]
                .split(';')
                .next()
                .unwrap_or_default()
                .split_once(':')
                .map(|(_, v)| v.trim().to_string())
                .unwrap_or_default();
            if !val.is_empty() {
                out.insert(
                    format!("unknown:style:{key}"),
                    serde_json::Value::String(val),
                );
            }
        }
    }
    out
}

fn push_unique(list: &mut Vec<String>, item: impl Into<String>) {
    let item = item.into();
    if !list.contains(&item) {
        list.push(item);
    }
}

// ============================================================ 单元测试 ===

#[cfg(test)]
mod tests {
    use super::*;

    fn plain_text(doc: &Document) -> String {
        doc.content
            .iter()
            .map(|b| {
                b.content
                    .iter()
                    .map(|i| i.text.as_str())
                    .collect::<Vec<_>>()
                    .join("")
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn png_b64() -> String {
        base64::engine::general_purpose::STANDARD.encode([0x89u8, b'P', b'N', b'G', 1, 2, 3, 4])
    }

    const MD5_SHOT: &str = "0D4A1B2C3D4E5F60718293A4B5C6D7E8";
    const MD5_SCAN: &str = "9D4A1B2C3D4E5F60718293A4B5C6D7E8";

    /// 真 .enex 的形状：正文是**一整段 CDATA 包起来的 ENML**。
    fn fixture() -> String {
        let enml = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE en-note SYSTEM \"http://xml.evernote.com/pub/enml2.dtd\">\n<en-note><div>甲</div><h2>小节</h2><div><b>粗</b>与<span style=\"font-style: italic\">斜</span></div><en-media hash=\"0D4A1B2C3D4E5F60718293A4B5C6D7E8\" type=\"image/png\" /><table><tbody><tr><td>左</td><td>右</td></tr></tbody></table><div><a href=\"https://example.com/x\">链接</a></div><div>末尾空白　</div></en-note>";
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE en-export SYSTEM \"http://xml.evernote.com/pub/evernote-export2.dtd\">\n\
<en-export export-date=\"20240101T000000Z\" version=\"6.5.1\">\n\
<note><title>报销单</title><content><![CDATA[{enml}]]></content>\
<created>20230102T030405Z</created><updated>20230203T040506Z</updated>\
<tag>财务</tag><note-attribute name=\"sticky\" value=\"true\"/><author>lhcz</author>\
<resource><data encoding=\"base64\">{png}</data><mime>image/png</mime>\
<resource-attributes><file-name>shot.png</file-name></resource-attributes>\
<md5>{md5a}</md5></resource>\n\
<resource><data encoding=\"base64\">{png}</data><mime>application/pdf</mime>\
<resource-attributes><file-name>scan.pdf</file-name></resource-attributes>\
<md5>{md5b}</md5></resource>\n</note>\n\
<note><title>第二条</title><content><![CDATA[<en-note><ul><li>无序一</li><li>无序二</li></ul><ol><li>有序一</li></ol></en-note>]]></content></note>\n\
</en-export>",
            png = png_b64(),
            md5a = MD5_SHOT,
            md5b = MD5_SCAN,
        )
    }

    #[test]
    fn one_enex_file_becomes_one_note_per_note_element() {
        let f = parse_enex(&fixture(), "ex").unwrap();
        assert_eq!(f.notes.len(), 2, "两条 <note> 就该出两条笔记");
        assert_eq!(f.notes[0].title, "报销单");
        assert_eq!(f.notes[1].title, "第二条");
    }

    #[test]
    fn cdata_payload_is_parsed_as_xml_not_as_literal_text() {
        // 一遍读会得到的最典型症状：整段 ENML 变成一个段落，标签名当文字出现在正文里
        let f = parse_enex(&fixture(), "ex").unwrap();
        let text = plain_text(&f.notes[0].doc);
        assert!(!text.contains("<div>"), "CDATA 没被第二遍解析：{text}");
        assert!(!text.contains("en-media"), "附件标签当成文字了：{text}");
        assert!(text.contains('甲'));
    }

    /// 实体引用在两遍解析里都必须被解成字符 —— 这条是给 XML 解析器换版本时用的**行为锚**。
    ///
    /// 为什么单独钉：`parse_enex` 是两遍读（第一遍切信封，第二遍把 CDATA 里的 ENML 当 XML 再解一次），
    /// 而"属性/文本里的 `&amp;`、`&#65;` 要不要解"正是 XML 解析器最容易出现版本差异的地方。
    /// 解析库升级时这一条如果变了，必须能看出来 —— 它不是"顺手测一下"，是升级的对照组。
    #[test]
    fn entity_references_are_decoded_in_both_parse_passes() {
        let enml =
            "<?xml version=\"1.0\"?><en-note><div>粗 &amp; 斜 &#65; &lt;尖&gt;</div></en-note>";
        let text = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <en-export export-date=\"20240101T000000Z\" version=\"6.5.1\">\n\
             <note><title>A &amp; B &#65; &lt;C&gt;</title><content><![CDATA[{enml}]]></content></note>\n\
             </en-export>"
        );
        let f = parse_enex(&text, "ent").expect("解析");
        assert_eq!(
            f.notes[0].title, "A & B A <C>",
            "标题里的实体引用没被解成字符（或解过了头）"
        );
        let body = plain_text(&f.notes[0].doc);
        assert!(
            body.contains("粗 & 斜 A <尖>"),
            "CDATA 里第二遍解析的实体没解：{body}"
        );
    }

    /// 没有分号的裸 `&` 不许把整份导入打死，也不许把那个节点变成空。
    ///
    /// 钉的是 0.42 里 `allow_dangling_amp` 这一格：默认（false）会让解析器在这一个字符上
    /// 报整篇 IllFormed —— 后果是"500 条笔记一条都导不进来"，而旧版本只是把这个节点
    /// 变成空串（同样是错，但方向不同）。这里两个都要挡住：既不能整篇失败，也不能丢字。
    #[test]
    fn a_lone_ampersand_survives_without_failing_the_import() {
        let text = concat!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
            "<en-export export-date=\"20240101T000000Z\" version=\"6.5.1\">\n",
            "<note><title>Tom & Jerry</title>\n",
            "<content><![CDATA[<?xml version=\"1.0\"?><en-note><div>a & b</div></en-note>]]></content>\n",
            "</note>\n",
            "<note><title>第二条</title>",
            "<content><![CDATA[<?xml version=\"1.0\"?><en-note><div>还在</div></en-note>]]></content></note>\n",
            "</en-export>",
        );
        let f = parse_enex(&text, "amp").expect("一个裸 & 不该让整份 .enex 导入失败");
        assert_eq!(f.notes.len(), 2, "后面的笔记不许被前面那个字符带走");
        assert_eq!(
            f.notes[0].title, "Tom & Jerry",
            "裸 & 要原样留着，不是变成空"
        );
        assert_eq!(plain_text(&f.notes[0].doc), "a & b");
    }

    #[test]
    fn headings_carry_level_and_lists_keep_their_kind() {
        let f = parse_enex(&fixture(), "ex").unwrap();
        let h = f.notes[0]
            .doc
            .content
            .iter()
            .find(|b| b.type_ == BlockType::Heading)
            .expect("h2 该变成 heading 块");
        assert_eq!(h.attrs.get("level").and_then(|v| v.as_i64()), Some(2));
        assert_eq!(
            h.content
                .iter()
                .map(|i| i.text.as_str())
                .collect::<String>(),
            "小节"
        );

        let n2 = &f.notes[1];
        let bullets: Vec<&Block> = n2
            .doc
            .content
            .iter()
            .filter(|b| b.type_ == BlockType::BulletList)
            .collect();
        let ordered: Vec<&Block> = n2
            .doc
            .content
            .iter()
            .filter(|b| b.type_ == BlockType::OrderedList)
            .collect();
        assert_eq!(
            bullets.len(),
            2,
            "<ul><li> 该是 bulletList：{:?}",
            plain_text(&n2.doc)
        );
        assert_eq!(ordered.len(), 1, "<ol><li> 该是 orderedList");
    }

    #[test]
    fn bold_tag_and_css_style_both_become_marks() {
        let f = parse_enex(&fixture(), "ex").unwrap();
        let run_with = |needle: char| {
            f.notes[0]
                .doc
                .content
                .iter()
                .flat_map(|b| b.content.iter())
                .find(|i| i.text.contains(needle))
                .unwrap_or_else(|| panic!("{needle} 不见了"))
                .clone()
        };
        assert!(
            run_with('粗')
                .marks
                .iter()
                .any(|m| m.kind == MarkKind::Bold),
            "<b> 没落成 bold"
        );
        assert!(
            run_with('斜')
                .marks
                .iter()
                .any(|m| m.kind == MarkKind::Italic),
            "span style=font-style:italic 没落成 italic（Evernote 主要靠这个）"
        );
    }

    #[test]
    fn links_keep_href() {
        let f = parse_enex(&fixture(), "ex").unwrap();
        let link = f.notes[0]
            .doc
            .content
            .iter()
            .flat_map(|b| b.content.iter())
            .find(|i| i.text.contains("链接"))
            .expect("链接文字该在");
        let m = link
            .marks
            .iter()
            .find(|m| m.kind == MarkKind::Link)
            .expect("<a> 该落成 link");
        assert_eq!(
            m.attrs.get("href").and_then(|v| v.as_str()),
            Some("https://example.com/x")
        );
    }

    #[test]
    fn resources_become_blocks_the_storage_layer_can_read() {
        let f = parse_enex(&fixture(), "ex").unwrap();
        let n = &f.notes[0];
        assert_eq!(n.resources.len(), 2, "两个 <resource> 都该解出来");
        let png = n.resources.iter().find(|r| r.mime == "image/png").unwrap();
        assert_eq!(
            png.sha256,
            notera_crypto::sha256_hex(&png.data),
            "键必须是自己算的 sha256，不是 Evernote 的 md5"
        );
        assert_eq!(png.filename.as_deref(), Some("shot.png"));
        let media: Vec<&Block> = n
            .doc
            .content
            .iter()
            .filter(|b| matches!(b.type_, BlockType::Image | BlockType::Attachment))
            .collect();
        assert_eq!(media.len(), 2, "内嵌的图片 + 只挂不嵌的 pdf 都要有块");
        assert!(media.iter().any(|b| b.type_ == BlockType::Image));
        assert!(media.iter().any(|b| b.type_ == BlockType::Attachment));

        // 这条钉的是"块 → 存储层认得的附件"那条边：字段名一漂，用户的附件就停在占位
        let got = notera_richtext::attachments(&n.doc);
        assert_eq!(got.len(), 2, "{got:?}");
        assert_eq!(got[0].sha256, n.resources[0].sha256);
        assert_eq!(got[0].filename.as_deref(), Some("shot.png"));
        assert_eq!(got[0].size, Some(n.resources[0].data.len() as i64));
        assert_eq!(got[0].role.as_deref(), Some("inline"));
        assert_eq!(got[1].role.as_deref(), Some("file"), "pdf 不该被当内嵌图片");
    }

    #[test]
    fn table_cells_keep_their_text_and_the_structure_is_named() {
        let f = parse_enex(&fixture(), "ex").unwrap();
        let n = &f.notes[0];
        let text = plain_text(&n.doc);
        assert!(text.contains("左 | 右"), "表格一行该是「左 | 右」：{text}");
        assert!(
            n.unknown_enml_tags.iter().any(|s| s == "table"),
            "表格没映射成结构必须报出来：{:?}",
            n.unknown_enml_tags
        );
    }

    #[test]
    fn what_the_schema_cannot_hold_is_named_not_dropped() {
        let f = parse_enex(&fixture(), "ex").unwrap();
        let n = &f.notes[0];
        for want in ["tag", "created", "updated", "author"] {
            assert!(
                n.unknown_fields.iter().any(|s| s == want),
                "{want} 没被记名（用户就该看到它没落地）：{:?}",
                n.unknown_fields
            );
        }
        assert!(n.undecodable_resources.is_empty());
    }

    #[test]
    fn a_media_reference_with_no_resource_is_reported() {
        // 只动正文里那一处 hash（第一个出现位），`<md5>` 保持原值 —— 全替会把两边一起改掉，
        // 于是引用又能配对上，测的就不是"悬空引用"了。
        let text = fixture().replacen(MD5_SHOT, "DEADBEEFDEADBEEF", 1);
        let f = parse_enex(&text, "ex").unwrap();
        assert!(
            !f.notes[0].dangling_media_hashes.is_empty(),
            "正文引用了不存在的资源必须报出来"
        );
        // 但两个资源的字节都还在
        assert_eq!(f.notes[0].resources.len(), 2);
    }

    #[test]
    fn non_base64_resource_is_named_and_not_guessed_at() {
        // 只把第一个资源改成别的编码：第二个必须照常解出来，
        // 否则"一条坏编码拖垮整批"这种错误实现照样能过。
        let text = fixture().replacen("encoding=\"base64\"", "encoding=\"quoted-printable\"", 1);
        let f = parse_enex(&text, "ex").unwrap();
        let n = &f.notes[0];
        assert_eq!(
            n.undecodable_resources.len(),
            1,
            "{:?}",
            n.undecodable_resources
        );
        assert!(n.undecodable_resources[0].contains("quoted-printable"));
        assert!(
            n.undecodable_resources[0].contains("shot.png"),
            "要能点名是哪个资源：{}",
            n.undecodable_resources[0]
        );
        assert_eq!(n.resources.len(), 1, "另一个资源照常进来");
        assert_eq!(n.resources[0].mime, "application/pdf");
    }

    #[test]
    fn block_ids_are_stable_and_unique() {
        let a = parse_enex(&fixture(), "ex").unwrap();
        let b = parse_enex(&fixture(), "ex").unwrap();
        let ids = |f: &EnexFile| {
            f.notes
                .iter()
                .flat_map(|n| n.doc.content.iter().map(|x| x.id.clone()))
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(&a), ids(&b), "同一份字节必须得到同一批块 id");
        let mut v = ids(&a);
        let total = v.len();
        v.sort();
        v.dedup();
        assert_eq!(
            v.len(),
            total,
            "块 id 重复会让 validate 直接拒（也毁掉三方合并的锚点）"
        );
    }

    #[test]
    fn empty_enex_yields_no_notes_but_does_not_error() {
        let f = parse_enex(
            r#"<?xml version="1.0"?><en-export version="1"></en-export>"#,
            "ex",
        )
        .unwrap();
        assert!(f.notes.is_empty());
    }

    #[test]
    fn malformed_xml_is_a_failure_not_a_silent_empty_import() {
        // 静默返回"0 条笔记"是最坏的一种：用户以为导成功了
        let e = parse_enex("<en-export><note><title>甲</title>", "ex");
        assert!(
            e.is_err(),
            "标签没闭合必须报错：{:?}",
            e.map(|f| f.notes.len())
        );
    }
}
