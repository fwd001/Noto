//! 公共 API 契约测试（跨 crate 冻结点）。
//!
//! store / sync / importer 三个 crate 正按同一份签名并行开发，任何一次"顺手改名"或
//! "多加一个字段"都会在别处编译失败。这个文件把契约钉成可机检的东西：
//! * 函数指针类型标注 → 签名一字不差；
//! * 结构体字面量 → 公共字段集合既不能少也不能多（**不能加字段**，因为别人用字面量构造）；
//! * 穷尽 match → 枚举变体集合与顺序无关地完整。

use notera_richtext::{
    canonical, extract, merge, normalize, parse, supports, to_json, validate, Block, BlockType,
    Document, Extracted, Inline, Mark, MarkKind, MergeOutcome, ReadOnlyReason, RichError,
    block_ids, DOC_FORMAT,
};
use std::collections::BTreeMap;

#[test]
fn function_signatures_are_exactly_the_contract() {
    let _: u16 = DOC_FORMAT;
    let _: fn(u16) -> bool = supports;
    let _: fn(&str) -> Result<Document, RichError> = parse;
    let _: fn(&mut Document) = normalize;
    let _: fn(&Document) -> Result<(), RichError> = validate;
    let _: fn(&Document) -> String = canonical;
    let _: fn(&Document) -> String = to_json;
    let _: fn(&Document) -> Extracted = extract;
    let _: fn(&Document) -> Vec<String> = block_ids;
    let _: fn(&Document, &Document, &Document) -> MergeOutcome = merge;
}

#[test]
fn struct_field_sets_are_frozen() {
    // 少字段编译不过；**多字段同样编译不过**（字面量构造是别人的用法）。
    let doc: Document = Document {
        v: DOC_FORMAT,
        content: Vec::new(),
    };
    let blk: Block = Block {
        id: "p1aaaaaa".into(),
        type_: BlockType::Paragraph,
        attrs: BTreeMap::new(),
        content: Vec::new(),
    };
    let inl: Inline = Inline { text: "字".into(), marks: Vec::new() };
    let mk: Mark = Mark { kind: MarkKind::Bold, attrs: BTreeMap::new() };
    let x: Extracted = Extracted {
        title: String::new(),
        plain_text: String::new(),
        summary: String::new(),
        char_count: 0,
        block_count: 0,
        has_attachment: false,
    };
    assert_eq!(doc.content.len(), 0);
    assert_eq!(blk.content.len(), 0);
    assert_eq!(inl.marks.len(), 0);
    assert_eq!(mk.attrs.len(), 0);
    assert_eq!(x.char_count, 0);
}

#[test]
fn enum_variant_sets_are_frozen() {
    let types = [
        BlockType::Paragraph,
        BlockType::Heading,
        BlockType::BlockQuote,
        BlockType::CodeBlock,
        BlockType::OrderedList,
        BlockType::BulletList,
        BlockType::ChecklistItem,
        BlockType::Image,
        BlockType::Attachment,
        BlockType::HorizontalRule,
        BlockType::Table,
        BlockType::TableRow,
        BlockType::TableCell,
        BlockType::Unknown("x".into()),
    ];
    assert_eq!(types.len(), 14);
    for t in &types {
        // 每个变体的 wire 名必须能原样解析回来（Unknown 尤其重要：原名不许丢）。
        let back = match t {
            BlockType::Unknown(n) => {
                assert_eq!(n, "x");
                assert_eq!(t.wire_name(), "unknown:x");
                BlockType::Unknown(n.clone())
            }
            other => BlockType::from_wire_name(&other.wire_name()),
        };
        assert_eq!(&back, t, "{t:?} 的 wire 名往返不一致");
    }

    let marks = [
        MarkKind::Bold,
        MarkKind::Italic,
        MarkKind::Underline,
        MarkKind::Strike,
        MarkKind::Code,
        MarkKind::Highlight,
        MarkKind::Link,
        MarkKind::FontSize,
        MarkKind::Color,
        MarkKind::AttachmentRef,
        MarkKind::Unknown("y".into()),
    ];
    assert_eq!(marks.len(), 11);
    for m in &marks {
        let back = match m {
            MarkKind::Unknown(n) => {
                assert_eq!(n, "y");
                assert_eq!(m.wire_name(), "unknown:y");
                MarkKind::Unknown(n.clone())
            }
            other => MarkKind::from_wire_name(&other.wire_name()),
        };
        assert_eq!(&back, m, "{m:?} 的 wire 名往返不一致");
    }

    // 未知类型的**原始名**在 Unknown 里，不是 `unknown:` 之后的残片被改写。
    assert_eq!(BlockType::from_wire_name("mermaid"), BlockType::Unknown("mermaid".into()));
    assert_eq!(BlockType::from_wire_name("unknown:mermaid"), BlockType::Unknown("mermaid".into()));
    assert_eq!(MarkKind::from_wire_name("sparkle"), MarkKind::Unknown("sparkle".into()));
}

#[test]
fn error_and_outcome_variants_are_matchable_exhaustively() {
    let errs = [
        RichError::Malformed("x".into()),
        RichError::InvalidNesting("x".into()),
        RichError::DuplicateBlockId("x".into()),
        RichError::UnsupportedVersion(2),
    ];
    assert_eq!(errs.len(), 4);
    for e in errs {
        let s = match &e {
            RichError::Malformed(_) => "Malformed",
            RichError::InvalidNesting(_) => "InvalidNesting",
            RichError::DuplicateBlockId(_) => "DuplicateBlockId",
            RichError::UnsupportedVersion(v) => {
                assert_eq!(*v, 2);
                "UnsupportedVersion"
            }
        };
        assert!(!e.to_string().is_empty(), "{s} 必须有可读文案（UI 只用这个字符串）");
        // 错误必须可克隆可比对（测试与 outbox 重放都依赖它）。
        assert_eq!(e.clone(), e);
    }

    let outcomes = [
        MergeOutcome::Converged,
        MergeOutcome::ReadOnly { because: ReadOnlyReason::DocVersionTooNew(9) },
        MergeOutcome::Conflict { conflicting_block_ids: vec!["a".into()] },
        MergeOutcome::AutoMerged {
            doc: Document { v: DOC_FORMAT, content: Vec::new() },
            taken_local: 1,
            taken_remote: 2,
        },
    ];
    for o in outcomes {
        match o {
            MergeOutcome::Converged => {}
            MergeOutcome::AutoMerged { doc, taken_local, taken_remote } => {
                let _: u32 = taken_local;
                let _: u32 = taken_remote;
                let _: &Vec<Block> = &doc.content;
            }
            MergeOutcome::Conflict { conflicting_block_ids } => {
                let _: &Vec<String> = &conflicting_block_ids;
            }
            MergeOutcome::ReadOnly { because } => match because {
                ReadOnlyReason::DocVersionTooNew(v) => assert_eq!(v, 9),
            },
        }
    }
    assert!(!supports(DOC_FORMAT + 1));
    assert!(supports(DOC_FORMAT));
}
