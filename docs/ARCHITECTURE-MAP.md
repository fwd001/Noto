# ARCHITECTURE-MAP

> **本文件是 Notera 的长期架构记忆。**
> 它的作用不是描述代码，而是让任何一次开发（包括下一次会话里的我）在动手前就知道：边界在哪、契约是什么、改这里会牵动什么、什么绝对不许做。

---

## 0. 开工前必读清单（每次，不可跳过）

```text
1. 读本文件（模块注册表 §2 · 不变式 §3 · 改动路由 §4 · 禁止模式 §5）
2. 读当前 Phase 与其出口条件（ARCHITECTURE.md §10 · TEST-PLAN.md §阶段出口条件）（另：分领域验收状态与 BLOCKED 项见 IMPLEMENTATION-STATUS.md）
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
| `notera-webdav` | DAV 语义、能力探测、原子写、路径安全 | core, net, crypto, sync（**仅端口契约**：`RemotePort`/`Commit`/`RemoteError`/`EntryRef`/`PeerLease`，以及 `Manifest` —— 后者是 CAS 提交的清单**自校验**，解析必须由 schema 属主做，适配器自带第二份解析器才是风险。本 crate 不得 re-export `SyncEngine`，引擎入口只有 sync 一处） | store, host；把同步判定搬进适配器 | L1,L2 | 2 |
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
| 命令面直接把 store/core 的类型序列化给界面 | 存储层字段名会漏到 wire 上，而界面按契约名取值 → 静默 `undefined`（曾让设置页三行统计恒为 `—`） | `edge:command-wire-is-camelCase`（出参类型必须显式声明 camelCase）+ `edge:stats-dto-covers-ui-reads`（界面读的每个键都要发得出）+ 真 `invoke` 的键集合断言 |
| 跨 host↔store 边界传 `kind` 用裸字符串 | 同一实体有两套词汇（线上短标记 `n/f/a` ↔ 库里长标记 `note/…`），传错词汇编译能过、UPDATE 匹配 0 行、待办静默停在 inflight | 参数类型是 `EntityKind`（词汇翻译只能在适配器一处发生） |

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
| Rust 测试 | 483 通过 / 0 失败 / 0 ignored（56 个测试二进制） | `cargo test --workspace` |
| 前端 | 171 通过（18 文件）、`vue-tsc` 无错误、构建 213 KB→gzip 73 KB | `npm --prefix apps/desktop test` / `run typecheck` / `run build` |
| 架构适应度 | 24/24（最后一条是"扫描台账"：任何源码门禁扫到 0 个文件即判失败 —— 此前有 8 条空转了很远，见 CHANGELOG） | `node scripts/arch-check.mjs` |
| L5 崩溃注入 | 小库矩阵 9 点 + 大库压实 1 点，逐个杀死真子进程 + 重启收敛（`crash_recovery` 2 条 + `compaction_crash` 1 条，名单由 `CRASH_POINTS_NEED_LARGE_LIBRARY` 减法拼回全表） | `NOTERA_CRASH_AT=<点> cargo test -p notera-host --test crash_recovery --test compaction_crash` |
| 附件续传 + Range 兼容 | 2/2（一条真杀进程重启接着要、一条让服务器**不理** Range 头看它当不当整份覆盖） | `cargo test -p notera-host --test attachment_resume` |
| 清单压实 + 分段读回 | 1/1（>200 条变更：分段落盘、索引不引用不存在的分段、空库设备追平 260 条） | `cargo test -p notera-host --test compaction` |
| 大库换设备追平 | 1/1（260 条变更 > 窗口上限：空库设备完整收敛、标题+内容哈希逐条一致） | `cargo test -p notera-host --test late_device` |
| 链路抖动收敛（§53 主循环） | 1/1（六轮各坏一次：停监听 / 建连就掐 / 读清单 500；恢复后本机账目归零，两台设备标题+内容哈希逐条一致） | `cargo test -p notera-host --test reconnect` |
| 契约图 | 59/59，交互后无运行时错误 | `node scripts/verify-diagram.mjs` |
| 浏览器端到端 | 35/35（真 Rust 核心，非 mock；含"设置页存服务器 → 能力块读回"、"库统计五行全是数字"、"删除 → 回收站 → 恢复 → 永久删除"、"勾一个文件夹 → 包就只有那一棵子树"、"侧栏建子文件夹 → '移动到'选得到"五条真实往返） | `notera-cli serve` + `npm run dev` + `node scripts/verify-app.mjs` |
| L5 纯黑盒 UAT | 10/10（只用点击/输入/键盘/文件选择器/刷新，零 `/cmd/*`、零读库；含"插图后屏幕上真的解出像素、刷新后仍在"） | `node scripts/verify-blackbox.mjs` |
| 真窗口 | debug 8/8 **且 release 8/8**（开发服务器关闭 → 资源走内嵌 `frontendDist`，页面 `http://tauri.localhost/`；`stats` 键集合 == 契约那 8 个 → 建笔记→落库→列表刷新读回→点开正文→截图，控制台 0 error） | `pnpm build` + `cargo build [--release] -p notera-desktop` + `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9223` + `node scripts/verify-tauri-window.mjs` |

**已建立**：12 个 crate + `apps/desktop`（Tauri 壳 + Vue 前端）+ `migrations/0001..0006` + 自建测试 WebDAV 服务器 + 上述四套验证脚本 + `docs/` 全套规格与 ADR-0001…0019。

**尚未做，且明确不算完成**：

| 项 | 状态 | 缺什么 |
|---|---|---|
| OS 钥匙串接入（`credential_ref`） | 未实现 | Phase 5 平台工作；当前只有 debug 构建下的 `NOTERA_DEV_WEBDAV_USER/SECRET`，release 一律进 `needs_credentials`。`caps.keychain` 已改口为 `none`（此前对 Windows 报 `credential_manager`，是要用户误信"口令进钥匙串了"） |
| 托盘 / 原生菜单 / 全局快捷键 / 通知 | 未实现 | 壳里一行相关代码都没有，但 `caps` 曾对 Windows 全报 true → 设置页摆出"关闭窗口时留在系统托盘"这种存了没人读的开关。现已按 as-built 报 false，UI 显示"此平台不可用"。**要恢复需评审**：分别需要 `tauri` 的 `tray-icon` 特性、`Menu::with_items`、`tauri-plugin-global-shortcut`、通知插件的实际调用 —— 都会动依赖图，按 §9 走，不"顺便"加 |
| 协议 §5 能力探测 | 已接入 | `notera-webdav/probe.rs` 五项探测 → `sync_accounts.cap_mask`；启动路径**先探后装**（`App::remote_for_sync`），所以本次会话就按实测策略写，不用等下次启动。探测失败只提示不降级（`sync.probeDeferred`），当天不重复探测。判定结果经 `AccountDto`（`capMask`/`writeStrategy`/`capsProbedAt`）显示到设置页的"服务器能力"块，S3 明确建议多设备串行编辑 —— §5 末行要求的正是这句话。`notera-cli dav-probe` 可强制重探并打印结论 |
| 协议 §11.4 尽力而为租约 | 已接入 | §11.2 第三层。开关由 §5 的探测结果决定：**S3 或探不到强 ETag 才开**（CAS 可信时白多两个请求没意义）。引擎在轮次开始贴自己的 `locks/<device>.json`（TTL 60s），在**写清单之前**看别人新不新鲜：新鲜就不提交清单，改动保持 dirty、状态显示 `sync.leaseHeld`，下一轮自动重来。读不到别人的租约 = 当作没人持有（这一层坏了绝不能变成永不同步）。不用 `LOCK`/`UNLOCK`。证据：引擎 7 例 + 适配器 6 例 + 两台设备真服务器 1 例 |
| 多服务器同时启用 | 按 ADR-0018 拒绝 | 确认点 `sync_rev` 是全局列，需要迁到按账户表 |
| 备份 / 恢复（§15） | 已实现 | `VACUUM INTO` 一致快照 + sha256/integrity_check 闸门 + 替换前留当前库 + 下次启动落地；恢复不在进程内换库（见 DATA-MODEL §15） |
| 导出 / 导入（ZIP bundle） | 已实现 | `notera-importer/bundle.rs`；导入走 `apply_remote` 同一条冲突安全路径，删除事实随包带走（防复活）。附件按**库**枚举（`Store::local_attachment_shas`）——曾经按一层目录名筛 64hex，而 blob 在 `<attachments>/<2hex>/<sha>` 两层里，于是"含附件的导出"其实一个附件都没有（见 CHANGELOG） |
| 编辑器"插入图片 / 附件"的入口 | **已接通**（按 D11 的 ③ 走） | 隐藏的 `<input type=file>` 取文件（WebView 里就是系统原生选择器），前端只负责把 File 变成 base64；sha256、落盘、`attachments`/`note_attachments`、上传队列仍全在核心。零新依赖，且**浏览器 dev 桥与真窗口是同一条代码路径** → 端到端真的点了一遍（`verify-app` 第 15 步）。形状集中在 `editor/attachmentWire.ts`；显示用的 data URL 只活在内存表里，**绝不写进块属性**（那等于把附件在正文里再存一份并跟着同步走）。顺序坑也在这里：首次挂载会翻转 `has_attachment` 并**推进笔记 rev**，所以必须"先落编辑 → 再写附件 → 接住新 rev → 才把块写进正文" |
| 冲突并排预览 `preview_text` | 已实现 | 前端一直在调、核心一直没有这条分支 → 每次 `unknown_command`，被"退回卡片摘要"的兜底盖住了。现在按 `(id, rev)` 取 revision 抽纯文本，并加门禁 `edge:declared-commands-exist`（前端声明的每个命令名，核心必须有分支） |
| 按文件夹部分导出 | 已实现 | **两个集合两种用途**：`Store::folder_closure`（子树 + 祖先链）只决定"哪些文件夹行要进包"（缺祖先就是外键接不上的废包），`Store::folder_subtree`（子树，不含祖先）决定"哪些内容算这一棵"——笔记按父本是否在子树里筛，附件按这些笔记筛（`attachment_shas_in_folders`）。曾用同一个闭包筛内容，于是勾一个子层会把默认本里那篇无关笔记连它的图片字节一起带走。包自己声明 `manifest.partial`，**因此禁止**用它走"仅在空库时导入"：`tombstones` 不记父本，笔记的永久删除公告无法归属到文件夹，当成整库还原就会让已删的笔记从别的设备回流（§8 硬性要求 6）。范围里出现未知文件夹 id → 拒绝，不是忽略 |
| 文件夹树的跨语言形状 | 已对齐 | `/cmd/list_folders` 下发**嵌套树**（`children`），前端 `stores/folders.ts::buildTree` 必须两种形状通吃（树 / 平铺）：它曾经先清空 `children` 再按顶层数组重建，等于把树里的子层全部丢弃 → 侧栏、"移动到"下拉、导出选择器一起失去子文件夹，而唯一的三级树测试喂的是平铺输入所以照绿。现在契约测试用真桥原样输出的 JSON；`flattenTree` 的 64 项上限见 CHANGELOG 已知限制 |
| 附件的第二条登记入口：从**清单**读引用 | 未做（已知缺口） | doc 这条路现在是通的（收到记录 → 按块上的 `sha256` 登记，见 DATA-MODEL §8，并有跨设备真服务器测试）。剩下的窗口是：A 上 `attach_blob` 成功了、但把引用写进正文的那一次保存没发生（崩溃 / 强杀）—— blob 已上传、`note_attachments` 有行，而**没有任何 doc 引用它**，B 侧因此无从得知。影响：A 上留一个既不回收也不分享的孤儿（不是数据丢失，也不覆盖任何东西）。补法需要 SYNC-PROTOCOL §13 的清单侧附件条目 + 在清单解析处调 `Store::register_remote_attachment`（那个函数的注释本来就写着"清单/记录里读到引用时调用"，清单那一半一直没人做）—— 属于协议改动，走 §9 评审 |
| 附件下载续传（§8 硬性要求 5 / §13） | 已实现 | `WebDavRemote::fetch_attachment_window` 在 §5 探到 `RANGE` 位时**每个窗口都带 Range**（包括第一窗，写死 `from>0` 才发会让首轮整块拉回、把"续传"变成一句空话）。响应解释只有一条判据：**没有 206 就是我们没拿到分片** —— 偏移一律按 0（服务器不支持 Range、或探测老实而正式请求被一个不认 Range 的节点接走，两种都给全文）。host 侧按 4 MiB 一片落到 `<blob>.part`：偏移 0 ⇒ **覆盖**，偏移等于手头长度 ⇒ 追加；拼完先自己核对 sha256，**不符就丢弃半截文件**并把账记成 failed，对得上才 `ingest_blob` 落成正式对象并删 `.part`。三条不可让步的点：没拼完之前**正式 blob 绝不出现**（否则半截文件被当成完整附件同步给别人）、outbox 在有进展但未完成时**保持 pending**（下一轮接着要，不重头再来）、`.part` 比对象总长还大（换过内容 / 上次崩在追加中间）先删再要。证据：`attachment_resume.rs` 两条 —— 真杀进程重启接着要（第二轮恰好一个 206），以及 `FAIL(ignore-range)` 下整份答复被当整份覆盖（字节不多不少）。变异验证两处：摘掉 `want_range` → 第一条红；拆掉"非 206 报 0" → 第二条与 4 条偏移判定一起红 |
| 附件分片上传 | 未做（已知缺口） | 下载侧已经是窗口化续传，**上传侧仍是一次 PUT**：一个 20 MiB 附件传一半断了，下一轮整份重传。补法要在 SYNC-PROTOCOL §13 里定"服务端如何容忍半截对象"（`.tmp-*` + `MOVE`，还是 PATCH/append 且需要新的能力探测位）—— 属于协议改动，走 §9 评审，不在这里顺手发明 |
| 冲突：远端那一版真的来到本机 | 已实现（本轮补上） | 判出 `UpdateUpdate` 时引擎**真的去取那条记录**并发 `ApplyOp::AdoptConflict`：`apply_remote` 的"冲突采纳"分支是唯一允许 `rev` 相等而内容不同的写入口（前提是本机那份已先存成副本笔记），采纳后 `rev == sync_rev` 因此本机不会把自己那一版推回去盖掉别人。面板两栏：右 `(noteId, remoteRev)`、左 `(copyNoteId, copyRev)`。证据：两台设备 + 真 TCP 服务器的分叉测试（去掉采纳就红）。仍欠的一块记在 CHANGELOG §已知限制：P11（删除 vs 修改）的服务器那一版同样没来到本机，右栏只有哈希。`用我这一版` 已不是空操作 —— `swap_conflict_sides` 真的把正文与副本互换并重新公告 |
| `.enex` 结构化导入 | 未实现 | 需要 XML 依赖 + ENML 映射与夹具 —— 动依赖图，按 §9 走人工评审 |
| macOS / Android / iOS 产物与签名 | BLOCKED | 需要对应硬件、证书与工具链；本机只有 Windows |
| CI workflow 文件 | 未创建 | 推送渠道待决（ARCHITECTURE-REVIEW §14）；本地等价检查已全部脚本化 |
| 真实公网 WebDAV 端点验证 | 未做 | 需要一个可写的真实服务器（现仅对自建测试服务器验证） |
| 端到端出现过一次 `edit_note → 400 stale_edit（expected 7，actual 8）` | **已定位并修掉** | 定位手段是给门禁加上"失败的 4xx 发生在哪一步"的归位信息；新加的附件步骤一复现就是它：核心**首次**挂附件会翻转派生列 `has_attachment`，而那是同一事务里推进笔记 rev 的动作，编辑器排队的自动保存却还带着旧 rev 出发 —— 用户插一张图，得到的是"这条笔记在别处被改动了"并把界面切走。修法是把顺序钉死：先落自己的编辑 → 再让核心写附件 → 接住 `attach_file` 回的新 rev → 才把附件块写进正文（`stores/editor.ts::attachFile`，带测试）。当时排除掉的两个假设也留档：自导入推 rev（证明幂等，包导回同一个库 rev/hash 一字不动）、并发 save（`saveChain` 已串行且 `doSave` 执行时才读 `rev.value`） |

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
