//! 检索双路径 + snippet 转义（ADR-0015 / DATA-MODEL §7.2）。
//!
//! 这些断言防的是**真实回归**：trigram 对 <3 字符查询静默返回 0 行（实测），
//! 若把两字中文交给 `MATCH`，用户会看到"没有结果"而数据其实在库里。
mod common;

use common::*;
use notera_store::{SearchPath, SearchQuery};

fn seed(store: &notera_store::Store, folder: &notera_core::EntityId) {
    for (i, body) in [
        "数据同步机制的设计说明",
        "冲突解决策略需要重新评估",
        "附件上传走独立队列",
        "Pure English note about synchronization",
        "两字命中：同步同步同步",
    ]
    .iter()
    .enumerate()
    {
        store.create_note(folder, doc_heading(&format!("标题 {i}"), body)).expect("create_note");
    }
}

#[test]
fn two_char_cjk_query_must_use_like_fallback_and_return_rows() {
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);
    seed(&store, &folder);

    let hits = store.search(&SearchQuery::new("同步")).unwrap();
    assert!(!hits.is_empty(), "两字中文查询绝不能返回 0 行（这是实测出的静默错误）");
    assert!(
        hits.iter().all(|h| h.path_used == SearchPath::LikeFallback),
        "两字必须全部走 LIKE：{:?}",
        hits.iter().map(|h| (h.note_id.to_string(), h.path_used)).collect::<Vec<_>>()
    );
    // "同步" 出现在第 1 条与第 5 条
    assert_eq!(hits.len(), 2, "命中集合必须是 LIKE 语义的 2 条：{hits:?}");

    // 直接证明"若走 MATCH 就会漏"：同一查询在 MATCH 下确实 0 行
    let conn = rusqlite::Connection::open(fx.db_file()).unwrap();
    let via_match: i64 = conn
        .query_row("SELECT COUNT(*) FROM notes_fts WHERE notes_fts MATCH '\"同步\"'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(via_match, 0, "trigram 对 2 字符查询不产生 token —— 正是必须分流的原因");
}

#[test]
fn three_char_cjk_query_must_use_fts_trigram() {
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);
    seed(&store, &folder);

    assert!(store.search(&SearchQuery::new("同步问")).unwrap().is_empty(), "库里没有该词，FTS 不该乱命中");

    let hits = store.search(&SearchQuery::new("步机制")).unwrap();
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].path_used, SearchPath::FtsTrigram, "三字必须走 FTS5 trigram");
    assert!(hits[0].snippet_html.contains("<mark>步机制</mark>"), "{}", hits[0].snippet_html);

    // 边界按码点：2 字 LIKE / 3 字 FTS（中英一致）
    assert_eq!(store.search(&SearchQuery::new("附件")).unwrap()[0].path_used, SearchPath::LikeFallback);
    assert_eq!(store.search(&SearchQuery::new("附件上")).unwrap()[0].path_used, SearchPath::FtsTrigram);
    assert_eq!(store.search(&SearchQuery::new("同步")).unwrap()[0].path_used, SearchPath::LikeFallback);
    assert_eq!(store.search(&SearchQuery::new("sy")).unwrap()[0].path_used, SearchPath::LikeFallback);
    assert_eq!(store.search(&SearchQuery::new("syn")).unwrap()[0].path_used, SearchPath::FtsTrigram);
}

#[test]
fn mixed_length_query_intersects_per_segment() {
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);
    seed(&store, &folder);

    // "同步"(LIKE) + "机制"(LIKE) 的交集只有第 1 条
    let hits = store.search(&SearchQuery::new("同步 机制")).unwrap();
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert!(hits[0].snippet_html.contains("<mark>"));

    // 长段走 FTS、短段走 LIKE，取交集
    assert_eq!(store.search(&SearchQuery::new("数据同步机制 独立")).unwrap().len(), 0, "两段无共同笔记");
    assert_eq!(store.search(&SearchQuery::new("数据同步机制")).unwrap().len(), 1);
    assert_eq!(store.search(&SearchQuery::new("同步机制 设计说明")).unwrap().len(), 1);
}

#[test]
fn snippet_is_escaped_before_marks_are_inserted() {
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);
    let evil = doc_heading(
        "危险内容 <script>alert(1)</script>",
        "正文里也有 <img src=x onerror=alert(2)> 与引号以及 & 符号，然后是 <b>同步协议</b> 结尾",
    );
    store.create_note(&folder, evil).unwrap();

    let hits = store.search(&SearchQuery::new("同步协议")).unwrap();
    assert_eq!(hits.len(), 1, "{hits:?}");
    let s = &hits[0].snippet_html;
    assert!(s.contains("<mark>同步协议</mark>"), "命中处必须高亮：{s}");
    // UI 直接渲染这段 HTML —— 唯一的例外是我们自己插入的 <mark>，别的裸标签一个都不许出现
    for forbidden in ["<script", "<img", "<b>", "<a ", "</script"] {
        assert!(!s.contains(forbidden), "snippet 泄漏裸标签：{s}");
    }
    let stripped = s.replace("<mark>", "").replace("</mark>", "");
    assert!(!stripped.contains('<'), "除 <mark> 外不许有任何裸 '<'（= 未转义）：{stripped}");
    assert!(!stripped.contains('>'), "除 </mark> 外不许有任何裸 '>'：{stripped}");
    assert!(s.contains("&lt;script&gt;") || s.contains("&lt;img"), "应看到转义后的实体：{s}");
    assert!(s.contains("&lt;/b&gt;"), "闭合标签同样要转义：{s}");
    assert!(s.contains("&amp;"), "& 必须转义成 &amp;：{s}");

    // 两字路径共用同一个转义函数
    let hits = store.search(&SearchQuery::new("同步")).unwrap();
    assert_eq!(hits[0].path_used, SearchPath::LikeFallback);
    assert!(!hits[0].snippet_html.contains("<script"), "{}", hits[0].snippet_html);
    // 标题命中时片段来自标题
    let hits = store.search(&SearchQuery::new("危险内容")).unwrap();
    assert!(hits[0].snippet_html.contains("危险内容"), "{}", hits[0].snippet_html);
}

#[test]
fn like_metacharacters_are_matched_literally() {
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);
    store.create_note(&folder, doc_text("百分之百 100% 覆盖 下划线 _下 与 感叹号 ! 标记")).unwrap();
    store.create_note(&folder, doc_text("完全不同的一句话")).unwrap();

    // 每个查询都含 LIKE 元字符：必须按字面量匹配，且绝不抛 SQL 错
    let cases: [(&str, bool); 5] = [("100%", true), ("_下", true), ("!", true), ("%%", true), ("[x]", false)];
    for (q, expect) in cases {
        let hits = store.search(&SearchQuery::new(q)).unwrap();
        assert_eq!(!hits.is_empty(), expect, "字面量匹配 {q:?} 得到 {hits:?}");
        for h in &hits {
            assert_eq!(h.path_used, SearchPath::LikeFallback, "短查询必须走 LIKE：{h:?}");
        }
    }
    assert!(store.verify().is_empty(), "{:?}", store.verify());
}

#[test]
fn soft_deleted_notes_are_not_searchable_and_purge_shrinks_fts() {
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);
    let n = store.create_note(&folder, doc_text("回收站里的同步说明")).unwrap();
    assert_eq!(store.search(&SearchQuery::new("回收站")).unwrap().len(), 1);
    store.delete_note(&n.id).unwrap();
    assert!(store.search(&SearchQuery::new("回收站")).unwrap().is_empty(), "回收站内容不进搜索结果");
    store.restore_note(&n.id).unwrap();
    assert_eq!(store.search(&SearchQuery::new("回收站")).unwrap().len(), 1);
    store.purge_note(&n.id).unwrap();
    assert!(store.search(&SearchQuery::new("回收站")).unwrap().is_empty());
    let st = store.stats().unwrap();
    assert_eq!(st.notes, 0);
    assert_eq!(st.fts_rows, 0, "purge 后 FTS 行数必须同步回落（I5）");
    assert!(store.verify().is_empty(), "{:?}", store.verify());
}

#[test]
fn limit_and_empty_query_behave() {
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);
    seed(&store, &folder);
    assert!(store.search(&SearchQuery::new("   ")).unwrap().is_empty());
    let all = store.search(&SearchQuery::new("同步")).unwrap();
    let capped = store.search(&SearchQuery { text: "同步".into(), limit: 1 }).unwrap();
    assert_eq!(capped.len(), 1);
    assert!(all.len() > capped.len());
    assert!(store.search(&SearchQuery::new("查无此词")).unwrap().is_empty());
}
