//! 富文本数据类型与它们的 wire 形态（DATA-MODEL.md §10）。
//!
//! ## preserve-unknown（I7）是本模块的存在理由
//! * 未知 `type` → [`BlockType::Unknown`]（**原名**一字不动地留在里面），wire 上写成
//!   `unknown:<原名>`；未知 mark 同理。
//! * 未知 `attrs` 键 → 原样留在 `BTreeMap` 里，`canonical()`/`to_json()` 必须写回去。
//! * wire 对象上出现的**未知顶层键**（未来客户端可能往块上直接挂字段）→ 折进 `attrs`，
//!   绝不丢弃。折叠是幂等的：第二次读进来时它已经在 `attrs` 里，位置不再变。
//!
//! 字段集合是跨 crate 契约（store/sync/importer 用结构体字面量构造），**禁止**增删公共字段。

use serde::de::Deserializer;
use serde::{Deserialize, Serialize, Serializer};
use std::collections::BTreeMap;

/// 本客户端支持的权威文档格式版本（契约冻结点：ARCHITECTURE-MAP §6「富文本 schema」）。
pub const DOC_FORMAT: u16 = 1;

/// 该文档版本本客户端能否理解（可写）。
pub fn supports(v: u16) -> bool {
    v <= DOC_FORMAT
}

/// 富文本层错误。词汇表刻意很窄：调用方只需要知道"这批数据能不能进权威表"（I6）。
#[derive(thiserror::Error, Clone, Debug, PartialEq, Eq)]
pub enum RichError {
    /// JSON 结构不合法 / 缺必备字段 / 类型不对。
    #[error("文档结构不合法: {0}")]
    Malformed(String),
    /// 块序列的嵌套关系不合法（表格族必须成组出现）。
    #[error("嵌套不合法: {0}")]
    InvalidNesting(String),
    /// 顶层块 id 重复：三方合并的锚点失效，必须拒绝而不是"猜哪个是真的"。
    #[error("块 id 重复: {0}")]
    DuplicateBlockId(String),
    /// 文档版本比本客户端支持的值新 → 只读（I7 / FWD-03）。
    #[error("文档版本 {0} 高于本客户端支持的 {DOC_FORMAT}，只能只读打开")]
    UnsupportedVersion(u16),
}

/// 权威文档：`{ "v":1, "content":[Block] }`。
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Document {
    pub v: u16,
    pub content: Vec<Block>,
}

/// 顶层块。每个块有稳定 `id`（I1），块级三方合并靠它对齐（CONFLICT-RESOLUTION §3.1）。
#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub id: String,
    pub type_: BlockType,
    pub attrs: BTreeMap<String, serde_json::Value>,
    pub content: Vec<Inline>,
}

/// 行内文本段。`marks` 顺序有意义（叠加顺序），不得排序。
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Inline {
    pub text: String,
    pub marks: Vec<Mark>,
}

/// 行内样式。`attrs` 里的未知键必须原样写回。
#[derive(Clone, Debug, PartialEq)]
pub struct Mark {
    pub kind: MarkKind,
    pub attrs: BTreeMap<String, serde_json::Value>,
}

/// 块类型。`Unknown` 装的是**原始类型名**（不是 `unknown:` 前缀之后的残片）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockType {
    Paragraph,
    Heading,
    BlockQuote,
    CodeBlock,
    OrderedList,
    BulletList,
    ChecklistItem,
    Image,
    Attachment,
    HorizontalRule,
    Table,
    TableRow,
    TableCell,
    Unknown(String),
}

/// mark 类型。`Unknown` 同上，存原始名。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MarkKind {
    Bold,
    Italic,
    Underline,
    Strike,
    Code,
    Highlight,
    Link,
    FontSize,
    Color,
    AttachmentRef,
    Unknown(String),
}

/// 前向兼容容器在 wire 上的前缀（DATA-MODEL §10.2 `unknown:*`）。
const UNKNOWN_PREFIX: &str = "unknown:";

impl BlockType {
    /// wire 上的类型名。`Unknown(n)` → `"unknown:<n>"`，因此原名可逐字符找回。
    pub fn wire_name(&self) -> String {
        match self {
            BlockType::Paragraph => "paragraph".into(),
            BlockType::Heading => "heading".into(),
            BlockType::BlockQuote => "blockquote".into(),
            BlockType::CodeBlock => "codeBlock".into(),
            BlockType::OrderedList => "orderedList".into(),
            BlockType::BulletList => "bulletList".into(),
            BlockType::ChecklistItem => "checklistItem".into(),
            BlockType::Image => "image".into(),
            BlockType::Attachment => "attachment".into(),
            BlockType::HorizontalRule => "horizontalRule".into(),
            BlockType::Table => "table".into(),
            BlockType::TableRow => "tableRow".into(),
            BlockType::TableCell => "tableCell".into(),
            BlockType::Unknown(n) => format!("{UNKNOWN_PREFIX}{n}"),
        }
    }

    /// wire 名 → 类型。未知一律落 `Unknown(原名)`，绝不报错、绝不丢弃。
    pub fn from_wire_name(name: &str) -> Self {
        match name {
            "paragraph" => BlockType::Paragraph,
            "heading" => BlockType::Heading,
            "blockquote" => BlockType::BlockQuote,
            "codeBlock" => BlockType::CodeBlock,
            "orderedList" => BlockType::OrderedList,
            "bulletList" => BlockType::BulletList,
            "checklistItem" => BlockType::ChecklistItem,
            "image" => BlockType::Image,
            "attachment" => BlockType::Attachment,
            "horizontalRule" => BlockType::HorizontalRule,
            "table" => BlockType::Table,
            "tableRow" => BlockType::TableRow,
            "tableCell" => BlockType::TableCell,
            other => BlockType::Unknown(
                other
                    .strip_prefix(UNKNOWN_PREFIX)
                    .unwrap_or(other)
                    .to_string(),
            ),
        }
    }

    /// 表格族：用于 `validate` 的结构约束（扁平模型里靠相邻关系表达层级）。
    pub fn is_table_family(&self) -> bool {
        matches!(self, Self::Table | Self::TableRow | Self::TableCell)
    }
}

impl MarkKind {
    pub fn wire_name(&self) -> String {
        let known = match self {
            MarkKind::Bold => Some("bold"),
            MarkKind::Italic => Some("italic"),
            MarkKind::Underline => Some("underline"),
            MarkKind::Strike => Some("strike"),
            MarkKind::Code => Some("code"),
            MarkKind::Highlight => Some("highlight"),
            MarkKind::Link => Some("link"),
            MarkKind::FontSize => Some("fontSize"),
            MarkKind::Color => Some("color"),
            MarkKind::AttachmentRef => Some("attachmentRef"),
            MarkKind::Unknown(n) => return format!("{UNKNOWN_PREFIX}{n}"),
        };
        known.unwrap_or("unknown:").to_string()
    }

    pub fn from_wire_name(name: &str) -> Self {
        match name {
            "bold" => MarkKind::Bold,
            "italic" => MarkKind::Italic,
            "underline" => MarkKind::Underline,
            "strike" => MarkKind::Strike,
            "code" => MarkKind::Code,
            "highlight" => MarkKind::Highlight,
            "link" => MarkKind::Link,
            "fontSize" => MarkKind::FontSize,
            "color" => MarkKind::Color,
            "attachmentRef" => MarkKind::AttachmentRef,
            other => MarkKind::Unknown(
                other
                    .strip_prefix(UNKNOWN_PREFIX)
                    .unwrap_or(other)
                    .to_string(),
            ),
        }
    }
}

// ------------------------------------------------------------------ serde ---
// 序列化：derive 出来的字段名就是 wire 名；两个枚举手写以拿到 `unknown:*` 形态。
// 反序列化：一律经 `Value` 走 `from_value`，因为我们要"宽容到不丢字节"，
// 而 derive 的 `deny_unknown_fields`/缺字段报错都做不到那个粒度。

impl Serialize for BlockType {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.wire_name())
    }
}

impl<'de> Deserialize<'de> for BlockType {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = serde_json::Value::deserialize(d)?;
        match v.as_str() {
            Some(s) => Ok(Self::from_wire_name(s)),
            None => Err(serde::de::Error::custom("块的 type 必须是字符串")),
        }
    }
}

impl Serialize for MarkKind {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.wire_name())
    }
}

impl<'de> Deserialize<'de> for MarkKind {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = serde_json::Value::deserialize(d)?;
        match v.as_str() {
            Some(s) => Ok(Self::from_wire_name(s)),
            None => Err(serde::de::Error::custom("mark 的 kind 必须是字符串")),
        }
    }
}

impl Serialize for Document {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut st = s.serialize_struct("Document", 2)?;
        st.serialize_field("v", &self.v)?;
        st.serialize_field("content", &self.content)?;
        st.end()
    }
}

impl<'de> Deserialize<'de> for Document {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = serde_json::Value::deserialize(d)?;
        Self::from_value(&v).map_err(serde::de::Error::custom)
    }
}

impl Serialize for Block {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut st = s.serialize_struct("Block", 4)?;
        st.serialize_field("id", &self.id)?;
        st.serialize_field("type", &self.type_)?;
        st.serialize_field("attrs", &self.attrs)?;
        st.serialize_field("content", &self.content)?;
        st.end()
    }
}

impl<'de> Deserialize<'de> for Block {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = serde_json::Value::deserialize(d)?;
        Self::from_value(&v).map_err(serde::de::Error::custom)
    }
}

impl Serialize for Inline {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut st = s.serialize_struct("Inline", 2)?;
        st.serialize_field("text", &self.text)?;
        st.serialize_field("marks", &self.marks)?;
        st.end()
    }
}

impl<'de> Deserialize<'de> for Inline {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = serde_json::Value::deserialize(d)?;
        Self::from_value(&v).map_err(serde::de::Error::custom)
    }
}

impl Serialize for Mark {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut st = s.serialize_struct("Mark", 2)?;
        st.serialize_field("kind", &self.kind)?;
        st.serialize_field("attrs", &self.attrs)?;
        st.end()
    }
}

impl<'de> Deserialize<'de> for Mark {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = serde_json::Value::deserialize(d)?;
        Self::from_value(&v).map_err(serde::de::Error::custom)
    }
}

// --------------------------------------------------------- 宽容解析（I7） ---

impl Document {
    /// 从 JSON 值构造。**不校验版本**（`v` 超前也要能读出来，供只展示用）。
    pub fn from_value(v: &serde_json::Value) -> Result<Self, RichError> {
        let map = v
            .as_object()
            .ok_or_else(|| RichError::Malformed("文档根必须是对象".into()))?;
        let dv = match map.get("v") {
            None | Some(serde_json::Value::Null) => DOC_FORMAT,
            Some(serde_json::Value::Number(n)) => {
                // 整数不写成浮点（§10.3 第 3 步）。2.0 这类"整数值的浮点"也接受。
                if let Some(i) = n.as_u64() {
                    u16::try_from(i)
                        .map_err(|_| RichError::Malformed(format!("v 超出 u16: {i}")))?
                } else if let Some(f) = n.as_f64() {
                    if f.fract() == 0.0 && f >= 0.0 && f <= u16::MAX as f64 {
                        f as u16
                    } else {
                        return Err(RichError::Malformed(format!("v 必须是整数: {f}")));
                    }
                } else {
                    return Err(RichError::Malformed("v 必须是整数".into()));
                }
            }
            Some(other) => return Err(RichError::Malformed(format!("v 不是数字: {other}"))),
        };
        let content = match map.get("content") {
            // `content` 是 schema 的必备部分（§10.2）。缺了它**不能**当成"空文档"：
            // 那等于把调用方给的一整份负载静默替换成空白笔记。
            None => return Err(RichError::Malformed("文档缺少 content 数组".into())),
            Some(serde_json::Value::Null) => {
                return Err(RichError::Malformed("文档 content 不能是 null".into()))
            }
            Some(serde_json::Value::Array(a)) => {
                let mut out = Vec::with_capacity(a.len());
                for (i, x) in a.iter().enumerate() {
                    out.push(Block::from_value(x).map_err(|e| context_block(e, i))?)
                }
                out
            }
            Some(_) => return Err(RichError::Malformed("content 必须是数组".into())),
        };
        Ok(Document { v: dv, content })
    }
}

impl Block {
    /// 从 JSON 值构造。未知顶层键折进 `attrs`（值一字不动），未知 `type` 落 `Unknown`。
    pub fn from_value(v: &serde_json::Value) -> Result<Self, RichError> {
        let map = v
            .as_object()
            .ok_or_else(|| RichError::Malformed("块必须是对象".into()))?;

        let id = match map.get("id") {
            None | Some(serde_json::Value::Null) => String::new(),
            Some(serde_json::Value::String(s)) => s.clone(),
            Some(other) => return Err(RichError::Malformed(format!("块 id 必须是字符串: {other}"))),
        };

        let type_ = match map.get("type") {
            None | Some(serde_json::Value::Null) => BlockType::Unknown(String::new()),
            Some(serde_json::Value::String(s)) => BlockType::from_wire_name(s),
            Some(other) => {
                return Err(RichError::Malformed(format!("块的 type 必须是字符串: {other}")))
            }
        };

        let mut attrs = BTreeMap::new();
        match map.get("attrs") {
            None | Some(serde_json::Value::Null) => {}
            Some(serde_json::Value::Object(m)) => {
                for (k, val) in m {
                    attrs.insert(k.clone(), val.clone());
                }
            }
            Some(other) => {
                return Err(RichError::Malformed(format!("块 attrs 必须是对象: {other}")))
            }
        }

        let mut folded_nodes: Vec<serde_json::Value> = Vec::new();
        let mut content: Vec<Inline> = Vec::new();
        match map.get("content") {
            None | Some(serde_json::Value::Null) => {}
            Some(serde_json::Value::Array(a)) => {
                for (i, x) in a.iter().enumerate() {
                    match x {
                        // 裸字符串也接受（ProseMirror 有时直接写字符串）。
                        serde_json::Value::String(s) => content.push(Inline {
                            text: s.clone(),
                            marks: Vec::new(),
                        }),
                        serde_json::Value::Object(_) => {
                            if x.get("text").is_some() || x.get("marks").is_some() {
                                content.push(Inline::from_value(x).map_err(|e| context_inline(e, i))?)
                            } else {
                                // 嵌套块：本模型是扁平的，折进 attrs 保证一个字节都不丢。
                                folded_nodes.push(x.clone());
                            }
                        }
                        other => {
                            return Err(RichError::Malformed(format!(
                                "块的 content[{i}] 不是行内节点: {other}"
                            )))
                        }
                    }
                }
            }
            Some(other) => {
                return Err(RichError::Malformed(format!("块 content 必须是数组: {other}")))
            }
        }
        if !folded_nodes.is_empty() {
            fold_json(&mut attrs, "nodes", serde_json::Value::Array(folded_nodes));
        }

        // 未知顶层键：折进 attrs，键名冲突时降级为 `top:<key>`（原值仍在，绝不覆盖）。
        const KNOWN: [&str; 4] = ["id", "type", "attrs", "content"];
        for (k, val) in map {
            if KNOWN.contains(&k.as_str()) {
                continue;
            }
            fold_json(&mut attrs, k, val.clone());
        }

        Ok(Block {
            id,
            type_,
            attrs,
            content,
        })
    }

    /// 纯文本（块内所有行内 text 串联，忽略 marks）。合并降级顺序 M1/M2/M3 靠它。
    pub fn plain_text(&self) -> String {
        let mut s = String::new();
        for i in &self.content {
            s.push_str(&i.text);
        }
        s
    }

    /// 空块：合并时"两侧都是新增"要用"无共同祖先"的语义（base 为空 → 任何属性差异
    /// 都是两侧新增，不存在"回退到 base 值"）。
    pub(crate) fn blank() -> Block {
        Block {
            id: String::new(),
            type_: BlockType::Unknown(String::new()),
            attrs: BTreeMap::new(),
            content: Vec::new(),
        }
    }
}

impl Inline {
    pub fn from_value(v: &serde_json::Value) -> Result<Self, RichError> {
        match v {
            serde_json::Value::String(s) => Ok(Inline {
                text: s.clone(),
                marks: Vec::new(),
            }),
            serde_json::Value::Object(map) => {
                let text = match map.get("text") {
                    None | Some(serde_json::Value::Null) => String::new(),
                    Some(serde_json::Value::String(s)) => s.clone(),
                    Some(other) => {
                        return Err(RichError::Malformed(format!("inline text 必须是字符串: {other}")))
                    }
                };
                let mut marks = Vec::new();
                match map.get("marks") {
                    None | Some(serde_json::Value::Null) => {}
                    Some(serde_json::Value::Array(a)) => {
                        for (i, x) in a.iter().enumerate() {
                            marks.push(Mark::from_value(x).map_err(|e| context_mark(e, i))?)
                        }
                    }
                    Some(other) => {
                        return Err(RichError::Malformed(format!("inline marks 必须是数组: {other}")))
                    }
                }
                // ProseMirror 的 text 节点带 `type:"text"`，那是本模型的固有事实，忽略。
                const KNOWN: [&str; 3] = ["text", "marks", "type"];
                let mut folded = Vec::new();
                for (k, val) in map {
                    if KNOWN.contains(&k.as_str()) {
                        continue;
                    }
                    folded.push(serde_json::json!({ "key": k, "value": val }));
                }
                if !folded.is_empty() {
                    // Inline 没有 attrs 容器（契约固定字段）→ 折成未知 mark，
                    // 于是"未知内容不被销毁"（INV-10）成立，且二次读取形态不变。
                    marks.push(Mark {
                        kind: MarkKind::Unknown("inlineFields".into()),
                        attrs: {
                            let mut m = BTreeMap::new();
                            m.insert("fields".into(), serde_json::Value::Array(folded));
                            m
                        },
                    });
                }
                Ok(Inline { text, marks })
            }
            other => Err(RichError::Malformed(format!(
                "inline 必须是对象或字符串: {other}"
            ))),
        }
    }
}

impl Mark {
    pub fn from_value(v: &serde_json::Value) -> Result<Self, RichError> {
        match v {
            // ProseMirror 有时把 mark 写成裸字符串名。
            serde_json::Value::String(s) => Ok(Mark {
                kind: MarkKind::from_wire_name(s),
                attrs: BTreeMap::new(),
            }),
            serde_json::Value::Object(map) => {
                let raw = match map.get("kind").or_else(|| map.get("type")) {
                    None | Some(serde_json::Value::Null) => String::new(),
                    Some(serde_json::Value::String(s)) => s.clone(),
                    Some(other) => {
                        return Err(RichError::Malformed(format!("mark kind 必须是字符串: {other}")))
                    }
                };
                let kind = MarkKind::from_wire_name(&raw);
                let mut attrs = BTreeMap::new();
                match map.get("attrs") {
                    None | Some(serde_json::Value::Null) => {}
                    Some(serde_json::Value::Object(m)) => {
                        for (k, val) in m {
                            attrs.insert(k.clone(), val.clone());
                        }
                    }
                    Some(other) => {
                        return Err(RichError::Malformed(format!("mark attrs 必须是对象: {other}")))
                    }
                }
                const KNOWN: [&str; 3] = ["kind", "attrs", "type"];
                for (k, val) in map {
                    if KNOWN.contains(&k.as_str()) {
                        continue;
                    }
                    fold_json(&mut attrs, k, val.clone());
                }
                Ok(Mark { kind, attrs })
            }
            other => Err(RichError::Malformed(format!(
                "mark 必须是对象或字符串: {other}"
            ))),
        }
    }
}

/// 把任意 JSON 折进 attrs：键位被占就用 `<key>#<n>` 追加，绝不覆盖已有值。
fn fold_json(attrs: &mut BTreeMap<String, serde_json::Value>, key: &str, val: serde_json::Value) {
    if !attrs.contains_key(key) {
        attrs.insert(key.to_string(), val);
        return;
    }
    for n in 1..=u32::MAX {
        let alt = format!("{key}#{n}");
        if let std::collections::btree_map::Entry::Vacant(e) = attrs.entry(alt) {
            e.insert(val);
            return;
        }
    }
}

fn context_block(e: RichError, i: usize) -> RichError {
    match e {
        RichError::Malformed(m) => RichError::Malformed(format!("content[{i}]: {m}")),
        other => other,
    }
}
fn context_inline(e: RichError, i: usize) -> RichError {
    match e {
        RichError::Malformed(m) => RichError::Malformed(format!("inline[{i}]: {m}")),
        other => other,
    }
}
fn context_mark(e: RichError, i: usize) -> RichError {
    match e {
        RichError::Malformed(m) => RichError::Malformed(format!("mark[{i}]: {m}")),
        other => other,
    }
}
