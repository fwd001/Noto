# ADR-0005: Revision 模型（Lamport 式单调整数，不用向量时钟）

## 状态
日期：2026-09-25
状态：Accepted

## 背景
判定"两边都改过"需要一个共同参照点。用墙上时间会被设备时钟差欺骗（I4/R3 明令禁止）；用向量时钟需要每设备稳定身份与因果历史。项目要的是"不丢内容"，不是"因果精确"——参照点选错会把协议复杂化而没有收益。

## 决策
- 每个可同步实体的 `rev` 是 Lamport 式单调整数：提交本地修改时 `rev = max(rev, remote_rev) + 1`，无需跨设备协调。
- 配套字段（缺一不可）：
  - `sync_rev`：本地与远端**最后一次确认一致**的 rev，即三方合并的 base
  - `remote_rev`：清单给出的服务器当前 rev
  - `sync_hash`：该一致点的内容哈希
  - `content_hash`：当前 `doc` 的 sha256（权威）
- 不使用向量时钟 / 版本向量。
- 关键论证：`sync_rev` 是双方都确认过的内容点，因此 base 文档可从本地 `note_revisions(note_id, rev = sync_rev)` 精确取回；服务器侧同一 rev 的内容与本地 base 必然相同（否则当初不会被确认为一致）。base / local / remote 三方齐备，`max+1` 保证跨设备单调，即可判定真并发。
- 冲突判定式：`rev ≠ sync_rev ∧ remote_rev ≠ sync_rev ∧ 两侧哈希均异于 sync_hash`。

## 备选方案与被否决的原因
- 向量时钟 / 版本向量：需要每设备稳定身份与完整因果历史，UI 无法向用户解释"你和设备 X 并发"；本项目要的是不丢内容而非因果精确，复杂度无对应收益。
- 墙上时间戳比较：多设备时钟不可信，直接违反 I4/R3；夏令时、休眠恢复、手工改表都会造成错误判定。
- 纯内容哈希 diff（无 rev）：能判"内容不同"，无法判"谁改过"，因此无法区分"仅远端改"与"双方都改"，冲突检测退化为整篇比对。

## 后果
正面：单调整数 + 一个共同祖先点即可完整判定 P1..P18；`rev` 参与 CAS 写入闸门（`local.rev ≤ R.rev` 且 hash 不同 → 不写，转冲突判定）。
代价：
- 无法表达因果先后细节（谁先写、哪台设备的改动覆盖了哪些块），只能判"并发"。
- `note_revisions` 必须保留 `rev >= sync_rev` 的行——这是**硬约束**（DATA-MODEL §4.4 第 1 条），违反即找不回 base。
- 该保留规则带来存储成本：每笔记额外保留历史行（其余按最近 200 行 GC），GC 逻辑必须与 `sync_rev` 联动，不能独立按时间清理。

## 验证方式
- 属性测试：对任意 `(base, local, remote)` 输入组合，引擎判定结果与 SYNC-PROTOCOL §7 的 P1..P18 判定表逐条一致（生成式用例覆盖 rev/hash 的相等与不等矩阵）。
- 收敛用例：两台设备各自打开又保存同一笔记（内容不变）必须判 P7 收敛、不产冲突副本，且本地 `rev` 不被二次递增（TEST-PLAN SY-CONV-*）。
- GC 断言：任意一轮 `note_revisions` GC 之后，`SELECT count(*) FROM note_revisions WHERE note_id=? AND rev=?`（`rev = sync_rev`）必须为 1；否则该次 GC 判为缺陷。
- 时钟无关性：L1 用 fake clock 把两侧 `updated_at` 人为错开 ±12 h，断言冲突判定结果不变（I4 的直接证明）。

## 关联
- DATA-MODEL.md §4 Revision 模型（字段、规则、共同祖先定义、保留窗口）、§0 不变式 I2/I4
- SYNC-PROTOCOL.md §0 R3、§7 计划判定表 P1..P18、§11.2 并发保护三层
- CONFLICT-RESOLUTION.md §1.1 判定式（base/local/remote 来源表）
- ADR-0004（清单承载 `rev`/`hash`）· ADR-0007（冲突处置）· ADR-0017（多窗口复用同一乐观并发机制）
