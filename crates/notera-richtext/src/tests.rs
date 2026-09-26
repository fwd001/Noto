//! 模型 / 规范化 / 校验 / 稳定序列化 的 L0 测试（TEST-PLAN.md RT-*、FWD-*、INV-10）。

use crate::codec::{canonical, parse, parse_for_read, parse_from_value, to_json, validate};
use crate::model::{BlockType, Document, MarkKind, RichError, supports, DOC_FORMAT};
use crate::{block_ids, extract};
use serde_json::{json, Value};

fn para(id: &str, text: &str) -> Value {
    json!({ "id": id, "type": "paragraph", "content": [{ "text": text }] })
}

fn doc(blocks: Vec<Value>) -> Value {
    json!({ "v": DOC_FORMAT, "content": blocks })
}

fn doc_text(d: &Document) -> String {
    d.content.iter().map(|b| b.plain_text()).collect::<Vec<_>>().join("\n")
}

// ------------------------------------------------------------------ 往返 ---

#[test]
fn parse_canonical_parse_is_fixed_point() {
    // FT-RT-12「零漂移」的最小版本：canonical(parse(x)) == canonical(parse(canonical(x)))。
    let raw = doc(vec![
        json!({ "id": "h1aaaa", "type": "heading", "attrs": { "level": 2 }, "content": [{ "text": "标题", "marks": [{ "kind": "bold" }] }] }),
        para("p1aaaa", "第一段"),
        json!({ "id": "cb1aaa", "type": "codeBlock", "attrs": { "lang": "rust" }, "content": [{ "text": "fn main() {}\n" }] }),
    ]);
    let first = parse(&raw.to_string()).unwrap();
    let c1 = canonical(&first);
    let second = parse(&c1).unwrap();
    let c2 = canonical(&second);
    assert_eq!(c1, c2, "canonical 必须是不动点：{c1}\n{c2}");
    assert_eq!(first, second);
    // 尾随换行不许被 trim（FT-RT-08）。
    assert!(c1.contains("fn main() {}\\n"));
}

#[test]
fn canonical_is_key_order_independent_so_hash_is_stable() {
    // §10.3：这是 content_hash 的前提。同一逻辑文档、不同键书写顺序 → 逐字节相同。
    let a = doc(vec![json!({
        "id": "p1aaaa", "type": "paragraph",
        "attrs": { "zdir": 1, "align": "left", "nested": { "y": 2, "b": 1 } },
        "content": [{ "text": "x", "marks": [{ "kind": "highlight", "attrs": { "color": "#fff", "alpha": 0.5 } }] }]
    })]);
    let b = doc(vec![json!({
        "content": [{ "marks": [{ "attrs": { "alpha": 0.5, "color": "#fff" }, "kind": "highlight" }], "text": "x" }],
        "attrs": { "nested": { "b": 1, "y": 2 }, "align": "left", "zdir": 1 },
        "type": "paragraph", "id": "p1aaaa"
    })]);
    // `v` 在 b 里缺失 → 视为 DOC_FORMAT，两者是同一逻辑文档。
    let da = parse(&a.to_string()).unwrap();
    let mut b2 = b.clone();
    b2["v"] = json!(DOC_FORMAT);
    let db = parse(&b2.to_string()).unwrap();
    let (ca, cb) = (canonical(&da), canonical(&db));
    assert_eq!(ca, cb, "键顺序影响了 canonical：\n{ca}\n{cb}");
    assert_eq!(
        notera_core::hash_json(&json!(ca)),
        notera_core::hash_json(&json!(cb))
    );
    // 整数不许写成浮点。
    assert!(ca.contains("\"zdir\":1"), "{ca}");
    assert!(!ca.contains("1.0"), "{ca}");
}

#[test]
fn to_json_is_pretty_and_order_insensitive() {
    let d = parse(&doc(vec![para("p1aaaa", "甲")]).to_string()).unwrap();
    let pretty = to_json(&d);
    assert!(pretty.contains('\n'), "展示用 JSON 应当可读");
    // 与 canonical 表达同一份数据。
    let again: Value = serde_json::from_str(&pretty).unwrap();
    assert_eq!(
        notera_core::canonical_json(&json!(d)),
        notera_core::canonical_json(&again)
    );
}

// ------------------------------------------------------- preserve-unknown ---

#[test]
fn unknown_block_type_mark_and_attr_all_survive_roundtrip() {
    // FWD-01/INV-10：旧客户端绝不能摧毁新内容。
    let raw = doc(vec![
        para("p1aaaa", "旧客户端看得懂的段落"),
        json!({
            "id": "mmmmmm", "type": "mermaid",
            "attrs": { "data-x": "y", "zoom": 1.5 },
            "content": [{ "text": "graph TD; A-->B" }]
        }),
    ]);
    let d = parse(&raw.to_string()).unwrap();
    let unknown = d
        .content
        .iter()
        .find(|b| matches!(b.type_, BlockType::Unknown(_)))
        .expect("未知 type 必须落 BlockType::Unknown");
    assert_eq!(unknown.type_, BlockType::Unknown("mermaid".into()));
    assert_eq!(unknown.attrs.get("data-x"), Some(&json!("y")), "未知 attr 必须原样保留");
    assert_eq!(unknown.attrs.get("zoom"), Some(&json!(1.5)));

    let c = canonical(&d);
    assert!(c.contains("\"unknown:mermaid\""), "{c}");
    assert!(c.contains("\"data-x\":\"y\""), "{c}");
    // 再走一遍：位置、属性、子树顺序都不许变。
    let d2 = parse(&c).unwrap();
    assert_eq!(canonical(&d2), c, "未知节点在往返中漂移了");
    assert_eq!(block_ids(&d2), vec!["p1aaaa", "mmmmmm"]);
}

#[test]
fn unknown_mark_survives_and_degrades_without_loss() {
    let raw = doc(vec![json!({
        "id": "p1aaaa", "type": "paragraph",
        "content": [{ "text": "闪光", "marks": [
            { "kind": "bold" },
            { "kind": "sparkle", "attrs": { "intensity": 3 } },
            { "kind": "unknown:already", "attrs": {} }
        ] }]
    })]);
    let d = parse(&raw.to_string()).unwrap();
    let marks = &d.content[0].content[0].marks;
    assert!(marks.iter().any(|m| m.kind == MarkKind::Unknown("sparkle".into())));
    assert!(marks.iter().any(|m| m.kind == MarkKind::Unknown("already".into())));
    assert!(marks.iter().any(|m| m.kind == MarkKind::Bold));
    let c = canonical(&d);
    assert!(c.contains("\"unknown:sparkle\"") && c.contains("\"intensity\":3"), "{c}");
    assert_eq!(canonical(&parse(&c).unwrap()), c);
}

#[test]
fn unknown_top_level_keys_are_folded_into_attrs_not_dropped() {
    // 未来客户端可能把字段直接挂在块上（不在 attrs 里）。那也是内容。
    let raw = doc(vec![json!({
        "id": "p1aaaa", "type": "paragraph", "text": "外挂字段",
        "content": [{ "text": "正文", "side": 7 }]
    })]);
    let d = parse(&raw.to_string()).unwrap();
    let c = canonical(&d);
    assert!(
        c.contains("\"key\":\"side\""),
        "inline 未知字段必须折进未知 mark 保留：{c}"
    );
    assert!(c.contains("外挂字段"), "块级未知字段必须留下：{c}");
    assert_eq!(canonical(&parse(&c).unwrap()), c, "折叠必须幂等");
}

#[test]
fn nested_block_children_are_folded_not_destroyed() {
    // 本模型是扁平的（列表用 indent 表达），但旧 fixture 可能带嵌套子块。
    let raw = doc(vec![json!({
        "id": "qqaaaa", "type": "blockquote",
        "content": [ { "id": "inner1", "type": "paragraph", "content": [{ "text": "被嵌套的段落" }] } ]
    })]);
    let d = parse(&raw.to_string()).unwrap();
    let c = canonical(&d);
    assert!(c.contains("被嵌套的段落") && c.contains("inner1"), "嵌套子块必须保留：{c}");
    assert_eq!(canonical(&parse(&c).unwrap()), c);
}

// ------------------------------------------------------------------ 版本闸门 ---

#[test]
fn too_new_doc_version_is_read_only_not_corrupted() {
    // FWD-03：parse 拒绝写回路径，merge 给出 ReadOnly，只读路径仍能看到全部内容。
    let raw = doc(vec![para("p1aaaa", "新版客户端写的一个段落")]);
    let mut future = raw.clone();
    future["v"] = json!(99);
    assert!(!supports(99));
    assert_eq!(
        parse(&future.to_string()),
        Err(RichError::UnsupportedVersion(99))
    );
    let ro = parse_for_read(&future).expect("只读路径必须能读出内容");
    assert_eq!(ro.content.len(), 1);
    assert!(canonical(&ro).contains("新版客户端写的一个段落"));
    // 结构问题在只读路径上同样要拒（只放版本闸门，不放质量闸门）。
    let broken = json!({ "v": 99, "content": [{ "id": "a1aaaaaa", "type": "paragraph", "attrs": 3 }] });
    assert!(matches!(
        parse_for_read(&broken),
        Err(RichError::Malformed(_))
    ));
}

#[test]
fn duplicate_block_id_is_rejected() {
    // 合并锚点唯一性：不能"猜哪个是真的"。
    let raw = doc(vec![para("dup1aaaa", "第一份"), para("dup1aaaa", "第二份")]);
    assert_eq!(
        parse(&raw.to_string()),
        Err(RichError::DuplicateBlockId(
            "dup1aaaa（content[0] 与 content[1]）".into()
        ))
    );
    // 完全相同的重复块是噪声，normalize 可以无损地去掉。
    let same = doc(vec![para("dup1aaaa", "同一份"), para("dup1aaaa", "同一份")]);
    let d = parse(&same.to_string()).unwrap();
    assert_eq!(block_ids(&d), vec!["dup1aaaa"]);
}

#[test]
fn malformed_inputs_are_rejected_not_half_accepted() {
    for bad in [
        "",
        "not json",
        "[]",
        "{\"v\":1,\"content\":{}}",
        "{\"v\":1,\"content\":[\"nope\"]}",
        "{\"v\":\"one\",\"content\":[]}",
        "{\"v\":1,\"content\":[{\"id\":1,\"type\":\"paragraph\"}]}",
    ] {
        assert!(
            matches!(parse(bad), Err(RichError::Malformed(_))),
            "畸形输入必须被拒：{bad}"
        );
    }
}

#[test]
fn table_family_must_stay_grouped() {
    // §10.3 第 2 步"嵌套合法"在扁平模型里的可表达形式。
    let good = doc(vec![
        json!({ "id": "tblaaaa", "type": "table" }),
        json!({ "id": "rwa1aaa", "type": "tableRow" }),
        para("cell1aa", "甲"),
        json!({ "id": "cell2aa", "type": "tableCell" }),
    ]);
    let mut good = good;
    good["content"][2]["type"] = json!("tableCell");
    assert_eq!(validate(&parse(&good.to_string()).unwrap()), Ok(()));

    let bad = doc(vec![
        para("p1aaaa", "正文"),
        json!({ "id": "cellbad", "type": "tableCell" }),
    ]);
    assert!(matches!(
        parse(&bad.to_string()),
        Err(RichError::InvalidNesting(_))
    ));
}

#[test]
fn normalize_strips_zero_width_but_keeps_emoji_joiners() {
    let raw = doc(vec![para(
        "p1aaaa",
        "零宽​﻿应被剥离，ZWJ‍家庭👨‍👩‍👧 必须原样保留",
    )]);
    let d = parse(&raw.to_string()).unwrap();
    let t = d.content[0].plain_text();
    assert!(!t.contains('​') && !t.contains('﻿'), "{t}");
    assert!(t.contains("👨‍👩‍👧"), "剥掉 ZWJ 会摧毁 emoji 序列：{t}");
    // 只有零宽字符的行内节点会被丢弃（它不携带任何信息），但空段落本身保留。
    let only_zw = doc(vec![para("p2aaaa", "​"), para("p3aaaa", "留着")]);
    let d2 = parse(&only_zw.to_string()).unwrap();
    assert_eq!(d2.content[0].id, "p2aaaa");
    assert!(d2.content[0].content.is_empty(), "空段落保留、空 inline 丢弃");
    assert_eq!(doc_text(&d2), "\n留着");
}

#[test]
fn blocks_without_ids_get_a_deterministic_derived_id() {
    // 外部导入/手搓 fixture 常常没有 id：不能因此拒收，也不能随机造（跨设备会撞）。
    let raw = json!({ "v": 1, "content": [{ "type": "paragraph", "content": [{ "text": "无 id" }] }] });
    let d1 = parse(&raw.to_string()).unwrap();
    let d2 = parse(&raw.to_string()).unwrap();
    assert_eq!(block_ids(&d1), block_ids(&d2), "派生 id 必须只由内容决定");
    let id = &block_ids(&d1)[0];
    assert!(id.starts_with("nb-") && id.len() >= 8 && id.len() <= 32, "{id}");
    // 同内容不同位置 → 同 id（所以两份文档合并时会被认出是同一块）；不同内容 → 不同 id。
    let other = json!({ "v": 1, "content": [{ "type": "paragraph", "content": [{ "text": "别的" }] }] });
    let d3 = parse(&other.to_string()).unwrap();
    assert_ne!(block_ids(&d1), block_ids(&d3));
}

#[test]
fn heading_and_list_defaults_are_filled_once() {
    let raw = doc(vec![
        json!({ "id": "h1aaaa", "type": "heading", "content": [{ "text": "无 level" }] }),
        json!({ "id": "li1aaa", "type": "bulletList", "content": [{ "text": "无 indent" }] }),
    ]);
    let d = parse(&raw.to_string()).unwrap();
    assert_eq!(d.content[0].attrs.get("level"), Some(&json!(1)));
    assert_eq!(d.content[1].attrs.get("indent"), Some(&json!(0)));
    // 幂等：第二遍不再改变任何东西。
    assert_eq!(canonical(&d), canonical(&parse(&canonical(&d)).unwrap()));
    // 已有值不许被默认值覆盖。
    let with = doc(vec![json!({ "id": "h2aaaa", "type": "heading", "attrs": { "level": 4 } })]);
    assert_eq!(parse(&with.to_string()).unwrap().content[0].attrs["level"], json!(4));
}

// ------------------------------------------------------------------- 派生 ---

#[test]
fn extract_counts_chinese_by_codepoint_not_byte() {
    // 中文按码点计数：按字节会放大 3 倍，预览与"字数"全错。
    let d = parse(
        &doc(vec![
            json!({ "id": "h1aaaa", "type": "heading", "attrs": { "level": 1 }, "content": [{ "text": "同步测试" }] }),
            para("p1aaaa", "中文 abc"),
        ])
        .to_string(),
    )
    .unwrap();
    let x = extract(&d);
    assert_eq!(x.title, "同步测试");
    assert_eq!(x.plain_text, "同步测试\n中文 abc");
    // 排除空白后的码点：同步测试(4) + 中文(2) + abc(3) = 9。
    assert_eq!(x.char_count, 9, "必须按码点：{:?}", x.plain_text);
    assert_eq!(x.block_count, 2);
    assert!(!x.has_attachment);
    // summary 跳过标题。
    assert_eq!(x.summary, "中文 abc");
    let bytes = x.plain_text.as_bytes().len();
    assert_eq!(bytes, 23, "顺带确认字节数远大于码点数：{bytes} vs {}", x.char_count);
}

#[test]
fn extract_title_falls_back_to_first_non_empty_block() {
    let d = parse(&doc(vec![para("p1aaaa", "   "), para("p2aaaa", "没有标题的笔记")]).to_string()).unwrap();
    let x = extract(&d);
    assert_eq!(x.title, "没有标题的笔记");
    let empty = extract(&parse(&doc(vec![]).to_string()).unwrap());
    assert_eq!(empty.title, "");
    assert_eq!(empty.char_count, 0);
    assert_eq!(empty.block_count, 0);
}

#[test]
fn extract_detects_attachment_refs_in_blocks_and_marks() {
    let d = parse(
        &doc(vec![
            para("p1aaaa", "带附件引用"),
            json!({ "id": "at1aaa", "type": "attachment", "attrs": { "sha256": "abc", "name": "x.pdf" } }),
        ])
        .to_string(),
    )
    .unwrap();
    assert!(extract(&d).has_attachment);
    let via_mark = parse(
        &doc(vec![json!({
            "id": "p2aaaa", "type": "paragraph",
            "content": [{ "text": "内联附件", "marks": [{ "kind": "attachmentRef", "attrs": { "sha256": "abc", "role": "file" } }] }]
        })])
        .to_string(),
    )
    .unwrap();
    assert!(extract(&via_mark).has_attachment);
}

#[test]
fn title_truncation_is_charwise_for_cjk() {
    let long: String = "测".repeat(300);
    let d = parse(&doc(vec![para("p1aaaa", &long)]).to_string()).unwrap();
    let x = extract(&d);
    assert_eq!(x.title.chars().count(), 200, "截断必须按码点，否则切半个汉字");
    assert!(x.title.is_char_boundary(x.title.len()));
}

// ------------------------------------------------------------------ 结构 ---

#[test]
fn block_ids_lists_top_level_in_document_order() {
    let d: Document = parse(&doc(vec![para("b3aaaaaa", "3"), para("b1aaaaaa", "1"), para("b2aaaaaa", "2")]).to_string()).unwrap();
    assert_eq!(block_ids(&d), vec!["b3aaaaaa", "b1aaaaaa", "b2aaaaaa"]);
}

#[test]
fn deserialize_from_value_uses_the_same_tolerant_path() {
    // 其他 crate 会直接 `serde_json::from_value::<Document>`，那条路必须与 parse 同语义。
    let v = doc(vec![json!({ "id": "p1aaaa", "type": "paragraph", "content": ["裸字符串也行"] })]);
    let d: Document = serde_json::from_value(v).unwrap();
    assert_eq!(d.content[0].plain_text(), "裸字符串也行");
    let err = serde_json::from_value::<Document>(json!({ "v": 1, "content": [3] }));
    assert!(err.is_err());
}

#[test]
fn parse_from_value_is_the_shared_entry_point() {
    let v: Value = json!({ "v": 1, "content": [para("p1aaaa", "值入口")] });
    assert_eq!(
        parse_from_value(&v).unwrap(),
        parse(&v.to_string()).unwrap()
    );
}

#[test]
fn attachments_lists_what_the_document_references_and_nothing_else() {
    let sha = "a".repeat(64);
    let v = doc(vec![
        para("p1aaaa", "普通段落不该被当成附件"),
        json!({ "id": "im1aaaaa", "type": "image", "attrs": {
            "sha256": sha, "role": "inline", "size": 2048, "mediaType": "image/png", "name": "shot.png" } }),
        // 还没落地的占位块：没有 sha256 就没有引用可登记
        json!({ "id": "im2aaaaa", "type": "image", "attrs": { "role": "inline", "pending": true } }),
        // 形态不对的 sha：`attachments` 主键上有 CHECK，塞进去会让整批写入回滚
        json!({ "id": "im3aaaaa", "type": "image", "attrs": { "sha256": "ABC123" } }),
        // 未知块类型带 sha256 也算（前向兼容：别的客户端用容器块装附件）
        json!({ "id": "xx1aaaaa", "type": "someFutureBlock", "attrs": { "sha256": sha, "role": "bogus" } }),
    ]);
    let got = crate::attachments(&parse_from_value(&v).unwrap());
    assert_eq!(got.len(), 2, "{got:?}");
    assert_eq!(got[0].block_id, "im1aaaaa");
    assert_eq!(got[0].sha256, sha);
    assert_eq!(got[0].role.as_deref(), Some("inline"));
    assert_eq!(got[0].size, Some(2048));
    assert_eq!(got[0].filename.as_deref(), Some("shot.png"));
    // 角色词汇之外的值一律丢回 None，由存储层按媒体类型判（表上有 CHECK(role IN (inline,file)))
    assert_eq!(got[1].role, None);
    assert_eq!(extract(&parse_from_value(&v).unwrap()).has_attachment, true, "畸形 sha 仍算\"这篇有附件\"，只是登记不了");
}
