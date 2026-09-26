# ADR-0019: 外部参照 —— Joplin 的同步与 AppFlowy 的编辑交互，各采纳什么

## 状态
日期：2026-09-26
状态：Accepted

取证边界：Joplin 一侧读的是官方同步规范页 `joplinapp.org/help/dev/spec/sync`（本轮实读，
下文引用其字段名与措辞）；其源码目录（GitHub / api.github.com）在本机不可达，
所以**没有逐行核对实现**，凡涉及实现细节处标 `[未核实]`。AppFlowy 一侧同理：
按其产品内可观察的编辑行为对齐，未读其 Dart 源码。

## 背景
两个参照物被点名：Joplin（同步核心设计）与 AppFlowy（UI 交互与原生感）。
它们与 Notera 的目标重合但不等价：Joplin 是 Electron + 全量列表式同步的老兵，
AppFlowy 是 Flutter + 块编辑器的后来者。逐条对照才能避免"抄了形式、丢了理由"。

## Joplin 的机制（按其规范页）
| 它的做法 | Notera 的对应物 | 判定 |
|---|---|---|
| `sync_items` 表 + 每项 `sync_time`（上次成功传输时间），据此筛出待同步项 | 实体行上的 `sync_rev/sync_hash` + `sync_operations`（outbox） | **等价，且更抗崩溃**：我们的"是否待传"是实体行的一列，与内容同事务提交；侧表要与主表保持同步，崩溃后可能留下"账上有货、内容已改"的错配 |
| 一轮里：拉远端 → 推本地 → 处理删除 | 一轮里：读清单 → 判定 P1..P18 → 先实体后清单（R2） | 采纳同一节奏；但我们把"删除"当成**内容**传播（tombstone 记录），不是"从两端抹掉" |
| "Uploading items as soon as possible helps limit conflicts" | `Scheduler` 的 `select!` 里 `wait_dirty(app)` 与 25s tick 并列：本地写完立刻起一轮 | **已实现**（不是愿望）。这也是为什么桌面 30s 预算能兑现 |
| 冲突判定比较 `updatedTime`，"heuristics decide which value should be kept" | 块级三方合并（base = `sync_rev`），失败保留双方 + 冲突副本 | **拒绝其基座**：`updatedTime` 是墙上时钟，两台设备时钟偏移就会静默选错一侧。主需求明令禁止以修改时间判定新旧 |
| 本地删除 ⇒ "it is also deleted from the server" | 删除写进记录内容（tombstone），文件夹删除不级联删笔记，`missing_remote` 永不删本地 | **拒绝**：直接远端删除正是"删了又回来 / 一台误删全端清空"的来源 |
| 每项 `sync_disabled` 可单独停止同步；`appMinVersion` 作为跨环境门槛 | 版本门槛已有（`protocol.json` 的 `protocol/min_protocol` 区间协商，§2）；单项停同步**没有** | 门槛等价；`sync_disabled` 记为**将来要做的功能**（文件夹级"不参与同步"），不是缺陷 |
| 项目集合按类型全量列举来比对 `[未核实]` | 清单两段式：基线分段 ⊕ 200 条变更窗口 | **拒绝**：实测 5000 条全量清单 438.7 KiB（gzip 186 KiB）vs 窗口 7.8 KiB，每轮成本与库规模同阶不可接受（ADR-0004） |

可借鉴但**尚未做**：Joplin 的冲突副本命名对用户是可读的（"Conflicted copy"）。
我们按 §6 用"原标题（本地副本）"，方向一致；差的是把**设备名**放进去 ——
这需要 OS 主机名/用户名的平台读取，归 Phase 5。

## AppFlowy 的编辑交互
观察到的、构成"原生感"的三条：打字即转换块型、`/` 唤起命令面板、面板可纯键盘走完；
另有块左侧悬浮的拖拽把手与"在此下方插入"按钮。

| 项 | 状态 |
|---|---|
| Markdown 输入缩写（`# `、`- `、`1. `、`> `、`[]/[x]`、``` ） | **已实现**（`editor/quickInsert.ts`，17 个单测） |
| `/` 命令面板（中英文关键词、↑↓/Enter/Tab/Esc） | **已实现**，端到端实测：9 项 → 敲 `cod` 滤到 1 项 → 回车真的变成代码块 |
| 转换不压平后续标记 | **已实现**（剥前缀只剥纯文本那几列） |
| 空块占位文案 | 早已有（`data-placeholder`） |
| 拖拽把手重排 + 悬浮"插入下方" | **未做**：`moveBlock` 命令已在，缺 DOM 与命中区设计 |
| 选中文字时浮动小工具条（而不是固定顶部条） | **未做**：当前是顶部工具条 + 快捷键 |

明确**不抄**的：AppFlowy 的块是"文档树 + 嵌套块"，我们的是扁平块序列 + `indent` 属性。
扁平模型让三方合并的块对齐是线性的（`richtext::merge`），换成树就要重新证明
"块 id 稳定 ⇒ 合并无损"这条不变式 —— 为一个观感改动去动冻结的数据模型，不值。

## 后果
正面：两条参照线都落到"采纳有理由的、拒绝有代价的"，不是照抄；
可感知的编辑手感提升已经进代码并有端到端证据。
代价：
- 与 Joplin 的差异要向外解释（尤其"为什么不用 updatedTime"），文档已承担这一职责。
- 命令面板与缩写新增了一批 UI 状态（面板开合、选中项），它们与只读态/冲突态互斥，
  组件里多了三条早退分支；已用纯函数把判定搬出组件，避免这些分支进单测盲区。

## 验证方式
- `editor/quickInsert.spec.ts` 17 条：每种缩写、"前缀后无空格不转换"、
  `[x]` 与 `[]` 的勾选差异、marks 不被压平、`/` 查询成立条件与过滤、选中后清掉查询串。
- `scripts/verify-app.mjs`：`# ` 转标题、`/` 面板出现→过滤→回车成代码块，
  且控制台零 error、无失败请求（真浏览器 + 真 Rust 核心）。
- 与 Joplin 的差异不需要运行时验证：`updatedTime` 与"直接远端删除"在我们这里
  **没有代码路径**，arch-check 与 sync 测试的断言（412 → 重算、tombstone 不复活）就是防线。

## 关联
- ADR-0004（清单两段式，实测尺寸）· ADR-0005（Lamport rev + 共同祖先，不用向量时钟）
- ADR-0006（tombstone 不自动 GC）· ADR-0007（冲突保留双方 + 副本）
- CONFLICT-RESOLUTION.md §6（冲突副本命名）· SYNC-PROTOCOL.md §2（版本协商）
- 参照物：`joplinapp.org/help/dev/spec/sync`（本轮实读）；AppFlowy 编辑器为行为观察
