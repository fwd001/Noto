//! 检索双路径（ADR-0015 / DATA-MODEL §7.2）—— 实测决定的硬规则。
//!
//! ```text
//! 查询字符数 >= 3 → FTS5 trigram MATCH（快，带高亮）
//! 查询字符数 <= 2 → notes.plain_text LIKE + COLLATE NOCASE（慢一个数量级但正确）
//! 混合长查询      → 按空白切分，逐段独立选路后取交集
//! ```
//!
//! 为什么不是"全交给 MATCH"：trigram 分词对 <3 字符查询**不产生任何 token**，
//! `MATCH` 静默返回 0 行且无错误信号（实测），用户会以为笔记不存在。
//! 因此 [`SearchHit::path_used`] 是必填字段，测试直接断言它。
//!
//! `notes_fts` 是 external-content 表且**无触发器**：索引行由 `store` 在与权威表同事务里
//! 用 `('delete', …)` + `(rowid, title, plain_text)` 维护（I5）。
//!
//! snippet 由本模块自行生成：**先转义再插 `<mark>`**（UI 直接渲染该 HTML）。

use crate::error::StoreError;
use crate::types::{SearchHit, SearchPath};
use notera_core::EntityId;
use rusqlite::Connection;

/// 短查询阈值：`>= 3` 走 FTS，`<= 2` 走 LIKE（ADR-0015）。
pub(crate) const FTS_MIN_CHARS: usize = 3;
/// snippet 窗口（字符）。
const SNIPPET_LEAD: usize = 40;
const SNIPPET_TAIL: usize = 120;
const DEFAULT_LIMIT: u32 = 50;

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

pub(crate) struct Hit {
    pub note_id: EntityId,
    pub score: f64,
    pub snippet_html: String,
    pub path_used: SearchPath,
}

pub(crate) fn run(
    conn: &Connection,
    q: &crate::types::SearchQuery,
) -> Result<Vec<SearchHit>, StoreError> {
    let text = q.text.trim();
    if text.is_empty() {
        return Ok(Vec::new());
    }
    let limit = if q.limit == 0 { DEFAULT_LIMIT } else { q.limit };
    // 混合长查询：按空白切分，逐段独立选路，最后取交集（DATA-MODEL §7.2）。
    let segments: Vec<&str> = text.split_whitespace().collect();
    let mut merged: Option<Vec<Hit>> = None;
    for seg in &segments {
        let part = run_one(conn, seg, limit.saturating_mul(4))?;
        merged = Some(match merged {
            None => part,
            Some(prev) => {
                let keys: std::collections::HashSet<EntityId> =
                    part.iter().map(|h| h.note_id.clone()).collect();
                prev.into_iter()
                    .filter(|h| keys.contains(&h.note_id))
                    .collect()
            }
        });
    }
    let Some(mut hits) = merged else {
        return Ok(Vec::new());
    };
    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.note_id.cmp(&b.note_id))
    });
    hits.truncate(limit as usize);
    Ok(hits
        .into_iter()
        .map(|h| SearchHit {
            note_id: h.note_id,
            score: h.score,
            snippet_html: h.snippet_html,
            path_used: h.path_used,
        })
        .collect())
}

fn run_one(conn: &Connection, seg: &str, fetch_limit: u32) -> Result<Vec<Hit>, StoreError> {
    let path = path_for(char_len(seg));
    let mut out = Vec::new();
    match path {
        SearchPath::FtsTrigram => {
            let expr = fts_phrase(seg);
            let mut stmt = conn.prepare(
                "SELECT n.id, -bm25(notes_fts) AS score, n.title, n.plain_text
                   FROM notes_fts
                   JOIN notes n ON n.rowid = notes_fts.rowid
                  WHERE notes_fts MATCH ?1 AND n.deleted_at IS NULL
                  ORDER BY bm25(notes_fts)
                  LIMIT ?2",
            )?;
            let rows = stmt.query_map(rusqlite::params![expr, fetch_limit as i64], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, f64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })?;
            for row in rows {
                let (id, score, title, plain) = row?;
                let hay = if find_ci(&title, seg).is_some() {
                    &title
                } else {
                    &plain
                };
                out.push(Hit {
                    note_id: parse_id(&id)?,
                    score,
                    snippet_html: snippet_html(hay, seg),
                    path_used: path,
                });
            }
        }
        SearchPath::LikeFallback => {
            let pattern = like_pattern(seg);
            let mut stmt = conn.prepare(
                "SELECT n.id, n.title, n.plain_text
                   FROM notes n
                  WHERE n.plain_text COLLATE NOCASE LIKE ?1 ESCAPE '!'
                    AND n.deleted_at IS NULL
                  ORDER BY n.updated_at DESC
                  LIMIT ?2",
            )?;
            let rows = stmt.query_map(rusqlite::params![pattern, fetch_limit as i64], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?;
            for row in rows {
                let (id, title, plain) = row?;
                let hay = if find_ci(&title, seg).is_some() {
                    &title
                } else {
                    &plain
                };
                out.push(Hit {
                    note_id: parse_id(&id)?,
                    score: occurrences(hay, seg) as f64,
                    snippet_html: snippet_html(hay, seg),
                    path_used: path,
                });
            }
        }
    }
    Ok(out)
}

fn parse_id(s: &str) -> Result<EntityId, StoreError> {
    EntityId::parse(s).map_err(|_| StoreError::Constraint(format!("库内 id 不是合法 UUID: {s}")))
}

fn occurrences(hay: &str, needle: &str) -> usize {
    let hc: Vec<char> = hay.chars().collect();
    let nc: Vec<char> = needle.chars().collect();
    if nc.is_empty() || hc.len() < nc.len() {
        return 0;
    }
    let mut n = 0usize;
    let mut i = 0usize;
    while i + nc.len() <= hc.len() {
        if hc[i..i + nc.len()]
            .iter()
            .zip(nc.iter())
            .all(|(a, b)| a.eq_ignore_ascii_case(b))
        {
            n += 1;
            i += nc.len();
        } else {
            i += 1;
        }
    }
    n
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
    fn snippet_of_two_char_query_marks_the_hit() {
        let s = snippet_html("数据同步机制说明", "同步");
        assert_eq!(s, "数据<mark>同步</mark>机制说明");
    }
}
