-- 0003_search.sql —— 检索索引（DATA-MODEL.md §5.3 逐字落地，ADR-0015）
--
-- 不使用触发器：派生文本需要 richtext→plain_text 转换，必须在 Rust 侧完成后同事务写入。
-- 维护点唯一：notera-store::search（见 §7）。
--
-- tokenize='trigram case_sensitive 0'：trigram 对 <3 字符查询不产生 token，
-- MATCH 会**静默返回 0 行**（实测），故 Store::search 必须按查询字符数分流。

CREATE VIRTUAL TABLE notes_fts USING fts5(
  title,
  plain_text,
  content='notes',
  content_rowid='rowid',
  tokenize = 'trigram case_sensitive 0'
);
-- 不使用触发器：派生文本需要 richtext→plain_text 转换，必须在 Rust 侧完成后同事务写入。
-- 维护点唯一：notera-store::SearchIndexer（见 §7）。
