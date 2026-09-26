//! 文档头部 front-matter：`---\nkey: value\n---`。
//!
//! 只**理解** `title` / `date` / `tags` 三个键；其余一律进 [`FrontMatter::ignored`]
//! 与 [`FrontMatter::entries`]，所以"没被理解的键"仍然在结果里可拿到 —— 静默丢弃
//! 与"我不认识所以删掉"是同一件事，这里两者都不做。
//!
//! 但被剥掉的那几行**不会**出现在笔记正文里（这是本 crate 明确记录的无损性偏离之一，
//! 理由：YAML 头是元数据不是正文；`ignored` 把它留给了调用方与导出侧）。

use std::collections::BTreeMap;

/// 一份 front-matter 的解析结果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FrontMatter {
    /// 文件确实以 `---` 开头并且找到了配对的结束线。
    pub present: bool,
    /// `title:` 的值（去引号）。
    pub title: Option<String>,
    /// `date:` 的值（原样字符串；本 crate 不猜时间格式，也不写 `notes.created_at`）。
    pub date: Option<String>,
    /// `tags:` 的值（支持 `a, b`、`[a, b]`、以及下面的 `- a` 块列表）。
    pub tags: Vec<String>,
    /// 除上述三键以外的全部键（重复键用 `key#2` 追加，绝不覆盖 —— 与 richtext 的
    /// `fold_json` 同一套做法）。
    pub ignored: BTreeMap<String, String>,
    /// 源顺序的全部 `key: value`，含被理解的那三个。
    pub entries: Vec<(String, String)>,
}

impl FrontMatter {
    /// 没有任何键（连 `---` 都没有）。
    pub fn none() -> Self {
        FrontMatter::default()
    }
}

/// 把文本切成 (front-matter, 正文)。没有 front-matter 时正文就是原文。
pub fn split(text: &str) -> (FrontMatter, String) {
    let lines: Vec<&str> = crate::markdown::split_lines(text);
    // 只允许第一行就是 `---`（BOM 已在解码阶段剥过，所以这里不处理 EF BB BF）。
    if lines.first().map(|l| l.trim_end()) != Some("---") {
        return (FrontMatter::none(), text.to_string());
    }
    let Some(close) = lines.iter().skip(1).position(|l| {
        let t = l.trim_end();
        t == "---" || t == "..."
    }) else {
        // 没有配对结束线：那 `---` 就是正文里的分割线，一个字都不许吃。
        return (FrontMatter::none(), text.to_string());
    };
    let close = close + 1; // 结束线自己的绝对下标
    let mut fm = FrontMatter { present: true, ..Default::default() };
    collect(&lines[1..close], &mut fm);
    (fm, lines[close + 1..].join("\n"))
}

/// 逐行吃 `key: value`，并支持 `tags:` 下面的 `- item` 块列表。
fn collect(lines: &[&str], fm: &mut FrontMatter) {
    let mut list_key: Option<String> = None;
    for raw in lines {
        let line = raw.trim_end();
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        if let Some(item) = line.trim_start().strip_prefix("- ") {
            if let Some(key) = &list_key {
                push(fm, key.clone(), unquote(item.trim()));
                continue;
            }
        }
        let Some((key, val)) = line.split_once(':') else {
            // 连冒号都没有：留着，等调用方报告（不做任何猜测）。
            push(fm, "unparsed".to_string(), line.trim().to_string());
            continue;
        };
        let key = key.trim().to_string();
        let val = val.trim();
        list_key = if val.is_empty() { Some(key.clone()) } else { None };
        if key == "tags" {
            for t in parse_tags(val) {
                push(fm, "tags".to_string(), t);
            }
            if val.is_empty() {
                list_key = Some("tags".to_string());
            }
            continue;
        }
        push(fm, key, unquote(val));
    }
}

/// `[a, b]` / `a, b` / 单值。
fn parse_tags(val: &str) -> Vec<String> {
    let inner = val.trim().trim_start_matches('[').trim_end_matches(']');
    if inner.trim().is_empty() {
        return Vec::new();
    }
    inner.split(',').map(|s| unquote(s.trim())).filter(|s| !s.is_empty()).collect()
}

/// 写入 entries，并把被理解的键抬到对应字段上。重复键不覆盖，降级为 `key#n`。
fn push(fm: &mut FrontMatter, key: String, val: String) {
    match key.as_str() {
        "title" if fm.title.is_none() => fm.title = Some(val.clone()),
        "date" if fm.date.is_none() => fm.date = Some(val.clone()),
        "tags" => fm.tags.push(val.clone()),
        _ => fold(&mut fm.ignored, &key, &val),
    }
    fm.entries.push((key, val));
}

fn fold(map: &mut BTreeMap<String, String>, key: &str, val: &str) {
    if !map.contains_key(key) {
        map.insert(key.to_string(), val.to_string());
        return;
    }
    for n in 1..=u32::MAX {
        let alt = format!("{key}#{n}");
        if let std::collections::btree_map::Entry::Vacant(e) = map.entry(alt) {
            e.insert(val.to_string());
            return;
        }
    }
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    let (f, l) = match (s.chars().next(), s.chars().last()) {
        (Some(f), Some(l)) => (f, l),
        _ => return s.to_string(),
    };
    if f == l && matches!(f, '"' | '\'' | '“' | '”') && s.chars().count() >= 2 {
        return s[f.len_utf8()..s.len() - f.len_utf8()].to_string();
    }
    s.to_string()
}

// ============================================================ 单元测试 ===

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn understands_three_keys_and_keeps_the_rest() {
        let text = "---\ntitle: \"我的笔记\"\ndate: 2026-03-01\ntags: [甲, 乙]\nauthor: 张三\nslug: my-note\n---\n正文第一段\n";
        let (fm, body) = split(text);
        assert!(fm.present);
        assert_eq!(fm.title.as_deref(), Some("我的笔记"));
        assert_eq!(fm.date.as_deref(), Some("2026-03-01"));
        assert_eq!(fm.tags, vec!["甲", "乙"]);
        // 不理解的键必须仍可拿到（一个都不许静默丢）
        assert_eq!(fm.ignored.get("author").map(String::as_str), Some("张三"));
        assert_eq!(fm.ignored.get("slug").map(String::as_str), Some("my-note"));
        // entries 是"源顺序的全部键值对"：tags 的两个值各占一条，所以是 6 条而不是 5 条
        let keys: Vec<&str> = fm.entries.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, vec!["title", "date", "tags", "tags", "author", "slug"]);
        assert_eq!(body, "正文第一段");
    }

    #[test]
    fn block_list_tags_and_duplicate_keys() {
        let text = "---\ntags:\n- 一\ntags: 二, 三\ntitle: A\ntitle: B\n---\nx\n";
        let (fm, _) = split(text);
        assert_eq!(fm.tags, vec!["一", "二", "三"]);
        assert_eq!(fm.title.as_deref(), Some("A"), "重复键先到先得，不覆盖");
        // 被"先到先得"挤掉的那个值不许消失：它降级进 ignored
        assert_eq!(fm.ignored.get("title").map(String::as_str), Some("B"));
    }

    #[test]
    fn unterminated_block_is_a_thematic_break_not_frontmatter() {
        let text = "---\ntitle: 这不是头部\n没有结束线\n";
        let (fm, body) = split(text);
        assert!(!fm.present);
        assert_eq!(body, text, "一个字都不许吃");
    }

    #[test]
    fn no_frontmatter_at_all() {
        let text = "# 标题\n正文\n";
        let (fm, body) = split(text);
        assert_eq!(fm, FrontMatter::none());
        assert_eq!(body, text);
    }

    #[test]
    fn crlf_head_block_still_recognised() {
        let text = "---\r\ntitle: T\r\n---\r\n正文\r\n";
        let (fm, body) = split(text);
        assert!(fm.present);
        assert_eq!(fm.title.as_deref(), Some("T"));
        assert!(body.contains("正文"));
    }
}
