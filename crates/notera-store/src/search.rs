//! 检索双路径 × 两档（ADR-0015 / DATA-MODEL §7.2）—— 实测决定的硬规则。
//!
//! ```text
//! 查询字符数 >= 3 → FTS5 trigram MATCH（快，带高亮）
//! 查询字符数 <= 2 → notes.plain_text LIKE + COLLATE NOCASE（慢一个数量级但正确）
//! 混合长查询      → 按空白切分，逐段独立选路后取交集
//!
//! 精准档 Exact = 每一段都**连着**出现
//! 模糊档 Fuzzy = 每一段的**每个三字串**都在同一条笔记里（允许中间隔话）；<3 字无 trigram ⇒ 不放宽
//! ```
//!
//! 为什么不是"全交给 MATCH"：trigram 分词对 <3 字符查询**不产生任何 token**，
//! `MATCH` 静默返回 0 行且无错误信号（实测），用户会以为笔记不存在。
//! 因此 [`SearchHit::path_used`] 是必填字段，测试直接断言它。
//!
//! 为什么交集只算 id：每段各带窗口的老写法会把"两个词都在"的那条截掉（真回归，见
//! `tests/search_paths.rs::intersection_is_not_truncated_by_the_per_segment_window`）。
//! 正文因此推迟到最后那一页才取。
//!
//! `notes_fts` 是 external-content 表且**无触发器**：索引行由 `store` 在与权威表同事务里
//! 用 `('delete', …)` + `(rowid, title, plain_text)` 维护（I5）。
//!
//! snippet 由本模块自行生成：**先转义再插 `<mark>`**（UI 直接渲染该 HTML）。

use crate::error::StoreError;
use crate::types::{MatchKind, SearchHit, SearchPath};
use notera_core::EntityId;
use rusqlite::Connection;
use std::collections::{HashMap, HashSet};

/// 短查询阈值：`>= 3` 走 FTS，`<= 2` 走 LIKE（ADR-0015）。
pub(crate) const FTS_MIN_CHARS: usize = 3;
/// snippet 窗口（字符）。
const SNIPPET_LEAD: usize = 40;
const SNIPPET_TAIL: usize = 120;
const DEFAULT_LIMIT: u32 = 50;
/// 「还有 N 条」那一次 count 的上界（见 `run_total`）：到顶了就说"以上"，不假装精确。
/// 对外可见：host 要用它把"没给 cap"归一成同一个数，别在两处各写一个 400。
pub const DEFAULT_TOTAL_CAP: u32 = 400;

pub(crate) fn char_len(s: &str) -> usize {
    s.chars().count()
}

pub(crate) fn path_for(query_chars: usize) -> SearchPath {
    if query_chars >= FTS_MIN_CHARS {
        SearchPath::FtsTrigram
    } else {
        SearchPath::LikeFallback
    }
}

/// LIKE 的通配符按字面量处理（`ESCAPE '!'`）：`!`/`%`/`_` 都要转义。
pub(crate) fn like_pattern(needle: &str) -> String {
    let mut out = String::with_capacity(needle.len() + 8);
    out.push('%');
    for c in needle.chars() {
        if matches!(c, '!' | '%' | '_') {
            out.push('!');
        }
        out.push(c);
    }
    out.push('%');
    out
}

/// 把一个词包成 FTS5 短语（内部双引号加倍），避免用户输入被当成语法。
pub(crate) fn fts_phrase(needle: &str) -> String {
    let mut out = String::with_capacity(needle.len() + 2);
    out.push('"');
    for c in needle.chars() {
        if c == '"' {
            out.push('"');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// HTML 转义（snippet 的唯一防线）。
pub(crate) fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// 大小写无关地定位 `needle` 在 `hay` 中的**字符**区间（不依赖字节索引，CJK 安全）。
fn find_ci(hay: &str, needle: &str) -> Option<(usize, usize)> {
    let hc: Vec<char> = hay.chars().collect();
    let nc: Vec<char> = needle.chars().collect();
    if nc.is_empty() || hc.len() < nc.len() {
        return None;
    }
    'outer: for start in 0..=(hc.len() - nc.len()) {
        for (i, n) in nc.iter().enumerate() {
            if !hc[start + i].eq_ignore_ascii_case(n) {
                continue 'outer;
            }
        }
        return Some((start, start + nc.len()));
    }
    None
}

/// 生成片段：截取窗口 → 三段各自转义 → 只在匹配段外包 `<mark>`。
/// 因此返回值里**不可能**出现来自数据的裸标签。
pub(crate) fn snippet_html(hay: &str, needle: &str) -> String {
    let hay_chars: Vec<char> = hay.chars().collect();
    let (ms, me) = match find_ci(hay, needle) {
        Some(r) => r,
        None => {
            // 未命中（例如只在另一列命中）：退化为开头片段，仍不加 mark。
            let take: String = hay_chars.iter().take(SNIPPET_LEAD + SNIPPET_TAIL).collect();
            let truncated = hay_chars.len() > SNIPPET_LEAD + SNIPPET_TAIL;
            return format!(
                "{}{}",
                html_escape(take.trim()),
                if truncated { "…" } else { "" }
            );
        }
    };
    let start = ms.saturating_sub(SNIPPET_LEAD);
    let end = (me + SNIPPET_TAIL).min(hay_chars.len());
    let before: String = hay_chars[start..ms].iter().collect();
    let hit: String = hay_chars[ms..me].iter().collect();
    let after: String = hay_chars[me..end].iter().collect();
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    out.push_str(&html_escape(before.trim_start()));
    out.push_str("<mark>");
    out.push_str(&html_escape(&hit));
    out.push_str("</mark>");
    out.push_str(&html_escape(after.trim_end()));
    if end < hay_chars.len() {
        out.push('…');
    }
    out
}

/// 一档捞回来的一行。正文不在这儿 —— 留到最后那一页才取。
struct Row {
    id: EntityId,
    score: f64,
}

/// 段内三字串（去重）。不足三字 ⇒ 空，调用方据此**不放宽**（trigram 档没有这些 token）。
pub(crate) fn trigrams(seg: &str) -> Vec<String> {
    let cs: Vec<char> = seg.chars().collect();
    if cs.len() < 3 {
        return Vec::new();
    }
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for w in cs.windows(3) {
        let t: String = w.iter().collect();
        if seen.insert(t.clone()) {
            out.push(t);
        }
    }
    out
}

/// 模糊档的 MATCH 表达式：这一段的**每一个**三字串都要在同一条笔记里，但不必连着。
/// 于是「同步协议」能捞回写作「同步协…步协议」的那条；只含「同步协」的一条进不来 ——
/// 放宽到"沾边就算"，结果列表就是噪音。
pub(crate) fn trigram_and(seg: &str) -> String {
    let tris = trigrams(seg);
    if tris.is_empty() {
        return fts_phrase(seg);
    }
    tris.iter()
        .map(|t| fts_phrase(t))
        .collect::<Vec<_>>()
        .join(" AND ")
}

fn ph(n: usize) -> String {
    format!("?{n}")
}

/// 造**一档**的那一条 SQL：所有词段的 AND 下推进这一句，`LIMIT` 因此是安全的。
///
/// 两个方向都踩过：
///  · 老写法每段各取 `limit*4` 条再在 Rust 里交集 ⇒ 某一段命中太多时，"两个词都在"的那条被
///    窗口截掉，用户看到"没有结果"而数据其实在库里（真回归，见
///    `tests/search_paths.rs::intersection_is_not_truncated_by_the_per_segment_window`）。
///  · 反过来"为了正确就不限条数"也不成立：5000 篇的库上实测常用词 62–149 ms，那是把正确性
///    换成了慢（读数在 `tests/search_latency.rs`）。
/// 两头都要的写法只有把交集交给 SQL。
///
/// ≥3 字的段用 `MATCH`（短语 = 精准；三字串 AND = 放宽）；≤2 字的段 trigram 档没有 token，
/// 只能按字面 `LIKE` 挂在同一句 WHERE 里。
///
/// 单独抽成纯函数是为了让它能被单测钉住：时间预算抓不住 2 倍级的慢化（实测把 `LIMIT` 放大
/// 400 倍 ⇒ p95 20.9 → 37.4 ms，门禁照绿），而"每段各取一批再交集"那种回归正是 2 倍级 ——
/// 抓得住它的只有这句 SQL 自己的形状。
///
/// 返回 `None` = 这一档无事可做（全短段的模糊档：没有 token 可放宽，别白扫一遍）。
pub(crate) fn tier_sql(
    segments: &[&str],
    fuzzy: bool,
    exclude: &[&str],
    limit: usize,
) -> Option<(String, Vec<String>)> {
    let mut fts_parts: Vec<String> = Vec::new();
    let mut likes: Vec<String> = Vec::new();
    for seg in segments {
        if char_len(seg) >= FTS_MIN_CHARS {
            let expr = if fuzzy {
                trigram_and(seg)
            } else {
                fts_phrase(seg)
            };
            fts_parts.push(format!("({expr})"));
        } else {
            likes.push(like_pattern(seg));
        }
    }
    // 全是短段 ⇒ 没有可放宽的东西：模糊档直接空，不许白扫一遍。
    if fuzzy && fts_parts.is_empty() {
        return None;
    }

    let use_fts = !fts_parts.is_empty();
    let mut sql = if use_fts {
        String::from(
            "SELECT n.id, -bm25(notes_fts) AS score \
               FROM notes_fts JOIN notes n ON n.rowid = notes_fts.rowid \
              WHERE notes_fts MATCH ",
        )
    } else {
        String::from("SELECT n.id, 0.0 AS score FROM notes n WHERE 1=1")
    };
    let mut params: Vec<String> = Vec::new();
    if use_fts {
        sql.push_str(&ph(params.len() + 1));
        params.push(fts_parts.join(" AND "));
    }
    for pat in likes {
        sql.push_str(" AND n.plain_text COLLATE NOCASE LIKE ");
        sql.push_str(&ph(params.len() + 1));
        sql.push_str(" ESCAPE '!'");
        params.push(pat);
    }
    if !exclude.is_empty() {
        let marks: Vec<String> = (0..exclude.len())
            .map(|i| ph(params.len() + 1 + i))
            .collect();
        sql.push_str(" AND n.id NOT IN (");
        sql.push_str(&marks.join(","));
        sql.push(')');
        params.extend(exclude.iter().map(|s| (*s).to_string()));
    }
    sql.push_str(" AND n.deleted_at IS NULL");
    sql.push_str(if use_fts {
        " ORDER BY bm25(notes_fts)"
    } else {
        " ORDER BY n.updated_at DESC"
    });
    // 只有 `limit` 这个数字直接拼：它是 usize，没有注入面。写成参数当然也行，但"页大小
    // 就是要的那一页"正是单测要钉的那一条，拼在这儿它才看得见、也才改得动。
    sql.push_str(&format!(" LIMIT {}", limit.max(1)));
    Some((sql, params))
}

/// 执行一档。分数：FTS 档用 bm25；LIKE 档没有 bm25 ⇒ 用 SQL 已经排好的新旧序（行号取负），
/// 这样后面"精准在前、模糊在后"的拼接顺序才不会被分数打乱。
fn tier(
    conn: &Connection,
    segments: &[&str],
    fuzzy: bool,
    exclude: &[EntityId],
    limit: usize,
) -> Result<Vec<Row>, StoreError> {
    let ids: Vec<&str> = exclude.iter().map(|e| e.as_str()).collect();
    let Some((sql, params)) = tier_sql(segments, fuzzy, &ids, limit) else {
        return Ok(Vec::new());
    };
    let use_fts = sql.contains("notes_fts");
    let mut stmt = conn.prepare_cached(&sql)?;
    let mut rows = stmt.query(rusqlite::params_from_iter(params.iter()))?;
    let mut out = Vec::with_capacity(limit.min(64));
    let mut rank = 0f64;
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let score: f64 = if use_fts { row.get(1)? } else { -rank };
        out.push(Row {
            id: parse_id(&id)?,
            score,
        });
        rank += 1.0;
    }
    Ok(out)
}

pub(crate) fn run(
    conn: &Connection,
    q: &crate::types::SearchQuery,
) -> Result<Vec<SearchHit>, StoreError> {
    let text = q.text.trim();
    if text.is_empty() {
        return Ok(Vec::new());
    }
    let limit = if q.limit == 0 { DEFAULT_LIMIT } else { q.limit } as usize;
    let segments: Vec<&str> = text.split_whitespace().collect();

    // 只要有一段走过 LIKE 兜底，整条查询就算兜底：host 那边那句 debug 日志要的是真话。
    let path = if segments
        .iter()
        .any(|s| path_for(char_len(s)) == SearchPath::LikeFallback)
    {
        SearchPath::LikeFallback
    } else {
        SearchPath::FtsTrigram
    };

    let mut picked = tier(conn, &segments, false, &[], limit)?;
    let exact_count = picked.len();
    let exact_ids: Vec<EntityId> = picked.iter().map(|r| r.id.clone()).collect();
    // 精准档**没填满**那一页 ⇒ 精准集已经全在 `exact_ids` 里 ⇒ 放宽档只补剩下的格子，
    // 并把它们整批排除（同一条笔记不许在两个档里各出现一次）。
    // 填满了 ⇒ 放宽档没有格子可站，跑它只是白烧一次索引查询。
    if exact_count < limit {
        let room = limit - exact_count;
        picked.extend(tier(conn, &segments, true, &exact_ids, room)?);
    }
    if picked.is_empty() {
        return Ok(Vec::new());
    }

    let ids: Vec<EntityId> = picked.iter().map(|r| r.id.clone()).collect();
    let texts = hydrate(conn, &ids)?;
    let mut out = Vec::with_capacity(picked.len());
    for (i, r) in picked.iter().enumerate() {
        let (title, plain) = texts
            .get(&r.id)
            .cloned()
            .unwrap_or_else(|| (String::new(), String::new()));
        let needle = anchor(&segments, &title, &plain);
        let hay = if needle.is_some() && find_ci(&title, needle.as_deref().unwrap_or("")).is_some()
        {
            &title
        } else {
            &plain
        };
        out.push(SearchHit {
            note_id: r.id.clone(),
            score: r.score,
            // 没锚点时片段照样给（开头一窗），只是不带 `<mark>`。
            snippet_html: snippet_html(hay, needle.as_deref().unwrap_or("")),
            path_used: path,
            match_kind: if i < exact_count {
                MatchKind::Exact
            } else {
                MatchKind::Fuzzy
            },
        });
    }
    Ok(out)
}

/// 匹配**总数**（带上界），给"搜索回满时那句「还有 N 条」"用（2026-10-09 用户拍板）。
///
/// 为什么是**带上界**的：真要求精确总数，就得把每一档的 `LIMIT` 拿掉去 count ——
/// 而 `search_latency.rs` 那条预算守的正是"常用词在 5000 篇上不许扫全表"，
/// 一个不限量的 count 会把那条预算一口吃光。所以这里复用**同一批带 `LIMIT` 的档**
/// （`tier`），取到 `cap` 行为止；调用方拿到 `n == cap` 时该说的是"还有 N 条**以上**"，
/// 而不是假装知道确切数字。成本形状与搜索本身一致（每档 ≤ cap 行），这句话不会让它变慢。
pub(crate) fn run_total(conn: &Connection, text: &str, cap: u32) -> Result<u32, StoreError> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(0);
    }
    let cap = if cap == 0 { DEFAULT_TOTAL_CAP } else { cap } as usize;
    let segments: Vec<&str> = text.split_whitespace().collect();
    let exact = tier(conn, &segments, false, &[], cap)?;
    let n_exact = exact.len();
    let fuzzy = if n_exact < cap {
        let exact_ids: Vec<EntityId> = exact.iter().map(|r| r.id.clone()).collect();
        tier(conn, &segments, true, &exact_ids, cap - n_exact)?
    } else {
        Vec::new()
    };
    Ok((n_exact + fuzzy.len()) as u32)
}

/// 高亮用的那一个串：先按整段找（精准），找不到再退回这一段的某个三字串（模糊）。
fn anchor(segments: &[&str], title: &str, plain: &str) -> Option<String> {
    for seg in segments {
        if find_ci(title, seg).is_some() || find_ci(plain, seg).is_some() {
            return Some((*seg).to_string());
        }
    }
    for seg in segments {
        for tri in trigrams(seg) {
            if find_ci(title, &tri).is_some() || find_ci(plain, &tri).is_some() {
                return Some(tri);
            }
        }
    }
    None
}

/// 只给最后那一页取正文。参数按 500 个一批切开（SQLite 的变量上限是 999）。
fn hydrate(
    conn: &Connection,
    ids: &[EntityId],
) -> Result<HashMap<EntityId, (String, String)>, StoreError> {
    const CHUNK: usize = 500;
    let mut out = HashMap::with_capacity(ids.len());
    for chunk in ids.chunks(CHUNK) {
        let marks: Vec<String> = (1..=chunk.len()).map(|i| format!("?{i}")).collect();
        let sql = format!(
            "SELECT id, title, plain_text FROM notes WHERE id IN ({})",
            marks.join(",")
        );
        let mut stmt = conn.prepare_cached(&sql)?;
        let args: Vec<String> = chunk.iter().map(|i| i.as_str().to_string()).collect();
        let mut rows = stmt.query(rusqlite::params_from_iter(args.iter()))?;
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            out.insert(
                parse_id(&id)?,
                (row.get::<_, String>(1)?, row.get::<_, String>(2)?),
            );
        }
    }
    Ok(out)
}

fn parse_id(s: &str) -> Result<EntityId, StoreError> {
    EntityId::parse(s).map_err(|_| StoreError::Constraint(format!("库内 id 不是合法 UUID: {s}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threshold_is_by_char_count_not_bytes() {
        assert_eq!(path_for(2), SearchPath::LikeFallback);
        assert_eq!(path_for(3), SearchPath::FtsTrigram);
        // "同步" 是 6 字节 2 字符 → 必须走 LIKE
        assert_eq!(char_len("同步"), 2);
        assert_eq!(path_for(char_len("同步")), SearchPath::LikeFallback);
        assert_eq!(path_for(char_len("同步协")), SearchPath::FtsTrigram);
    }

    #[test]
    fn like_escapes_wildcards_as_literals() {
        assert_eq!(like_pattern("a%b"), "%a!%b%");
        assert_eq!(like_pattern("100_"), "%100!_%");
        assert_eq!(like_pattern("!x"), "%!!x%");
        assert_eq!(fts_phrase("say \"hi\""), "\"say \"\"hi\"\"\"");
    }

    #[test]
    fn snippet_escapes_before_marking() {
        let hay = "前文 <script>alert(1)</script> 同步协议 后文";
        let s = snippet_html(hay, "同步协");
        assert!(s.contains("<mark>同步协</mark>"), "{s}");
        assert!(
            !s.contains("<script>"),
            "裸标签绝不允许出现在 snippet 里: {s}"
        );
        assert!(s.contains("&lt;script&gt;"), "{s}");
    }

    #[test]
    fn trigram_expr_requires_every_window_of_the_segment() {
        assert_eq!(trigrams("同步"), Vec::<String>::new());
        assert_eq!(trigrams("同步协"), ["同步协".to_string()]);
        assert_eq!(
            trigrams("同步协议"),
            ["同步协".to_string(), "步协议".to_string()]
        );
        // 重复的串只留一份，否则 "aaa" 这种会把同一个条件 AND 两遍
        assert_eq!(trigrams("aaa"), ["aaa".to_string()]);
        assert_eq!(trigram_and("同步协议"), "\"同步协\" AND \"步协议\"");
        // <3 字没有窗口：表达式退回整段短语（也就是不放宽）
        assert_eq!(trigram_and("同步"), "\"同步\"");
    }

    /// 时间预算抓不住 2 倍级的慢化（今天实测：`LIMIT` 放大 400 倍 ⇒ p95 20.9 → 37.4 ms 仍绿），
    /// 所以"每段各取一批再交集"那类回归只能由 SQL 自己的形状来拦。这四条就是那把锁。
    #[test]
    fn tier_sql_keeps_the_page_size_at_the_requested_page() {
        let (sql, params) = tier_sql(&["同步协议"], false, &[], 7).expect("一段长查询");
        assert!(sql.ends_with("LIMIT 7"), "页大小必须就是要的那一页：{sql}");
        // 正对照：老写法的窗口是 limit 的若干倍，那正是被这条抓回来的形状
        assert!(
            !sql.contains("LIMIT 28") && !sql.contains("LIMIT 35"),
            "{sql}"
        );
        assert_eq!(params, vec!["(\"同步协议\")".to_string()]);
    }

    #[test]
    fn tier_sql_ands_every_segment_into_one_statement() {
        // 一段长 + 一段短：两句都要在这**一条** SQL 里，缺一句就是"没有结果"的那种错
        let (sql, params) = tier_sql(&["数据同步机制", "独立"], false, &[], 50).expect("混合长度");
        assert_eq!(
            sql.matches("COLLATE NOCASE LIKE").count(),
            1,
            "短段要挂在同一句 WHERE 里：{sql}"
        );
        assert_eq!(sql.matches(" AND ").count(), 2, "{sql}"); // MATCH ∧ LIKE ∧ deleted_at
        assert_eq!(params.len(), 2, "{params:?}");
        assert_eq!(params[0], "(\"数据同步机制\")");
        assert_eq!(params[1], "%独立%");

        // 两段长查询进同一个 MATCH 表达式，而不是两条 SQL
        let (two, p2) = tier_sql(&["数据同步", "独立队列"], false, &[], 50).expect("两段长");
        assert!(!two.contains("COLLATE"), "{two}");
        assert_eq!(p2.len(), 1, "{p2:?}");
        assert_eq!(p2[0], "(\"数据同步\") AND (\"独立队列\")");
    }

    #[test]
    fn tier_sql_never_puts_user_text_in_the_statement() {
        let hostile = "') OR 1=1 --";
        let (sql, params) =
            tier_sql(&[hostile], false, &["01a1-注入"], 20).expect("带注入面的一段");
        assert!(!sql.contains("1=1 --"), "用户文本漏进 SQL 串了：{sql}");
        assert!(!sql.contains("01a1-注入"), "排除集也要走参数：{sql}");
        assert!(
            params.iter().any(|p| p.contains("1=1")),
            "参数里该有它：{params:?}"
        );
        assert!(params.iter().any(|p| p == "01a1-注入"), "{params:?}");
        // 占位符数量与参数数量必须相等，否则 SQLite 直接报错（那就是一条会红的真回归）
        assert_eq!(sql.matches('?').count(), params.len(), "{sql} / {params:?}");
    }

    #[test]
    fn all_short_segments_have_no_fuzzy_tier_at_all() {
        assert!(
            tier_sql(&["同步", "机制"], true, &[], 20).is_none(),
            "短段没有 trigram token，模糊档不许白扫一遍全表"
        );
        // 正对照：混进一段长的，模糊档就有活干，且放宽的只有那一段
        let (_sql, params) =
            tier_sql(&["同步", "数据同步"], true, &[], 20).expect("混合长度有模糊档");
        assert_eq!(params.len(), 2, "{params:?}"); // MATCH 表达式 + 短段的 LIKE
        assert_eq!(params[0], "(\"数据同\" AND \"据同步\")", "{params:?}");
        assert_eq!(params[1], "%同步%", "短段仍按字面 LIKE：{params:?}");
    }

    #[test]
    fn snippet_of_two_char_query_marks_the_hit() {
        let s = snippet_html("数据同步机制说明", "同步");
        assert_eq!(s, "数据<mark>同步</mark>机制说明");
    }
}
