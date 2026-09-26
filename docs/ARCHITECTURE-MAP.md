# ARCHITECTURE-MAP

> **本文件是 Notera 的长期架构记忆。**
> 它的作用不是描述代码，而是让任何一次开发（包括下一次会话里的我）在动手前就知道：边界在哪、契约是什么、改这里会牵动什么、什么绝对不许做。

---

## 0. 开工前必读清单（每次，不可跳过）

```text
1. 读本文件（模块注册表 §2 · 不变式 §3 · 改动路由 §4 · 禁止模式 §5）
2. 读当前 Phase 与其出口条件（ARCHITECTURE.md §10 · TEST-PLAN.md §阶段出口条件）
3. 读与本次改动相关的 ADR（§7 索引）
4. 若触碰协议/数据：读 SYNC-PROTOCOL.md + DATA-MODEL.md 对应小节
5. git status（确认工作树干净）+ 跑一遍当前测试基线，记录起点是否已绿
6. 确认本次改动的"最小范围"：列文件清单，写进提交说明
```

第 5 步是硬要求：**不知道起点是否绿，就不能声称自己弄绿了**。

---

## 1. 分层与依赖方向

```text
L0 UI/平台  →  L1 host/cli  →  L2 领域服务  →  L3 基础设施  →  L4 core  →  L5 外部世界
```

只允许向下依赖。唯一"向上"通道：`notera-host` 的事件总线（订阅，非调用）。

---

## 2. 模块注册表

| crate / 目录 | 职责（一句话） | 允许依赖 | **禁止**依赖 | 主测试层 | 阶段 |
|---|---|---|---|---|---|
| `notera-core` | 类型、ID、时钟、错误词表、不变式断言 | （无） | 任何 crate、任何 IO/async | L0 | 1 |
| `notera-richtext` | 文档模型、normalize/validate/canonical、派生、三方合并 | core | store/net/sync（不得知道同步） | L0（属性测试） | 1,2 |
| `notera-crypto` | 信封 seal/open、sha256、argon2id | core | store/net/sync | L0 | 2 |
| `notera-store` | SQLite、迁移、仓储、FTS、outbox 持久化、tombstone | core, richtext, crypto | net, webdav, sync（**存储层不知道同步**） | L1 | 1 |
| `notera-net` | 唯一 HTTP 出口：代理、TLS、超时、退避、审计 | core, config | webdav, sync, store | L1 | 2,3 |
| `notera-webdav` | DAV 语义、能力探测、原子写、路径安全 | core, net, crypto, sync（**仅端口契约**：`RemotePort`/`Commit`/`RemoteError`/`EntryRef`） | store, host；把同步判定搬进适配器 | L1,L2 | 2 |
| `notera-sync` | 状态机、plan、push/pull、冲突编排、幂等、租约 | core, richtext, crypto, store, webdav, config | UI、平台 API | L1–L4 | 2 |
| `notera-config` | 设置、账户、代理 profile、凭据引用 | core, store | sync, webdav | L0,L1 | 1,3 |
| `notera-importer` | 导出/导入/备份/恢复 | core, richtext, crypto, store, sync | net, webdav（复用 sync，不自己发请求） | L1,L3 | 7 |
| `notera-host` | 装配根、用例编排、调度、事件总线、Commands、启动自检 | 以上全部 | 直接 SQL、直接 HTTP | L1,L5 | 1,4 |
| `notera-cli` | 诊断与 E2E 驱动（sync-once / net probe / verify） | host 及其全部 | 独立实现任何逻辑 | L3,L4 | 2 |
| `notera-test-webdav` | 真实 HTTP + 故障注入（仅 dev/test） | 无（独立实现 DAV 子集） | 不得被产品 crate 依赖 | 测试基建 | 1末,2 |
| `apps/desktop/src` | 三栏 UI、编辑器 View、命令构造、4 态徽标 | 仅经 Commands/事件与 host 交互 | **reqwest/webdav/协议词汇** | L5 | 4 |
| `platform/*` | 窗口、托盘、菜单、通知、后台任务、分享、钥匙串 | Tauri 插件 + 原生 API | 业务规则、同步判定 | L5,L6 | 4,5 |

**判据**：若一个 crate 需要知道"同步"这件事，它就不该知道。业务规则放领域层，否则 Rust 侧测不到。

---

## 3. 不变式（违反即 P0，与是否"功能正常"无关）

### 数据层（DATA-MODEL §0）

| # | 不变式 |
|---|---|
| I1 | 可同步实体拥有稳定 ID，跨设备不变，删除后永不复用 |
| I2 | 内容变更必产生严格递增的 `rev`（`rev = max(rev, remote_rev)+1`） |
| I3 | 删除是状态不是消失；删除事实必须比数据活得更久 |
| I4 | 同步判定不依赖 UI、不依赖 mtime、不依赖墙上时钟相等 |
| I5 | 派生数据（title/plain_text/summary/FTS）与权威数据同事务更新 |
| I6 | 校验失败的数据永不进入权威表 |
| I7 | 未知字段/节点/属性必须原样保留；`doc.v` 超前 → 只读 |
| I8 | 任何本地写入不依赖网络可达 |

### 协议层（SYNC-PROTOCOL §0）

| # | 不变式 |
|---|---|
| R1 | 记录文件是正确性来源，清单只是索引与公告 |
| R2 | 写入顺序固定：实体记录 → 附件 → 清单 |
| R3 | 远端状态判定只用 `rev` + `content_hash` |

### 冲突与删除

| # | 不变式 |
|---|---|
| C1 | 用户输入过的内容不会因同步/冲突/崩溃/他设备操作而静默消失 |
| C2 | 远端缺失（404）永不导致本地删除，只导致补传 |
| C3 | 服务器侧异常（清单大面积消失）触发人工确认闸门，不自动等价于"用户删了" |
| C4 | 删除 vs 修改必进冲突收件箱，且本地内容先完整保留 |

### 网络与平台

| # | 不变式 |
|---|---|
| N1 | 所有 HTTP 出口唯一收敛于 `notera-net`（无旁路） |
| N2 | 网络/代理/证书故障不阻塞本地任何操作，也不改变 outbox 内容 |
| P1 | 首帧渲染路径上不存在网络等待 |
| P2 | 平台差异只出现在 `platform/*` 与 host 的能力协商，不进领域层 |

---

## 4. 改动路由表（要做 X → 改哪里 / 必须同时改什么 / 不能碰什么）

| 想做的事 | 主改位置 | 必须同步改 | 禁止 |
|---|---|---|---|
| 加一个笔记字段（同步） | `migrations/00NN_*.sql` + `notera-store` 行映射 + `notera-core` 类型 | `notera-crypto` 信封、`notera-sync` plan 判定、契约图字段表、TEST-PLAN 契约用例、`fixtures/envelope-v1.json` | 只改 UI 不改库；在 UI 里算派生值 |
| 加一个笔记字段（不同步） | `notera-config`（`settings`，scope=device/ui） | 归类说明写进 DATA-MODEL §6 | 放进 `notes` 表并"顺手"上传 |
| 改富文本节点类型 | `notera-richtext`（+ `doc_format` 迁移） | canonical 序列化测试、preserve-unknown 用例、编辑器映射 | 在同步层特判节点类型 |
| 改冲突策略 | `notera-richtext::merge` + CONFLICT-RESOLUTION.md | 属性测试（任意三方输入必过 validate）、§1.2 排除表、UI 收件箱 | 为"少弹冲突"而放宽检测 |
| 改清单结构 | `notera-sync::manifest` + SYNC-PROTOCOL §4 | 尺寸实测、压实逻辑、D1–D4 恢复阶梯、契约图 `Manifest` 字段、fixtures | 让清单成为正确性来源（违反 R1） |
| 改写入原子性 | `notera-webdav`（S1/S2/S3） | `cap_mask` 探测、崩溃点矩阵 C1–C10、412 重算路径 | 引入 `LOCK` 作为正确性依赖 |
| 加代理模式 | `notera-net` + `notera-config` | PROXY.md §2 能力表、RouteProof、差分测试、`net probe` | 在 `notera-webdav` 里处理代理 |
| 加平台能力（托盘/后台…） | `platform/<os>` + `PlatformCaps` | host 能力协商、UI 按能力渲染、PLATFORM.md §2 矩阵 | 在 UI 判断机型 |
| 加 Tauri 命令 | `notera-host::commands` + `apps/desktop/src/api` | DTO 类型、错误→UserReason 映射、命令契约测试 | 在命令处理器里写业务规则或等网络 |
| 改搜索行为 | `notera-store::search` | 双路径阈值（≥3 字 MATCH / ≤2 字 LIKE）、性能预算、FTS 重建与 verify | 把 <3 字查询交给 MATCH |
| 改同步定时 | `notera-host::SchedulerPolicy` + SYNC-PROTOCOL §14 | 空轮 1 请求 0 字节断言、移动端诚实预算 | 把周期调短来"假装更快" |
| 加测试 | `tests/<层>` | 该用例的注入方式与后置条件断言写清 | 用 mock 替代真实 HTTP/真实进程崩溃 |

---

## 5. 禁止模式（CI grep / 依赖闸门强制）

| 禁止 | 为什么 | 闸门 |
|---|---|---|
| UI 层出现 `WebDAV\|ETag\|manifest\|tombstone\|revision\|pull\|push` | 用户不该感知同步 | eslint + grep |
| `notera-net` 之外出现 `reqwest::Client\|hyper::\|rustls::ClientConfig` | 出口唯一（N1） | Cargo 依赖图 + grep |
| 任何地方用文件 mtime / `updated_at` 比较判定新旧 | 时钟不可信（I4/R3） | code review + 属性测试 |
| `notera-store` 依赖 `notera-sync` | 存储不知道同步 | `cargo tree` 断言 |
| 运行时 `ALTER TABLE`（迁移外） | 迁移契约 | grep |
| 直接 `UPDATE notes` 绕过 `commit_*` | 派生列会漂移 | grep + 架构测试 |
| 用 `unwrap()` 处理外部输入/网络/DB 结果 | 崩溃面 | clippy `-D clippy::unwrap_used`（业务代码） |
| `#[ignore]` / skip 测试来让 CI 变绿 | 隐藏问题 | CI 统计 ignored 数并告警 |
| 提交信息里写"CI 已通过"而无 Actions 运行 URL | GitHub 本机不可达（B4） | 提交规范 + 本文 §8 |
| 把 mock 当真实 HTTP 测同步 | 需求明令禁止假 API | 测试层约束：L2 以上必须真 socket |
| 在 UI 线程调用 argon2id / 全量校验 | 实测 ≈400 ms，会卡 | 性能断言 |
| 让清单成为正确性来源（如"清单没有就当用户删了"） | 违反 R1/C2/C3 | 协议测试用例 |

---

## 6. 契约冻结点（跨版本不可破坏）

| 契约 | 载体 | 变更规则 |
|---|---|---|
| 信封字段集 | `records/<kind>/<id>.json` | 加可选字段=次版本；改语义/删除=破坏性→`protocol`+1 |
| 清单结构 | `manifest/index.json` | 同上；`checksum` 自校验必须向后兼容 |
| 远端目录布局 | `.notes/**` | `protocol.json.layout` 字段控制 |
| 富文本 schema | `doc.v` + `doc_format` 列 | 未知节点必须 preserve（I7） |
| `rev` 语义 | 全协议 | 永不改为"时间戳"或"哈希" |
| ID 形态 | UUIDv7 字符串 | 长度与字符集不变（路径安全） |
| 错误词表 | `notera-core::ErrorCode` | 新增值必须同时补：用户文案 key、retryable、是否阻塞本地 |
| 同步 4 态 | UI `SyncBadge` | 不得增加第 5 态（协议细节一律折叠） |

金样本在 `fixtures/`，双向兼容测试在 `tests/contract/`：新客户端读旧样本、旧客户端读新样本（只读降级）。

---

## 7. 决策索引（ADR）

| # | 决策 | 状态 |
|---|---|---|
| 0001 | 技术栈与分层（Tauri 2 + Rust 核心 + Vue 3，依赖方向） | Accepted |
| 0002 | 记录信封 v1 即预留 E2EE 结构（`enc.alg=none`） | Accepted |
| 0003 | WebDAV 布局：每实体一文件 + 先实体后清单 | Accepted |
| 0004 | 清单两段式（基线分段 ⊕ 变更窗口）+ 自校验 + prev 回退 | Accepted |
| 0005 | Revision：Lamport 式单调整数 + 内容哈希，不用向量时钟 | Accepted |
| 0006 | Tombstone 不自动 GC；文件夹删除不级联删笔记 | Accepted |
| 0007 | 冲突：块级三方合并，失败保留双方 + 冲突副本 | Accepted |
| 0008 | 富文本独立模型，编辑器只是 View | Accepted |
| 0009 | 附件 SHA256 内容寻址 + 独立队列 | Accepted |
| 0010 | 统一网络层与 App 级代理（含可证伪测试方法） | Accepted |
| 0011 | 平台能力抽象与后台同步诚实预算 | Accepted |
| 0012 | SQLite 迁移契约（forward-only + user_version + 迁移前备份） | Accepted |
| 0013 | 自建真实 HTTP 测试 WebDAV 服务器 | Accepted |
| 0014 | 构建矩阵与版本策略（三平台产物 + 单一版本源） | Accepted |
| 0015 | 检索：FTS5 trigram + 短查询 LIKE 双路径（实测驱动） | Accepted |
| 0016 | 本地静态加密（SQLCipher） | **Proposed**（待内网合规要求确认） |
| 0017 | 多窗口并发编辑同一条笔记 | **Proposed**（Phase 4 决策） |
| 0018 | 单一活跃同步账户；多服务器推迟到"按账户确认点"落地 | Accepted |
| 0019 | 外部参照：Joplin 同步与 AppFlowy 编辑交互，采纳与拒绝 | Accepted |

---

## 8. 当前状态快照

**阶段**：Phase 1–5 的核心已落地，Windows x64（GNU 工具链）debug 构建**可运行**；macOS/Android/iOS 仅有架构预留，未在本机验证。

**实测基线**（2026-09-26，本机 `stable-x86_64-pc-windows-gnu`）：

| 门禁 | 结果 | 怎么复现 |
|---|---|---|
| Rust 测试 | 389 通过 / 0 失败 / 0 ignored（44 个测试二进制） | `cargo test --workspace` |
| 前端 | 121 通过（12 文件）、`vue-tsc` 无错误、构建 199 KB→gzip 68 KB | `npm --prefix apps/desktop test` / `run typecheck` / `run build` |
| 架构适应度 | 18/18 | `node scripts/arch-check.mjs` |
| 契约图 | 59/59，交互后无运行时错误 | `node scripts/verify-diagram.mjs` |
| 浏览器端到端 | 25/25（真 Rust 核心，非 mock） | `notera-cli serve` + `npm run dev` + `node scripts/verify-app.mjs` |
| 真窗口 | 8/8（invoke 建笔记→落库→刷新读回→点开正文，控制台 0 error） | `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9223 target/debug/notera-desktop.exe` + `node scripts/verify-tauri-window.mjs` |

**已建立**：12 个 crate + `apps/desktop`（Tauri 壳 + Vue 前端）+ `migrations/0001..0005` + 自建测试 WebDAV 服务器 + 上述四套验证脚本 + `docs/` 全套规格与 ADR-0001…0019。

**尚未做，且明确不算完成**：

| 项 | 状态 | 缺什么 |
|---|---|---|
| OS 钥匙串接入（`credential_ref`） | 未实现 | Phase 5 平台工作；当前只有 debug 构建下的 `NOTERA_DEV_WEBDAV_USER/SECRET`，release 一律进 `needs_credentials` |
| 协议 §5 能力探测、§11.3 租约 | 未接入 | `RemotePort` 这 7 个方法表达不了它们；需要扩端口或加 host 侧编排 |
| 多服务器同时启用 | 按 ADR-0018 拒绝 | 确认点 `sync_rev` 是全局列，需要迁到按账户表 |
| 备份/恢复、`.enex` 结构化导入 | BLOCKED | 前者需要 `rusqlite/backup` 特性（依赖变更需评审），后者需要 XML 依赖 + ENML 映射与夹具 |
| macOS / Android / iOS 产物与签名 | BLOCKED | 需要对应硬件、证书与工具链；本机只有 Windows |
| CI workflow 文件 | 未创建 | 推送渠道待决（ARCHITECTURE-REVIEW §14）；本地等价检查已全部脚本化 |
| 真实公网 WebDAV 端点验证 | 未做 | 需要一个可写的真实服务器（现仅对自建测试服务器验证） |

**阻塞项**：B1 MSVC 链接器（已用 GNU host 绕行）· B3 无 Android 工具链 · B4 GitHub 不可达 · B5 无真实 WebDAV 端点 · B6 iOS 证书 · B7 商标核查。详见 ARCHITECTURE.md §8。


---

## 9. 变更协议（需求与架构冲突时）

**禁止**自行修改架构来迁就需求。必须：

```text
STOP
 → 在 docs/adr/ 新建编号 ADR（状态 Proposed），写明：
     冲突点 · 现有架构为何这样 · 需求为何要改 · 影响面（数据/协议/测试/工期）· 备选方案与代价
 → 在本文件 §7 索引登记
 → 向人说明并等待决定
 → 决定后：更新受影响的规范文档 + 契约图 + 测试矩阵，再动代码
```

同样禁止"顺便"：顺手重构、顺手升级依赖、顺手换编辑器、顺手改协议 —— 一律走上面的流程。

每个 Phase 必须输出：目标 / 范围 / 不做什么 / 修改文件 / 架构影响 / 测试计划 / 实现 / 测试结果 / 风险 / Git Commit / 下一阶段。
