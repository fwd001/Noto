# ADR-0012: SQLite 迁移契约（forward-only + user_version）

## 状态
日期：2026-09-25
状态：Accepted

## 背景
本地库是唯一的离线真相来源，用户不会因为"升级失败"而原谅丢数据。需求明令禁止运行时隐式 `ALTER`。迁移机制必须能在崩溃后判定"要么旧 schema 完整、要么新 schema 完整"，并且对"库比程序新"这种情况有确定行为。

## 决策
- 形态：`migrations/NNNN_name.sql`，编号严格递增，**纯 forward-only**；不存在 down 脚本、不存在条件回滚。
- 版本载体：`PRAGMA user_version` 表示已应用到的编号（实测 `applied=[1, 2, 3] version=3`）。
- 迁移前必须做物理备份：`notera.sqlite.pre-migration.<from_ver>`，成功后保留 1 份。
- 每个迁移文件在**单事务**内执行，失败即整体回滚且不推进 `user_version`；SQLite 不支持事务内完成的 `ALTER` 变体（重建表）必须在文件头显式标注 `-- no-transaction` 并走重建流程。
- 禁止运行时隐式 `ALTER TABLE`（CI grep 闸门强制；迁移执行器自身所在文件需显式 allowlist，allowlist 变更须两人评审）。
- 遇到 `user_version > 本版本支持值` → **进入只读模式并停止同步**，绝不降级写回、不执行迁移、不改文件。
- 已发布过的迁移文件禁止修改（CI 对 `migrations/**` 的 diff 做守卫）。

## 备选方案与被否决的原因
- 启动时按需 `ALTER TABLE`（比较列存在性再补）：需求明令禁止；不可审计——同一份代码在不同库上产生不同 schema，故障无法复现。
- 引入重型迁移框架（diesel/sqlx migrate/refinery 等）：v1 阶段收益不足，且 bundled SQLite 版本可控（实测 3.53.2），多一层依赖多一处跨端构建风险（ADR-0014）。
- 允许降级（down 脚本）：旧版本无法理解新语义（新枚举值、新列的含义），降级写回即损坏风险；"能回退"在这里不是安全性而是幻觉。

## 后果
正面：升级路径是文件序列，可读、可审、可逐版本重放；只读闸门让"新库配旧程序"变成可见的停止而不是静默损坏。
代价：
- 必须维护历史版本升级路径矩阵，CI 要跑"每个历史版本 → 最新"（M2），路径数随迁移数线性增长，且需要保留 v_k 时代的代码或 fixture `.db` 二进制。
- 需要重建表的大改动（如拆 `note_docs` 旁表）成本明显高于"加一列"，会反过来抑制合理重构。
- 备份文件占用双倍磁盘（迁移窗口内），对移动端存储要额外提示。

## 验证方式
CI 四项真实执行（grep 只是下限，不替代执行）：
- M1 空库 → 最新：新建临时库 `migrate(None→HEAD)`，断言无 SQL 错误且 `user_version == HEAD`。
- M2 逐版本升级链：对 `migrations/` 每个前缀长度 k 构建 v_k 库再升到 HEAD；矩阵项数 < `count(migrations)` 也判失败（防"悄悄少测一个"）。
- M3 升级后数据完整性：迁移前灌确定性合成数据并记录每表**行数** + 规范化后的 **SHA-256 校验和**，升级后逐表比对，允许差异必须写进 `expected_diffs/<N>.json` 白名单。
- M4 未知未来版本只读：打开 `user_version = HEAD+1` 的库，断言进入只读、给出可操作文案、不发任何写请求、不改文件（比对 mtime_ns 与哈希）。
- M5 grep 闸门：`crates/ apps/` 内 `.rs` 命中 `ALTER TABLE|CREATE TABLE|DROP TABLE|PRAGMA user_version =` 即 fail（allowlist 除外）。
- 崩溃回归：`CRASH(during-migration, version=k)` 对每个迁移版本各一次（CP-08），断言无"表已改列、数据未搬"的中间态。

## 关联
- DATA-MODEL.md §2 迁移机制、§5 DDL（`0001_init` … `0005_views`）、§12 WAL/事务参数
- CI-CD.md §Migration 契约（M1–M5 表与 grep 片段）
- ARCHITECTURE-MAP.md §5 禁止模式（迁移外 DDL）
- 实测：`docs/evidence/probe-windows-gnu.txt` 的 `migration-user_version`（`applied=[1, 2, 3] version=3`）、`wal-tx-atomicity`（`journal=wal foreign_keys=1 rows_after_commit_and_rollback=["kept", "kept2"]`，即已提交保留、未提交整体丢弃）
- ADR-0013（迁移期崩溃注入需要 test-webdav 的 fs 模式跨重启保留状态）· ADR-0014（bundled SQLite 版本可控是"不引框架"的前提）
