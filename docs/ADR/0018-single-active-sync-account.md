# ADR-0018: 单一活跃同步账户（多服务器扇出延后到按账户确认点落地）

## 状态
日期：2026-09-25
状态：Accepted

## 背景
`sync_operations`（outbox）、`sync_remote_index`、`sync_accounts` 都是**按账户**记账的，
但实体上的确认点 `notes.sync_rev / sync_hash`（DATA-MODEL §4.2）是**全局一列**。

这两套作用域同时存在会产出一个静默错误：

```text
设备本地有一条笔记（rev=7, sync_rev=0）
  → 账户 A 推送成功：mark_synced(Note, id, 7) → sync_rev := 7   （全局）
  → 账户 B 的脏集判定 rev <> sync_rev 此刻为假
  → B 永远收不到这条笔记，也没有任何地方报告这件事
```

outbox 的账户扇出已经修好（`dedupe_key` 含账户前缀，见 commit `fix(store)`），
但那只是"队列层"正确；**"是否还需要推"这个判断住在脏集里，而脏集用的是全局确认点**。
只要同时启用两台服务器，第二台就会静默落后——这违反"不允许静默"这一条底线，
而它的修复需要改冻结的数据模型（`sync_rev/sync_hash` 迁到按账户的关联表），
属于架构变更，不能顺手做。

## 决策
1. **当前版本只允许一个启用状态的同步账户。** 第二个账户在启用时（配置保存路径）返回
   明确错误码 `multi_account_unsupported`，UI 显示"当前版本一台设备只支持一台同步服务器，
   请先停用现有服务器再添加新的"，并保留其已存配置不删。
2 outbox 的**按账户扇出保留**（不撤回 `dedupe_key` 的账户前缀）。它是未来多服务器的
   前置条件，且现在的成本是一行字符串，删掉它反而要让将来的改动重新踩同一个坑。
3. `sync_accounts.enabled` 的"本地哨兵账户 `local`（enabled=0 但持续留痕）"这一特例不变。
4. 将来支持多服务器的**唯一**正确形状：把确认点搬到 `sync_entity_state(account_id, kind,
   entity_id, rev, hash)`，脏集与冲突基线（`sync_rev` 作为共同祖先）都按账户取。
   这需要新的迁移号、`ApplyOp::MarkSynced` 带账户、以及 L3 双服务器场景测试。

## 备选方案与被否决的原因
- **什么都不做**：第二台服务器静默收到部分数据，用户以为"两台都有"。否决——这是数据安全问题，不是体验问题。
- **把脏集改成"按账户的 remote_index 对比"**：`sync_remote_index` 确实按账户存了 rev，
  看起来可以替掉全局 `sync_rev`。否决——`sync_rev` 同时是**冲突检测的共同祖先**
  （CONFLICT-RESOLUTION 的三方合并基线），拿清单缓存当基线会把"我观测到的远端"和
  "我提交成功并被确认的远端"混成一件事，冲突判定会放松。
- **现在就做完整按账户确认点**：正确但属于冻结契约的变更（迁移 + 引擎 + 主机 + 测试矩阵），
  且当前没有任何用户场景需要两台服务器；按"不为假想需求提前设计"的约定推后。

## 后果
正面：不存在"看起来同步了其实只同步了一半"的状态；错误在配置入口就爆炸，不进入数据路径。
代价：
- 多服务器（例如家里 NAS + 公司内网）这个真实需求暂时不可用，产品上是能力缺口。
- outbox 里带着永远只被一个账户消费的扇出，多写几行数据库。可接受（每轮行数与实体数同阶，不是与库规模同阶）。

## 验证方式
- L1：`configure_account` 在已有 1 个 enabled 账户时启用第 2 个 → 返回 `multi_account_unsupported`，
  且第一个账户的配置未被改写。
- L2：`GET /api/account` 与 `sync_status` 在停用→再启用同账户后回到 `unconfigured → online`，
  中间不产生"部分推送"状态。
- L3（将来）：双服务器场景 —— 同一设备把笔记推到 A、再从 B 拉取，B 必须能独立收敛；
  该测试在按账户确认点落地前**不可能通过**，因此它就是那条改造的验收标准。
- store 侧回归：`every_write_enqueues_one_outbox_row_per_enabled_account_and_supersedes_old_rev`
  继续锁住"注册之后的写入必须扇出到每个账户"这一条，防止扇出被顺手删掉。

## 关联
- DATA-MODEL.md §4.2（`rev/sync_rev/remote_rev/sync_hash` 四字段的语义分工）、§5.2（`sync_*` 表）
- SYNC-PROTOCOL.md §6（脏集与读取代价）、§10（bootstrap）
- CONFLICT-RESOLUTION.md（`sync_rev` 作为三方合并基线）
- ADR-0005（`rev`/`sync_rev` 判定）· ADR-0004（清单两段式）
- ARCHITECTURE-REVIEW.md §14 待决项（多账户 UI）——本 ADR 把它从"要不要做 UI"改为"先补按账户确认点"
