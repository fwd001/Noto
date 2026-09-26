//! 解析 / 规范化 / 校验 / 稳定序列化（DATA-MODEL.md §10.3）。
//!
//! 顺序是规范的一部分：`parse = 读入 → normalize → validate`，任一步失败即拒绝提交，
//! 坏数据绝不进入权威表（I6）。

use crate::model::{
    supports, Block, BlockType, Document, Mark, MarkKind, RichError,
};
use notera_core::canonical_json;
use serde_json::Value;
use std::collections::BTreeMap;

/// 解析 JSON 文本 → 规范化 → 校验。失败返回 [`RichError`]，成功返回可直接入库的文档。
///
/// 注意 `doc.v` 超前会返回 [`RichError::UnsupportedVersion`]：这是"禁止写回"的闸门。
/// 只读展示需要文档本体时用 [`parse_for_read`]。
pub fn parse(raw: &str) -> Result<Document, RichError> {
    let v = serde_json::from_str::<Value>(raw).map_err(|e| RichError::Malformed(e.to_string()))?;
    parse_from_value(&v)
}

/// 同 [`parse`]，但入口是已经解好的 JSON 值（store 侧存的是 `doc` 列的文本，
/// sync 侧从信封 `payload` 拿的是值，两者共用这一条路径）。
pub fn parse_from_value(v: &Value) -> Result<Document, RichError> {
    let mut doc = Document::from_value(v)?;
    normalize(&mut doc);
    validate(&doc)?;
    Ok(doc)
}

/// **只读**解析：normalize 后跳过版本闸门，其余校验照做。
///
/// 存在的理由：`doc.v` 超前时 UI 仍需完整显示内容（FWD-03/FWD-04），
/// 而 [`parse`] 必须拒绝同一份数据（I7 禁止写回）。调用方拿到结果后
/// 必须自行保证不回写，或用 [`merge`] 的 `ReadOnly` 分支作为闸门。
pub fn parse_for_read(v: &Value) -> Result<Document, RichError> {
    let mut doc = Document::from_value(v)?;
    normalize(&mut doc);
    validate_shape(&doc)?;
    Ok(doc)
}

/// 规范化（§10.3 第 1 步）。就地修改，永不失败、永不 panic。
pub fn normalize(doc: &mut Document) {
    for b in doc.content.iter_mut() {
        normalize_block(b);
    }
    dedupe_identical_blocks(&mut doc.content);
}

fn normalize_block(b: &mut Block) {
    for i in b.content.iter_mut() {
        i.text = strip_zero_width(&i.text);
        // 剥离空 marks：无名 mark 与完全重复的 mark。
        let mut kept: Vec<Mark> = Vec::with_capacity(i.marks.len());
        for m in std::mem::take(&mut i.marks) {
            if m.kind.wire_name().is_empty() || degenerate_unknown(&m.kind) {
                continue;
            }
            if !kept.iter().any(|k| k == &m) {
                kept.push(m);
            }
        }
        i.marks = kept;
    }
    // 丢弃无 text 的空 inline（无文字又无样式的行内节点没有任何信息）。
    b.content.retain(|i| !i.text.is_empty() || !i.marks.is_empty());
    if b.id.is_empty() {
        // 手搓/外部导入的块可能没 id。合并必须有锚点，这里按内容派生一个确定性 id，
        // 从而"同一内容 → 同一 id"，跨设备也不会撞出两个不同含义的 id。
        b.id = derived_id(b);
    }
    apply_default_attrs(b);
}

/// 无名或名字为空的未知节点视为"空 marks"，可以安全丢弃。
fn degenerate_unknown(kind: &MarkKind) -> bool {
    matches!(kind, MarkKind::Unknown(n) if n.is_empty())
}

/// 零宽字符：只剥掉确定无语义的三个。
///
/// **不**剥 `U+200D`（ZWJ，emoji 家庭序列依赖它）与 `U+200C`（ZWNJ，波斯语/阿拉伯语
/// 正字法依赖它）——剥了就是摧毁用户输入，违反 §0。
/// 用转义写死：字面量本身是不可见字符，留在源码里既读不出也改不动。
const ZERO_WIDTH: [char; 3] = ['\u{200B}', '\u{FEFF}', '\u{2060}'];

fn strip_zero_width(s: &str) -> String {
    if !s.chars().any(|c| ZERO_WIDTH.contains(&c)) {
        return s.to_string();
    }
    s.chars().filter(|c| !ZERO_WIDTH.contains(c)).collect()
}

/// 完全相同（id 与内容都相同）的重复块：留第一个，这是无损的。
/// **内容不同的同 id 块**必须留给 `validate` 判 `DuplicateBlockId` —— 静默丢一块
/// 等于违反 C1。
fn dedupe_identical_blocks(blocks: &mut Vec<Block>) {
    let mut i = 0;
    while i < blocks.len() {
        let mut j = i + 1;
        while j < blocks.len() {
            if blocks[i].id == blocks[j].id && blocks[i].same_content(&blocks[j]) {
                blocks.remove(j);
            } else {
                j += 1;
            }
        }
        i += 1;
    }
}

/// 补齐必备 attr 默认值（§10.3 第 1 步的最后一项）。只补"缺失"，不覆盖已有值。
fn apply_default_attrs(b: &mut Block) {
    match b.type_ {
        BlockType::Heading => {
            b.attrs.entry("level".into()).or_insert(Value::from(1u8));
        }
        BlockType::OrderedList | BlockType::BulletList | BlockType::ChecklistItem => {
            b.attrs.entry("indent".into()).or_insert(Value::from(0u8));
        }
        _ => {}
    }
}

/// 内容派生的确定性块 id（`nb-` + 12 hex = 15 字符，落在 §10.2 的 8..32 区间内）。
///
/// 只在"进来的块根本没有 id"时使用（外部导入、手搓 fixture）。派生规则只看内容，
/// 因此同一份内容在任何设备上得到同一个 id，不会造成"两个 id 指同一块"。
pub(crate) fn derived_id(b: &Block) -> String {
    let probe = Block {
        id: String::new(),
        type_: b.type_.clone(),
        attrs: b.attrs.clone(),
        content: b.content.clone(),
    };
    let h = notera_core::hash_json(&to_value(&probe));
    format!("nb-{}", h.short())
}

/// 校验（§10.3 第 2 步）：版本闸门 + 结构 + id 唯一 + 嵌套。
pub fn validate(doc: &Document) -> Result<(), RichError> {
    if !supports(doc.v) {
        return Err(RichError::UnsupportedVersion(doc.v));
    }
    validate_shape(doc)
}

/// 除版本闸门外的一切校验。`parse_for_read` 用它：v 超前的文档结构照样要合法，
/// 但**不能**因此拒绝对外展示（只读降级是 I7 的另一半）。
fn validate_shape(doc: &Document) -> Result<(), RichError> {
    let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
    for (i, b) in doc.content.iter().enumerate() {
        if b.id.is_empty() {
            return Err(RichError::Malformed(format!(
                "content[{i}] 缺少块 id（normalize 之后不可能为空，说明数据绕过了 normalize）"
            )));
        }
        if let Some(prev) = seen.insert(b.id.as_str(), i) {
            return Err(RichError::DuplicateBlockId(format!(
                "{}（content[{prev}] 与 content[{i}]）",
                b.id
            )));
        }
        validate_nesting(doc, i)?;
    }
    Ok(())
}

/// 扁平模型里的"嵌套"= 表格族必须成组、顺序正确（§10.3 第 2 步：`tableCell`
/// 不得直接含 `table`；本模型没有子块，所以等价于"cell 不能脱离 row 出现"）。
fn validate_nesting(doc: &Document, i: usize) -> Result<(), RichError> {
    let b = &doc.content[i];
    if !b.type_.is_table_family() {
        return Ok(());
    }
    let prev = i.checked_sub(1).and_then(|j| doc.content.get(j));
    match b.type_ {
        BlockType::TableRow => {
            let ok = matches!(
                prev.map(|p| &p.type_),
                Some(BlockType::Table) | Some(BlockType::TableRow) | Some(BlockType::TableCell)
            );
            if !ok {
                return Err(RichError::InvalidNesting(format!(
                    "content[{i}] 的 tableRow 不在 table 之后（块 {}）",
                    b.id
                )));
            }
        }
        BlockType::TableCell => {
            let ok = matches!(
                prev.map(|p| &p.type_),
                Some(BlockType::TableRow) | Some(BlockType::TableCell)
            );
            if !ok {
                return Err(RichError::InvalidNesting(format!(
                    "content[{i}] 的 tableCell 不在 tableRow 之内（块 {}）",
                    b.id
                )));
            }
        }
        _ => {}
    }
    Ok(())
}

/// 稳定序列化（§10.3 第 3 步）：键按 Unicode 码点升序、数组顺序不动、无多余空白、
/// 整数不写成浮点。**这就是 content_hash 的输入**，所以它的稳定性 = 哈希稳定性。
pub fn canonical(doc: &Document) -> String {
    canonical_json(&to_value(doc))
}

/// 单块的稳定序列化（合并算法逐块比哈希用）。
pub(crate) fn block_canonical(b: &Block) -> String {
    canonical_json(&to_value(b))
}

/// 展示用 JSON：与 `canonical` 同一套键序（所以字段书写顺序无关），带缩进便于人读。
pub fn to_json(doc: &Document) -> String {
    let mut out = String::new();
    write_pretty(&to_value(doc), 0, &mut out);
    out
}

pub(crate) fn to_value(v: &impl serde::Serialize) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

fn write_pretty(v: &Value, depth: usize, out: &mut String) {
    let pad = "  ".repeat(depth + 1);
    let close = "  ".repeat(depth);
    match v {
        Value::Object(m) => {
            if m.is_empty() {
                out.push_str("{}");
                return;
            }
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            out.push_str("{\n");
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push_str(",\n");
                }
                out.push_str(&pad);
                out.push_str(&Value::String((*k).clone()).to_string());
                out.push_str(": ");
                write_pretty(&m[*k], depth + 1, out);
            }
            out.push('\n');
            out.push_str(&close);
            out.push('}');
        }
        Value::Array(a) => {
            if a.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push_str("[\n");
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push_str(",\n");
                }
                out.push_str(&pad);
                write_pretty(x, depth + 1, out);
            }
            out.push('\n');
            out.push_str(&close);
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

/// 顶层块 id 列表（顺序即文档顺序）。UI/诊断与合并测试都用它。
pub fn block_ids(doc: &Document) -> Vec<String> {
    doc.content.iter().map(|b| b.id.clone()).collect()
}

/// 一份文档相对另一份有多少块内容不同（`AutoMerged` 的 taken_* 计数）。
pub(crate) fn count_changed_blocks(base: &Document, other: &Document) -> u32 {
    let mut n = 0u32;
    for b in &other.content {
        let same_as_base = base
            .content
            .iter()
            .any(|x| x.id == b.id && x.same_content(b));
        if !same_as_base {
            n += 1;
        }
    }
    n
}

impl Block {
    /// 内容相等（不含 id）：合并算法判断"这块被改过吗"的唯一依据。
    pub fn same_content(&self, other: &Block) -> bool {
        self.type_ == other.type_
            && self.attrs == other.attrs
            && self.content == other.content
    }

    /// 内容哈希用的规范化 JSON（逐块比较用）。
    pub fn content_key(&self) -> String {
        block_canonical(self)
    }
}
