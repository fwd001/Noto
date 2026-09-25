# ADR-0015: 检索双路径（FTS5 trigram + 短查询 LIKE，实测驱动）

## 状态
日期：2026-09-25
状态：Accepted

## 背景
中文没有空格分词。实测（5000 条中文笔记，rusqlite 0.40.2 bundled SQLite 3.53.2，Windows x64）：

| 查询 | 路径 | 结果 | 耗时 |
|---|---|---|---|
| ≥3 字中文 `MATCH` | FTS5 trigram | 正确 | p50 = 32 µs、p95 = 1.4 ms，索引 6.0 MiB |
| **2 字中文 `MATCH`** | FTS5 trigram | **静默返回 0 行** | 22 µs |
| 2 字中文 `LIKE` | content 表 `plain_text LIKE` | 正确（416/5000） | ≈ 4.1 ms |
| 2 字中文 `LIKE` | FTS 列 `LIKE` | 正确（416/5000） | ≈ 5.8 ms |

关键事实不是"慢"，而是**错**：trigram 分词对 <3 字符查询不产生任何 token，`MATCH` 返回 0 行且没有任何错误信号。用户看到"没有结果"，而数据其实在库里。

## 决策
- FTS5 external-content 表 + `tokenize='trigram case_sensitive 0'`。
- 查询长度 ≥3 → `MATCH`（快，带 snippet 高亮）；≤2 → content 表 `LIKE` + `COLLATE NOCASE`（慢一个数量级但正确）。
- 混合查询按空格切分，逐段独立选路后取交集。
- 派生列 `plain_text` 与权威数据在同一事务内维护（I5）。
- 提供 `search_generation` 版本号、抽样 verify 与后台重建；重建期间旧索引继续可查。
- **禁止**把 <3 字查询直接交给 `MATCH`。

## 备选方案与被否决的原因
- `unicode61` 分词：连续中文被当成单个 token，子串检索完全失效（搜"同步"命中不了"数据同步机制"）。
- 引入中文分词器 / 把 jieba 编译进 SQLite：跨四端构建复杂度与体积大增（Android NDK、iOS 静态库），而 trigram 在目标数据量下已满足性能预算。
- 只用 `LIKE`（不建 FTS）：20000 条量级下全表扫描成本不可控，且失去 snippet 高亮与相关性排序。
- 只提示用户"至少输入 3 字"：把实现缺陷转嫁给用户，且中文两字词（"同步""笔记""项目"）极常见，等于功能不可用。

## 后果
正面：≥3 字走索引（p50 32 µs 级）；≤2 字结果正确；兜底路径明确走 content 表（实测比 FTS 列 `LIKE` 快约 1.7 ms）。
代价：
- 两套查询路径需各自测试，且必须验证二者结果一致性（同一查询不同路径的命中集合）。
- 2 字查询在 20000 条规模**需重测**；超预算则需引入 bigram 辅助表（Phase 7 待验证项，届时另立 ADR）。
- FTS 索引 6.0 MiB @5000 条是额外磁盘成本，且 `plain_text` 派生列与索引必须与权威表同步，多一条漂移路径。

## 验证方式
- 必须存在黑盒用例：5000 条样本（含"同步"者 416 条）→ UI 搜索框输入 `同步` → **断言列表条目数 = 416，禁止返回 0**（FT-SRCH-01）；单字 `笔` 断言等于 L1 预计算的 golden 数（FT-SRCH-02）。
- 路径断言：`notera-cli diag search-plan "同步"` 输出所选路径，断言 ≤2 字走 LIKE、≥3 字走 MATCH（FT-SRCH-03）。
- 一致性抽样：随机取 N 条查询，断言 LIKE 与 MATCH 的命中集合在语义上一致（长查询差集为空）。
- 完整性自修：人为制造 `notes_fts` 与 `notes` 脱钩 → `verify_search()` 必须报不一致并触发后台重建，重建后 `search_generation` +1、命中数回到 golden。
- 性能回归：≥3 字 p50 ≤60 µs / p95 ≤2000 µs（容差基于实测 32 µs / 1404 µs，PERF-02）；2 字 ≤15 ms @5000 条（PERF-03）。
- LIKE 转义用例：输入 `%_[]()'"\\`、`%%` 按字面量匹配且不抛 SQL 错误（FT-SRCH-07）。

## 关联
- DATA-MODEL.md §7.2 检索策略（实测表与写死的策略）、§7.1 派生列、§7.3 完整性（`search_generation` / `verify_search`）
- ARCHITECTURE.md §6 实测驱动的设计（前两行结论）
- TEST-PLAN.md FT-SRCH-01..11、PERF-02/03/04
- ARCHITECTURE-MAP.md §4 改动路由（"改搜索行为"一行：禁止把 <3 字查询交给 MATCH）
- 实测：`docs/evidence/probe-windows-gnu.txt` 的 `fts5-trigram-cjk`、`fts5-search-5000-notes`（`2char: MATCH->0 (22us, WRONG) content-table LIKE->416 (4126us)`）
- ADR-0008（`plain_text` 由富文本模型派生）· ADR-0012（`0003_search.sql` 迁移承载 FTS 虚表）
