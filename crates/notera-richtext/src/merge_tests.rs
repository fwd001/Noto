//! 三方合并的 L0 测试（CONFLICT-RESOLUTION §3.2/§3.3/§3.4，TEST-PLAN SY-CONF-01..08，
//! INV-01/INV-10/INV-15）。
//!
//! 断言强度按 TEST-PLAN 的要求：**不是"没有报错"，而是"两份内容都能被逐字节找回"**。

use crate::codec::{block_canonical, canonical, parse, parse_from_value, validate};
use crate::merge::{merge, MergeOutcome, ReadOnlyReason};
use crate::model::{Block, Document, Inline, Mark, MarkKind};
use crate::{block_ids, supports, BlockType, DOC_FORMAT};
use serde_json::{json, Value};
use std::collections::BTreeSet;

fn para(id: &str, text: &str) -> Value {
    json!({ "id": id, "type": "paragraph", "content": [{ "text": text }] })
}

/// 类型化的块（属性测试里直接改文档结构，不绕 JSON）。
fn block(id: &str, text: &str) -> Block {
    Block {
        id: id.to_string(),
        type_: BlockType::Paragraph,
        attrs: Default::default(),
        content: vec![Inline { text: text.to_string(), marks: vec![] }],
    }
}

fn marked(id: &str, text: &str, marks: &[&str]) -> Value {
    json!({
        "id": id, "type": "paragraph",
        "content": [{ "text": text, "marks": marks.iter().map(|m| json!({ "kind": m })).collect::<Vec<_>>() }]
    })
}

fn checklist(id: &str, text: &str, checked: Option<bool>) -> Value {
    json!({
        "id": id, "type": "checklistItem",
        "attrs": match checked { Some(c) => json!({ "checked": c }), None => json!({}) },
        "content": [{ "text": text }]
    })
}

fn doc(blocks: Vec<Value>) -> Document {
    parse_from_value(&json!({ "v": DOC_FORMAT, "content": blocks })).expect("fixture 必须合法")
}

/// 故意绕开 `validate`：喂给 merge 那些"根本进不了权威表"的畸形文档（硬性要求 3）。
fn raw(blocks: Vec<Value>) -> Document {
    let mut d = crate::model::Document::from_value(&json!({ "v": DOC_FORMAT, "content": blocks }))
        .expect("结构本身必须可解析");
    crate::normalize(&mut d);
    d
}

fn text_of(d: &Document) -> String {
    d.content.iter().map(|b| b.plain_text()).collect::<Vec<_>>().join("\n")
}

fn auto(outcome: MergeOutcome) -> Document {
    match outcome {
        MergeOutcome::AutoMerged { doc, .. } => doc,
        other => panic!("期望 AutoMerged，实际 {other:?}"),
    }
}

fn ids_of(d: &Document) -> BTreeSet<String> {
    d.content.iter().map(|b| b.id.clone()).collect()
}

fn conflict_ids(outcome: &MergeOutcome) -> Vec<String> {
    match outcome {
        MergeOutcome::Conflict { conflicting_block_ids } => conflicting_block_ids.clone(),
        other => panic!("期望 Conflict，实际 {other:?}"),
    }
}

fn base() -> Document {
    doc(vec![para("b1aaaaaa", "第一段原文"), para("b2aaaaaa", "第二段原文")])
}

/// INV-01 的可机检形式：AutoMerged 之后，任何一侧的块要么还在结果里，要么有
/// **可证明的理由**消失（被另一侧删除且本侧未改、被同内容的新块去重、被折进更大的块）。
fn assert_no_silent_loss(b: &Document, l: &Document, r: &Document, m: &Document) {
    for (side, other) in [(&l, &r), (&r, &l)] {
        for x in &side.content {
            if m.content.iter().any(|y| y.id == x.id) {
                continue;
            }
            let in_base = b.content.iter().any(|y| y.id == x.id);
            let unchanged = b
                .content
                .iter()
                .any(|y| y.id == x.id && y.same_content(x));
            if in_base && unchanged && !other.content.iter().any(|y| y.id == x.id) {
                continue; // 无争议的删除
            }
            if m.content.iter().any(|y| {
                y.type_ == x.type_ && y.plain_text() == x.plain_text()
            }) {
                continue; // 同内容去重
            }
            if m.content.iter().any(|y| {
                let (a, c) = (y.plain_text(), x.plain_text());
                !c.is_empty() && (a.contains(c.as_str()) || c.contains(a.as_str()))
            }) {
                continue; // 被折进超集/追加块
            }
            panic!(
                "静默丢失了一侧的块：{}\nmerged = {}",
                block_canonical(x),
                canonical(m)
            );
        }
    }
}

// ---------------------------------------------------------- 六个必备合并场景 ---

#[test]
fn sy_mrg_01_different_blocks_auto_merge_without_conflict() {
    // SY-CONF-02/07：两侧改**不同块** → 无损自动合并，且不得产生任何冲突提示。
    let local = doc(vec![para("b1aaaaaa", "本地改过的第一段"), para("b2aaaaaa", "第二段原文")]);
    let remote = doc(vec![para("b1aaaaaa", "第一段原文"), para("b2aaaaaa", "远端改过的第二段")]);
    let out = merge(&base(), &local, &remote);
    let merged = match out {
        MergeOutcome::AutoMerged {
            doc,
            taken_local,
            taken_remote,
        } => {
            assert_eq!((taken_local, taken_remote), (1, 1));
            doc
        }
        other => panic!("异块改动必须自动合并：{other:?}"),
    };
    assert!(text_of(&merged).contains("本地改过的第一段"));
    assert!(text_of(&merged).contains("远端改过的第二段"));
    validate(&merged).unwrap();
    assert_no_silent_loss(&base(), &local, &remote, &merged);
}

#[test]
fn sy_mrg_02_same_block_text_conflict_never_picks_a_winner() {
    // SY-CONF-01/08：同一块两侧都改且无法证明无损 → 必须上报，绝不"整篇二选一"。
    let local = doc(vec![para("b1aaaaaa", "START middle"), para("b2aaaaaa", "第二段原文")]);
    let remote = doc(vec![para("b1aaaaaa", "start MIDDLE"), para("b2aaaaaa", "第二段原文")]);
    let out = merge(&base(), &local, &remote);
    assert_eq!(conflict_ids(&out), vec!["b1aaaaaa".to_string()]);
    assert!(matches!(out, MergeOutcome::Conflict { .. }), "同块文本冲突不能自动合并");
}

#[test]
fn sy_mrg_03_delete_versus_edit_is_a_conflict_not_a_silent_delete() {
    // §1.2 明写"一侧改、另一侧删除 → 是冲突"（C4/P11）。
    let deleted = doc(vec![para("b2aaaaaa", "第二段原文")]);
    let edited = doc(vec![para("b1aaaaaa", "远端把第一段改了"), para("b2aaaaaa", "第二段原文")]);
    let out = merge(&base(), &deleted, &edited);
    assert_eq!(conflict_ids(&out), vec!["b1aaaaaa".to_string()]);
    // 镜像：改的一方当 local、删的一方当 remote，结论必须一样。
    assert_eq!(
        conflict_ids(&merge(&base(), &edited, &deleted)),
        vec!["b1aaaaaa".to_string()]
    );

    // 而"一侧删、另一侧没动"是无争议的删除：必须采纳，不能制造假冲突。
    match merge(&base(), &deleted, &base()) {
        MergeOutcome::AutoMerged {
            doc,
            taken_local,
            taken_remote,
        } => {
            assert_eq!(block_ids(&doc), vec!["b2aaaaaa"]);
            assert_eq!((taken_local, taken_remote), (0, 0), "删除不算内容贡献");
            assert_no_silent_loss(&base(), &deleted, &base(), &doc);
        }
        other => panic!("无争议的删除不该升级成冲突：{other:?}"),
    }
}

#[test]
fn sy_mrg_04_identical_content_converges() {
    // §1.2 第一行：两侧最终内容相同 → 收敛（回环同步、两台设备各自打开又保存）。
    let same = doc(vec![para("b1aaaaaa", "两边都改成一样"), para("b2aaaaaa", "第二段原文")]);
    assert_eq!(merge(&base(), &same, &same), MergeOutcome::Converged);
    assert_eq!(merge(&base(), &base(), &base()), MergeOutcome::Converged);
    // 内容相同但键序不同也必须判收敛（canonical 稳定性直接决定这一条）。
    let reordered: Document = parse(&canonical(&same)).unwrap();
    assert_eq!(merge(&base(), &same, &reordered), MergeOutcome::Converged);
}

#[test]
fn sy_mrg_05_format_only_difference_auto_merges_via_m1() {
    // §3.3 M1：纯文本相同、只有 marks/attrs 不同 → 采纳带格式的一侧，不进冲突收件箱。
    let b = doc(vec![para("b1aaaaaa", "第一段原文")]);
    let bold = doc(vec![marked("b1aaaaaa", "第一段原文", &["bold"])]);
    let italic = doc(vec![marked("b1aaaaaa", "第一段原文", &["italic"])]);
    let merged = auto(merge(&b, &bold, &italic));
    assert_eq!(text_of(&merged), "第一段原文", "M1 不许改变一个字");
    assert_eq!(merged.content[0].content[0].marks.len(), 1, "必须带走一侧的格式");
    validate(&merged).unwrap();
    // 结果与"谁是 local"无关（INV-15 的收敛前提）。
    assert_eq!(
        canonical(&merged),
        canonical(&auto(merge(&b, &italic, &bold))),
        "M1 的幸存者由内容决定，否则两台设备各留一份格式"
    );
    // 只有一侧改格式 → 采纳那一侧，另一侧的（无）格式不算丢。
    let plain = doc(vec![para("b1aaaaaa", "第一段原文")]);
    let m2 = auto(merge(&b, &bold, &plain));
    assert_eq!(m2.content[0].content[0].marks.len(), 1);
}

#[test]
fn sy_mrg_06_pure_appends_are_concatenated_via_m3() {
    // §3.3 M3：两侧都是对 base 的纯追加 → 拼接，两份内容都在（INV-01）。
    let b = doc(vec![para("b1aaaaaa", "hello")]);
    let local = doc(vec![para("b1aaaaaa", "hello world")]);
    let remote = doc(vec![para("b1aaaaaa", "hello there")]);
    let merged = auto(merge(&b, &local, &remote));
    let t = text_of(&merged);
    assert!(t.starts_with("hello"), "{t}");
    assert!(t.contains("world") && t.contains("there"), "两侧追加都必须在：{t}");
    assert_eq!(t, "hello there world", "追加段按文本字典序拼接，保证跨设备一致");
    validate(&merged).unwrap();
    assert_no_silent_loss(&b, &local, &remote, &merged);
    // 交换角色必须逐字节相同，否则下一轮又冲突。
    assert_eq!(
        canonical(&merged),
        canonical(&auto(merge(&b, &remote, &local)))
    );
}

// ---------------------------------------------------------- 其余降级顺序覆盖 ---

#[test]
fn m2_superset_wins_and_keeps_both_visible_parts() {
    // §3.3 M2：base ⊂ local ⊂ remote（逐字符包含）→ 采纳超集侧。
    let b = doc(vec![para("b1aaaaaa", "ab")]);
    let local = doc(vec![para("b1aaaaaa", "abc")]);
    let remote = doc(vec![para("b1aaaaaa", "abcde")]);
    let merged = auto(merge(&b, &local, &remote));
    assert_eq!(text_of(&merged), "abcde");
    assert_no_silent_loss(&b, &local, &remote, &merged);
    assert_eq!(
        canonical(&merged),
        canonical(&auto(merge(&b, &remote, &local)))
    );
}

#[test]
fn m2_does_not_fire_when_a_deletion_is_disguised_as_a_subset() {
    // M2 的包含关系必须链到 base：一侧删了字、一侧加了字 → 不能自动"取超集"，
    // 那等于替用户决定是否删除。
    let b = doc(vec![para("b1aaaaaa", "abcd")]);
    let local = doc(vec![para("b1aaaaaa", "ab")]); // 删掉了 cd
    let remote = doc(vec![para("b1aaaaaa", "abcdef")]); // 追加了 ef
    let out = merge(&b, &local, &remote);
    // local 的 "ab" 含于 remote 的 "abcdef"，但 local 的文本不含 base → M2 前提不成立。
    // M3 也不成立（"ab" 不是 "abcd" 的超集追加）。
    assert_eq!(conflict_ids(&out), vec!["b1aaaaaa".to_string()], "{out:?}");
}

#[test]
fn m4_checklist_text_and_checked_state_both_survive() {
    // §3.3 M4 / FT-CHK-02：一侧勾选、一侧改文本 → 两者都要保住，
    // 禁止出现"只剩一项勾选"的静默覆盖。
    let b = doc(vec![checklist("c1aaaaaa", "买牛奶", Some(false))]);
    let local = doc(vec![checklist("c1aaaaaa", "买牛奶", Some(true))]); // 只勾选
    let remote = doc(vec![checklist("c1aaaaaa", "买低脂牛奶", Some(false))]); // 只改文本
    let merged = auto(merge(&b, &local, &remote));
    let blk = &merged.content[0];
    assert_eq!(blk.plain_text(), "买低脂牛奶", "文本取改动侧");
    assert_eq!(blk.attrs.get("checked"), Some(&json!(true)), "勾选取改动侧");
    validate(&merged).unwrap();
    assert_no_silent_loss(&b, &local, &remote, &merged);
}

#[test]
fn m4_contradictory_checked_state_defaults_to_unchecked_but_keeps_evidence() {
    // §3.3 M4 末段："冲突则默认未勾选并保留另一状态"。
    let b = doc(vec![checklist("c2aaaaaa", "买牛奶", None)]);
    let local = doc(vec![checklist("c2aaaaaa", "买牛奶", Some(true))]);
    let remote = doc(vec![checklist("c2aaaaaa", "买牛奶", Some(false))]);
    let merged = auto(merge(&b, &local, &remote));
    assert_eq!(merged.content[0].attrs.get("checked"), Some(&json!(false)));
    assert!(
        merged.content[0].attrs.contains_key("conflict:checked"),
        "被丢弃的勾选状态必须留档，不能蒸发：{:?}",
        merged.content[0].attrs
    );
}

#[test]
fn attr_conflict_degrades_to_audit_not_content_conflict() {
    // §4 原则：属性冲突降级为"审计 + 提示"，只有内容冲突才升级成收件箱。
    let blk = |align: &str| {
        json!({
            "id": "p1aaaaaa", "type": "paragraph",
            "attrs": { "align": align }, "content": [{ "text": "同一句话" }]
        })
    };
    let b = doc(vec![blk("left")]);
    let out = merge(&b, &doc(vec![blk("center")]), &doc(vec![blk("right")]));
    let merged = auto(out);
    assert_eq!(text_of(&merged), "同一句话");
    assert!(
        merged.content[0].attrs.contains_key("conflict:align"),
        "被丢弃的属性值必须留档：{:?}",
        merged.content[0].attrs
    );
    assert_eq!(
        canonical(&merged),
        canonical(&auto(merge(
            &b,
            &doc(vec![blk("right")]),
            &doc(vec![blk("center")])
        ))),
        "属性取舍不能取决于角色"
    );
}

#[test]
fn new_blocks_from_both_sides_are_all_kept() {
    // §3.2 第 3 步：两侧各加新块 → 全部并入，不丢任何一侧。
    let local = doc(vec![
        para("b1aaaaaa", "第一段原文"),
        para("b2aaaaaa", "第二段原文"),
        para("n1aaaaaa", "本地新加"),
    ]);
    let remote = doc(vec![
        para("b1aaaaaa", "第一段原文"),
        para("b2aaaaaa", "第二段原文"),
        para("n2aaaaaa", "远端新加"),
    ]);
    let merged = auto(merge(&base(), &local, &remote));
    assert!(text_of(&merged).contains("本地新加"));
    assert!(text_of(&merged).contains("远端新加"));
    assert_eq!(merged.content.len(), 4);
    assert_no_silent_loss(&base(), &local, &remote, &merged);
}

#[test]
fn same_text_different_id_new_blocks_are_deduped_once() {
    // §3.2 第 3 步末：两台设备各加了一个"同文本不同 id"的新块 → 只保留一个。
    let local = doc(vec![
        para("b1aaaaaa", "第一段原文"),
        para("b2aaaaaa", "第二段原文"),
        para("laaaaaaaa", "顺手记一句"),
    ]);
    let remote = doc(vec![
        para("b1aaaaaa", "第一段原文"),
        para("b2aaaaaa", "第二段原文"),
        para("reaaaaaaaa", "顺手记一句"),
    ]);
    let merged = auto(merge(&base(), &local, &remote));
    let texts: Vec<String> = merged.content.iter().map(|x| x.plain_text()).collect();
    assert_eq!(
        texts.iter().filter(|t| *t == "顺手记一句").count(),
        1,
        "{texts:?}"
    );
    // 幸存者只由内容决定（最小 id），所以两台设备会选同一个。
    assert_eq!(
        canonical(&merged),
        canonical(&auto(merge(&base(), &remote, &local)))
    );
    assert_no_silent_loss(&base(), &local, &remote, &merged);
}

#[test]
fn empty_new_blocks_are_never_deduped() {
    // 两个"空段落"不是重复内容，它们是用户真的按了两次回车。
    let b = doc(vec![para("b1aaaaaa", "正文")]);
    let local = doc(vec![para("b1aaaaaa", "正文"), para("e1aaaaaa", "")]);
    let remote = doc(vec![para("b1aaaaaa", "正文"), para("e2aaaaaa", "")]);
    let merged = auto(merge(&b, &local, &remote));
    assert_eq!(merged.content.len(), 3, "空块必须都保留");
}

#[test]
fn order_change_on_one_side_is_respected() {
    // §3.2 第 4 步：一侧调顺序、另一侧没调 → 采纳调整后的顺序。
    let b = doc(vec![para("x1aaaaaa", "一"), para("x2aaaaaa", "二"), para("x3aaaaaa", "三")]);
    let local = doc(vec![para("x3aaaaaa", "三"), para("x1aaaaaa", "一"), para("x2aaaaaa", "二")]);
    let merged = auto(merge(&b, &local, &b.clone()));
    assert_eq!(block_ids(&merged), vec!["x3aaaaaa", "x1aaaaaa", "x2aaaaaa"]);
}

#[test]
fn contradictory_reorders_are_reported_as_conflict() {
    // §3.2 第 4 步："两侧都调整了顺序 → 顺序判为冲突"。
    let b = doc(vec![para("y1aaaaaa", "一"), para("y2aaaaaa", "二"), para("y3aaaaaa", "三")]);
    // 两侧都调序且方向矛盾，同时各改一个块的文本以躲过文档级收敛。
    let local = doc(vec![
        para("y3aaaaaa", "三·本地"),
        para("y1aaaaaa", "一"),
        para("y2aaaaaa", "二"),
    ]);
    let remote = doc(vec![
        para("y2aaaaaa", "二"),
        para("y3aaaaaa", "三"),
        para("y1aaaaaa", "一·远端"),
    ]);
    let out = merge(&b, &local, &remote);
    let ids = conflict_ids(&out);
    assert!(ids.len() >= 2, "矛盾的调序必须上报：{out:?}");
    for id in &ids {
        assert!(id.starts_with('y'), "冲突 id 应当是真实块 id：{id}");
    }
}

// ---------------------------------------------------------------- 版本闸门 ---

#[test]
fn doc_version_too_new_short_circuits_to_read_only() {
    // FWD-03 + I7：版本闸门在合并之前，防止"降级重写"。
    let mut future = json!({ "v": 99, "content": [para("b1aaaaaa", "新版客户端写的内容")] });
    future["content"][0]["id"] = json!("b1aaaaaa");
    assert!(!supports(99));
    assert_eq!(
        parse(&future.to_string()),
        Err(crate::RichError::UnsupportedVersion(99))
    );
    // 只读路径仍要能完整读出内容（否则用户看不到自己的笔记）。
    let ro = crate::parse_for_read(&future).unwrap();
    assert_eq!(ro.v, 99);
    assert!(canonical(&ro).contains("新版客户端写的内容"));
    // merge 必须拒绝，且给出的原因就是版本闸门。
    let mut too_new = base();
    too_new.v = 99;
    assert_eq!(
        merge(&base(), &too_new, &base()),
        MergeOutcome::ReadOnly {
            because: ReadOnlyReason::DocVersionTooNew(99)
        }
    );
    assert_eq!(
        merge(&base(), &base(), &too_new),
        MergeOutcome::ReadOnly {
            because: ReadOnlyReason::DocVersionTooNew(99)
        }
    );
    // 三方都能合并的最大版本 = DOC_FORMAT（闸门不误伤）。
    let mut current = base();
    current.v = DOC_FORMAT;
    assert!(!matches!(
        merge(&base(), &current, &base()),
        MergeOutcome::ReadOnly { .. }
    ));
}

// ------------------------------------------------------------------ 纯函数 ---

#[test]
fn merge_does_not_mutate_its_inputs() {
    let b = base();
    let l = doc(vec![para("b1aaaaaa", "本地改"), para("b2aaaaaa", "第二段原文")]);
    let r = doc(vec![para("b1aaaaaa", "第一段原文"), para("b2aaaaaa", "远端改")]);
    let (cb, cl, cr) = (canonical(&b), canonical(&l), canonical(&r));
    let _ = merge(&b, &l, &r);
    assert_eq!(
        (canonical(&b), canonical(&l), canonical(&r)),
        (cb, cl, cr),
        "merge 必须是纯函数"
    );
}

#[test]
fn prop_merge_is_deterministic() {
    // INV-15：同一三元组输入 → 同一 canonical JSON 输出（重复 50 次）。
    let b = base();
    let l = doc(vec![
        para("b1aaaaaa", "第一段原文"),
        para("b2aaaaaa", "第二段原文"),
        para("n1aaaaaa", "新"),
    ]);
    let r = doc(vec![para("b1aaaaaa", "改过的第一段"), para("b3aaaaaa", "远端独有")]);
    let first = merge(&b, &l, &r);
    for i in 0..50 {
        assert_eq!(merge(&b, &l, &r), first, "第 {i} 次结果不同 = 不确定");
    }
}

#[test]
fn malformed_documents_never_panic() {
    // 硬性要求 3：任意输入（含畸形文档）都要返回一个 MergeOutcome。
    let nasty: Vec<Document> = vec![
        Document::default(),
        doc(vec![para("z1aaaaaa", "")]),
        doc(vec![json!({ "id": "zzzzzzzz", "type": "completely:unknown", "attrs": { "a": [1, {"b": null}] } })]),
        raw(vec![
            json!({ "id": "t1aaaaaa", "type": "table" }),
            json!({ "id": "t3aaaaaa", "type": "tableCell" }), // 中间行被删：结构不合法
        ]),
        doc(vec![
            json!({ "id": "t1aaaaaa", "type": "table" }),
            json!({ "id": "t2aaaaaa", "type": "tableRow" }),
            json!({ "id": "t3aaaaaa", "type": "tableCell", "content": [{ "text": "格" }] }),
        ]),
        // 同 id 不同内容：连 `parse` 都进不来，只能直接构造（合并必须扛住）。
        raw(vec![para("d1aaaaaa", "甲"), para("d1aaaaaa", "乙")]),
        raw(vec![
            json!({ "id": "", "type": "paragraph", "content": [{ "text": "无 id" }] }),
            json!({ "type": "paragraph", "content": [{ "text": "也无 id" }] }),
        ]),
        raw(vec![json!({ "id": "n1aaaaaa", "type": "paragraph", "content": [{ "text": "带未知键", "future": [1, 2] }] })]),
    ];
    for a in &nasty {
        for b in &nasty {
            for c in &nasty {
                let out = merge(a, b, c);
                match out {
                    MergeOutcome::AutoMerged { doc, .. } => {
                        assert!(supports(doc.v));
                        validate(&doc)
                            .expect("AutoMerged 的产物必须过 validate（§3.4）");
                        assert_ne!(canonical(&doc), canonical(a));
                    }
                    MergeOutcome::Conflict {
                        conflicting_block_ids,
                    } => {
                        assert!(!conflicting_block_ids.is_empty(), "Conflict 必须指出是哪几块");
                    }
                    MergeOutcome::Converged | MergeOutcome::ReadOnly { .. } => {}
                }
            }
        }
    }
}

#[test]
fn merge_survives_docs_that_only_contain_unknown_and_empty_nodes() {
    let weird = doc(vec![
        json!({ "id": "u1aaaaaa", "type": "unknown:" }),
        json!({ "id": "u2aaaaaa", "type": "unknown:未来节点", "attrs": { "x": { "y": [1, 2] } } }),
        json!({ "id": "u3aaaaaa", "type": "paragraph", "content": [{ "text": "", "marks": [{ "kind": "unknown:sparkle", "attrs": { "w": 1 } }] }] }),
    ]);
    // 远端在保留全部未知节点的前提下新增一个更新的未知节点。
    let mut remote_blocks: Vec<Value> = weird
        .content
        .iter()
        .map(|b| serde_json::to_value(b).unwrap())
        .collect();
    remote_blocks.push(json!({ "id": "u4aaaaaa", "type": "brandNew", "attrs": { "keep": true } }));
    let remote = doc(remote_blocks);
    let merged = auto(merge(&weird, &weird.clone(), &remote));
    let c = canonical(&merged);
    assert!(c.contains("unknown:未来节点"), "未知节点不许被降级丢弃：{c}");
    assert!(c.contains("\"y\":[1,2]"), "未知节点的嵌套属性必须在：{c}");
    assert!(c.contains("unknown:brandNew"), "新增未知节点必须在：{c}");
    assert!(c.contains("\"unknown:sparkle\""), "未知 mark 必须在：{c}");
    validate(&merged).unwrap();
    assert_no_silent_loss(&weird, &weird.clone(), &remote, &merged);
}

// ---------------------------------------------------------------- 属性测试 ---

/// 确定性 PRNG（xorshift64*）：属性测试必须可复现，所以不用系统随机数。
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next_u64() % (n as u64)) as usize
        }
    }
    fn chance(&mut self, percent: u64) -> bool {
        self.next_u64() % 100 < percent
    }
}

const IDS: [&str; 7] = [
    "a1aaaaaa",
    "a2aaaaaa",
    "a3aaaaaa",
    "a4aaaaaa",
    "a5aaaaaa",
    "a6aaaaaa",
    "a7aaaaaa",
];
const TEXTS: [&str; 8] = [
    "",
    "甲",
    "甲乙",
    "甲乙丙",
    "hello",
    "hello world",
    "同步测试",
    "一段带 空格 的文字",
];
const TYPES: [&str; 6] = [
    "paragraph",
    "heading",
    "checklistItem",
    "codeBlock",
    "unknown:mermaid",
    "blockquote",
];
const MARKS: [&str; 4] = ["bold", "italic", "unknown:sparkle", "link"];

fn rand_block(rng: &mut Rng, id: &str) -> Value {
    let mut attrs = serde_json::Map::new();
    if rng.chance(40) {
        attrs.insert("align".into(), json!(TEXTS[rng.below(TEXTS.len())]));
    }
    if rng.chance(30) {
        attrs.insert("checked".into(), json!(rng.chance(50)));
    }
    if rng.chance(20) {
        attrs.insert("data-x".into(), json!({ "deep": rng.below(3) }));
    }
    let mut marks = Vec::new();
    for m in MARKS.iter().take(1 + rng.below(MARKS.len())) {
        if rng.chance(60) {
            marks.push(json!({ "kind": m, "attrs": { "z": rng.below(2) } }));
        }
    }
    json!({
        "id": id,
        "type": TYPES[rng.below(TYPES.len())],
        "attrs": Value::Object(attrs),
        "content": [{ "text": TEXTS[rng.below(TEXTS.len())], "marks": marks }]
    })
}

fn rand_doc(rng: &mut Rng) -> Document {
    let mut picked: Vec<&str> = IDS.to_vec();
    // 洗牌后截断：顺序与集合都随机，但每个 id 至多出现一次。
    for i in (1..picked.len()).rev() {
        let j = rng.below(i + 1);
        picked.swap(i, j);
    }
    let keep = rng.below(IDS.len() + 1);
    let blocks: Vec<Value> = picked[..keep].iter().map(|id| rand_block(rng, id)).collect();
    parse_from_value(&json!({ "v": DOC_FORMAT, "content": blocks }))
        .unwrap_or_else(|_| Document::default())
}

/// 从 base 派生一侧的编辑：改文本 / 改格式 / 改属性 / 纯追加 / 换类型 / 删块 / 加块 / 调序。
///
/// 全程操作**类型化**文档（而不是 JSON），免得测试自己被索引越界绊倒 —— 我们要测的是
/// merge 的健壮性，不是 serde_json 的 Index 行为。
fn edit(rng: &mut Rng, base: &Document) -> Document {
    let mut d = base.clone();
    for blk in d.content.iter_mut() {
        match rng.below(6) {
            0 => {} // 不动
            1 => {
                blk.content = vec![Inline { text: TEXTS[rng.below(TEXTS.len())].into(), marks: vec![] }];
            }
            2 => {
                let m = MarkKind::from_wire_name(MARKS[rng.below(MARKS.len())]);
                let mk = Mark { kind: m, attrs: Default::default() };
                if let Some(first) = blk.content.first_mut() {
                    first.marks = vec![mk];
                } else {
                    blk.content = vec![Inline { text: String::new(), marks: vec![mk] }];
                }
            }
            3 => {
                blk.attrs.insert("note".into(), json!(rng.below(3)));
            }
            4 => {
                // 纯追加（M3 的输入形态）。
                let extra = "追加";
                if let Some(last) = blk.content.last_mut() {
                    last.text.push_str(extra);
                } else {
                    blk.content = vec![Inline { text: extra.into(), marks: vec![] }];
                }
            }
            _ => {
                blk.type_ = BlockType::Paragraph;
                blk.content = vec![Inline { text: format!("{}·改", blk.plain_text()), marks: vec![] }];
            }
        }
    }
    // 删块。
    d.content.retain(|_| !rng.chance(15));
    // 加块（id 唯一，不与现有块撞）。
    if rng.chance(40) {
        let id = format!("new{}", rng.below(900) + 100);
        if !d.content.iter().any(|b| b.id == id) {
            d.content.push(Block {
                id,
                type_: BlockType::Paragraph,
                attrs: Default::default(),
                content: vec![Inline { text: TEXTS[rng.below(TEXTS.len())].into(), marks: vec![] }],
            });
        }
    }
    // 调序。
    if rng.chance(25) && d.content.len() > 1 {
        let i = rng.below(d.content.len());
        let j = rng.below(d.content.len());
        d.content.swap(i, j);
    }
    d.v = DOC_FORMAT;
    d
}

#[test]
fn prop_merge_never_panics_and_always_returns_validatable_output() {
    // CONFLICT-RESOLUTION §8 的测试钩子：任意三方输入 → 不 panic、输出必过 validate、
    // 且 merged 包含两侧所有非冲突块（assert_no_silent_loss）。
    let mut rng = Rng(0xC0FF_EE00_1234_5678);
    let mut kinds = [0u32; 4];
    for round in 0..800 {
        let b = rand_doc(&mut rng);
        let l = edit(&mut rng, &b);
        let r = edit(&mut rng, &b);
        // 10% 的版本扰动：任一侧 v 超前 → 只读闸门。
        let (l, r) = if rng.chance(10) {
            let mut l2 = l.clone();
            l2.v = rng.below(3) as u16 + DOC_FORMAT;
            (l2, r)
        } else {
            (l, r)
        };
        let out = merge(&b, &l, &r);
        assert_eq!(out, merge(&b, &l, &r), "第 {round} 轮输出不确定");
        kinds[match out {
            MergeOutcome::Converged => 0,
            MergeOutcome::AutoMerged { .. } => 1,
            MergeOutcome::Conflict { .. } => 2,
            MergeOutcome::ReadOnly { .. } => 3,
        }] += 1;
        match out {
            MergeOutcome::AutoMerged { doc, .. } => {
                validate(&doc).unwrap_or_else(|e| {
                    panic!("第 {round} 轮合并产物不合法（{e}）：{}", canonical(&doc))
                });
                assert_ne!(canonical(&doc), canonical(&b), "H(merged) 必须不同于 H(base)");
                assert_no_silent_loss(&b, &l, &r, &doc);
            }
            MergeOutcome::Conflict {
                conflicting_block_ids,
            } => {
                assert!(!conflicting_block_ids.is_empty());
            }
            MergeOutcome::ReadOnly { because } => {
                assert!(matches!(because, ReadOnlyReason::DocVersionTooNew(v) if !supports(v)));
            }
            MergeOutcome::Converged => {}
        }
    }
    // 生成器必须真的覆盖到多个分支（否则属性测试是假的）。
    assert!(kinds[1] > 50, "AutoMerged 分支未被覆盖：{kinds:?}");
    assert!(kinds[2] > 50, "Conflict 分支未被覆盖：{kinds:?}");
    assert!(kinds[3] > 0, "ReadOnly 分支未被覆盖：{kinds:?}");
}

#[test]
fn prop_swapping_sides_yields_the_same_merged_content() {
    // INV-15 的对称面：合并产物不能取决于"哪台设备是本地"。taken_* 会交换，
    // 但文档本体与冲突 id 集合必须一致 —— 否则两台设备各算一份、下一轮继续冲突。
    let mut rng = Rng(0xABCD_0123_4444_7777);
    let mut merged_cases = 0;
    for _ in 0..500 {
        let b = rand_doc(&mut rng);
        let l = edit(&mut rng, &b);
        let r = edit(&mut rng, &b);
        let a = merge(&b, &l, &r);
        let s = merge(&b, &r, &l);
        match (a, s) {
            (
                MergeOutcome::AutoMerged {
                    doc: d1,
                    taken_local: tl1,
                    taken_remote: tr1,
                },
                MergeOutcome::AutoMerged {
                    doc: d2,
                    taken_local: tl2,
                    taken_remote: tr2,
                },
            ) => {
                merged_cases += 1;
                assert_eq!(canonical(&d1), canonical(&d2));
                assert_eq!((tl1, tr1), (tr2, tl2), "交换角色后 taken 计数应镜像交换");
            }
            (
                MergeOutcome::Conflict {
                    conflicting_block_ids: mut c1,
                },
                MergeOutcome::Conflict {
                    conflicting_block_ids: mut c2,
                },
            ) => {
                c1.sort();
                c2.sort();
                assert_eq!(c1, c2, "冲突集合不能取决于角色");
            }
            (MergeOutcome::Converged, MergeOutcome::Converged) => {}
            (MergeOutcome::ReadOnly { .. }, MergeOutcome::ReadOnly { .. }) => {}
            (x, y) => panic!("交换角色后结果种类变了：\n{x:?}\n{y:?}"),
        }
    }
    assert!(merged_cases > 20, "AutoMerged 覆盖不足：{merged_cases}");
}

#[test]
fn prop_auto_merged_result_is_a_fixed_point_for_the_next_round() {
    // 收敛性：把合并结果当 base 再合并一次，必须立刻收敛（同步引擎要的"一轮解决"）。
    let mut rng = Rng(0x1234_5678_9ABC_DEF0);
    let mut cases = 0;
    for _ in 0..400 {
        let b = rand_doc(&mut rng);
        let l = edit(&mut rng, &b);
        let r = edit(&mut rng, &b);
        if let MergeOutcome::AutoMerged { doc, .. } = merge(&b, &l, &r) {
            cases += 1;
            // 结果当 base，两侧都不再改 → 收敛。
            assert_eq!(merge(&doc, &doc, &doc), MergeOutcome::Converged);
            // 结果当 base，一侧改 → 仍是 AutoMerged，不会凭空变冲突。
            let mut changed = doc.clone();
            changed.content.push(block("zzfinish", "又加了一段"));
            match merge(&doc, &changed, &doc) {
                MergeOutcome::AutoMerged { doc: m, .. } => {
                    assert!(m.content.iter().any(|x| x.id == "zzfinish"), "新加的一段必须进来");
                }
                other => panic!("单侧改动不该升级成冲突：{other:?}"),
            }
        }
    }
    assert!(cases > 50, "覆盖不足：{cases}");
}

#[test]
fn taken_counts_are_truthful_and_unknown_types_survive_merge() {
    let local = doc(vec![para("b1aaaaaa", "本地改一"), para("b2aaaaaa", "本地改二")]);
    match merge(&base(), &local, &base()) {
        MergeOutcome::AutoMerged {
            taken_local,
            taken_remote,
            ..
        } => assert_eq!((taken_local, taken_remote), (2, 0)),
        other => panic!("{other:?}"),
    }
    // 未知节点在合并里是普通块：不特殊处理、不被"清洗"。
    let b = doc(vec![para("k1aaaaaa", "正文")]);
    let remote = doc(vec![
        para("k1aaaaaa", "正文"),
        json!({ "id": "k2aaaaaa", "type": "mermaid", "attrs": { "code": "A-->B" } }),
    ]);
    let merged = auto(merge(&b, &b.clone(), &remote));
    assert!(matches!(&merged.content[1].type_, BlockType::Unknown(n) if n == "mermaid"));
    assert_eq!(merged.content[1].attrs.get("code"), Some(&json!("A-->B")));
    assert_eq!(ids_of(&merged), ids_of(&remote));
}
