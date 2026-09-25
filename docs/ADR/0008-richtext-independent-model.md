# ADR-0008: 富文本独立模型（编辑器只是 View）

## 状态
日期：2026-09-25
状态：Accepted

## 背景
远端存的是什么格式，决定了三件事：换编辑器要不要改协议、哈希稳不稳定、旧客户端会不会摧毁新内容。同时块级三方合并（ADR-0007）需要一个不漂移的锚点。把编辑器的序列化产物当权威格式，会把这三个问题全部变成协议问题。

## 决策
- 内部格式是**编辑器无关的块级树**：`{v, content:[Block]}`；`Block = {id, type, attrs?, content?: Inline[]}`，每个顶层块有稳定 `id`（合并锚点）。
- HTML / Markdown 只是**导出视图**，从不作为同步载体。
- 写入前顺序执行，任一失败即拒绝提交：`normalize()` → `validate()` → `canonical()` → `content_hash = sha256(canonical(doc))`。
  - `normalize`：拆零宽字符、剥空 marks、按 `id` 去重、补默认属性
  - `validate`：类型与嵌套合法、`id` 文档内唯一、`v ≤` 支持版本
  - `canonical`：对象键按 Unicode 码点升序、无无意义空白、UTF-8、**整数不写成浮点**
- 前向兼容：未知 `type` / `attrs` / `mark` 一律 preserve-unknown 原样写回（`unknown:<type>` 容器）。
- `doc.v` 高于本客户端支持版本 → 该笔记**只读打开**，禁止任何写回，UI 提示"请升级以编辑"。

## 备选方案与被否决的原因
- 直接同步 HTML：解析歧义（属性顺序、自闭合、实体、空白折叠）导致规范化不稳定，进而哈希抖动——同一份内容每次保存 `hash` 不同，P7 收敛判定失效；换编辑器即协议重写。
- 同步编辑器原生 JSON（如 ProseMirror doc）：把协议绑死在编辑器选型上，等于用协议文件为某个库的内存结构背书。
- Markdown 源文：无法无损表达 checklist 勾选态、highlight、附件块与块 `id`；导出可以，作为权威载体不行。

## 后果
正面：内容哈希稳定 → 收敛判定与去重可靠；合并有锚点；换编辑器不触碰同步层；旧客户端不会摧毁新类型内容。
代价：
- 需要自己维护 schema 演进与迁移函数（`doc.v` + `doc_format`），每加一种节点都要补 canonical 与 preserve-unknown 用例。
- 编辑器实现工作量高于"直接套一个富文本框"：每个块类型要写双向映射（模型 ↔ 视图），且映射必须无损。
- 派生列（`title`/`plain_text`/`summary`/`char_count`）必须由本模型单向导出并与权威数据同事务更新，多一条一致性维护路径（I5）。

## 验证方式
- 往返测试：`doc → canonical → doc` 逐字节稳定；同一逻辑文档在键插入顺序不同的 map 下 canonical 输出一致（实测已验：`{"a":2,"m":{"b":2,"k":1},"z":1}`）。
- 未知节点往返用例：含 `unknown:foo` 与新 attrs 键的文档，被旧版本客户端读取并保存后，未知内容逐字节保留（TEST-PLAN 富文本矩阵"未知节点往返"、FWD-*）。
- `doc.v` 超前用例：断言笔记只读、无任何写请求发出（`STATS` 中 PUT/MOVE 计数为 0）、重启后仍只读（P16/I7）。
- 合并输出必过 `validate()`，不通过即丢弃合并结果退回保留双方（ADR-0007 §3.4），有对应单元用例。
- 导出视图回归：HTML/Markdown 导出不反馈进 `content_hash`（导出前后 doc hash 不变）。

## 关联
- DATA-MODEL.md §10 富文本模型（原则、结构、规范化与稳定序列化、前向兼容）、§7.1 派生列
- CONFLICT-RESOLUTION.md §3.1 为什么按块而不是按字符
- SYNC-PROTOCOL.md §7 P16（版本超前 → 只读）
- 实测：`docs/evidence/probe-windows-gnu.txt` 的 `canonical-json-for-hashing`
- ADR-0007（块级合并）· ADR-0015（`plain_text` 是检索的输入）· ADR-0002（信封 `payload` 即此文档）
