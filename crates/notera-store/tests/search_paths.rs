//! 检索双路径 + snippet 转义（ADR-0015 / DATA-MODEL §7.2）。
//!
//! 这些断言防的是**真实回归**：trigram 对 <3 字符查询静默返回 0 行（实测），
//! 若把两字中文交给 `MATCH`，用户会看到"没有结果"而数据其实在库里。
mod common;

use common::*;
use notera_store::{MatchKind, SearchPath, SearchQuery};

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
        store
            .create_note(folder, doc_heading(&format!("标题 {i}"), body))
            .expect("create_note");
    }
}

#[test]
fn two_char_cjk_query_must_use_like_fallback_and_return_rows() {
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);
    seed(&store, &folder);

    let hits = store.search(&SearchQuery::new("同步")).unwrap();
    assert!(
        !hits.is_empty(),
        "两字中文查询绝不能返回 0 行（这是实测出的静默错误）"
    );
    assert!(
        hits.iter().all(|h| h.path_used == SearchPath::LikeFallback),
        "两字必须全部走 LIKE：{:?}",
        hits.iter()
            .map(|h| (h.note_id.to_string(), h.path_used))
            .collect::<Vec<_>>()
    );
    // "同步" 出现在第 1 条与第 5 条
    assert_eq!(hits.len(), 2, "命中集合必须是 LIKE 语义的 2 条：{hits:?}");

    // 直接证明"若走 MATCH 就会漏"：同一查询在 MATCH 下确实 0 行
    let conn = rusqlite::Connection::open(fx.db_file()).unwrap();
    let via_match: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM notes_fts WHERE notes_fts MATCH '\"同步\"'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        via_match, 0,
        "trigram 对 2 字符查询不产生 token —— 正是必须分流的原因"
    );
}

#[test]
fn three_char_cjk_query_must_use_fts_trigram() {
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);
    seed(&store, &folder);

    assert!(
        store
            .search(&SearchQuery::new("同步问"))
            .unwrap()
            .is_empty(),
        "库里没有该词，FTS 不该乱命中"
    );

    let hits = store.search(&SearchQuery::new("步机制")).unwrap();
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(
        hits[0].path_used,
        SearchPath::FtsTrigram,
        "三字必须走 FTS5 trigram"
    );
    assert!(
        hits[0].snippet_html.contains("<mark>步机制</mark>"),
        "{}",
        hits[0].snippet_html
    );

    // 边界按码点：2 字 LIKE / 3 字 FTS（中英一致）
    assert_eq!(
        store.search(&SearchQuery::new("附件")).unwrap()[0].path_used,
        SearchPath::LikeFallback
    );
    assert_eq!(
        store.search(&SearchQuery::new("附件上")).unwrap()[0].path_used,
        SearchPath::FtsTrigram
    );
    assert_eq!(
        store.search(&SearchQuery::new("同步")).unwrap()[0].path_used,
        SearchPath::LikeFallback
    );
    assert_eq!(
        store.search(&SearchQuery::new("sy")).unwrap()[0].path_used,
        SearchPath::LikeFallback
    );
    assert_eq!(
        store.search(&SearchQuery::new("syn")).unwrap()[0].path_used,
        SearchPath::FtsTrigram
    );
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
    assert_eq!(
        store
            .search(&SearchQuery::new("数据同步机制 独立"))
            .unwrap()
            .len(),
        0,
        "两段无共同笔记"
    );
    assert_eq!(
        store
            .search(&SearchQuery::new("数据同步机制"))
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        store
            .search(&SearchQuery::new("同步机制 设计说明"))
            .unwrap()
            .len(),
        1
    );
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
    assert!(
        !stripped.contains('<'),
        "除 <mark> 外不许有任何裸 '<'（= 未转义）：{stripped}"
    );
    assert!(
        !stripped.contains('>'),
        "除 </mark> 外不许有任何裸 '>'：{stripped}"
    );
    assert!(
        s.contains("&gt;") && s.contains("&lt;b&gt;"),
        "应看到转义后的实体：{s}"
    );
    assert!(s.contains("&amp;"), "& 必须转义成 &amp;：{s}");
    assert!(s.starts_with('…'), "片段被截断时要带省略号：{s}");

    // 两字路径共用同一个转义函数
    let hits = store.search(&SearchQuery::new("同步")).unwrap();
    assert_eq!(hits[0].path_used, SearchPath::LikeFallback);
    assert!(
        !hits[0].snippet_html.contains("<script"),
        "{}",
        hits[0].snippet_html
    );
    // 标题命中时片段来自标题
    let hits = store.search(&SearchQuery::new("危险内容")).unwrap();
    assert!(
        hits[0].snippet_html.contains("危险内容"),
        "{}",
        hits[0].snippet_html
    );
}

#[test]
fn like_metacharacters_are_matched_literally() {
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);
    store
        .create_note(
            &folder,
            doc_text("百分之百 100% 覆盖 下划线 _下 与 感叹号 ! 标记"),
        )
        .unwrap();
    store
        .create_note(&folder, doc_text("双写百分号 %% 转义 与 方括号 [x] 的笔记"))
        .unwrap();
    store
        .create_note(&folder, doc_text("完全不同的一句话"))
        .unwrap();

    // 每个查询都含 LIKE/FTS 元字符：必须按字面量匹配，且绝不抛 SQL 错。
    // 路径仍按码点数分流：≤2 字 LIKE、≥3 字 FTS。
    let cases: [(&str, bool); 6] = [
        ("100%", true),
        ("_下", true),
        ("!", true),
        ("%%", true),
        ("[x]", true),
        ("!%", false),
    ];
    for (q, expect) in cases {
        let hits = store.search(&SearchQuery::new(q)).unwrap();
        assert_eq!(!hits.is_empty(), expect, "字面量匹配 {q:?} 得到 {hits:?}");
        let want = if q.chars().count() >= 3 {
            SearchPath::FtsTrigram
        } else {
            SearchPath::LikeFallback
        };
        for h in &hits {
            assert_eq!(h.path_used, want, "路径选择错误：{q:?} → {h:?}");
        }
    }
    assert!(store.verify().is_empty(), "{:?}", store.verify());
}

#[test]
fn soft_deleted_notes_are_not_searchable_and_purge_shrinks_fts() {
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);
    let n = store
        .create_note(&folder, doc_text("回收站里的同步说明"))
        .unwrap();
    assert_eq!(store.search(&SearchQuery::new("回收站")).unwrap().len(), 1);
    store.delete_note(&n.id).unwrap();
    assert!(
        store
            .search(&SearchQuery::new("回收站"))
            .unwrap()
            .is_empty(),
        "回收站内容不进搜索结果"
    );
    store.restore_note(&n.id).unwrap();
    assert_eq!(store.search(&SearchQuery::new("回收站")).unwrap().len(), 1);
    store.purge_note(&n.id).unwrap();
    assert!(store
        .search(&SearchQuery::new("回收站"))
        .unwrap()
        .is_empty());
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
    let capped = store
        .search(&SearchQuery {
            text: "同步".into(),
            limit: 1,
        })
        .unwrap();
    assert_eq!(capped.len(), 1);
    assert!(all.len() > capped.len());
    assert!(store
        .search(&SearchQuery::new("查无此词"))
        .unwrap()
        .is_empty());
}

// ------------------------------------------------------------ 模糊档（P4 / 用户口径「精准 + 模糊共同搜」）---

/// 精准档要的是**连着出现**；模糊档要的是**这段的三字串都在同一条笔记里**，允许中间隔着别的话。
/// 用户打「同步协议」却忘了原文写作「同步协调…下一步协议」—— 老规则一句都不回，
/// 那是"没有结果"，不是"没有"。
#[test]
fn fuzzy_tier_returns_the_spaced_out_hit_exact_missed() {
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);
    store
        .create_note(
            &folder,
            doc_text("先说同步协调的事，之后再谈下一步协议的附件"),
        )
        .unwrap();

    let hits = store.search(&SearchQuery::new("同步协议")).unwrap();
    assert_eq!(hits.len(), 1, "模糊档要把这条捞回来：{hits:?}");
    assert_eq!(hits[0].match_kind, MatchKind::Fuzzy);
    assert_eq!(hits[0].path_used, SearchPath::FtsTrigram);
}

/// 模糊不是"沾边就算"：只命中两段三字串里的一段，仍然不该出现。
/// 没有这条，模糊档会退化成"含'同步协'的都算"，结果列表就是噪音。
#[test]
fn fuzzy_tier_still_requires_every_trigram_of_the_segment() {
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);
    // 只有「同步协」，没有「步协议」
    store
        .create_note(&folder, doc_text("今年开始同步协调，明年再说"))
        .unwrap();
    assert!(
        store
            .search(&SearchQuery::new("同步协议"))
            .unwrap()
            .is_empty(),
        "缺一截三字串就不该进模糊档"
    );
}

/// 两档**同时**跑，但精准的排在前面（用户那句"共同的去搜索"）。
#[test]
fn exact_hits_rank_above_fuzzy_hits() {
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);
    store
        .create_note(
            &folder,
            doc_text("先说同步协调的事，之后再谈下一步协议的附件"),
        )
        .unwrap();
    let exact = store
        .create_note(&folder, doc_text("同步协议的实现细节"))
        .unwrap();

    let hits = store.search(&SearchQuery::new("同步协议")).unwrap();
    assert_eq!(hits.len(), 2, "{hits:?}");
    assert_eq!(hits[0].note_id, exact.id, "精准那条必须在第一：{hits:?}");
    assert_eq!(hits[0].match_kind, MatchKind::Exact);
    assert_eq!(hits[1].match_kind, MatchKind::Fuzzy);
}

/// ≤2 字的段没有 trigram token ⇒ 它自己**不可能**被放宽；但同一个查询里的长段照常进模糊档。
/// 这条判的是混合长度查询的组合规则：**每一段都要满足**，短段按精准、长段按放宽。
#[test]
fn short_segment_has_no_fuzzy_tier_but_does_not_poison_the_long_one() {
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);
    // 「同步协」「步协议」都在，但不连着；「说明」连着出现；这条**没有**「附件」。
    store
        .create_note(
            &folder,
            doc_text("先说同步协调的事，之后再谈下一步协议的说明"),
        )
        .unwrap();

    // 短段「附件」根本不在 ⇒ 两段不许各自为政，整条查询必须空
    assert!(
        store
            .search(&SearchQuery::new("同步协议 附件"))
            .unwrap()
            .is_empty(),
        "短段没命中时不许靠长段放宽凑结果"
    );

    // 短段「说明」在 ⇒ 长段按三字串放宽，这一条进模糊档
    let hits = store.search(&SearchQuery::new("同步协议 说明")).unwrap();
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].match_kind, MatchKind::Fuzzy);
}

// ------------------------------------------------------------ 交集完整性（「匹配所有关键字」）---

/// 老写法是"每段各取 `limit*4` 条再取交集"：某一段命中太多时，**同时含两个词的那条会被窗口
/// 截掉**，用户得到的是一句"没有结果"。这条测试把窗口调到最小、并把那篇笔记刻意排成最旧，
/// 于是老实现必然回 0 条。
#[test]
fn intersection_is_not_truncated_by_the_per_segment_window() {
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);
    let both = store
        .create_note(&folder, doc_text("同步机制的两段式说明"))
        .unwrap();
    // 六条只含「同步」的新笔记：LIKE 档按 updated_at DESC 取，窗口 limit*4 = 4 会把最旧那条挤掉
    for i in 0..6 {
        store
            .create_note(&folder, doc_text(&format!("第 {i} 次同步的会议记录")))
            .unwrap();
    }
    let conn = rusqlite::Connection::open(fx.db_file()).unwrap();
    conn.execute(
        "UPDATE notes SET updated_at = '2000-01-01T00:00:00+00:00' WHERE id = ?1",
        [both.id.to_string()],
    )
    .unwrap();
    drop(conn);

    let hits = store
        .search(&SearchQuery {
            text: "同步 机制".into(),
            limit: 1,
        })
        .unwrap();
    assert_eq!(hits.len(), 1, "两个词都在的那条必须回来：{hits:?}");
    assert_eq!(hits[0].note_id, both.id, "{hits:?}");
}

/// 「还有 N 条」那一次 count（§3.3 的后一半，2026-10-09 用户拍板）。
///
/// 三条差分，缺一条就抓不住对应的坏法：
///  · 上限**够大**时回的是**真数**（夹具里恰好有两篇含「同步」，写死这条，不由被测方算出来）；
///  · 上限**小于真数**时回的就是上限 —— 界面据此说"还有 N 条**以上**"，不假装精确；
///  · 两档**不许重**：既被精准档命中、又在模糊档沾边的那些只算一次（把两档条数直接相加的实现会红）。
#[test]
fn search_total_counts_the_union_caps_the_cost_and_never_double_counts() {
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);
    seed(&store, &folder);
    // 夹具里含「同步」的恰好两篇（第五篇标题+正文各一次算同一篇；英文那篇不算）
    assert_eq!(
        store.search(&SearchQuery::new("同步")).unwrap().len(),
        2,
        "夹具变了：先修这条期望值，别让它跟着被测方算"
    );
    assert_eq!(
        store.search_total("同步", 100).unwrap(),
        2,
        "上限够大时必须是真数"
    );
    assert_eq!(
        store.search_total("同步", 1).unwrap(),
        1,
        "上限小于真数时就回上限"
    );
    assert_eq!(store.search_total("   ", 100).unwrap(), 0, "空查询 0 条");
    assert_eq!(
        store.search_total("查无此词呀", 100).unwrap(),
        0,
        "没有就是 0"
    );

    // 两档重叠：这一篇被精准档命中，而模糊档的放宽集里也有它 ⇒ 只能算一次。
    let careful = store
        .create_note(&folder, doc_heading("跨档", "数据同步协议的边界条件"))
        .unwrap();
    let after_exact = store.search_total("数据同步协议", 100).unwrap();
    assert!(
        store
            .search(&SearchQuery::new("数据同步协议"))
            .unwrap()
            .iter()
            .any(|h| h.note_id == careful.id),
        "前置：这一篇要被搜到"
    );
    // 再造一条**只**进模糊档的（三段三字串都在、但没连着）—— 它也要算进总数，且只算一次
    let spaced = store
        .create_note(
            &folder,
            doc_heading(
                "只进模糊",
                "先做数据同步的预演，同步协调另开一条线，下一步协议的边界还没定",
            ),
        )
        .unwrap();
    let both = store.search_total("数据同步协议", 100).unwrap();
    assert_eq!(
        both,
        after_exact + 1,
        "只进模糊档的那一篇没算进来（或算重了）：{both} vs {after_exact}"
    );
    assert!(
        store
            .search(&SearchQuery::new("数据同步协议"))
            .unwrap()
            .iter()
            .any(|h| h.note_id == spaced.id),
        "前置：那条隔开的笔记要被模糊档捞到"
    );
    // 上限顶住的读数形状：界面上那句"以上"就是从这个关系来的
    assert_eq!(
        store.search_total("数据同步协议", 1).unwrap(),
        1,
        "上限顶住时回上限"
    );
}
