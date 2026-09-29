# Notera 测试计划（Phase 0 — Architecture）

| 项 | 内容 |
| --- | --- |
| 适用产品 | Notera（working name）：local-first 跨平台笔记，Tauri 2 + Rust core + Vue 3/TypeScript + SQLite，同步后端为用户自有 WebDAV |
| 首发目标 | Windows x64 / macOS arm64 / Android arm64-v8a；iOS 15+ 仅架构预留，Phase 1 不构建 |
| 优先级（不可下调） | 数据安全 > 同步正确性 > 稳定性 > 用户体验 > 原生平台体验 > 性能 > 可维护性 > 新功能 |
| 实测证据源 | `docs/evidence/probe-windows-gnu.txt`（16 项黑盒探测，Windows 11 x64 10.0.26200，rustc 1.98.1，host `x86_64-pc-windows-gnu`）；下文以 `PROBE:<check-id>` 引用具体行 |
| 网络现实 | 本机可达 crates.io / npm；**github.com 与 api.github.com 不可达（HTTP 000）**。凡依赖 GitHub 的执行结果一律标 `BLOCKED`，禁止"假定已绿" |

## 测试哲学

1. **行为优先，黑盒优先**。测试断言"用户能完成什么、能看到什么"，不断言"代码怎么写的"。私有函数签名、内部数据结构布局、模块内部分层都不构成独立的 PASS 判据；它们只在没有等价黑盒判据时才作为辅助证据。
2. **数据库检查是辅助证据，不是判据**。查 SQLite 表、看 `sync_operations`、dump manifest 用来**定位失败原因**和**证明不可见副作用**；对外可观测结果（列表条目数、渲染节点数、另一台设备上的内容）才是 PASS 条件。任何只有 DB 断言、没有用户可见断言的行，视为未完成。
3. **不存在"部分通过"**。测试状态只有 `PASS` / `FAIL` / `BLOCKED` 三种。
4. **SKIP / TODO / `#[ignore]` / "理论通过" / "按代码推演应当正确" / "仅手工测试过" 一律不计为 PASS**。手工观察要成为 PASS，必须转成可重放的脚本化步骤（L5 Playwright）并留下产物（截图/日志/`/_fs/dump`）。
5. **不可测的东西要显式挂账**。缺少条件时标 `BLOCKED` 并写清 **原因 / 影响 / 所需条件**（见 `## 待人工确认`），不允许通过弱化断言、缩小输入域或改成"只验证不 panic"来消音。
6. **低优先级永远不得让路于高优先级**。为了让 L5 变绿而放宽一条数据安全不变式，是缺陷不是修复。同优先级冲突时取更严的一侧。
7. **HTTP/WebDAV 层禁止 mock**。所有涉及协议往返的测试都走真实 socket（`notera-test-webdav`，127.0.0.1 真实 TCP + 真实 TLS/自签 CA 用例）。协议 bug 只能被真实实现抓到。
8. **静默覆盖 = P0**。任何一次"用户输入内容在无提示、无冲突副本的情况下被远端或本地写覆盖"，无论多小、多难复现，都定级 P0 并阻断发布。

## 测试分层

### L0..L6

| 层 | 名称 | 目的 | 依赖 | 运行位置 | 期望耗时上限 | 进 PR 门 |
| --- | --- | --- | --- | --- | --- | --- |
| L0 | 单元（Rust + TS） | 纯函数/领域模型/错误映射/序列化；不含 IO | `cargo test --workspace --lib`、vitest | 本机 / CI job A | 90 s | 是（CI 执行：`BLOCKED`，见 §待人工确认 Q1） |
| L1 | 组件 | store(migration/repository/FTS5/tombstone) 与 sync 状态机在**真实临时 SQLite + 进程内 test-webdav（127.0.0.1 真实 TCP）+ fake clock** 下的行为 | `notera-test-webdav`、tempdir、`notera-store` test fixtures | 本机 / CI job A | 3 min | 是（同上 BLOCKED） |
| L2 | 协议契约 | golden JSON fixture 逐字节锁定 envelope / `protocol.json` / `manifest/index.json` / `manifest/seg-*.json`；schema 校验；**双向兼容**（新客户端读旧 fixture + 旧客户端读新 fixture） | `tests/fixtures/protocol/**`、`notera-richtext` canonical JSON | 本机 / CI job A | 2 min | 是（协议变更时强制） |
| L3 | 多客户端 E2E | Client A/B/C 三个独立进程（`notera-cli sync-once`）对同一 test-webdav 收敛 | L1 环境 + 三个隔离数据目录 | 本机 / CI job B | 全量 8 min；smoke 子集 90 s | smoke 是、全量 nightly |
| L4 | 崩溃 / 杀进程 | 在每个已记录提交点中途 `exit`/`taskkill /F`，重启后断言不变式 | `NOTERA_CRASH_AT` 调试钩子（仅 test feature 编译）+ fs 模式 test-webdav | 本机 / CI job B | 6 min | 否；nightly + 阶段出口必跑 |
| L5 | 黑盒 UAT | Playwright 驱动 Tauri WebView：只允许点击、输入、重启 App、切换网络。**禁止**直接调 Tauri command、禁止读写 DB 来"帮它通过" | `apps/desktop` release/debug 包 + WebView2 | 本机（Windows）/ CI job C（macOS/Android 手工清单 BLOCKED） | 单轮 20 min | smoke 10 条是（BLOCKED） |
| L6 | 平台构建冒烟 | 三个 target 各自：构建 → 安装 → 起窗 → 打开 SQLite → 完成一轮真实 sync（对 test-webdav）→ 退出 → 再启动 | toolchain + 真机/模拟器 | 本机（Windows-gnu）+ CI job D | 25 min | 否；发布门必绿 |

### crate → 测试面映射

| crate | 可测面（用户可见后果） | 主层 | 关键行 |
| --- | --- | --- | --- |
| `notera-core` | ID 唯一/单调/路径安全（UUIDv7）、时钟偏移容忍、错误分类不丢失 | L0 | SY-ID-01/02，INV-07 |
| `notera-richtext` | canonical JSON 稳定、三方合并、纯文本抽取、未知节点保留 | L0/L2/L3 | RT-*、FWD-*、SY-MRG-01/02 |
| `notera-store` | migration 升降级、FTS5 与 LIKE 兜底、tombstone 不复活、事务原子性 | L1/L5 | SRCH-*，FT-IO-*，INV-04/08（PROBE:`wal-tx-atomicity`、`migration-user_version`） |
| `notera-crypto` | envelope `aes-256-gcm-siv` nonce/长度/篡改拒绝、sha256 已知向量、argon2id 预算 | L0/L2 | INV-12，PERF-08（PROBE:`envelope-aes-256-gcm-siv`、`envelope-tamper-detected`、`sha256-known-answer`、`kdf-argon2id`） |
| `notera-net` | 代理生效、超时/取消、退避重试、自签 CA 握手 | L1/L3 | SY-FAULT-*，PERF-11（PROBE:`proxy-config-actually-honored`、`timeout-and-cancel`、`https-tls-handshake`） |
| `notera-webdav` | PROPFIND/PUT/MOVE/DELETE、原子写（`.tmp-*` → MOVE）、校验、`If-Match` 语义 | L1/L3/L4 | CP-01..04（PROBE:`webdav-custom-verbs`、`precondition-412-plumbing`） |
| `notera-sync` | manifest 提交点、rev/base/hash、冲突判定、outbox、状态机 | L1/L2/L3/L4 | SY-*、INV-* |
| `notera-config` | 首配/改 URL/换账号/配置损坏回退 | L1/L5 | FT-SETUP-*，SY-ID-03 |
| `notera-importer` | 导出/导入/备份/恢复的无损性与可中断性 | L1/L5 | FT-IO-*，CP-09 |
| `notera-host` | 启动顺序、后台任务不占 UI 线程、事件不外泄内部状态 | L1/L5 | PERF-01/08，FT-OFF-01 |
| `notera-cli` | `sync-once`、`diag` 输出可作为 L3/L5 的第三方裁判 | L3 | 全部 SY-* 的执行入口 |
| `notera-test-webdav` | 自身即被测对象：verb 覆盖、`Depth`/`Destination`/`Overwrite`、ETag、`If-Match`/`If-None-Match`（412/304）、chunked、`/_fs/dump` 一致性、`mem`/`fs` 两模式 | L0/L1 | 其自测必须与 `notera-webdav` 客户端测试分离编写 |

### 记法（test-webdav 控制调用）

| 记法 | 展开 |
| --- | --- |
| `FAIL(kind, target, times)` | `POST /_control/inject-failure`，`kind∈{status, abort, latency, corrupt-body, partial-write, hang, close-listener}`；`target` 为路径 glob；`times` 为命中次数（默认 1，防粘连） |
| `FAIL(status=401,target=.notes/**)` | 返回 401，不落盘 |
| `FAIL(abort,target=records/**)` | 接受连接后 TCP 中断（半写响应） |
| `FAIL(corrupt-body,target=.notes/manifest/index.json)` | 200 + 截断/坏 JSON/hash 不符 |
| `FAIL(partial-write,target=.notes/records/n/n1.json)` | 服务端**已落盘**但返回 500 |
| `FAIL(ignore-range)` | 摘掉请求里的 `Range` 头再交给处理器 ⇒ 答 **200 + 全文**。§14 兼容矩阵那一类"探测时答 206、正式请求被一个不认 Range 的节点接走" |
| `OFF` / `ON` | `POST /_control/stop` / `/_control/start`；`OFF(close-listener)` 模拟服务器不可达，`OFF(进程 kill)` 模拟主机消失 |
| `RESTART` | `fs` 模式下重启服务器进程（保留权威状态），用于断言客户端不依赖服务端内存态 |
| `DUMP` | `GET /_fs/dump?prefix=.notes/` → 服务端权威文件清单 + 内容 hash（**唯一允许的服务端状态断言手段**） |
| `STATS` | `GET /_control/inspect` → 请求序列（method/path/请求头/字节数/响应码），用于断言"先写记录再写 manifest"、请求数、字节数、幂等重放次数 |
| `RESET` | `POST /_control/reset`，回到空 root prefix |
| `CRASH(点)` | 令被测进程在指定提交点前 `process::exit(9)`（非优雅），等价 `taskkill /F` |
| `A/B/C sync` | 在隔离数据目录运行的三个 `notera-cli` 实例各执行一次 `sync-once` |

**这张记法表自己的账（2026-09-27 逐项对过工装实现）**：
`hang` 的 `target=` 形态此前**不存在**（只有全局 `timeout_all`），已补成 `Injection::hang_for` +
`hang_on("GET *<path>")`，与 `status_for` 共用同一套规则文法（`["METHOD "]pathglob`），
并由 `notera-test-webdav/tests/injection.rs::hang_on_follows_the_same_rule_grammar_as_status_for`
钉住文法本身。**2026-09-28 同一套文法又补了 `abort` 的 `target=` 形态**（`Injection::abort_for` +
`abort_on("MOVE *")` / `abort_on("GET *<sha>")`）：此前 `FAIL(abort)` 只有按序号的 `drop_after_n`，
一开就从那一条起**全部**断连，造不出"只掐某一个动作"这一形 —— 而 §27 的「上传中断」要的恰恰是
"暂存写完了、只有发布被掐"。新旋钮与 `status_for`/`hang_for` **共用同一个 `rule_hit`**（同一个理由：
两套各写一遍迟早漂成两种语法），文法另有一条单测盯着
（`injection.rs::abort_on_follows_the_same_rule_grammar_as_status_for`，变异自证 M22 让它读错字段 → 红）。
这条加完**第一次跑就红在自己的前置上** —— 规则当时还没接进服务器的判定，MOVE 拿到了 201，
于是 FT-ATT-22 报"注入压根没打中"而不是"产品没问题"；这类"先验探针"的账本仓已经记过两次。
**仍未实现的是 `times`（命中次数，防粘连）**：`Injection` 里没有这个字段，
所以本表下方凡是写成 `times=1 / times=2 / times=99` 的行（SY-FAULT-02/03/04/09/10 那几条）
**都不可能按字面跑起来** —— 那是"设计了注入语法但没实现"的一格，按 §40 记在这里而不是当作已有。
要做的是给 `status_for` 加一个剩余次数（命中一次减一、归零后规则失效），改动只在工装、不碰产品。

## 功能测试矩阵

### 笔记生命周期

| ID | 前置 | 操作 | 预期（可观测） | 层 | 阶段 |
| --- | --- | --- | --- | --- | --- |
| FT-NOTE-01 | 干净库，列表为空 | 新建笔记 → 输入"测试甲" → 点外部失焦 | 左侧列表出现 1 条，标题=第一行文本"测试甲"；条目总数 0→1；关闭再打开 App 文本仍在 | L5,L1 | P2 |
| FT-NOTE-02 | 已存在笔记 N | 编辑正文第 2 段追加 5 字 → 等待自动保存 | 重开后含追加后的完整文本；`notera-cli diag show <N>` 的 `rev` 比编辑前 +1（辅助证据） | L5,L1 | P2 |
| FT-NOTE-03 | 已存在笔记 N | 删除 N | N 从活动列表消失、出现在回收站；正文与 `rev` 可回读；`deleted_at` 非空（辅助） | L1,L5 | P2 |
| FT-NOTE-04 | N 在回收站 | 恢复 N | N 回到原文件夹、原位置，正文 hash 与删除前一致；回收站条目数 -1 | L1,L5 | P2 |
| FT-NOTE-05 | N 在回收站，另一设备 B 有 N 的旧副本 | 永久删除 N → B 同步 | 本地 N 不可见且不再出现在导出结果；远端记录仍存在且 `deleted_at` 非空、`purged` 置位；**B 同步后 N 不复活**（INV-02） | L3,L1 | P5 |
| FT-NOTE-06 | 回收站有 100 条 | 清空回收站 | 100 条全部 `purged=true`；tombstone 条目仍保留在 manifest 中（**不自动 GC**，INV-11）；再次启动 App 不复活任何一条 | L1,L3 | P5 |
| FT-NOTE-07 | 已存在笔记 N | 空标题 / 仅空白 / 5000 字单段 / 含 emoji + RTL + 零宽字符 | 四组输入均保存成功且往返 hash 稳定；空标题笔记在列表中显示占位而非空白行；不出现保存报错 | L0,L5 | P2 |
| FT-NOTE-08 | 1000 条笔记 | 连续快速删除 20 条 → 全部恢复 | 20 条全部回到列表，条目数精确 +20，无一丢失 | L5,L1 | P3 |

### 文件夹与移动

| ID | 前置 | 操作 | 预期（可观测） | 层 | 阶段 |
| --- | --- | --- | --- | --- | --- |
| FT-FLDR-01 | 根目录 | 新建文件夹"工作/项目 A"（含 `/`） | 树中出现对应层级（`/` 作为名字字符被保留，不得被解析为路径分隔）；面包屑显示完整名 | L5 | P2 |
| FT-FLDR-02 | 同级已有"笔记" | 重命名为同级同名文件夹 | 拒绝并给出可见错误提示；两个文件夹的条目与笔记数不变（不产生"吞掉一个"的状态） | L1,L5 | P2 |
| FT-FLDR-03 | 文件夹 F 含 3 篇笔记 | 删除 F | 提示 F 内笔记数=3；确认后 3 篇进入回收站（不是被孤立/丢失）；恢复后回到原 F 路径 | L1,L5 | P2 |
| FT-FLDR-04 | 文件夹 A、B | 把 A 移动到 B 下；再把 B 移动到 A 下（第二次为环） | 第一次树层级正确；第二次被拒绝并提示原因；树中无节点消失、无自嵌套死循环（遍历在 100ms 内返回） | L1,L5 | P2 |
| FT-FLDR-05 | 文件夹 F | 重命名 F → 同步 → 设备 B 同步 | B 上 F 的新名生效，且 F 内 3 篇笔记仍在 F 内（移动/重命名不改变笔记 id，INV-09） | L3 | P5 |
| FT-MOVE-01 | 笔记 N 在 F1，F2 存在 | 拖拽 N 到 F2 | F1 条目数 -1、F2 +1；N 的 `rev` +1；正文 hash 不变（**纯移动不得改写正文内容**） | L1,L3 | P2 |
| FT-MOVE-02 | N 在 F1 | 离线把 N 移到 F2 → 设备 B 在 F1 编辑 N 正文 → 双向同步 | 冲突被识别为"位置 + 正文"变化；结果中 N 的正文为两侧合并或产生冲突副本，且 F1/F2 归属明确唯一；**不出现同一 id 的两条笔记正文各缺一半**（INV-01） | L3 | P5 |
| FT-MOVE-03 | 1000 条笔记 | 一次移动 100 条到另一文件夹 | 操作在 2 s 内完成（暂定，待基线）；100 条归属正确；剩余 900 条不受影响 | L5 | P3 |

### 搜索

数据集统一为 5000 条中文样本（PROBE:`fts5-search-5000-notes`：含连续 2 字 "同步" 的笔记 416 条）。判定以 **UI 结果列表条目数** 为准，`plan=` 仅作定位辅助。

| ID | 前置 | 操作 | 预期（可观测） | 层 | 阶段 |
| --- | --- | --- | --- | --- | --- |
| FT-SRCH-01 | 5000 条样本，含"同步"者 416 条 | 搜索框输入 `同步`（2 字 CJK） | 列表条目数 = 416。**禁止返回 0**：`MATCH` 对 <3 字符 CJK 会静默 0 命中（PROBE 实测 2char MATCH→0 / 22µs），必须走 content 表 `LIKE` 兜底 | L1,L5 | P3 |
| FT-SRCH-02 | 同上 | 输入单字 `笔` | 列表条目数 = 该字在 content `LIKE` 下的真实命中数（由 L1 预先算出的 golden 数），非 0 | L1,L5 | P3 |
| FT-SRCH-03 | 同上 | 输入 3 字 CJK `笔记本` | 条目数 = MATCH 命中数；`notera-cli diag search-plan` 显示走 FTS 路径；p50 ≤ 60µs、p95 ≤ 2000µs（PROBE 实测 32µs / 1404µs，容差用于回归） | L1 | P3 |
| FT-SRCH-04 | 同上（含英文样本） | 输入 `webdav`（大小写混打 `WebDAV`） | 两次结果条目数相同（大小写不敏感）；首条相关度排名一致 | L1,L5 | P3 |
| FT-SRCH-05 | 同上 | 输入中英混合 `同步 sync` | 条目数 = 两个约束同时满足的 golden 数；无 panic、无 0 命中兜底误判 | L1 | P3 |
| FT-SRCH-06 | 同上 | 清空搜索框 | 列表回到全量/默认排序，条目数 = 未过滤条目数；不残留高亮 | L5 | P3 |
| FT-SRCH-07 | 同上 | 输入 `%_[]()'"\\`、`%%`、单引号串 | 结果按字面量匹配，不抛 SQL 错误；条目数 = content `LIKE` 转义后的 golden 数；UI 无错误态 | L1,L5 | P3 |
| FT-SRCH-08 | 同上 | 输入必然 0 命中的 `zzzqqq中` | 条目数 0 且显示空结果文案（不是空白区域，也不是"加载失败"） | L5 | P3 |
| FT-SRCH-09 | 大量结果（416 条） | 搜索"同步"后滚动到底 | 全部 416 条可达、无重复条目、无跳号；命中片段高亮出现在每条 | L5 | P3 |
| FT-SRCH-10 | 同上 | 连续输入 `同`→`同步`→`同步引`（每步间隔 <100ms） | 最终列表对应最后一次查询；不出现旧查询结果覆盖新结果的乱序（fake clock + 请求序号断言） | L1,L5 | P3 |
| FT-SRCH-11 | 笔记在回收站 | 搜索其正文独有词 | 回收站笔记不出现在主列表（默认排除）；切到回收站视图可搜到 | L1,L5 | P3 |

### 固定、附件、Checklist

| ID | 前置 | 操作 | 预期（可观测） | 层 | 阶段 |
| --- | --- | --- | --- | --- | --- |
| FT-PIN-01 | 3 篇笔记 | 固定第 3 篇 | 第 3 篇进入列表顶部固定区；取消固定后回到原排序位置 | L5 | P2 |
| FT-PIN-02 | 固定 N | 把 N 移到另一文件夹 → 设备 B 同步 | B 上 N 仍在固定区且归属新文件夹；固定状态不因移动丢失（pin 与 folder 是两个独立字段） | L3 | P5 |
| FT-PIN-03 | 固定 10 篇 | 重启 App | 10 篇全部保持固定且顺序不变 | L5 | P3 |
| FT-ATT-01 | 一篇笔记 | 插入 1 张 PNG（1.2 MiB） | 图片可见；导出包内存在该图且 sha256 与源文件一致；远端 `DUMP` 出现 `attachments/<2hex>/<sha256>` 且路径前 2 hex 与 sha256 前缀吻合 | L1,L3 | P6 |
| FT-ATT-06 | 一篇打开中的笔记 | 插入图片（前端 file input → base64 → `attach_file`） | 顺序必须是"落自己的编辑 → 核心写附件 → 接住新 rev → 才把附件块写进正文"：核心首次挂附件会翻转 `has_attachment` 并在同一事务推进 rev，顺序反了编辑器就把自己判成 `stale_edit`（用户看到的是"这条笔记在别处被改动了"）。断言：调用序列、随后那次保存的 `expectedRev` == 附件返回的 rev、正文里**没有** base64、失败时占位块被撤干净且不多存一版 | L2,L5 | P6 |
| FT-ATT-07 | 挂了一个附件的库 | `attach_file`（bytesBase64）→ `attachment_data` 读回 | sha256 由**核心**算（前端给的键不算数）；字节逐字节相同；`localPath` 与 `bytesBase64` 给两个或给零个都拒；坏 base64 不许"尽量解"；超过 32 MiB 在**读字节之前**就拒；`attachment_data` 的 sha 参数必须是 64 位小写 hex（它会被拼进 blob 路径，不校验等于给 `../../` 开门），形态对但盘上没有 → `attachment_missing` 而不是空成功 | L2 | P6 |
| FT-ATT-02 | 已有附件 | 在设备 B 打开同一条笔记 | 附件按内容寻址取回并渲染；本地 sha256 校验通过；不产生第二份副本（同 sha256 只落一个文件）。**已有真服务器证据**：`notera-host/tests/sync_once.rs::an_attachment_follows_its_note_to_a_second_device` —— A 上传 → B 拉记录 → 登记 → 下载 → 两边字节与源一致 → 走 `attachment_data`（界面那条读路径）读得回来 → `attachment_refs==1`。注意它同时钉住一条顺序事实：引用住在 **doc** 里，所以"编辑器把附件块写进正文"那次保存必须发生，`attach_blob` 单独存在时第二台设备无从得知要取哪个 blob（另一半缺口见 ARCHITECTURE-MAP §尚未做） | L3 | P6 |
| FT-ATT-03 | 20 MiB 附件，链路 `FAIL(latency,target=attachments/**)` 限速 1 Mbit | 上传附件的同时编辑并同步另一条纯文本笔记 | 文本笔记在设备 B 于 ≤25 s（设计周期）内可见；附件队列未完成不影响文本轮（`STATS` 显示文本轮请求不含 `attachments/**` 等待）；UI 不出现整体阻塞（输入延迟 <100 ms） | L3,L5 | P6 |
| FT-ATT-04 | 上传中断（`FAIL(abort,target=attachments/**)`） | 下一轮同步 | 附件在后续轮重试成功；服务端仅存在 `.tmp-*` 残留，`DUMP` 中无残缺正式对象；引用该附件的 manifest 不出现（INV-09）。**2026-09-27 更正：这一行长期只有声称、没有实证**（全仓从未对 `attachments/**` 注入过任何东西）。同一性质更狠的形态（服务器只读到半份 body）现由 FT-ATT-15 真跑过；纯 `abort` 那一形 2026-09-28 按**动作**拆开真打了两条 —— FT-ATT-22（发布那一步 MOVE 被掐）与 FT-ATT-24（复读校验的 GET 被掐），工装为此补了 `abort_for` | L3,L4 | P6 |
| FT-ATT-05 | 删除含附件笔记 → 清空回收站 | `DUMP` + 检查 tombstone | `purged` 置位；tombstone 不被自动 GC（INV-11） | L3 | P6 |
| FT-ATT-08 | 干净库（没有任何 `attachments` 行） | `apply_remote` 一条 doc 里带 `image` 块的远端笔记 | 与笔记**同一事务**登记 `attachments` + `note_attachments`：`attachment_refs == 1`、起始态是 `missing`/`unknown`、`attachment_downloads()` 真的给出这一条（漏登记 = 第二台设备永远占位 + 引用计数恒为 0 让 GC 删掉还在用的 blob）。畸形 sha（非 64 位小写 hex）不入库、也不许把整批同步拖回滚；重放同一条记录不产生第二行 | L1,L2 | P6 |
| FT-ATT-09 | 4 MiB + 1234 字节的附件，服务器支持 Range | 一轮只取回一个 4 MiB 窗口后**真杀掉进程**，重启再跑一轮 | 第一轮：`.part` 恰好 4 MiB、**正式 blob 不存在**、outbox 仍是 pending（没下完不许结清）。第二轮：只发**一个** `Range: bytes=4194304-…` 请求（请求日志逐条核对，不是"看起来变快了"），拼完字节与源**逐字节相同**、`.part` 被删、`local_state=available`。拼接后 sha256 与期望不符 → 丢弃半截 + 记 failed，绝不把半截文件当完整附件（§10）。服务器探得不支持 Range 时整块取，不发一个注定被答 200 的请求骗自己。证据：`notera-host/tests/attachment_resume.rs::a_partial_attachment_keeps_its_progress_and_finishes_after_a_restart`。门禁自证：把 `want_range` 写死成 `false` → 立刻红 | L3,L4 | P6 |
| FT-ATT-10 | 同 FT-ATT-09 的库，但第二轮起服务器 `FAIL(ignore-range)` | 重启后再跑一轮 | 服务器把我们的 Range **没理**、答 200 + 全文 ⇒ 客户端必须**当整份覆盖**，不许往 4 MiB 半截后面追加（那会拼出一份内容重复、哈希永远对不上的文件，用户看到的是"这张图永远下不下来"）。断言：该轮请求日志恰好一条 `GET -> 200`、正式 blob 与源**逐字节相同**、`.part` 不在、账上 `available`。证据：`attachment_resume.rs::a_server_that_ignores_range_still_lands_the_right_bytes`；变异自证：把"非 206 一律报偏移 0"那一句拆掉 → 本条与 `notera-webdav` 的 4 条偏移判定一起红 | L3,L4 | P6 |
| FT-ATT-11 | 两台设备，图已在服务器、B 已下完 | **删掉 B 盘上那份 blob**（磁盘清理 / 杀毒隔离 / 换盘没搬完），重启 B 再跑一轮 | `local_state=available` 而盘上没有的行**两个队列都看不见**（available 不进下载队列、present 不进上传队列），所以附件轮开工前必须先做一次磁盘体检把它降级。断言：本轮 `down==1`、重下回来的字节与源**逐字节相同**、账上重新 `available`、且删文件之后、同步之前**正文照旧读得到**（§8 收尾句）。证据：`attachment_faults.rs::a_lost_local_blob_is_repaired_and_never_strands_the_note`；变异自证 M1 拆掉体检调用 → 本条与 FT-ATT-12 一起红（`round=(0,0,0)`） | L3,L4 | P6 |
| FT-ATT-12 | 同 FT-ATT-11 的库 | 把 B 盘上那份 blob **截断一半**（长度与账不符）后重启 | 坏文件必须**先挪开再重下**：`ingest_blob` 见目标已存在就不覆盖，留着它的后果是"重下成功、账也标回 available、盘上还是那半截"——比不修更骗人。但**不删**：改名成 `<sha>.corrupt` 留在原地（§8 那条序：没有替代就不销毁），等重下拿到哈希对得上的替代之后才由下载那条分支清掉。断言：`down==1`、正式 blob 等于整份源字节、`.part` 不在、**清理边**（补齐后不留 `.corrupt`）、**不循环边**（再跑一轮必须 `(0,0,0)` 且状态仍 `available`，否则"哈希相符就不动"形同虚设、每条附件每轮重下一遍）。尺寸对不上但哈希相符的那些**不降级**（哈希才是身份）。证据：`attachment_faults.rs::a_truncated_local_blob_is_replaced_not_reused`；变异自证 M2（不挪坏文件）与 M11（不做清理）各杀一次 | L3,L4 | P6 |
| FT-ATT-12b | 两台设备，B 已下完好字节 | **同长度**原地改坏 B 那份 blob 的若干字节（`stat` 看不出来，体检因此不会降级它），再走界面那条读路径 | 管的是体检边界之外那一格：**读侧不许把错字节画出来**。`attachment_data` 必须复算 sha256，对不上就报 `attachment_corrupt` 且不带 `bytesBase64`，界面留占位（附件不阻塞正文），正文与引用都还在。单独一条的理由：缓存按 sha 存，一次不验就等于整个会话都在显示一份**不属于这个 sha** 的内容，而屏幕上没有任何地方说它坏了。证据：`attachment_faults.rs::a_same_length_local_corruption_is_refused_by_the_reader_not_drawn`；变异自证 M12：拆掉读侧那次哈希 → 本条红（真的拿到了一整串 base64 错字节；断言消息只打键名与长度，不把 payload 刷进 CI 日志） | L2,L3 | P6 |
| FT-ATT-12c | B 的 blob 被截断（体检要挪开它），同时**服务器上那份已被删**（重下注定没有替代） | 跑一轮附件 | 挪开的东西不许消失：正式位置腾出来（否则 `ingest_blob` 写不进新的）、`.corrupt` 里那份**逐字节等于**挪开前那一份、账上落到 `missing`/`absent`（本机没有、服务器也没有 —— 而不是 `available` 那种谎）、正文可用。这条就是 §8"没有替代就不销毁"的凭据。证据：`attachment_faults.rs::corrupt_local_bytes_are_parked_not_destroyed_when_the_server_has_nothing`；变异自证 M10：把 `park_corrupt_blob` 换回 `remove_file` → 本条红（"体检把本机最后一份坏字节销毁了"） | L3,L4 | P6 |
| FT-ATT-13 | 两台设备 + `Backend::Fs` | 把服务器上那份对象的原地改成**同长度坏字节**，`RESTART` 让服务器以磁盘为准，再让 B 同步 | 客户端拿到"看起来对"的字节时**绝不落进正式 blob**：拼完先复算 sha256（`run_attachment_round`），落盘前 `ingest_blob` 再算一次 —— 两处独立拦截，实测**同时拆掉两处判据（M3b）才让本条变红**，这正是内容寻址要的纵深。断言：`down==0`、正式 blob 位置为空、账上不许 `available`、正文照旧可用、界面走 `attachment_*` 命令读到的是**具名失败**而不是那张坏图。注意 Fs 后端平时服务**内存表**：不改完磁盘就 `RESTART`，测的其实是"客户端拿到了好字节"（第一版就这么假绿过）。证据：`attachment_faults.rs::a_corrupted_remote_blob_is_refused_and_the_note_stays_readable` | L3,L4 | P6 |
| FT-ATT-14 | 两台设备，正文与队列都就绪 | 附件请求一律 `FAIL(abort)`（连接被掐，不回应），恢复后再跑一轮 | 掐断的那轮 `down==0`、正式 blob 位置**一个字节都不许有**、账上不许 `available`、正文继续可用；恢复后一轮补齐且字节一字不差（不留永远下不完的队列）。注入必须打在**附件轮窗口上**：挂在 B 开机之前测到的是"网络不通"（`/_control/inject` 会把已服务计数清零，`abort_after(1)` 于是先掐掉清单）。证据：`attachment_faults.rs::a_dropped_connection_mid_download_promotes_nothing`；变异自证 M4 把传输失败写成 `remote_state=absent` → 恢复段红（(0,0,0)） | L3,L4 | P6 |
| FT-ATT-15 | 源设备正文已推、图待传 | `FAIL(truncate-upload,bytes=400)` 只让附件 PUT 走一半，恢复后再跑一轮 | 半上传不许算成功（`up==0`）、`DUMP` 里**不得出现**含该 sha 的正式对象（INV-09）、账上更不许说远端 `present`（那样下一台设备会永远等一份不存在的字节）、正文可用；恢复后同一台设备重试成功且服务器上**最终恰好一份**。这条补的是 FT-ATT-04 一直只有文档声称、从未跑过的那个缺口。证据：`attachment_faults.rs::a_half_uploaded_attachment_lands_nothing_and_is_retried`；变异自证 M5 在上传失败分支写 `present` → 本条红 | L3,L4 | P6 |
| FT-ATT-16 | 两台设备，服务器上那份被删（网页端手删 / 网盘回收） | 对该对象注入 `FAIL(status=404,target=GET ***<sha>)`，之后**再走两轮完整同步** | 404 是一个**结论**不是悬案：`down==0`、`local_state` 不许 `available`、`remote_state` 落成 `absent`、`.part` 不留、正文可用；关键在后半段 —— 后续轮次里针对该 sha 的请求数必须是 **0**（`attachment_downloads()` 的注释一直声称"就此收手、不每轮空转"，此前无任何测试撑着）。证据：`attachment_faults.rs::a_missing_remote_object_is_marked_absent_and_stops_being_retried`；变异自证 M6 拆掉 `absent` 那一句 → 本条红（停在 `unknown`，即每轮重问） | L3,L4 | P6 |
| FT-ATT-17 | 两台设备 + 一台只有附件端点不答应的服务器 | 对**那一个对象**的 GET 注入 `FAIL(hang,target=GET *<sha>)`（其余端点照常），文本侧同时推一条新笔记 | 三件事分开证：① **文本轮没被拖住** —— 在附件请求还挂着的时候并发跑 B 的 `sync_once`，它要在 `per_request/4`（≈11 s）之内完成并把新笔记送到（这条同时兜住 §13 的队列隔离与 §28 的「网络问题永远不会让本地数据不可用」）；② 附件轮**自己会放手** —— 实测一轮耗在 45.01 s，正好等于 `notera_net::Timeouts::per_request`（重试与整体共用同一份预算，不是每试一次 45 s），期间不落正式 blob、不留 `.part`、账上不许 `available`、`failed` 计数 +1（状态页要看得出这一轮白跑）；③ 端点恢复后这一条**补得回来**（超时是"这次没取到"，不是"这条坏了"）。前置断言：请求日志里必须先出现一条 `GET … -> 0`，否则测的是"什么都没发生"。证据：`attachment_faults.rs::a_hanging_attachment_endpoint_never_blocks_the_text_round`；变异自证 M7 把 `Injection::hangs()` 写死成 `false` → 前置断言当场红（报的是"注入没打中"而不是"产品通过"） | L3,L4 | P6 |
| FT-ATT-18 | 源设备正文已推、图待传；服务器**先落成对象再回 412**（`FAIL(partial-write,target=MOVE *,412)` 的 `post:` 形态 = 副作用做完再改状态码） | 跑一轮附件 | 这是"并发下别人先传了同一份内容"的真实形态：MOVE 被拒，但服务器上确实有这一份。断言：`up==1`、`DUMP` 里恰好一份且其 `sha256` 就是我们要传的字节、账上 `available`/`present`。它同时挡住建议的另一端 —— 把 412 一律当失败会让一条本来成功的上传永远传不上去。证据：`attachment_faults.rs::a_refused_move_that_really_landed_counts_as_a_success`；变异自证 M8b：把 405/412 那条臂改成直接 `Err(Precondition)` → 本条红（`round=(0,0,1)`） | L3,L4 | P6 |
| FT-ATT-19 | 同上，但服务器**拒绝且什么都没落成**（裸 `FAIL(status=412,target=MOVE *)`，网关凭空造前置失败就是这形态） | 跑一轮附件，再清掉注入跑第二轮 | **present 是一个主张，不是一次收据**：旧实现这里直接 `return Ok(())`，跳过了紧跟其后的复读，于是账上凭空多出一个 `remote_state=present` —— 第二台设备从此永远等一份服务器上并不存在的字节。断言：`up==0`、`DUMP` 里没有该对象、账上不许 `present`、正文可用、**下一轮能真的传上去**（412 是"这次没成"不是"这条坏了"）。改前实测红在 `round=(1, 0, 0)`，改后 2/2 绿。证据：`attachment_faults.rs::a_refused_move_that_landed_nothing_is_not_a_success` | L3,L4 | P6 |
| FT-ATT-20 | 两台设备，源设备**正文里声明的图片尺寸比真实字节大**（块属性由客户端各自写，`upsert_attachment_row` 的冲突规则又是 `MAX(旧, 新)` —— 只许涨不许落） | 目标设备走完下载，再连跑两轮附件轮 | 磁盘体检为判身份已经把整份字节读完复算过 sha256，**哈希相符就意味着盘上那个长度是实测真值**，于是它必须回填登记尺寸（`Store::set_attachment_sizes`，只改那一列），而不是只记一句 debug 就 `continue`。断言四段：① 一轮之后 `attachment_repair_candidates()` 里这条的 size == 盘上长度；② 快路径判据（`stat` 长度 == 登记值且 > 0）**真的成立** —— 否则"下一轮只 stat"是好听话；③ 改尺寸的代价没碰字节（文件逐字节相同、没有 `.corrupt` 现场、两半状态仍是 `available`/`present`）；④ 针对该 sha 的 GET 数**没有增加**（完好的一份字节不许被重下）+ 第二轮仍是 `(0,0,0)` 且尺寸不漂回去。修之前实测红在 `left: 13096 / right: 9000`（前置成立、轮次 `(0,0,0)` 也成立，唯独尺寸永远不改）；变异自证 M19：把回填的值从实测长度换成"登记里那个旧值" → 同一条红在同一个断言。**没有声称测过 IO 次数** —— 工装不计数，能数的只有 GET 条数与判据落在哪条 `if` 上。证据：`attachment_faults.rs::a_hash_verified_row_has_its_size_corrected_once_and_then_stops_being_work` | L3,L4 | P6 |
| FT-ATT-21 | 两台设备，B 有**三条**已下完的图，然后整个本机 blob 目录被清空（换盘没搬完 / 杀毒按目录隔离） | 手动跑一次体检（`sweep_lost_local_blobs(cap)`）两轮，再跑一轮完整附件轮 | §48 缺口 G4：修之前是"候选不分页 + 每行一个写事务"，一轮里能攒出成百上千次提交排队占住写锁，而用户那次保存正排在锁后面。现在断言：① `cap=2` 的一轮**只降 2 条**（多降一条就是分页没起作用），且第三条此刻仍是 `available` 且远端态没被顺手改；② 降下来的两条当轮就出现在下载队列里（`attachment_downloads`），第二轮补齐剩下那条；③ 三份字节全部逐字节补回来后体检**收手**（再扫一次返回 0，循环得断掉）。变异自证 **M20**：把 cap 忽略掉（`usize::MAX`）→ 红在 `cap=2 却降了 3 条`。**这一条能量化什么、不能量化什么都写在测试的 doc 注释里**：能数的是"一轮降了几条"，**数不清"提交了几次"**（Store 不暴露事务计数，为这条加一个只有测试在读的计数器就是 §39 禁的东西）—— 所以"一次批量写"那一半靠 `set_attachments_locally_missing` 只有一个 `write_tx` 这个代码事实，它的**语义**由 FT-ATT-21s 钉。证据：`attachment_faults.rs::the_sweep_demotes_at_most_its_cap_per_round_and_picks_the_rest_next` | L3,L4 | P6 |
| FT-ATT-21s | 同上，但只到存储层：三条 `available`/`present` 的行 + 一个库里不存在的 sha | 调一次 `set_attachments_locally_missing(&[点名两条 + 那条幽灵])` | 批量这条路最容易悄悄做错的三件事一次钉住：**点名的全降到 `missing`**、**没点名的那行不许被顺手动到**（批量语句写成"整表 available 都降"就是这种错）、**远端态一字不动**（顺手写 absent 等于把这张图判死，下载队列口径是 present/unknown，从此再也不去问一次），外加返回条数**不虚报**（幽灵不计、已经降过的重复调用计 0 —— host 那声 warn 就靠这个差值）。变异自证 **M21a** 去掉 `local_state='available'` 那道 guard → 红在"已经不是 available 的行被重复计入成功条数"；**M21b** 改成整表形式 → 红在"库里没有的那条不许算成已降级：3"。证据：`notera-store/tests/attachment_queue.rs::a_batched_demotion_moves_exactly_the_listed_rows` | L1,L3 | P6 |
| FT-ATT-22 | 源设备正文已推、图待传；**发布那一步（MOVE）连接被掐**（`FAIL(abort,target=MOVE *)` —— 工装为此新加一个按规则掐的旋钮 `abort_for`，与 `status_for`/`hang_for` 共用 `rule_hit`） | 跑一轮附件，清掉注入再跑一轮 | §27「上传中断」此前**只有文档声称**：FT-ATT-04 那行一直写着 `FAIL(abort,target=attachments/**)`，而从没真打过。切点选在 MOVE 而不是 body 半路，因为那一形前面全都成功（暂存整份已写完），客户端手里只剩"不知道成没成"。判据：本轮 `up==0` 且 `failed==1`、DUMP 里**不许**出现含该 sha 的正式对象、本机那份**独家字节**逐字节还在、账上保持精确的一对 `(available, unknown)`（"不知道服务器有没有"不许被写成任一侧的结论）、正文可用；恢复后重试 `up==1`、最终恰好一份且那份的 `sha256` 就是要传的字节。前置里还钉着"本轮恰好一条 PUT + 恰好一条 MOVE 且那条 MOVE 无响应"。证据：`attachment_faults.rs::a_severed_publish_step_rolls_the_upload_back_and_the_retry_lands_once`；**这一条第一次跑就是红的**，而红的是我的工装：`aborts()` 当时还没接进服务器的判定，MOVE 拿到 201 → 前置断言当场报"注入压根没打中"（不是"产品没问题"）。变异自证 **M23**：上传失败分支写 `present` → 本条与 FT-ATT-15/19/23 同时红 | L3,L4 | P6 |
| FT-ATT-23 | 同上，但发布那一步**收到一个裸 404**（`FAIL(status=404,target=MOVE *)`；网关把目标判成没有这个集合、暂存被服务器侧清理回收、反代把不认识的动词转成 404 都是这形态） | 跑一轮附件，清掉注入再跑一轮 | **上传侧的 404 不是关于远端的结论** —— 下载侧那个 404 才是（FT-ATT-16 记 `absent` 收手）。这里两个方向都不许：读成 `present` 会让对面永远等一份不存在的东西，读成 `absent` 是凭一次失败的请求给服务器的存在性下判断。判据：`up==0`、`failed==1`、DUMP 无正式对象、账上**精确**保持 `(available, unknown)`、那次 404 之后**发过 DELETE**（别把暂存留成垃圾，§11.3）、正文可用；恢复后 `up==1` 且最终恰好一份。变异自证 **M24**：把下载侧那条判据错搬到上传侧（失败时写 `absent`）→ **只有本条红**（旧那批用的是 `assert_ne!(present)`，抓不到 `absent`；这就是精确状态对的用处）；**M25**：去掉那次 `best_effort_delete` → 红在"那份暂存没去清"。证据：`attachment_faults.rs::a_not_found_answer_on_the_publish_step_is_not_read_as_a_conclusion` | L3,L4 | P6 |
| FT-ATT-24 | 同上，但**只有最后那次复读校验被掐**（`FAIL(abort,target=GET *<sha>)`）：PUT 与 MOVE 都成功了，服务器上**确实有**那份字节 | 跑一轮附件，清掉注入再跑一轮 | 最容易漏的一格，因为"报成功"看着无害 —— 可客户端**没有证据**（内容没比对过）。§13 那句"present 必须出自一次读回的内容比对"就是在这一格成立：判据是本轮 `up==0`、账上仍是 `(available, unknown)`，而 DUMP 里那份对象**已经落成**（前置，否则测不到这一形）。恢复那一轮顺带把 §13 承认的**例外**跑成门禁：`has_attachment` 的 HEAD 命中就直接算过，那个 present 是**借来的**（没有内容比对）—— 它今天可接受的唯一理由是消费侧下载会复验（FT-ATT-13 已证），代价与成因记在 SYNC-PROTOCOL §13 与 G5；这里钉的是"不许重复落第二份对象"。变异自证 **M26**：读回失败当成功（那条捷径）→ 本条红在 `round=(1,0,0)`。证据：`attachment_faults.rs::a_severed_read_back_after_a_landed_upload_records_no_present` | L3,L4 | P6 |
| FT-ATT-25 | 两台设备：图在服务器上，B 已下过一次；然后 B 本机那份被删，且账上留着 `absent`（网页端删过、后来运维又把目录恢复 —— 账上是**旧的**否定结论） | 先跑一轮证明后台**不动**，再点界面那颗「重试取回」，再跑一轮 | `absent`/`error` 是"收手的结论"，而重开它的凭据只能是用户的一次意图（2026-09-28 的决定）。判据：前置那一轮必须 `(0,0,0)`（否则这条测不到"点了一下才活过来"）；命令走**真 dispatch**，返回键集合逐字钉成 `[localState, remoteState, sha256]`（TS 类型是断言不是校验，`stats` 那条边就是这么漂过的）；`local_state` 一字不动（本机有没有是磁盘上的事实）；`remote_state` 落成 `unknown`；**待办要落一条**（不然那个"待同步"永远 0，用户以为没发生）；下一轮 `down==1`、字节逐字相同、两半落回 `available`/`present`、正文可用。证据：`attachment_faults.rs::a_user_retry_reopens_an_absent_attachment_and_the_next_round_fetches_it`；变异自证 **M28**：把远端态"撤"成它原来那个否定结论 → 红在 `remoteState` 那一条 | L2,L3,L4,L5 | P6 |
| FT-ATT-26 | 同上，但故意把动作用在**不该用**的三种位置 | 走真命令面分别调用 | 三种拒绝各有各的具名码与文案，因为用户下一步要做的事不一样：没登记这一行 → `attachment_not_registered`（两条命令都一样）；本机明明有 → `nothing_to_retry`（说"重试成功"会让用户等一次不会发生的下载）；本机那份不在 / 内容与 sha 不符 → `nothing_to_upload`（把对不上号的字节传上去会污染所有引用同一 sha 的笔记，比传失败严重）。外加两条"拒绝不许顺手做事"：这条命令**不删不改本机那份文件**（挪开/销毁是磁盘体检那条路，有它自己的凭据要求），也**不动账**。证据：`attachment_faults.rs::the_two_recovery_actions_refuse_the_cases_where_they_would_lie`；变异自证 **M31**：去掉"本机那份必须真是它"那道哈希 guard → 红在"内容与 sha 不符的一份字节被允许按用户要求传上服务器" | L2,L3 | P6 |
| FT-ATT-27 | 两台 + `Backend::Fs`：A 传过好字节，随后**服务器上那份被原地改坏**（长度不变、内容不对），而 A 自己看不到任何异常 | A 上点「重新上传本机这份」→ 跑一轮 → 再让一台**从没见过的**设备去取 | G5 的正解：坏对象只有"还握着好字节的设备"能修，而它自己不觉得有问题，所以那条 HEAD 跳过的省流量规则必须给它让路。判据分三段：① 意图落成账上的 `remote_state='error'`（不是内存标志 —— 重启/离线/预算用完都不能丢）；② 那一轮 `up==1`、DUMP 里**恰好一份**对象且它的 sha256 就是要传的字节（覆盖真发生、没落第二份）；③ 全新设备 C 能取回好字节并落回 `available`/`present`。证据：`attachment_faults.rs::a_user_reupload_replaces_a_corrupt_remote_object_for_every_device`；变异自证 **M30**：让 force 恒为 false（即账上 error 也照样走 HEAD 跳过）→ 红在"服务器上那份坏字节没被换成好的"。另：覆盖之后**仍走复读校验**，被服务器拒（不许改已存在对象）就判失败，不留"以为传好了" | L3,L4 | P6 |
| FT-ATT-28 | 真浏览器 + 真核心（`verify-app` lane，非 mock） | 把本机那份字节删掉 → 刷新重开那条笔记 → 看占位、点两颗按钮 | 界面侧只证"点得动、答得回人话"：占位上**两颗粒可命名的按钮**都在；点「重新上传本机这份」得到核心的具名拒绝且显示成中文；点「重试取回」得到真答复（`不需要取回` 或 `已重新排进下载队列`，两种都诚实）且**答复里没有漏出 `cmd.*` / `error.*` 键名**；正文仍在（附件坏了不许把笔记一起带走）。为什么两种答复都收：降级那条路是后台 20 s 一跳，把它写成"必须是某一句"就是一条闪烁的门禁 —— 这里要钉的是"核心回过话、且用人话"。**"真取回来/真换掉"那两条不在这一格**，由 FT-ATT-25/27 在 Rust 侧证。步骤名见 `scripts/verify-app.mjs` 的「坏图占位上的两颗自救按钮」。<br>**这一步顺手立的规矩**：lane 里那两条全局不变量（控制台零 error / 网络请求零失败）原本会把"核心故意的具名拒绝"也算成坏了 —— 那等于逼着所有人别去点失败面。现在改成按 `命令名:错误码` **精确配对**放行（配不上对的 4xx 照旧算坏，console 那条还要 location.url 正是同一支命令），并由 `assertRefusalsSeen()` 反查"声明了却没发生"（声明空转也算失败）。命令名与载荷形状另有 L1 契约测试 `stores/attachmentRecovery.spec.ts`（5 条：两条命令的键/载荷逐字、两个失败码显示成中文且不漏键名、块上没有 sha 时一条命令都不许发） | L5 | P6 |
| FT-ATT-29 | 一台设备把那条带图笔记**永久删除**（链接被 CASCADE 带走 → 引用归零），而服务器上有副本（`present`）；另一形是笔记只在**回收站**里 | 先软删 → 跑回收；再还原 → 永久删 → 跑回收；然后再跑一整轮附件轮 | §8 GC 的第一步。判据五条：① 软删那一格**一条都不许收**（回收站里的笔记仍算引用，用户随时可能还原）；② 收 = 字节**挪**进 `<data>/attachments-quarantine/xx/<sha>` 且逐字节完整，正式位置腾出来 —— "隔离"与"销毁"的差别只能靠盘上有没有第二个位置来证；③ 账上写 `local_state='missing'` 且 `deleted_at` **有值**（宽限期靠它起算），远端态一字不动；④ 这一行立刻离开三个后台口径（下载队列、体检候选、GC 候选集本身）—— 其中"下载队列"那条是可数的：`missing ∧ present` 正是下载取活条件，唯一拦住它的就是 `deleted_at IS NULL`；⑤ 再跑一整轮附件轮，**为该 sha 发出去的请求数不变**、隔离区那份字节没被动过（两个后台循环不许抢同一行）。证据：`attachment_gc.rs::an_unreferenced_blob_is_quarantined_not_deleted`；变异自证 **M32**（候选集不再问引用计数）→ 红在②之前的"判据没成立却把正式位置上的字节动了"；**M35**（把 `rename` 改成 `remove_file`，即就地销毁）→ 红在"隔离=挪开，字节必须完整还在盘上" | L2,L3,L4 | P6 |
| FT-ATT-29s | 同一台设备上三条零引用 blob（都传上去过） | `cap=0` 跑一次；然后 `cap=1` 连跑三轮 | 每轮量有界（§48 G4 那条理由在 GC 上同样成立：用户刚清空回收站时零引用的行可以有成百上千，一轮搬完就是把常驻循环那一格变成一段长 IO + 一段长写锁，而用户那次保存正排在后面）。判据：`cap=0` 一条都不动；每轮**恰好**认领一条，且被认领的那条**立刻**离开候选集（否则上界就成了"同一批反复搬"）；还没轮到的那些字节一个都不许动；三轮后三条各被认领一次、不重不漏。证据：`attachment_gc.rs::the_reclaim_quarantines_at_most_its_cap_per_round_and_picks_the_rest_next`；变异自证 **M37**（候选查询的 `LIMIT ?1` 改成 `LIMIT ?1 * 100`）→ 红在"这一轮只该认领一条" | L2,L3 | P6 |
| FT-ATT-30 | A 与 B 共用同一张图（内容寻址同一个 sha）；B 永久删掉自己那条笔记 → 被 B 的 GC 隔离；随后 **A 新建一条笔记再引用同一份字节**并推上去，B 拉到那条正文（登记这一步把 `deleted_at` 清回 NULL，行重新进下载队列） | B 跑一轮附件轮 | 撤销期内的恢复**必须是本地的**：跳过一次网络拉取要有读侧的持久来源（就是隔离区那个文件本身），不是猜的。判据五条：① 该 sha 的 **GET 计数不变**（这条是整个测试的落点，打在请求日志上而不是"看起来好了"）；② 字节回到正式位置且逐字节相同；③ 隔离区那一份被挪走而不是留下第二份（同一份内容占两处 = GC 反而让盘更大）；④ 账回 `available` 且 `deleted_at` 是 NULL（留着标记这一行永远排不进上传队列）；⑤ 编辑器那条 `attachment_data` 拿到的是字节不是具名错误。恢复前先复算 sha256 —— 对不上号的一份不许挪回正式位置。证据：`attachment_gc.rs::a_reference_returning_inside_the_grace_period_comes_back_without_a_single_request`；变异自证 **M34**（下载轮那一步的本地恢复整条短路掉）→ 红在"却还是去服务器要了一遍" | L3,L4 | P6 |
| FT-ATT-31 | 已隔离的一行 + 两个宽限期界（远古 / 未来，由调用方给，不依赖真时钟）；另一形是**仍有引用**的行 | 依次用两个 cutoff 调销毁；再造一条仍在用的图跑一遍 | 真删那一步的判据全部可核对：① 宽限期内销毁必须回 0 且字节原样留在隔离区（那是撤销的全部窗口）；② 到期且仍零引用才动手 —— **行与那份字节两样一起消失**（少删一样就是磁盘上的永久垃圾），账上彻底没有这一行（留着就是下一台设备等一份并不存在的东西）；③ 有引用的行既进不了销毁清单也删不掉（`WHERE NOT EXISTS` 先问一遍，`note_attachments.sha256 ON DELETE RESTRICT` 是机器兜底）。存储侧同批两条：`attachment_queue.rs::the_quarantine_mark_only_takes_rows_that_are_still_unreferenced`（隔离那一步只动仍零引用的行、远端态一字不动、附件待办一起结掉、重复点不重复计入成功条数）与 `a_purge_refuses_rows_that_are_still_referenced_or_absent`。**M33**（隔离语句的 `WHERE` 里不再复查引用）→ 红在存储侧那条；**M36**（`deleted_at < ?1` 反着比）→ 红在"宽限期内就销毁 = 撤销窗口根本不存在" | L2,L3 | P6 |
| FT-ATT-32 | 一台设备把图隔离了（`deleted_at` 有值 → 三个后台口径都看不见它），用户这时在占位上点「重试取回」 | 走真命令面调 `attachment_retry`，不发一轮请求 | 那颗按钮对"已被本机隔离"这一格也必须真的做事：只撤远端结论 + 入队的话，那一行带着隔离标记永远进不了队列 → 用户点了、什么都没发生、而待办还挂着（比不点更坏）。判据：命令不拒、字节**回到正式位置**、**GET 计数不变**、账回 `available` 且 `deleted_at` 清成 NULL。证据：`attachment_gc.rs::a_user_retry_recovers_a_quarantined_attachment_locally_before_asking_the_server`；变异自证 **M39**（retry 那一步不先试本地恢复）→ 红在"必须真把字节放回正式位置"；**M38**（`set_attachment_states` 里 `THEN NULL ELSE deleted_at` 改成 `THEN deleted_at`）→ 红在本条最后那句 `deleted_at is_none()`（同一处改动对 FT-ATT-30 **是绿的** —— 那条走的是 upsert 登记，它自己就把标记清了；这条才是不经登记、只有这一处代码能清标记的那一格。第一次跑 M38 我打在 30 上，绿了，按 §40 那是要当成"这条规则没有门禁"的） | L3,L4 | P6 |
| FT-ATT-33 | 已隔离的一行，而隔离区里那份**同长度**坏掉（位腐；或那个目录被磁盘清理连根删了） | 点「重试取回」→ 跑一轮附件轮 | 这一条是被 FT-ATT-32 逼出来的：本地补回失败时，若只改远端态 + 入队，那一行仍然隐形 —— 一次请求都不会发，而那条待办永远关不掉。判据：① 命令不拒且**隔离标记被撤**（撤掉的代价 = 宽限期结束，而这正是用户那一刻要的）；② 该 sha 的 GET 计数**增加**（这条把这颗按钮的"静默失效"变成可数证据）；③ 从服务器回来的那份内容对得上、图能再画出来；④ 坏掉的隔离副本被落好的那份替换掉，不留第二份。同长度这一句是刻意的：体检只判"在不在 + 长度对不对"，同长度的坏字节它不动（§8 那条边界），所以这条路必须自己站得住。证据：`attachment_gc.rs::a_corrupt_quarantine_copy_does_not_silently_defeat_the_retry_button`；变异自证 **M40**（`release_attachment_quarantine` 改成什么都不清）→ 红在"标记还留着，这一行就永远排不进下载队列" | L3,L4 | P6 |
| FT-ATT-34 | 一台设备上：`attach_blob` 挂好一张图（原件，链接由挂载写入）→ 用**服务器那一版的正文**直接 `create_note` 造冲突副本（生产里 `App` 的 AdoptConflict 分支就是这一形）→ 再拿 `edit_note` 把同一张图片块搬进一条原本不带附件的笔记（复制图片块 / 采纳冲突改写正文是这一形），这两步都不调 `attach_blob` | 永久删除原件 → 跑 `reclaim_unreferenced_blobs` | GC 的判据是"零引用 = 可回收"，而这个数**只能由正文派生**；本机 create/edit 那两条路不登记时，副本与"搬进来的那条"在账上是无主的，原件一被永久删除（CASCADE 带走唯一链接）那份字节就会在 30 天后被销毁 —— 而笔记还画着它。判据五条：① 三条笔记都在引用同一份字节时 `attachment_refs` 必须是 **3**（少一个就是那个洞的正身）；② 永久删除原件后必须还剩 **2**；③ `reclaim_unreferenced_blobs` 回 **0**（一条都不许收）；④ 正式位置那份字节逐字节相同，且两条笔记的 `attachment_data` 都拿得到字节不是具名错误；⑤ 两条新笔记的 `has_attachment` 派生列为真（否则前面几条是被"根本没建出来"混过去的）。口径是**只登记、不 prune**：多算引用只是少收一点磁盘，少算引用是丢数据，方向不能反。证据：`attachment_gc.rs::a_local_note_that_references_an_image_in_its_doc_protects_its_bytes`；变异自证 **M46**（`create_note` 那一步传空引用表）→ 红在"三条却只认两条"；**M46b**（`commit_edit` 那一步同样传空表）→ 红在同一句的 3→2；这条测试因此对**两支**写路径各自会红。顺带把 apply 那侧的重复登记收掉（更新分支原本登记两遍，`commit_edit` 一遍 + 尾部一遍），移动后的那一遍由既有门禁 `attachments_travel_between_two_devices_through_the_real_server` 咬住 —— **M46c**（apply 新建分支传空表）红在那条的"B 上这条笔记必须真的引用着这份字节" | L2,L3 | P6 |
| FT-ATT-35 | 两条都 `available ∧ present` 的附件行，各自挂一支**已经 failed** 的上传待办（走公开的 `outbox_take` + `outbox_state` 标出来的，不手改表）；其中一条的笔记被永久删除 ⇒ 零引用 | `mark_attachments_quarantined` 只点名那一条零引用的 | 隔离要把这一行的待办**连 `failed` 一起**结掉，但**不许顺手动别人的**：判据 ① 结完之后 `outbox_len(Failed)` 必须从 2 掉到 **1**（留着那条 failed 就等于"待发操作"永久虚高 —— `outbox_pending` 的口径是 `('pending','inflight','failed')`，而被隔离的行已经离开下载与上传两个队列的取活范围，从此没有任何消费者会去结它，§18 要的是这个数诚实）；② 剩下的那一条必须**仍然是仍被引用那行的**待办（`entity_key` 逐条比对）—— GC 不是替用户吞掉同步失败的那只手。与引擎那条 `outbox_settle` 的区别写在 `mark_attachments_quarantined` 的注释里：那条是"这轮做完了"的结清、`failed` 归退避重试管；这一条的前提是"这一行被 GC 认领了"。证据：`attachment_queue.rs::the_quarantine_settles_failed_attachment_ops_too`；变异自证 **M48**（把 settle 语句的 `IN (...)` 改回两支）→ 红在"还剩 2 条"。**这条门禁第一次跑红红的是它自己的前置**：`outbox_take` 会把取到的行写成 inflight，我分两次取，第二次自然什么都取不到 —— 报的是"前置缺待办"而不是判据，按 §40 记在这里 | L2 | P6 |
| FT-ATT-36 | 一台设备：真上传一张图 → 永久删除那条笔记 → 隔离完成（账上 `missing ∧ present ∧ deleted_at`，字节在隔离区），手上有一个真适配器与可注入的服务器 | 分三次改服务器的回答，每次跑 `confirm_still_remote` + `purge_verified_blobs` | 销毁是本仓唯一不可逆的那一步，它的凭据必须是**当下问到的事实**，不能是隔离那一刻写下的旧结论（账上那个 `present` 已经隔了整个宽限期；对面设备可能还在画这张图，而本机那条 SQL 看不见别人的账）。三种回答各钉一遍：① **问不到**（这里注入 503）→ 清单必须是空的、字节一根汗毛都不动、`remote_state` **仍是 `present`**（把 503 写成"没有"就是拿猜代替问，§27 那条边界在这里反着再用一遍）；而且这一问**必须真的发出去过** —— 判据打在 HEAD 计数上（`heads_for_sha` 增加），否则"我们先问了服务器"这句注释在代码被改回去之后照样能留着；② **404** → 清单空、字节还在隔离区、这一行记成 `absent`（它是这张图现在唯一的地方）；③ 记过结论之后**不再每轮重问**：`gc_ready_to_purge` 只收 `present`，所以第二次调用时该 sha 的 HEAD 计数**必须一字不动**（§27/§28 那句"不许空转"在这一格的样子）；④ 服务器确认还在 → 这才轮到销毁，行与隔离区那份字节一起消失。代价照实写在 DATA-MODEL §8 / SYNC-PROTOCOL §13：判成 `absent` 的那一份从此不由 GC 销毁，只能由引用回来或用户点「重新上传本机这份」接手 —— 宁可多占磁盘，不可少问一句。证据：`attachment_gc.rs::nothing_is_destroyed_until_the_server_confirms_it_still_has_the_bytes`；变异自证 **M49**（`Ok(true)` 放宽成 `Ok(_)`，也就是把 404 当 200）→ 红在"服务器已经没有副本了还销毁 = 把最后一份字节删掉"；**M49b**（把候选集那句 `AND remote_state='present'` 去掉）→ 红在"同一扇门每 20 秒敲一次 = 空转"。两条都实测跑过红。另：`attachment_gc_scale.rs` 那格规模基准从此**只量本地两步**（删行 + 删文件），不含这一次 HEAD —— 它的注释里写明了读数没包含什么 | L3,L4 | P6 |
| FT-ATT-37 | 同一份数据目录上开**两个 store**（各自的连接、各自的候选查询；共享的正是跨进程时也共享的那两样 —— 同一份 SQLite 与同一份磁盘）。一条带图笔记被永久删除 ⇒ 两边**都**先拿到同一条回收候选 | 两边各跑一次回收；再让两边各拿一份"服务器确认还在"的到期清单，各跑一次销毁 | 独立审查第 5 条。**先纠正一处前提**：§48 原本写着"第二个进程会被锁挡住"，那是没验过的直觉 —— `store/pool.rs` 只有 `PRAGMA busy_timeout=5000`，**没有单实例锁**，第二个 store 打得开。前提既然挡不住，就只能量后果，判据两条：① 回收只许有**一家**把这一条算成自己认领的（`moved == 1`），且隔离区里恰好一份、内容一字不差、正式位置空着；② 销毁只许**数到一次**（`destroyed == 1`），两边读同一份账都得"这行没了"，隔离区不许留下第二份。**为什么它安全而不是运气**：互斥不在"挪文件"那一步，而在账上 —— `mark_attachments_quarantined` 的 `WHERE local_state='available' AND deleted_at IS NULL` 与 `purge_attachment_rows` 的 `DELETE ... WHERE NOT EXISTS` 都是 compare-and-set，而 host 只按**真删到的那些 sha** 去动文件。变异自证 **M50b**（把"只报告真删掉的那些"改成"报告整份清单"）→ 红在"同一行被销毁两次"。**同时记一条否定的结果**：**M50**（把 `mark` 的 `== 1` 放宽成永远算成功）**没有把这条门禁打红** —— 两边各自重查候选集，第二条在第二家那里已经不是候选了。所以这条测的是"销毁的互斥在返回值上"，而"认领不重复计数"由 FT-ATT-31 里那句幂等断言看着：**两个 guard 各管一头，不合成一个说法**。**仍未验**：真起两个 OS 进程跑同一轮 GC（这里是一个进程里的两个 store，交错度低于真进程）—— 按 §40 记在 §48 追记，不算已收口 | L3 | P6 |
| FT-ATT-38 | 一条仍被笔记引用的附件：上传待办走公开接口落到 `failed`（`outbox_take` → `finish_attachment_ops(sha, false)`，不手改表） | 先看"服务器还没有这份"时收口器怎么动，再把 `remote_state` 写成 `present`（另一轮成功、或对面设备传上去后清单确认）后跑一次 `settle_satisfied_attachment_ops` | 这一条是"§18 要求待发计数诚实"的另一半，也是**把一句写错的注释换成实测**：代码注释原本说 `failed` 行"归退避重试管"，实情是**没有任何后台消费者会再去碰它** —— 文本引擎按 `local_views` 规划、附件轮按 `attachments` 的状态挑活、`outbox_take` 在生产里没有调用方，而 `outbox_pending` 的口径含 `failed`。判据两条，方向相反：① **状态没满足时一条都不许结**（返回 0）—— 那是"这一项确实还没传上去"，藏掉它就是替用户宣布成功；② **状态满足了就必须结掉**（返回 1，且 `outbox_len(Failed)` 归 0）—— 已经传上去了还挂着一支 failed = 计数永远不会掉。与 FT-ATT-35 的分工：那条管"GC 认领了一行、这行不再有任何活可干"，这一条管"笔记还在引用、活干完了"。证据：`attachment_queue.rs::a_failed_upload_is_closed_once_a_later_round_proves_it_uploaded`；变异自证 **M51**（收口器的 `IN (...)` 去掉 `failed`）→ 红在第②条那句"已经传上去了的那支 failed 必须被结掉"，而第①条在两种版本下都绿 —— 它钉的是"不许过度收口"那一半，本来就不该被这个变异影响。同批把 `outbox_settle` 那句错注释改成实情（failed 的三条出路：下一次变更重新入队 / 附件侧两个收口器 / 用户点重试） | L2 | P6 |
| FT-ATT-39 | **两个真的 OS 进程**开在同一份数据目录上：父进程真上传一张图（`seed_uploaded`），再用公开接口造 24 份"零引用而 `present`"的字节（`attach_blob` 真写字节 → `set_attachment_states(..., "present")` → 永久删那条笔记），两边各自先 `gc_quarantine_candidates` 把 25 条清单取在手里并落到栅栏文件，才等同一个放行信号 —— "两边握着一份还没被认领的同一清单"这一格是被造出来的，不是撞运气的 | 放行后两边同时跑认领（`reclaim_unreferenced_blobs`）；再各自向服务器问"还在不在"（`confirm_still_remote`）拿到同一份销毁清单，第二道栅栏放行后同时动手（`purge_verified_blobs`）；子进程最后再读一次账 | GC 是本仓唯一一段"两个东西可以同时销毁用户字节"的代码，而它**没有单实例锁兜底**（`store/pool.rs` 只有 `PRAGMA busy_timeout`，"应用被开了两次"在桌面上是能发生的形状），所以这一条把 FT-ATT-37 的"同进程两 store"升级成真的跨进程。判据四条：① 认领总数必须**正好等于**候选份数（多了 = 同一行被两家各认领一次）；② 没有哪一份还压在正式位置；③ 每一份的字节都必须**在隔离区且内容一字不差**；④ 销毁只许数到一次，且两家事后各读各的账都得到 `absent\|absent`。③ 抓出来的是一条真的丢数据缺陷：`quarantine_move` 在 `rename` 之前先 `remove_file(目标)`，而那个目标可能正是对手刚搬进去的**唯一一份** —— 删掉它之后自己的 `rename` 又因为源已经不在而失败，于是正式位置与隔离区同时空着，而账上写着"已隔离"。修法是去掉那一句预删（`rename` 在 Unix 与 Windows 都会原子替换目标，预删只是造窗口）。证据：`attachment_gc.rs::two_os_processes_on_one_data_dir_claim_and_destroy_the_bytes_exactly_once`（子进程模式 `gc_race_child_process`；栅栏带 `stop` 与超时原因，父进程 panic 时子进程不会把测试二进制占着不让重新构建）。变异自证 **M53**（`reclaim` 把"账上真认领了几条"换成"我搬了几条"）→ 红在①，实测父 24 / 子 22 / 候选 25 ⇒ 两边在 21 条上真重叠；**同一条变异打在 FT-ATT-37 上仍然是绿的**（那条按顺序调用两家，第二家重查候选已经空了），所以这条门禁有 FT-ATT-37 抓不到的独有能力。**M52**（`purge_verified_blobs` 报告整份清单而不是真删掉的那些）→ 红在④。**M54**（把那一句 `remove_file` 加回去）→ 红在③，修前实测 **4/5 轮命中、单轮最多 14 份**，修后 **24/24 绿**。两道栅栏之间的顺序也是判据的一部分：认领与销毁之间必须等对面那一家把账写完 —— 那一行由赢的一家登记，输的一家抢在它提交之前读"到期清单"会读到空（实测 3/33 轮红在这个前置上，插桩给 `1/0` 而同一瞬直接再问服务器答 `Ok(true)`，说明错的是读的时机，不是账也不是字节）。仍没验的：这份证据是同一台 Windows 机器上的两个进程；两台真设备各自跑 GC 属于 G7/G8，不在这一条里 | L3 | P0 |
| FT-ATT-40 | 一份库里三条附件：两条的**登记尺寸被写成比实测大**（正文里的声明值是客户端各写各的，而 `upsert_attachment_row` 的规则是 `MAX(旧, 新)` —— 偏大那一路它自己纠不掉），第三条尺寸是对的；另外点名一条**库里根本没有**的 sha | `Store::set_attachment_sizes([(sha, 实测长度)…, 那条不存在的])` **一次调用**；另有一形：`sweep_lost_local_blobs` 在一条都没有降级的健康库上跑一轮 | 这是 §48 缺口 G4 漏下的另一半 —— 当年只把体检的**降级**批量成一次写事务，尺寸回填还是每条一次提交。2026-09-28 量出来它不是理论账：一轮慢路 ~320 ms，而"读 1 KiB×200"与"读 64 KiB×100"（IO 差 30 倍）耗时一样，夹具里同样条数的逐行**写提交**是 100 次 106 ms / 1000 次 1151 ms（≈1.1 ms/次），说明那一轮的钱花在提交上不在哈希上；刚导入 / 刚升级过的库可以有成百上千条声明尺寸是偏的，逐行提交就是成百上千次排队占住写锁，而用户那次保存正排在后面。断言四段：① 点名的两条都改成给定长度，**没点名的那条一个字不动**（批量语句漏掉 `WHERE sha256` 就是全表形状）；② 库里没有的那条**不计入成功数**（返回 2 而不是 3 —— host 那声 warn 的判据就是这个差值，静默吞掉就是 §39 禁的那种 fallback）；③ `size` 允许**往小**改（写成 `MAX` 就等于没回填，那一行此后每轮整份重读重哈希）；④ 两半状态与 `deleted_at` 没被带跑，且空输入是 `Ok(0)` 而不是错（健康库每轮都攒出零条）。另有一条**顺序**账不在这里、由 FT-ATT-20 钉：回填必须落在 `lost.is_empty()` 那个早退**之前** —— 把 `set_attachment_sizes` 那一段挪到早退之后，FT-ATT-20 立刻红（它的现场正是"一条都没降级、只有尺寸是偏的"），变异自证 **M63** | L2 | P6 |
| FT-CHK-01 | 新笔记 | 建 checklist 3 项，勾选第 1、3 项 | 重开后勾选状态为 `[x][ ][x]`；纯文本抽取输出与该状态一致 | L1,L5 | P2 |
| FT-CHK-02 | 同 checklist | 设备 A 勾第 1 项、设备 B 勾第 2 项（同一 base） → 双向同步 | 结果为两项都勾（块级三方合并不丢勾选）或产生冲突副本且两份内容完整可见；**禁止出现"只剩一项勾选"的静默覆盖**（INV-01/05） | L3 | P5 |
| FT-CHK-03 | checklist 中间项 | 在第 2 项内换行 / 删除整项 | 项序连续无空项；重开后条目数 = 操作后预期数 | L5 | P2 |

### 原始指令 §27「附件故障注入」十条 —— 实证账（2026-09-27）

判定口径只有一条：**这条有没有一条会因为它而红的自动测试**（§40：不接受"理论通过 / 应该没问题"）。

| §27 原句 | 现状 | 证据 / 缺口 |
| --- | --- | --- |
| 上传中断 | **已覆盖（2026-09-28 补齐，且补的是更阴的那一格）** | FT-ATT-15（半上传：服务器只读到半份 body）+ FT-ATT-22（**发布那一步 MOVE 被掐**：前面全都成功，客户端只剩"不知道"）+ FT-ATT-24（**复读校验被掐**：对象已经在服务器上了，但我们没证据）。FT-ATT-04 那行原先写的 `FAIL(abort,target=attachments/**)` 之所以一直只是文档，是因为工装只有按序号的 `drop_after_n`（一开就从那一条起**全部**断），造不出"只掐这一个动作"—— 今天补了 `abort_for`（与 `status_for`/`hang_for` 同一份 `rule_hit` 文法），并且它**第一次跑就红在自己的前置上**（规则还没接进服务器判定） |
| 下载中断 | 已覆盖 | FT-ATT-14 |
| Range 恢复 | 已覆盖 | FT-ATT-09、FT-ATT-10（含"服务器探测答 206、正式请求忽略 Range"） |
| 错误 hash | 已覆盖 | FT-ATT-13（客户端复算 + `ingest_blob` 两处独立拦截，实测两处同时拆掉才红） |
| 文件不存在 | 已覆盖（两半都在） | 远端没有 → FT-ATT-16；本机没有 → FT-ATT-11 |
| 远端文件损坏 | 已覆盖 | FT-ATT-13（`Backend::Fs` 上原地改坏磁盘字节 + `RESTART`，DUMP 里核对服务器发的确实不是原字节） |
| 本地附件缺失 | 已覆盖，**且这一条暴露了一个真缺陷** | FT-ATT-11：体检前这类行两个队列都看不见 → 那张图永久坏掉而系统以为自己修好了。体检自己那两笔账另有门禁：FT-ATT-20（2026-09-28 补：哈希算完了却不回填尺寸 → 同一行每轮整份重读重哈希）、FT-ATT-21 / FT-ATT-21s（同日补：候选不分页 + 每行一个写事务 → 整个目录被删时一轮里 N 次提交把用户的保存顶在写锁外，§48 G4） |
| 网络超时 | 已覆盖（2026-09-27 补齐，先修了工装） | FT-ATT-17。此前做不出这条的原因很具体：注入器只有**全局** `timeout_all`，一挂就把清单与探测一起挂住，测到的是"网络不通"。现在有了 `hang_for`（与 `status_for` 同套 glob 文法），才能表达"只有那一个附件对象不答应"。文法本身另有一条单测盯着（`injection.rs::hang_on_follows_the_same_rule_grammar_as_status_for`），因为匹配器一旦漂掉，消费者的失败会长得像"产品没问题" |
| 服务器返回 412 | 已覆盖（2026-09-27 补齐，**并抓到一条真缺陷**） | FT-ATT-18 / FT-ATT-19：`post:MOVE→412`（真的并发落成了）当成功、裸 `MOVE→412`（什么都没落成）不许当成功。旧实现从 MOVE 的 405/412 那条臂**直接 `return Ok(())`，跳过了紧跟其后的复读校验**，于是账上凭空记一个 `remote_state=present`；第二台设备从此永远等一份服务器上没有的字节。改后判据只有一条：**present 必须出自一次读回的内容比对，不是出自一个状态码**。文本侧的 412 重规划另有 SY-FAULT-10 |
| 服务器返回 404 | **已覆盖（两侧都在，2026-09-28 补齐上传侧）** | 下载侧 → FT-ATT-16（404 是结论：记 `absent` 后收手，后续轮次零请求）；上传侧 → FT-ATT-23（一次失败的 MOVE 上的 404 **不是**结论：账上精确保持 `(available, unknown)`，两个方向都不许写，且暂存要去清） |

一句话账（按上表逐行数，2026-09-28 更新）：**十条现在十行都有会红的自动测试**，其中三行是"两侧都在"的（文件不存在 = 远端 FT-ATT-16 / 本机 FT-ATT-11；服务器返回 404 = 下载侧 FT-ATT-16 / 上传侧 FT-ATT-23；上传中断 = 半上传 FT-ATT-15 / 发布被掐 FT-ATT-22 / 复读被掐 FT-ATT-24），两行各带一条体检自己的账（本地附件缺失那行的 FT-ATT-20 / FT-ATT-21+21s）。**剩下没做的不是这十句里的某一句，而是组合**：一次里同时坏两种（比如"发布被掐 + 对面同时在传同一份"）、以及 GC 与体检的竞争 —— 见下面 §3.4 的"还没测的组合"。另有一条跨章节的账照旧记着：记法表里的 `times` 参数**工装里没有实现**，所以 `SY-FAULT-02/03/04/09/10` 那些写着 `times=` 的行都还不能按字面跑起来 —— 见上面"这张记法表自己的账"。

### 原始指令 §28「代理故障注入」十二条 —— 实证账（2026-09-27）

同一把尺子：**这条有没有一条会因为它而红的自动测试**。此前 §28 的全部证据都是
`notera-cli net probe` 手工跑出来的一次性 PROBE（PROXY.md §9 把三条证据链写得很清楚，
但"跑过一次"和"每次提交都会跑"是两件事）。今天把能变成门禁的先变成了门禁：
`crates/notera-webdav/tests/proxy_routing.rs`。

| §28 原句 | 现状 | 证据 / 缺口 |
| --- | --- | --- |
| direct | 已覆盖 | 全仓每一条同步测试都是直连；`proxy_routing.rs` 里另有"直连写入 → 换回直连读出"两条腿 |
| HTTP proxy | **已覆盖（今天新增门禁）** | `an_http_proxy_route_is_the_only_way_through`：服务器挂"只接受经代理到达"的策略 ⇒ 配了 HTTP 代理必须成、直连必须 403，并且要读服务器自己的 `rejections_403_not_proxied` 计数。**判据是差分的** —— 这是唯一能证"真的用了代理"的形态。变异自证 M9：把 `configure()` 里那条 `b.proxy(p)` 换成 `no_proxy()` → 两条测试同时红（`配了 HTTP 代理却被服务器拒掉：403` / `指向死代理的请求居然成功了`） |
| SOCKS5 | **未覆盖（BLOCKED）** | 只有配置解析与 `proxy_url` 构造的单测（含"明文 http 代理不许带凭据"那条）。端到端做不了的原因很具体：本工装的"代理"其实是**源站自己**扮的 —— 它认识 CONNECT 与绝对形式，但不认识 SOCKS 握手。解除条件：写一个会答 `05 00` 并连到**另一个**监听端的迷你 SOCKS5 应答器，代理与源站分开 |
| 错误密码 | **未覆盖（BLOCKED）** | 需要一个会回 `407 Proxy-Authenticate` 的代理，工装没有。已覆盖的是最容易出错的那一半：口令不外泄（`proxy_credentials_never_leak_into_the_route_proof` + notera-net 的 `debug_never_leaks_the_password` / `userinfo_is_stripped_everywhere`） |
| 错误 proxy host | 已覆盖（端口形态） | `a_dead_proxy_fails_and_a_bypassed_host_still_works` 第一腿：指向没人监听的端口必须失败；成功就等于"配了代理等于没配" |
| 代理 DNS | 已覆盖 | 同一测试 ①b 腿：`no-such-proxy-host.invalid` 必须失败（静默直连会被当场抓住） |
| TLS 失败 | **未覆盖（BLOCKED）** | `TlsPolicy` 三种构造有单测（Strict / CaBundle 解析 / Pin 归一化），但**没有一次真 TLS 握手失败**：工装是明文 HTTP 服务器。与 PROXY.md U2/U3 同一条根因，解除条件同 B2（可访问的真实端点） |
| 超时 | 已覆盖（同步路径上） | 预算是 `Timeouts::per_request`，生效证据在 FT-ATT-17（挂死的附件请求在 45 s 内自己放手而文本轮照常）。"代理握手超时"没单独做，与 TLS 那条同根 |
| 取消 | 部分（只有 PROBE） | PROXY.md §7 记的 402–416 ms 实测是手工 PROBE，不是门禁。要做成门禁需要 `hang_for`（今天已有）+ 一个把"取消发生"打进审计的断言点 |
| 重试 | 部分 | 传输失败的重试与 `Retry-After` 两种形态有单测（`retry_after_accepts_both_forms`）；**"观察到一次带退避的第二次尝试"这种端到端断言仍缺**，因为它需要 `times`（见本文开头"这张记法表自己的账"） |
| 退避 | 已覆盖（纯函数层） | `delay_is_monotonic_and_capped`、`jitter_is_reproducible_and_bounded` |
| 恢复 | 已覆盖 | 同一测试第四腿：撤掉死代理换回直连，同一份内容立刻可读（前一次失败不在出口层留脏状态）；跨进程恢复在 `reconnect.rs` |
| **保证句「网络问题永远不会让本地数据不可用」** | 已覆盖（今天补齐关键一环） | FT-ATT-17（只有附件端点挂死时，正文与新笔记的同步照常完成）、`reconnect.rs`（六轮各坏一次，恢复后两台逐条一致、待办归零）、`crash_recovery.rs`（崩在提交点之后数据仍完整） |

一句话账（按上表逐行数，13 行 = §28 原句 12 条 + 最后那句保证句）：**9 行已覆盖、2 行部分（取消、重试）、3 行未覆盖**（SOCKS5 端到端、407 错误密码、真 TLS 握手失败）。今天又补了两条：App 侧那条代理通路（`notera-host/tests/proxy_account.rs`：配置的代理 → `net_proxy()` → 出口客户端 → 真同步）与"撤掉代理必须失败"的差分腿 —— 审查指出这属于"实现齐全、单测全绿、没人调用"那个老形状，现在它有人叫了。
四条缺口的根因是同一件事：**工装的"代理"由源站扮演，且没有 TLS**。一次解掉前三条要做的是把测试拓扑改成
"客户端 → 真转发代理（HTTP，可选 407 / SOCKS5）→ 独立源站（可 TLS）" —— 那是**一个新的 harness 组件**，
不是几条用例，故按 §40 记在这里而不是顺手做完。

### 富文本节点往返

统一做法：golden 文档 fixture → 客户端加载 → 立即保存 → 导出 canonical JSON → 与 fixture **逐字节**比较；同时断言纯文本抽取等于 fixture 的 `plain` 字段（PROBE:`canonical-json-for-hashing`）。

| ID | 节点 | 操作 | 预期（可观测） | 层 | 阶段 |
| --- | --- | --- | --- | --- | --- |
| FT-RT-01 | paragraph | 3 段普通文本往返 | canonical JSON 相等；渲染段落数 3 | L2,L5 | P2 |
| FT-RT-02 | heading | H1–H6 各 1 个 | 6 个 heading 的 `level` 属性精确保留；大纲视图条目数 6 | L2,L5 | P2 |
| FT-RT-03 | text + marks | 对同一段依次加 bold / italic / underline / strike / code / highlight（6 个子用例 + 1 个全叠加用例） | 6/6 单 mark 的 canonical JSON key 精确、属性顺序稳定（hash 不因键序而变）；全叠加时 6 个 mark 同时存在，渲染样式 6/6 可见；撤销一次只移除一个 mark | L2,L0,L5 | P2 |
| FT-RT-04 | link | 插入 `https://example.com/a?b=1&c=中文`，再编辑链接文字 | `href` 字符级保留（含 `&`、未编码中文）；点击在新窗口打开（不劫持当前文档）；纯文本抽取含链接文字不含裸 URL 重复 | L2,L5 | P2 |
| FT-RT-05 | ordered / bullet list | 嵌套 3 层列表 + 中间插入项 | 有序列表编号由渲染得出、模型中不存字面序号；嵌套深度与类型逐项保留；插入后编号连续 | L2,L5 | P2 |
| FT-RT-06 | checklist | 见 FT-CHK-*，另测 checklist 嵌于 list 内 | 节点类型仍为 checklist（不被降级为普通 list）；勾选属性往返一致 | L2,L5 | P2 |
| FT-RT-07 | quote | 嵌套 quote 2 层 + quote 内含 code block | 层级与子节点类型保留 | L2,L5 | P2 |
| FT-RT-08 | code block | 语言 `rust` + 含中文注释 + 尾随空行 | `language`、`value` 逐字节相同（含尾随换行不被 trim） | L2,L5 | P2 |
| FT-RT-09 | image | 内联图片节点往返（引用 sha256，不内嵌 base64） | 节点中不出现 base64 数据；引用路径解析成功、渲染尺寸属性保留 | L2,L3 | P6 |
| FT-RT-10 | attachment | 非图片文件（.pdf 3 MiB）附件节点 | 节点含 sha256 引用；文件名与字节数往返一致；下载后文件与源文件 hash 相同 | L2,L3 | P6 |
| FT-RT-11 | horizontalRule | 插入 → 两端各留一段 | HR 存在且位置正确；删除相邻段落不吞掉 HR | L2,L5 | P2 |
| FT-RT-12 | 混合长文档（含上述全部 12 类节点各 ≥2 个） | 打开 → 不编辑 → 保存 → 导出 | canonical JSON 与输入 fixture 逐字节相同（**零漂移**）；节点总数、类型计数与 fixture 一致 | L2 | P2 |
| FT-RT-13 | 混合长文档 | 打开 → 编辑第 5 段 → 保存 | 除第 5 段路径外，其余节点在 canonical JSON 中 byte-range 相同（最小写放大，为三方合并服务） | L2,L0 | P2 |

### 前向兼容（不可协商）

| ID | 前置 | 操作 | 预期（可观测） | 层 | 阶段 |
| --- | --- | --- | --- | --- | --- |
| FWD-01 | fixture 含 1 个未知节点类型 `{"type":"mermaid",...}` 与未知属性（`data-x:"y"`），由新版客户端写入 | 旧版客户端（Phase 本客户端）打开 → 编辑另一段 → 保存 → 导出 | 未知节点**原样保留**（type 名、全部属性、子树顺序逐字节不变）；未知属性不丢失；**旧客户端不得销毁新内容**（INV-10） | L2,L3 | P2 |
| FWD-02 | 同 fixture 的未知节点出现在文档末尾/中间/唯一子节点三种位置 | 同 FWD-01 | 三种位置均保留；插入编辑不改变未知节点相对顺序 | L2 | P2 |
| FWD-03 | 文档 `v` = 客户端支持版本 + 1 | 打开该文档 | 以 **只读** 打开：编辑器不可输入（工具栏全部禁用/隐藏）、保存按钮不可用；页面给出可见的版本过旧提示；关闭不产生任何写入（`rev` 不变，无 PUT 请求，`STATS` 请求数 0） | L1,L5 | P2 |
| FWD-04 | 只读文档在新版客户端被编辑后同步回旧客户端 | 旧客户端再次打开 | 仍是只读；内容完整显示；不出现"降级重写" | L3 | P5 |
| FWD-05 | `protocol.json` 的 `protocol` 字段 = 客户端支持值 + 1 | 启动同步 | 明确拒绝并可见提示（含期望/实际协议号）；不写入任何远端对象（`DUMP` 前后一致）；本地仍完全可编辑（INV-06） | L1,L2,L5 | P4 |
| FWD-06 | envelope 含未知顶层字段 / 未知 `kind` | 客户端拉取 | 未知字段 round-trip 保留（不被 strip 后重推）；未知 `kind` 记录**不进入**本地权威表（INV-08），但计入诊断日志且不中断整轮 | L1,L2 | P4 |

### 外观、启动、离线、保存、导入导出

| ID | 前置 | 操作 | 预期（可观测） | 层 | 阶段 |
| --- | --- | --- | --- | --- | --- |
| FT-THEME-01 | 系统浅色 | 启动 App | 首帧即浅色：从窗口出现到首帧，`<html>` 主题标记无 `dark→light` 反向跳变（截图序列逐帧比对，白闪帧数 0） | L5 | P3 |
| FT-THEME-02 | 运行中切换系统主题（自动模式） | 观察 | ≤100 ms 内跟随；无重载、编辑光标位置与滚动位置保持 | L5 | P3 |
| FT-THEME-03 | 手动指定深色后重启 | 重启 | 保持用户覆盖，不回到"跟随系统"；深色下 6 种 mark（尤其 highlight）与 code block 背景可读（对比度截图基线，diff ≤1%） | L5 | P3 |
| FT-BOOT-01 | 有 3 条未同步本地变更 | 完全退出 App → 重新启动 | 3 条变更全部在；outbox 计数 3（辅助）；列表条目数与退出前一致；无重复笔记 | L5,L1 | P1 |
| FT-BOOT-02 | 崩溃（`CRASH` 于本地应用事务中） | 重启 | App 正常启动；无"数据库损坏"提示；已提交内容全在、未提交段落整段缺失（而非半新半旧）；`sync_operations` 中 `inflight` 归位（INV-04） | L4 | P4 |
| FT-OFF-01 | `OFF`（服务器停止）+ 断网 | 连续使用全部功能 5 分钟：新建/编辑/删除/恢复/建夹/移动/固定/checklist/插图/搜索/导出 | 所有操作均成功且无"网络错误"类阻断；期间 `STATS` 请求数 0（本地写入不依赖网络，INV-06） | L3,L5 | P4 |
| FT-OFF-02 | FT-OFF-01 之后 | `ON` → 触发同步 | 一轮（≤2 轮，含冲突）内全部离线变更出现在远端；`DUMP` 中每条变更实体的 `rev` 单调递增、无空洞；本地内容 0 丢失 | L3 | P4 |
| FT-OFF-03 | 离线期间删除 N，另一设备 B 修改 N | 双方上线并同步 | 冲突按 delete+update 策略处理（见 SY-CONF-04），两侧内容均保留或明确择一 + 冲突副本；无静默丢失 | L3 | P5 |
| FT-SAVE-01 | 一篇笔记 | 以 30 ms 间隔连续输入 20 个字符后停止 | 停止后 ≤1500 ms 内出现一次本地落盘；20 次按键产生的**落盘次数 ≤3**（暂定 debounce 基线）；期间网络请求数 0 | L1,L5 | P2 |
| FT-SAVE-02 | 同上 | 输入中立即关闭窗口 / `CRASH` 于最后一次 debounce 之前 | 重启后至少保留最后一个已落盘版本；缺失内容 ≤ debounce 窗口内字符数（可见、可解释），且不产生空笔记垃圾条目 | L4,L5 | P4 |
| FT-SAVE-03 | 编辑中 | 断网、磁盘写满（模拟只读目录） | 保存失败必须有可见提示且**内存态内容不丢**；重试成功后提示消失；绝不静默标记为"已保存" | L5 | P3 |
| FT-SAVE-04 | 一条已落库的笔记，正文里有一段可加粗的文字，列表里还有另一条（要点"全部"再点回来制造这一形） | 加粗 → 自动保存出发（这一支还在飞、回包未到）→ 立刻点"全部"再点回这条笔记 | **在飞的那一支保存落地之前不许开始回读**：`flush()` 原来只等"待发的 debounce"，漏的正是这里 —— 回读发生在写落地之前，拿到的是"这次写之前"的快照，刚存的加粗被当成旧数据盖掉；而那支保存的回包发现"正文又变了"，会照着被盖掉的版本**再存一次**，用户的编辑就此消失（实测形状：`edit_note rev=6 bold=1 → 200`，25 ms 后 `edit_note rev=7 bold=0 → 200`；屏幕上明明有粗体，刷新后没了）。判据两条各管一头：① 那个标记还在；② 这一轮**只许**发出带标记的那一支写 —— 任何"把没标记的那一版写回库"都是丢数据。工装要点：`get_note` 必须返回"最近一次**已应用**的那版"（一个会随写推进的最小核心），否则这条门禁测的是我搭的假数据而不是竞态。证据：`stores/editor.spec.ts`「正在飞的保存与重新打开」；变异自证 **M47**（把 `await saveChain` 换成 `void saveChain`，即修好之前那句）→ 红在"标记还在"那一句。浏览器那一侧同批补了写序列插桩：`verify-app` 任何一步红时，会把每支 `edit_note` 的 id/rev/块数/带不带标记按时间打出来 —— 这条缺陷就是从那里认出来的 | L1,L4 | P2 |
| FT-IO-01 | 500 条笔记 + 20 附件 | 导出完整包 → 在干净设备导入 | 条目数 500、文件夹树结构一致、每篇正文 canonical JSON hash 一致、20 附件 sha256 一致；固定/回收站状态按导出规格保留 | L1,L5 | P6 |
| FT-IO-02 | 同上 | 导出 → 导入到**已有同名数据**的设备 | 明确策略结果可观测：不覆盖已有内容，冲突条目以副本形式并存（无静默覆盖，INV-01）；导入报告"新增 X / 冲突 Y / 跳过 Z"，X+Y+Z = 500 | L1,L5 | P6 |
| FT-IO-03 | 干净设备 | 导入被截断的包（尾部 1 字节删除）/ hash 不符的包 / 含未知节点类型的包 | 前两者：整体失败或按报告逐条失败，本地不出现半包状态，原库条目数不变（INV-08）；第三者：成功导入且未知节点原样保留（FWD-01 同源） | L1 | P6 |
| FT-IO-04 | 有备份文件 | 备份 → 修改 30 条 → 恢复备份 | 恢复后为备份时点状态（条目数与 3 类 hash 精确匹配）；恢复前自动生成"恢复前快照"并可回退；恢复不改变远端（不自动推送恢复结果，除非用户触发） | L1,L5 | P6 |
| FT-IO-05 | 恢复进行中 | `CRASH`（CP-09） | 重启后**要么**旧库完整可用**要么**新库完整可用，不存在混合状态；原库文件未被破坏（hash 或条目数可核对） | L4 | P6 |
| FT-IO-06 | 从外部格式导入（Markdown/纯文本各 1 组） | 导入含中文文件名、CRLF、BOM、无扩展名的文件 | 每篇正文可读、条目数等于源文件数；文件名冲突时自动改名而非覆盖；0 字节文件按报告跳过 | L1 | P6 |
| FT-IO-07 | 一篇挂了真实 blob 的笔记（`attach_blob` 走产品路径，落 `<attachments>/<2hex>/<sha>`） | 经真 `dispatch` 导出（`includeAttachments: true`）→ 立刻 `read_bundle` 回读 | 包里的附件**条目数与字节**必须等于库里的那一个：`counts.attachments == 1`、`attachments[0].0 == sha256`、字节逐字节相同；不勾这个开关时必须是 0。导出与"平铺 / `{req:…}`"两种封套形状都要覆盖，输出路径也必须按用户指的位置写 | L2 | P6 |
| FT-IO-08 | 一个非空的库 | 导出整库 → 把同一个包导回**同一个库**（merge） | 幂等：笔记 rev 与 content_hash 一字不动（rev 一动，编辑器手里的 `expectedRev` 就成了旧的，用户会看到"这条笔记在别处被改动了"的假冲突），且不造出任何 open 冲突行 | L2 | P6 |
| FT-IO-09 | 「默认本 / 项目 / 项目·子夹 / 平级的别的」各带笔记与附件，**默认本自己也有一篇带附件的笔记** | 只导「项目·子夹」→ 导进干净库 | 包里恰好 = 子夹 + 祖先链（3 个文件夹）+ 范围内 1 篇笔记 + 它引用的附件，平级文件夹的笔记一个字节都不进，**祖先文件夹自己的那篇笔记与那个附件也不进**（祖先只是外键骨架，不是内容）；报告 `scope=="folders"`；包 `manifest.partial==true`；干净库导入后按原 id 读回、父本仍是那个子夹。范围里出现不存在的文件夹 id → 拒绝而不是忽略；`partial` 包走 `intoEmpty` → 响亮拒绝（缺笔记的永久删除公告，当成整库还原会让已删的笔记从别的设备回流） | L2 | P6 |
| FT-IO-10 | 一个只含 blob 字节、库里没有 `attachments` 行的干净库 | 还原带附件的包 | 走 `restore_blob`：校验 sha → 落盘 → **登记行**（`ingest_blob` 只会 UPDATE，行不在就整次导入失败）。远端态必须留在 `unknown` 并排进上传队列（写成 `present` = 谎报服务器已有，还原出来的附件永远不补传，第三台设备拿不到）；哈希不符一律拒收且不改已有远端态；重复还原不把 `present` 退回 `unknown` | L1,L2 | P6 |
| FT-CONF-06 | 一条 open 冲突 | 打开并排预览 | 两侧文本分别来自 `preview_text(id, rev)` 且**互不相同**（两版一模一样就说明面板在做样子）。注意真冲突里 `localRev` 与 `remoteRev` 常常**相等**（两侧各自从同一确认点推到同一个 rev），所以取法必须是右 `(noteId, remoteRev)` / 左 `(copyNoteId, copyRev)` —— 两栏都按 `noteId` 取就必然同款（`conflicts.spec.ts` 钉住）。核心没有的 rev 报 `not_found`，不许用空串冒充某一版；前端声明的每个命令名在核心都有分支（arch-check `edge:declared-commands-exist`） | L2 | P5 |
| UI-TREE-01 | 真浏览器 + 真 Rust 核心，停在设置页 | 侧栏点「＋」在默认本下建一个子层 → 回「全部笔记」→ 打开一篇笔记 | 子文件夹**出现在侧栏**且在"移动到"下拉里**选得到**（路径显示为 `默认本 / 子层`）。这一条是给跨语言形状准备的：`list_folders` 下发嵌套树，前端曾把 `children` 抹平 → 子层在整个界面里隐形，而平铺输入的单元测试全绿 | L4 | P6 |
| UI-NAV-01 | 停在设置页 | 点侧栏「全部笔记」/「最近删除」/ 某个文件夹 | 三者都必须把人带回列表视图（以前只改 `notes.mode`，界面纹丝不动 = 死按钮），且 `data-active` 只在 workspace 下点亮，不在设置页上假装"全部笔记被选中" | L4 | P6 |
| SY-CONF-07 | 两台设备真的把同一条笔记改成分叉的两份（真 TCP WebDAV，走 `App::sync_once` 这条产品路径） | 先 A 推起点 → A 本地改不推 → B 拉、改、推 → A 再同步 | A 侧恰好一张 open 卡片；**正文是 B 那一版**（§6.1 的采纳），**A 那一版完整活在副本里**；采纳后 `rev == sync_rev`，所以 A 的下一轮只推那篇副本、绝不把自己那一版盖回服务器（否则就是静默覆盖别人已确认的内容）；一次冲突只产生一篇副本，不逐轮刷；两栏预览各读各自那份 | L3 | P6 |
| SY-CONF-08 | 一条**没被用户处理**的 P11 冲突（删除 vs 修改；引擎故意不自动收敛） | A 连续同步三轮 | 未处理卡片始终 **恰好一张**、"本地副本"笔记始终 **恰好一篇**（每轮重复登记 = 骚扰 + 往用户库里塞垃圾）；任意一侧又改了才算新事实，允许进新卡片 | L3 | P6 |
| SY-CONF-09 | 两侧内容完全一样的"各自保存过一次"（生产形状：本地整条 sha256 vs 清单 12 位短哈希） | 一轮判定 | 判 **P7 收敛**，不是一张冲突卡片。`notera_core::same_content_hash` 是唯一比较处：容忍 `sha256:` 前缀与十六进制大小写，短形式须 ≥12 位且为真前缀；它只回答"是不是同一版"，写库仍由 I6 整值复核把关。旧 p7 单测两侧喂同一个假串，形状差测不出来 —— 新用例必须按生产的两种形状喂 | L1 | P5 |
| SY-CONF-10 | 一张已采纳的 UpdateUpdate 卡片 | 用户点"用我这一版" | 正文与副本**互换**（两版各有一处存放，谁都没被吃掉），正文变脏并重新公告；另一台同步后**同时持有两份**；卡片关闭 | L3 | P6 |
| FT-SETUP-01 | 全新安装 | 配置 WebDAV URL + 账号密码（含自签 CA） → 测试连接 | 成功时有可见确认；失败时提示区分 DNS / 拒绝连接 / TLS 不受信 / 401 / 403，不得只给"网络错误" | L5 | P4 |
| FT-SETUP-02 | 已配置 | 改为错误密码 → 同步 → 改回 | 错误期间本地不受影响且保留配置；恢复后一轮内追上；失败不删远端任何对象（`DUMP` 前后一致） | L3,L5 | P5 |
| FT-CRED-01 | 空凭据库 | `credential_store::put` 一条带非 ASCII 用户名与口令的目标，再 `get` | 原样读得回来（编码没走形）；覆盖同名后第二次说了算；`remove` 之后再 `get` 是 `None`，**再 remove 一次仍算成功**（删账户与回滚都这么调）。非 Windows 上断言的是具名错误 `credential_unavailable` 与 `None`，不假装通过。证据：`credentials.rs::a_stored_secret_comes_back_byte_identical_and_removal_is_idempotent`；变异 **M60** | L2 | P4 |
| FT-CRED-02 | 同上 | 正好 256 个 UTF-16 单元（全角字符）与 257 个各存一次 | 256 存得进且读得回；257 报 `credential_too_long` 并**带着实际单元数**，且**上一次那份还在** —— 被拒绝的写入不许顺手清掉已有的。上限的来源是系统侧的真实约束（generic credential 的 blob 512 字节）。证据：`credentials.rs::the_blob_limit_is_enforced_at_the_boundary_without_storing_anything`；变异 **M61**（把上限判断改成 `>` 的邻位） | L2 | P4 |
| FT-CRED-03 | 全新数据目录 | 只填用户名、**不填口令**去配置账户 | `hasCredential` 必须是 **false**。这一格以前是 true（引用照写、口令其实被丢掉），于是设置页说"口令已设置"而发布版永远同步不了 —— 指示说谎 + 核心能力静默失效。证据：`credentials.rs::a_draft_without_a_password_stops_claiming_a_credential_exists`；变异 **M55**（把引用退回"填过就算"） | L2,L5 | P0 |
| FT-CRED-04 | 已存过口令的账户 | 重新配置同一条账户、**不重填口令**（只改标签） | 引用还在、系统里那条口令一字不变 —— "没重填"不是"清空"（界面上那格显示的就是"已设置"的占位提示）。删账户时系统里那条必须跟着走。证据：`credentials.rs::an_account_reconfigured_without_a_password_keeps_the_stored_secret`；变异 **M59**（删账户不带走凭据） | L2 | P4 |
| FT-CRED-05 | 解析点 | 开发环境变量里放一个**别的**值，账户口令存进系统 | `App::secret_for` 必须回**系统里那一条**（顺序错了就会用环境里的旧值掩盖界面上刚改的口令，而 release 根本没有这个变量）。另验配置文件字节里没有明文口令。证据：`lib.rs::account_mapping_roundtrips_without_leaking_secrets`；变异 **M56**（不往系统存）、**M57**（先看环境变量） | L2 | P0 |
| FT-CRED-06 | 代理口令 | 配一条带用户名与口令的 HTTP 代理 → `net_proxy` | 引用要解析成真凭据（`username`/`password` 都在），删账户时代理口令也带走。以前这里一看到引用就报 `proxy_credentials_pending`，"填了代理口令"在出口层永远不成立（0.0.31 起那个码叫 `proxy_credential_missing` —— 0.0.29 已经能真存代理口令，"当前版本还不能安全地保存它们"那句说明说反了）。证据：`lib.rs::proxy_credentials_resolve_from_the_system_store`；变异 **M58** | L2 | P4 |
| FT-CRED-07 | 三条凭据错误码（`credential_too_long` / `credential_unavailable` / `credential_store_failed`）与那条代理码 `proxy_credential_missing` | ① `messageFor('cmd.<码>')` 逐条取文案；② `arch-check` 第 30 条扫 `CmdError::of(…)` 的第一个实参 | **0.0.31 补的这条登记的是一条"门禁盲区"，不是一条行为**：0.0.29 写的是 `CmdError::of(e.code())`，算出来的码不被"错误码必须登记文案"那条扫描看见，于是漏登记既不编译报错也不测试失败，界面安静退成通用兜底（非 Windows 配账号必踩，Windows 上口令超 256 单元也踩）。现在：三条码在 `App::secret_err` 里写成字面量 → 被第 12 条看见；`i18n.spec.ts` 的 `COMMAND_CODES` 里有这四条，`it.each` 断言"取到的不是通用兜底且长度 > 6"；新增第 30 条 `hygiene:error-code-must-be-literal` 把"算出来的码"这一形判红。变异自证 **M65**：把一个臂换成 `let code = "…"; CmdError::of(code, …)` → **29/30 红在那一行**（打印出 `crates/notera-host/src/lib.rs → CmdError::of(code)`），还原 → 30/30。**这条不声称测过"非 Windows 真机上那句话长什么样"** —— 本机没有那个平台，能证的只有"码有文案、且形状不再被漏"（见 ADR-0020 第 7 条） | L1 | P0 |

## 同步测试矩阵

约定：除注明外，前置均为"设备 A 已与 test-webdav 完成一轮同步、双方收敛"；`FAIL(...)` 见 §记法；每行结尾标注**必须成立的不变式**（见 §不变式）。

### 收敛类

| ID | 场景 / 前置 | 注入 | 操作 | 预期 + 不变式 | 层 | 阶段 |
| --- | --- | --- | --- | --- | --- | --- |
| SY-CONV-01 | Local→Remote，A 新建 1 条 | — | `A sync` → `B sync` | B 列表出现该条，正文 hash 与 A 相同；远端存在 `records/n/<id>.json` 且其**写入次序早于**引用它的 manifest（`STATS` 顺序断言）。INV-01,09 | L1,L3 | P4 |
| SY-CONV-02 | Remote→Local，远端被外部改动（直接 `PUT` 一个新 rev 到 test-webdav） | — | `B sync` | B 采纳远端内容；本地 `rev` 更新为远端值；不产生本地新 rev（不"回写抖动"）。INV-05,07 | L3 | P4 |
| SY-CONV-03 | Both changed（A 改第 1 段，B 改第 3 段，共同 base） | — | `A sync` → `B sync` → `A sync` | 段落级三方合并成功：A、B 最终第 1、3 段均为各自新版本，第 2 段一致；`rev` 各自连续。INV-01,03 | L3 | P5 |
| SY-CONV-04 | 无变化 | — | 连续 `B sync` ×3 | 每轮 `STATS` 请求数 ≤2 且 `manifest/index.json` 响应码 304；无 PUT；远端字节写入 0。INV-05 | L3 | P4 |
| SY-CONV-05 | create + create，**不同 id** | — | A、B 各建 1 条后双向同步 | 两条并存，条目数 +2；无冲突副本。INV-01 | L3 | P5 |
| SY-CONV-06 | create + create，**同一 id**（人为构造：A 的实体文件被 B 以同 id 不同内容 PUT） | `FAIL(corrupt-body)` 不适用 → 直接 `PUT records/n/<same-id>.json` 两份不同 rev/base | `A sync` → `B sync` | 两侧内容均保留（合并或冲突副本），hash 与 rev 决定唯一胜出者；**绝不出现只留一份、另一份消失**。INV-01,02,05 | L3 | P5 |

### 冲突类

| ID | 场景 / 前置 | 注入 | 操作 | 预期 + 不变式 | 层 | 阶段 |
| --- | --- | --- | --- | --- | --- | --- |
| SY-CONF-01 | update + update（同一段两侧不同改） | — | 双向同步 | 检测为冲突（base/rev 两侧均前进）；结果 = 原笔记 + 一条标题含"冲突副本"的笔记，两者正文各自完整可见；A、B 最终条目数一致。INV-01,03 | L1,L3,L5 | P5 |
| SY-CONF-02 | update + update（不同段，可块级合并） | — | 双向同步 | 自动合并成功、**不产生**冲突副本；合并结果含两侧段落。INV-01 | L3 | P5 |
| SY-CONF-03 | delete + delete（两侧各自删除同一实体） | — | 双向同步 | 实体活动列表消失、只产生一份 tombstone（`deleted_at` 非空且 `rev` 唯一胜出）；恢复时不出现"恢复出两份"。INV-02,11 | L3 | P5 |
| SY-CONF-04 | delete + update（A 删除，B 修改） | — | 双向同步 | 冲突可见：要么保留被修改内容 + 冲突副本，要么删除胜出但修改内容可在回收站/副本中读到；禁止"内容蒸发"。B 侧同步后条目数在 A、B 一致。INV-01,02 | L3,L5 | P5 |
| SY-CONF-05 | update + delete（同 SY-CONF-04 的镜像：先删后改） | — | 双向同步 | 同 SY-CONF-04 判据；两份历史均不消失。INV-01,02 | L3 | P5 |
| SY-CONF-06 | move + update（A 移动文件夹，B 改正文） | — | 双向同步 | 最终归属唯一（同一 id 只在一个文件夹下）、正文为 B 的新版本或合并版本；不出现同一 id 两条笔记。INV-01,09 | L3 | P5 |
| SY-CONF-07 | 段落级自动合并成功（A 改列表、B 改段落，base 相同） | — | 双向同步 | 合并后节点计数 = A 改动 + B 改动；`rev` +1 且非冲突路径（无"冲突副本"出现）。INV-01,03 | L1,L3 | P5 |
| SY-CONF-08 | 段落级自动合并失败（同一段两侧都改且非纯插入） | — | 双向同步 | 退回 keep-both：生成冲突副本；UI 有可点击的冲突提示；两份正文 hash 分别等于 A、B 版本。INV-01,03 | L1,L3,L5 | P5 |
| SY-CONF-09 | 冲突副本本身再次冲突（三方 base 分叉） | — | 连续 3 轮同步 | 每轮结果确定（同一输入 → 同一 canonical JSON 输出）；副本命名不重复覆盖（第二次产生可区分后缀）；条目数不无限增长（≤ 冲突数 + 1）。INV-01,05 | L3 | P5 |

### 故障注入类

| ID | 场景 | 注入 | 操作 | 预期 + 不变式 | 层 | 阶段 |
| --- | --- | --- | --- | --- | --- | --- |
| SY-FAULT-01 | offline → online | `OFF(close-listener)` 持续 3 轮，`ON` | `A sync` ×3 → `ON` → `A sync` | 离线期错误分类为"服务器不可达"（非"未授权"）；outbox 保留全部变更（计数不变）；恢复后一轮内远端追平，`DUMP` 与本地权威表条目集合一致。INV-06,08 | L3 | P5 |
| SY-FAULT-02 | online → offline（轮中途断网） | `FAIL(abort,times=1)` 于第 2 个 PUT | `A sync` | 本轮失败但远端只存在早于失败点的完整对象；`manifest/index.json` 未更新（`DUMP` 中 manifest hash 与轮前相同）；下轮 `A sync` 从 0 副作用继续成功。INV-09 | L3,L4 | P5 |
| SY-FAULT-03 | retry（可重试错误） | `FAIL(status=503,times=2)` | `A sync` | 观察到 2 次带退避的重试（`STATS` 时间戳间隔单调增大且有上限），第 3 次成功；用户侧无报错。INV-06,08 | L1,L3 | P5 |
| SY-FAULT-04 | retry 耗尽 | `FAIL(status=503,times=99)` | `A sync` | 在配置的尝试上限后停止（请求总数 = 上限，不无限循环）；状态置为"同步失败/待重试"可见；本地内容完好。INV-06 | L1,L3 | P5 |
| SY-FAULT-05 | timeout | `FAIL(hang,ms=30000)`，客户端预算 400 ms 级（PROBE:`timeout-and-cancel` 实测 414 ms 取消 400 ms 预算） | `A sync` | 轮在预算 + 少量抖动内被取消（非永久挂起）；连接资源释放（连续 10 次不耗尽句柄/内存，RSS 增长 ≤5%）；无重复提交。INV-06,07 | L1,L3 | P5 |
| SY-FAULT-06 | auth 401 | `FAIL(status=401,target=.notes/**)` | `A sync` | 分类为凭据失效并**可见提示重新登录**；不进入无限重试（PUT/GET 总请求数 ≤ 首试 + 极小常数）；outbox 完整保留；本地可继续编辑。INV-06 | L1,L3,L5 | P5 |
| SY-FAULT-07 | forbidden 403 | `FAIL(status=403,target=.notes/manifest/**)` | `A sync` | 分类为权限不足（区别于 401）；提示中含被拒路径前缀；无破坏性重试；`DUMP` 未变。INV-06 | L1,L3 | P5 |
| SY-FAULT-08 | quota 507 | `FAIL(status=507,target=attachments/**)` | `A sync` | 附件队列挂起并显示"空间不足"；**文本同步当轮仍然成功**（INV-04 之外的队列隔离）；文本实体 rev 在远端可见。INV-06,13 | L3,L5 | P6 |
| SY-FAULT-09 | partial upload（服务端已落盘但响应失败） | `FAIL(partial-write,target=.notes/records/n/x.json)` | `A sync` → `A sync` | 第 1 轮标失败；第 2 轮幂等重推同一 rev 后收敛，`DUMP` 中该对象内容 = 本地版本；manifest 仅在记录写成功后引用它；无孤儿被本地当作"已同步"。INV-09,12 | L3,L4 | P5 |
| SY-FAULT-10 | 412 precondition | `FAIL(status=412,target=.notes/manifest/index.json,times=1)`（或客户端持旧 ETag） | `A sync` | 引擎**重新规划本轮**而非报错退出（PROBE:`precondition-412-plumbing`：412 与 204 均作为状态返回）：先 GET 最新 manifest → 合并 → 重试；最终收敛且 rev 无空洞；无数据丢失。INV-05,09 | L1,L3 | P5 |
| SY-FAULT-11 | server unavailable（TLS 层） | 服务器换自签 CA / `OFF(进程 kill)` | `A sync` | 不受信 CA 时明确提示"证书不受信任"并要求确认（可导入自定义 CA），且**不静默降级为明文 HTTP**；恢复后可同步。INV-06 | L1,L5 | P4 |
| SY-FAULT-12 | 代理异常 | 配置指向死代理（PROBE:`proxy-config-actually-honored`） | `A sync` | 明确报"代理不可达"且遵循 `no_proxy`；错误分类不与 401 混淆。INV-06 | L1 | P4 |

### 完整性 / 孤儿类

| ID | 场景 | 注入 | 操作 | 预期 + 不变式 | 层 | 阶段 |
| --- | --- | --- | --- | --- | --- | --- |
| SY-INT-01 | corrupted manifest | `FAIL(corrupt-body,target=.notes/manifest/index.json)` | `A sync` | 本轮中止且**不改动本地权威表**（条目数与全部正文 hash 与轮前一致）；错误分类为"远端 manifest 损坏"可见；不"以空 manifest 继续"（否则等于清空本地）。INV-08 | L3,L4 | P5 |
| SY-INT-02 | manifest 引用了不存在的实体文件（missing entity file） | 直接 `DELETE records/n/<id>.json`（manifest 仍引用） | `A sync` | 该实体被标为 `missing_remote` 并可诊断；**MUST NOT 导致本地删除**（本地条目仍在，hash 不变）；本轮其余实体正常同步；不把缺失当作 tombstone 传播给 C。INV-03,08 | L3 | P5 |
| SY-INT-03 | entity file orphaned（远端有文件、manifest 不引用） | 直接 `PUT records/n/<new-id>.json` 而不更新 manifest | `A sync` → `B sync` | 双方均**不采纳**该孤儿实体（条目数不变）；`notera-cli diag` 能列出孤儿清单；不自动删除远端孤儿（保守）；不引发 rev 冲突。INV-09 | L3 | P5 |
| SY-INT-04 | 附件对象缺失但 manifest 引用 | 直接 `DELETE attachments/**` 中一个对象 | `B sync` | 文本正文正常显示；附件位显示"附件不可用"（不是空白、不是崩溃）；不阻塞其他附件；本地已有缓存时不重新下载失败即视为内容丢失。INV-08,13 | L3,L5 | P6 |
| SY-INT-05 | 校验失败的数据 | `FAIL(corrupt-body,target=records/**)`（200 + 篡改 ct，使 tag 校验失败） | `A sync` | 拒绝写入权威表（条目 hash 集合与轮前完全一致）；错误可见；下一轮成功拉取正确版本后条目更新。INV-08 | L1,L3 | P5 |
| SY-INT-06 | `.tmp-*` 残留 | `FAIL(abort,target=.notes/records/**)` 于 PUT 中途 | `DUMP` 检查 → `A sync` | 正式路径无残缺对象；`.tmp-*` 不计入有效状态；新一轮使用不同 tmp 名成功、不覆盖不冲突。INV-09 | L1,L3 | P5 |
| SY-INT-07 | 服务端重启（fs 模式） | `RESTART` | `A sync` → `B sync` | 收敛结果与重启前一致（ETag/manifest 均从磁盘恢复）；客户端不依赖服务端内存态、无"看起来同步了但实际丢失"。INV-05,09 | L1,L3 | P4 |
| SY-INT-08 | duplicate push（幂等） | 对同一实体连发 2 次 `A sync`（人为阻止本地 last-pushed 缓存 / 重复执行同 payload） | `A sync` ×2 | 第 2 次请求 `DUMP` 前后 hash 相同、服务端对象不变；客户端不把重复成功当作新 rev（本地 rev 不变）；无冲突副本产生。INV-07,12 | L1,L3 | P4 |
| SY-INT-09 | 大 manifest 分片 | 预置 5000 实体（manifest ≈438.7 KiB raw / **186 KiB gzip**，实测） | `B sync`（首次全量） | 分片 `manifest/seg-*.json` 全部拉齐；条目数 = 5000；`index.json` 引用的每个 seg 都存在（无部分应用状态）；本轮传输字节 ≤ 实测 gzip 之和 + 5%（回归容差）。INV-08,09 | L3 | P7 |
| SY-INT-09a | 压实与分段读回（SY-INT-09 的机制部分，规模缩到 260 条） | A 写 260 条（超过 `WINDOW_MAX=200`）并同步收敛 → 读服务器；B 空库入伙 | **已实现**（`notera-host/tests/compaction.rs`）：索引里出现分段引用、每个被引用的分段确实在盘上且 `bytes`/`count` 与实文件一致、窗口回落到 200 以内；B 靠分段基线追平到 260 条、标题+内容哈希逐条一致、FTS 行数==笔记数。**分段容量这一层已强制**（`compaction_respects_the_segment_target`：2500 条 → 多段、每段 ≤ `SEGMENT_TARGET`、条目不丢、新段必进 `touched`）；**5000 条的结构尺寸已实测**（压实后索引 711 B、分段 [2000,2000,1000]、`分段 ⊕ 窗口` 折回 5000 条不丢，断言在 `manifest.rs::index_stays_small_at_five_thousand_entities`，变异自证：不窗口清空 → 尺寸断言红）；**端到端真跑 5000 条：本机一次性实测过，但不是门禁**（A 推 5000 = 51 轮 / 79.5 s；空库 B 追平 = 26 轮 / 67 s，连跑不等待，debug 构建 + 内存内真 TCP 服务器）。一次约 150 s 会把整套预算吃掉一个数量级，所以没留在默认套件里；这条规模下暴露并修掉的真实缺陷是"压实触发条件只看重叠率 → 每次编辑重写整份基线分段"（下表原行的其余部分保持未验证）。变异自证：把引擎的压实接线拆掉 → 本条红；让 `compact()` 只重写已有分段 → `first_compaction_*` 单元测红 | L3 | P7 |
| SY-INT-10 | 落后设备超过变更窗口 | 预置 A 落后：远端已有 200+ 条变更（实测 200-entry 窗口 ≈16 KiB raw / 7.8 KiB gzip），A 的 last-sync rev 早于窗口起点 | `A sync` | 自动降级为"全量 manifest 拉取"（而非静默丢变更）；同步后 A 条目数与 B 完全一致；`notera-cli diag` 报告窗口溢出原因；无实体被跳过。INV-05,08 | L1,L3 | P7 |
| SY-INT-11 | 链路抖动（反复断连重连） | A 连着写 6 条，每轮换一种坏法轮着来：① 服务器整个停监听（拔网线）② 连接建了就被掐（`FAIL(abort)`）③ 握手能过但**读清单**回 500（`FAIL(status=500,target=.notes/manifest/*)`）；每轮之后同地址恢复 | 链路恢复后按调度器节奏继续跑 | **坏的那一轮绝不报成功**（只允许 Failed/Partial，或直接拒发）；坏轮之后**账必须还欠着**（`outbox_pending>=1`、`dirty_notes>=1`，界面那个"待同步"就来自这里）；断网期间刚写的笔记**本机立刻读得回来**（I8）；恢复后跑到本机结清（两个计数归零），B 拉两轮后与 A 的**标题+内容哈希逐条一致**（6 条，不多不少不重）；服务器上不留 `.tmp-*` 半截对象。证据：`notera-host/tests/reconnect.rs`。门禁自证：把"拉清单失败"改成"报 Converged" → 只有第③种坏法能让这条红（前两种在握手阶段就被拒，走不到引擎那一支）——**因此三种都得留着** | L3,L4 | P5 |
| SY-INT-12 | 干净设备入伙一个**变更数 > `WINDOW_MAX`(200)** 的库（实测 260 条） | A 写 260 条并同步收敛 → B（空库）入伙 | **已实现并通过**（`notera-host/tests/late_device.rs`）：清单先自证公告了 261 条（260 篇 + 1 个文件夹），B 必须完整收敛到 260 条、与 A 的**标题+内容哈希逐条一致**、FTS 行数 == 笔记数，且追平后 B 再写一条要能双向传到 A。曾经的缺陷与判据：`local_views()` 只报脏行 → 追平的设备被当成"本地 0 条" → 每轮把 261 条全判成 P2 重下、200 次请求预算被重复劳动吃光、缺的 63 条永远排不到；`seq_applied` 又曾有两个写点（host 的 `StoreManifest` 顺手落 + 引擎收尾无条件落），配合 304 快路径就产生"账上干净、库里少 63 条、徽标说已同步"的谎报。修法：`Store::synced_heads` → `LocalPort::synced_heads`（默认空实现）→ 引擎拉之前先问"这一版本机有了吗"，rev+哈希都对上就不发这个请求；落账收成引擎唯一写点且"被预算截断就不落"。变异自证：拆掉跳过 → 停在 197/260 红 | L3 | P5 |
| SY-INT-13 | 一台还在追平的设备（本轮被 `round_request_cap` 截断） | A 推 260 条收敛 → B 空库入伙，看它**第一版之后**与**追平之后**的同步状态 | 截断的那一版之后，`sync_status.badge` 必须是 `syncing`（不许 `synced`，也不许刷 `lastSuccessAt`）；纯拉侧的 `pendingOps` 天生是 0，所以"待同步数量"这条**不足以**说明问题，判据只能看徽标；追平之后回到 `synced`。附带修掉调度器的同类问题：`Partial` 不再等 25 秒下一拍而是 1 秒后续跑（5000 条 26 轮：按实测轮数与 67 s 传输折算约 96 s，而不是 ~11 分钟）。变异自证：拿掉 `Partial → Syncing` 分支 → 断言立刻红。**续跑的判据也钉住**：`should_drain` = `Partial` 且 `pushed+pulled>0` —— 没有进展的 `Partial`（404 / 请求出错）退回 25 秒节拍，不许 1 秒无界热转圈；六种结果由 `drain_only_follows_real_progress` 覆盖，把"看进展"半个条件拆掉即红 | L2,L3 | P5 |
| SY-INT-14 | **千条规模的库**换设备追平（体量而非边界；260 条那两条钉的是"跨过窗口上限"） | A 写 1000 条并公告 → B（空库）入伙追平 → 双方再各跑一轮 → B 只加 1 条 | 已实现并通过（`notera-host/tests/big_library.rs`，实测 12.8 s）：① 公告与追平的**轮数都有界**（≤16，`round_request_cap=200` 算错的话可以一直 `Partial` 转圈）；② 索引只装引用（≤8 KiB；窗口 ≤`WINDOW_MAX`；分段 `count` == 盘上真实条目数；清单公告总数 == 笔记数 + 1）；③ 截断那一轮之后徽标 `syncing`、追平后 `synced`；④ **逐条 (标题, 内容哈希) 一致，且比对必须读满**（`limit: 0` 在 store 里是"默认 500 条"，两台各 1001 条时会各回前 500 行而"相等" —— 本轮实测踩过，故三个测试都显式传 `limit`）；⑤ 追平后的**空轮 = 1 次请求 + 清单 304 + 上下行 0 字节**（PERF-05 的量级版：12 条时索引才 506 B，`sync_cost` 的 ≤2 KiB 咬不住）；⑥ 一条增量不重写基线分段。第一次跑就抓到**默认本每台设备各造一个 id**：两台各造一条 → 互相拉回 → 侧栏两个「默认本」、清单从 1001 涨到 1002（变异自证：把 id 换回 `EntityId::new()` → "A 上有 2 个文件夹" 立刻红）。注：界面不受 500 那条默认值影响，`stores/notes.ts` 一直是 `limit: 200 + offset` 翻页。**这一条刻意不钉**的：追平过程中基线分段每轮整份重下（实测 5 轮，末版单份 86 KiB）—— 它要等"远端视图可持久化"才修得动，原因/实测/解除条件见 CHANGELOG 已知限制 与 `patches/README.txt`；不把现状写死成期望值；⑦ **2026-09-27 补上并改成钉死**：追平期间每个基线分段**最多整份下载一次**（按 `GET seg-*.json` 且 200 逐名计数）—— 接上"远端视图可持久化 + 分段内容哈希缓存"之后，实测从每轮一次（5 轮 5 次）降到 1 次，且 1000/1000 照样追平。变异自证两次：把分段缓存的写入摘掉 → "seg-0000.json 被整份下载了 6 次"；把视图落盘的条件写成 `!capped`（粗了：预算常花在逐条正文上，那时索引投影是完整的）或让写侧按 `"note"` 而不是线上标签 `"n"` 匹配（整表被"未知类型"静默跳过）→ 都红在"第二台设备停在 198/1000" | L3 | P5 |

### 身份 / 时钟 / 顺序类

| ID | 场景 | 注入 | 操作 | 预期 + 不变式 | 层 | 阶段 |
| --- | --- | --- | --- | --- | --- | --- |
| SY-ID-01 | clock skew：本机时间比真实时间快/慢 30 分钟与 24 小时 | fake clock 偏移 | `A sync` ×2 → `B sync` | `updated_at` 记录为偏移值但不参与胜出判定（胜出仍由 `rev`/`base`/`hash` 决定）；**不发生内容丢失**；固定/排序不倒退到错误位置造成"笔记消失"；同偏移下重复同步结果一致。INV-01,05 | L1 | P5 |
| SY-ID-02 | 跨设备 id 冲突 | 两台设备并发创建 100 条 | 各 `sync` 后合并 | 200 条全部存在、id 全局唯一（UUIDv7 路径安全且单调，PROBE:`uuidv7-monotonic-pathsafe`）；无覆盖、无重名导致的合并。INV-01 | L1,L3 | P5 |
| SY-ID-03 | 新设备重新登录同一账号（含换密码/换设备名） | `RESET` 不适用：保留远端，清空本地目录 | C 配置同一账号 → `C sync` | 首次为全量拉取：条目数与 A 一致；不向远端写任何新对象（`DUMP` 前后一致，除 `device` 字段外无变化）；不产生冲突副本；C 的本地 rev 起点 = 远端 rev。INV-05,09 | L3 | P5 |
| SY-ID-04 | 同账号但 `device` 标识改变 | 修改 device 字段 | `A sync` | 仅 `device` 变化不产生新 rev；`hash` 不因 device 字段变化而改变胜出判定（若协议规定 hash 覆盖 payload 而非信封，L2 golden 锁定该结论）。INV-07,12 | L1,L2 | P5 |
| SY-ID-05 | 两个客户端指向不同 root prefix | 配置 B 到 `.notes2/` | `A sync` → `B sync` | 互不影响：`DUMP prefix=.notes/` 与 `prefix=.notes2/` 各自自洽；不发生跨 prefix 串写；UI 中切换 prefix 后条目数按该 prefix 状态变化。INV-09 | L3 | P4 |
| SY-ID-06 | `protocol.json` 缺失 / 首次初始化 | `RESET` 后 `DUMP` 为空 | `A sync`（首次） | 创建 `.notes/protocol.json` + `manifest/index.json`；重复首次（两个客户端同时 init）结果确定且只有一份有效 manifest 胜出，无 `.tmp` 覆盖正式对象。INV-09,12 | L1,L3 | P4 |

## 不变式（Invariants）

每条不变式必须被 §同步测试矩阵、§崩溃点矩阵、§多设备场景中**至少一行**断言，并且各配一条 property-based（quickcheck 风格）测试。表内"属性测试"列给出落点 crate 与建议测试名。

| # | 不变式 | 可观测判据 | 属性测试（crate::测试名） |
| --- | --- | --- | --- |
| INV-01 | 用户输入的内容永不静默消失：任何同步轮之后，任一同步前存在的实体正文要么仍在权威表中，要么以冲突副本形式可见 | 对比轮前后"正文字符多重集"：每个轮前正文要么是某轮后正文的子序列，要么有可区分的冲突副本 | `notera-sync::tests::prop_content_never_silently_lost`（生成随机 A/B 编辑序列） |
| INV-02 | 删除不会因同步而复活：本地或远端已提交的删除，在任何后续轮次中不得使该实体重新出现在活动列表 | 每轮后重放随机操作序列（含删除），断言"活动集合"中无已删 id 重现（除非用户显式恢复） | `notera-sync::tests::prop_deletes_are_not_resurrected` |
| INV-03 | 远端缺失不导致本地删除：manifest 未列出或实体文件缺失（`missing_remote`）不产生本地删除；仅显式 tombstone（`deleted_at`）才删本地 | 随机生成"远端裁剪"的 manifest，断言本地删除集合 ⊆ tombstone 集合 | `notera-sync::tests::prop_missing_remote_never_deletes_local` |
| INV-04 | 崩溃后 `sync_operations.inflight` 归位且数据一致：进程被 kill 后重启，不残留 inflight 标记，且权威表处于某个已提交状态 | 每次崩溃-重启后断言 `inflight = 0` 且权威表 hash 等于某个历史合法快照（非新旧混合） | `notera-store::tests::prop_inflight_recovers_to_committed_snapshot` |
| INV-05 | 已提交 manifest 之后远端状态单调不回退：manifest 的 rev 序列 / seg 集合一旦提交不被更旧内容覆盖 | 随机交错多轮提交，断言已提交 manifest 的 (最大 rev, 条目数) 单调不减；旧 ETag 写入被 412 拒绝 | `notera-webdav::tests::prop_manifest_monotonic_after_commit` |
| INV-06 | 本地写入不依赖网络：任何本地编辑/新建/删除/恢复在无网络下都成功落盘，且不产生"待网络"的错误态 | 服务器 `OFF` 时随机执行 100 次本地写，断言全部成功、`STATS` 请求数 0、重启后可读 | `notera-store::tests::prop_local_writes_work_without_network` |
| INV-07 | 同一 `rev` 重复上传结果相同（幂等）：同一 (id, rev, payload, hash) 的 PUT 序列，任意重复不改变远端最终状态 | 随机重放 PUT 序列（含 0/1/N 次重复），断言 `DUMP` 最终状态相同（canonical 比较） | `notera-sync::tests::prop_republish_same_rev_is_idempotent` |
| INV-08 | 校验失败的数据永不写入本地权威表：schema/hash/AEAD/tag 任一校验失败 → 权威表字节级不变 | 随机注入坏 fixture（截断、hash 不符、未知 kind、tag 篡改），断言轮后权威表 hash 不变 | `notera-store::tests::prop_invalid_payloads_never_reach_authoritative_tables` |
| INV-09 | manifest 是唯一提交点：不存在"manifest 引用了尚未写入的记录/附件"的可观测窗口 | 从任意时刻 `DUMP` 快照构造，断言 manifest 引用集合 ⊆ 已存在对象集合（含崩溃点取样） | `notera-sync::tests::prop_manifest_reference_closure_holds` |
| INV-10 | 未知内容不被销毁：未知节点类型/属性/未知 envelope 顶层字段在任意 打开→编辑→保存 往返中原样保留 | 随机向 fixture 注入未知 type/attr/字段，往返后断言其在 canonical JSON 中 byte 级存在 | `notera-richtext::tests::prop_unknown_nodes_and_attrs_survive_roundtrip` |
| INV-11 | tombstone 永不自动 GC：任何轮次、任何保留策略下，已提交 tombstone 不被客户端静默移除 | 生成含 N 个 tombstone 的 manifest，随机同步 M 轮后断言 tombstone 计数 ≥ N（仅显式 purge 才变） | `notera-store::tests::prop_tombstones_never_auto_gc` |
| INV-12 | 加密信封开销与完整性精确可预测：`ct` 长度 = `payload` 长度 + **恰好 16 字节**；nonce 12 字节；篡改必被拒 | 随机 payload 长度 0..1 MiB 往返：长度等式成立；翻转任意 1 位 → 解密返回 `aead::Error`（PROBE:`envelope-aes-256-gcm-siv`、`envelope-tamper-detected`） | `notera-crypto::tests::prop_envelope_length_and_tamper_rejection` |
| INV-13 | 附件队列永不阻塞文本队列：附件在飞时文本轮照常完成 | 随机交错 K 个大附件 + T 个文本变更，断言每个文本变更在 ≤ 设计周期 25 s 内到达对端 | `notera-sync::tests::prop_text_sync_not_blocked_by_attachments` |
| INV-14 | 冲突判定唯一由 `(base, rev, hash)` 三元组决定，与 `updated_at`/`device` 无关 | 随机制造时钟偏移与设备名变化，断言冲突集合不变（结果只依赖分叉结构） | `notera-sync::tests::prop_conflict_detection_independent_of_clock_and_device` |
| INV-15 | 段落级合并结果对三方输入是确定的（同一三元组输入 → 同一 canonical JSON 输出） | 同一输入重复执行 50 次 + 输入排列不变性检查（A/B 交换后结果按规则对称） | `notera-richtext::tests::prop_merge_is_deterministic` |

## 多设备场景

设备定义：A = Windows x64 桌面（Tauri App），B = macOS arm64（或第二桌面实例），C = Android arm64-v8a（或第三桌面实例）。L3 用三个 `notera-cli sync-once` 进程模拟（隔离数据目录 + 各自 device id），L5 用真机复核关键场景。

| ID | 场景 | 断言（客观） | 层 | 阶段 |
| --- | --- | --- | --- | --- |
| MD-01 | 三端并发新建 50 条（A/B/C 各 50，离线各自完成）→ 依次上线同步 | 最终每台条目数 = 150 + 冲突副本数；无 id 冲突；三方最终 manifest hash 相同；INV-01,02,09 | L3 | P5 |
| MD-02 | A 编辑 → B 删除同一条 → C 未参与 → 三方同步 | delete+update 策略结果在三方一致（条目数、正文 hash 集合相同）；不出现"C 仍显示而 A/B 已删"的长期分叉（≤2 轮收敛） | L3 | P5 |
| MD-03 | A 与 B 同时改同一段，C 离线 3 天后上线 | C 上线后一次拉取即达到与 A/B 相同的最终状态（含冲突副本）；C 的 outbox 空；INV-05 | L3 | P5 |
| MD-04 | 附件：A 插图、B 插同名不同内容图（sha256 不同） | 两个 `attachments/<2hex>/<sha256>` 并存不覆盖（内容寻址、immutable）；三方渲染一致；INV-09 | L3 | P6 |
| MD-05 | 落后设备超过变更窗口：C 落后 300 条变更后上线（200 条窗口实测 ≈16 KiB raw / 7.8 KiB gzip） | C 走全量拉取路径；最终条目数与 A/B 一致；INV-05,08 | L3 | P7 |
| MD-06 | 服务器中途重启（fs 模式）+ 一轮网络闪断 | 三方最终收敛与 MD-01 相同；无实体丢失；`DUMP` 中无 manifest 回退；INV-05,09 | L3,L4 | P5 |

### 必测主场景（A 创建 → B 同步 → C 同步 → A 修改 → B 修改 → C 离线 → 恢复网络 → 三方收敛）

| 步 | 操作 | 期望可观测状态 | 检查手段 |
| --- | --- | --- | --- |
| 1 | A：新建笔记 N，正文 `P1 P2 P3`（三段），同步 | 远端存在 `records/n/<idN>.json`，`rev=1`、`hash` 与 canonical 正文一致；manifest 早于该文件的引用顺序正确 | `DUMP` + `STATS` |
| 2 | B：同步 | B 列表出现 N，正文三段齐全，`rev=1` | L3 CLI 报告 + L5 UI 计数 |
| 3 | C：同步 | C 与 B 完全一致（同 `rev`、同正文 hash） | L3 |
| 4 | A：修改 `P2→P2'`，同步 | 远端 `rev=2`、`base=1`；B/C 仍为 `rev=1`（本地不被 A 的推送改写） | `DUMP` |
| 5 | B：修改 `P2→P2''`（基于 `rev=1`），同步 | B 的 push 遇分叉（`base=1` vs 远端 `rev=2`）→ 段落级三方合并：同一段两侧均改 ⇒ 冲突 ⇒ keep-both，生成"N 冲突副本"；远端 `rev=3` | `DUMP` + `STATS` |
| 6 | C：进入离线（`OFF` 或拔网），并在本地改 `P3→P3'''` | 本地写入成功、无网络错误、`rev` 本地 +1、outbox=1 | L3/L5（`STATS` 请求数 0，INV-06） |
| 7 | 恢复网络（`ON`）→ 依次 `A sync`、`B sync`、`C sync`、`A sync`、`B sync`、`C sync`（两轮三方） | 见步 8 | — |
| 8 | **断言三方最终收敛且不丢内容** | (a) A、B、C 的"活动实体 id 集合"完全相同；(b) 三方 `manifest/index.json` 的 ETag/hash 相同；(c) `P3'''` 在**三方**都可见（C 的修改未被覆盖）；(d) `P2'` 与 `P2''` 均存在——在同一正文中合并，或分别位于 N 与其冲突副本中；(e) 冲突副本总数 ≤ 冲突数（步 5、步 7 各至多 1），即 ≤2；(f) 无"空笔记"垃圾条目产生；(g) 所有 `rev` 连续无空洞 | L3（`DUMP` + canonical 对比）+ L5（UI 逐台核对） |
| 9 | 再跑一轮三方 `sync`（no-op 轮） | 每轮请求数 ≤2、PUT 数 0、manifest 响应 304；条目数不变（收敛是**稳定不动点**，非"看起来一致"） | `STATS` |

## 崩溃点矩阵

统一注入：`CRASH(<点>)` = `NOTERA_CRASH_AT=<点>`（仅 test feature 编译，release 构建必须忽略该变量）+ 外部 `taskkill /F` 双路径验证。每个点的通用重启后断言 = **INV-04 + INV-09 + INV-01**。

| ID | 提交点 | 注入方式 | 重启后预期（客观） | 覆盖测试 | 阶段 |
| --- | --- | --- | --- | --- | --- |
| CP-01 | 写 `.tmp-*` 之前 | `CRASH(before-tmp-write)` | `DUMP` 中无新对象、无新 `.tmp-*`；manifest ETag 不变；本地变更仍在 outbox；下轮从头完整推送 | L4:`crash_before_tmp_write` | P4 |
| CP-02 | `.tmp-*` 已写、MOVE 之前 | `CRASH(after-tmp-before-move)` | 允许存在 `.tmp-*` 残留；**正式路径无残缺对象**；新一轮使用不同 tmp 名后成功；manifest 未引用未完成对象（INV-09） | L4:`crash_after_tmp_before_move` | P4 |
| CP-03 | 记录 MOVE 成功、manifest 未提交 | `CRASH(after-record-move)` | 远端存在"孤儿对象"，本地视该实体为未同步（rev 不算已确认）；后续轮补齐 manifest 引用后收敛；`B sync` 在补齐前**不采纳**该对象（条目数不变） | L4:`crash_after_record_move_before_manifest` | P4 |
| CP-04 | manifest 写入中途（index.json / seg-* 部分落盘） | `CRASH(during-manifest-write)` + `FAIL(abort,target=.notes/manifest/**)` 双路径 | 服务器要么旧 manifest 完整、要么新 manifest 完整（禁止半 JSON）；若为 tmp+MOVE 实现则旧内容原样保留；客户端读到损坏 manifest 时中止且不改本地（SY-INT-01，INV-08） | L4:`crash_during_manifest_write` | P5 |
| CP-05 | manifest 已提交、本地 ack 之前 | `CRASH(after-manifest-commit-before-local-ack)` | 重启后本地 ack 补做（outbox 项被清或是幂等重推）；对端同步后内容 = 已提交版本；本地 `rev` 不回退、不重复递增；无冲突副本产生（远端胜出者即本地写入者） | L4:`crash_after_commit_before_ack` | P5 |
| CP-06 | 附件上传中途 | `CRASH(during-attachment-upload)` | 正式路径无残缺附件；文本轮不受影响（INV-13）；重启后附件重试成功且 sha256 一致；引用该附件的 manifest 未提前出现 | L4:`crash_during_attachment_upload` | P6 |
| CP-07 | 本地应用事务中途（写权威表/FTS 之间） | `CRASH(during-local-apply)` | SQLite 无损坏（`PRAGMA integrity_check` = ok，辅助证据）；FTS 与 content 表条目数一致（否则视为索引脱钩）；重启后本地内容等于某个已提交快照（不出现"半个实体"）；tombstone 与实体状态一致 | L4:`crash_during_local_apply`（PROBE:`wal-tx-atomicity`） | P4 |
| CP-08 | migration 中途 | `CRASH(during-migration, version=k)`，对每个迁移版本 k 各一次 | 要么旧 schema + 旧数据完整可用、要么新 schema + 全部数据 + `user_version=k`（PROBE:`migration-user_version`）；不存在"表已改列、数据未搬"的中间态；重启可自动完成或明确报错并可回滚到备份 | L4:`crash_during_migration_vK`（k 遍历全部迁移） | P1 |
| CP-09 | 导入/恢复中途 | `CRASH(during-import)` / `CRASH(during-restore)` | 原库条目数与全部正文 hash 不变（INV-08）；导入结果要么全部生效要么全部不生效（或有明确的"部分成功"报告且可重放）；备份文件本身不被消耗/破坏，可再次恢复 | L4:`crash_during_import` | P6 |
| CP-10 | 首次初始化（`protocol.json`/空 manifest 创建）中途 | `CRASH(during-init)` | 重启后 init 幂等完成；不产生两份并存的 manifest；`RESET` 之前状态可判定 | L4:`crash_during_init` | P4 |

## 性能与容量预算

`状态` 列：**已实测** = 有 `docs/evidence/probe-windows-gnu.txt` 数字支撑；**待建基线** = 本文件提出的暂定阈值，首次测量后固化（固化前不得作为放行依据，但也不得因未固化而删除该行）。

| ID | 预算 | 阈值 | 测量点 / 方法 | 状态 |
| --- | --- | --- | --- | --- |
| PERF-01 | 冷启动到首帧可用（空库 / 5000 条 / 20000 条） | **已定口径（2026-09-27）：≤1000 ms / ≤1500 ms** —— 按“用户双击图标到看见内容”，**含** WebView2 启动那一段；判据取 REPS 遍里最好的一遍 | L5：从进程创建到列表首帧（Playwright 首条列表项可见 + 截图时间戳）；Windows x64 release | **已实测并按新口径转绿（重编 release 壳 + 现编前端产物，3 遍）**：空库最好 **866 ms**（三遍 1528 / 875 / 866），5000 条最好 **984 ms**（1276 / 984 / 1024）；20000 条那一档 1322 ms。**首遍明显慢（1528 ms）不是噪声而是会被感知到的事实**：新启动的进程要过杀毒与文件系统缓存，所以这里同时记最好与最差，不拿最好值冒充日常体验。纯应用侧（把 CDP 可接那段 595~615 ms 扣掉）约 250~400 ms；0 → 20000 条只让首帧多 ~400 ms（列表 200 条一页 + 虚拟化，符合设计）<br>**2026-09-28 在新编的 0.0.21 release 壳上重测：空库那一档今天没有达到 ≤1000 ms，按实记为未达成（§48 新增缺口 G13），不擅自改口径。**同一台机器、同一会话里的对照：新壳（`target/x86_64-pc-windows-gnu/release`，tauri build 出来的那个）best-of-3 = **1134 / 1184 ms**，上一批那个旧 release 壳 best-of-3 = **1090 ms** ⇒ 差异 44~94 ms，落在跑与跑的散布之内，**不能把这点差归到我这几批代码上**；但**历史上那个 866 ms 今天复现不出来**，所以"已达成"这句话不能再挂着。5000 条那一档 best = **1088~1102 ms**（预算 ≤1500 ✓），滚动 p95 17 ms ✓，RSS 31.5 起步 / 5000 条 46.3~46.6 / 滚完 47.0~47.4 MiB ✓ —— 也就是**只有空库那一格从"达标"退回"未达标"**。<br>两条方法论的账也要记：① **一个全新数据目录的第一次启动是 6.2~8.1 s**（本次四组里每一组的首遍都是这个量级），而 `REPS` 取最好的一遍会把它藏起来 —— 那恰恰是新装用户的**第一次**体验，所以这一档从今往后同时记"首遍/最好"，不只看最好值；② 我一开始复用了上一批的 `RUN_TAG=cur`，那条 lane 的"空库"目录里其实躺着 27 号那批留下的 342 KB WAL 与旧行，量出来的 4695 ms 是**夹具的账不是产品的账** —— 换 `RUN_TAG` 用全新目录才作数（已写进 lane 的复现约定）。 |
| PERF-02 | 搜索 p50 / p95（≥3 字 CJK，5000 条） | p50 ≤60 µs、p95 ≤2000 µs | L1 直测；实测基线 **p50=32 µs / p95=1404 µs**，FTS 索引 **6.0 MiB**（PROBE:`fts5-search-5000-notes`） | 已实测（阈值含回归容差）。**另有一条端到端口径（2026-09-27，20000 条真库）**：走 dev 桥的 `search` 命令、10 次取最好，2 字 CJK（`LIKE` 兜底那条路）44 ms、1 字 47 ms、6 字短语 84 ms（返回都被 `limit: 80` 截住）、`list_notes` 首页 200 条 45 ms、`stats` 195 ms。这两个数**不能混着引用**：µs 那组是 SQL 查询本体，ms 这组是"进程外一发请求"的墙上时间（含玩具 HTTP 服务器、JSON 序列化、片段渲染），后者才是用户手感的上界，而它离"卡"还远 |
| PERF-03 | 1–2 字 CJK 查询延迟 | 暂定 ≤15 ms @5000 条 | L1；实测 content 表 `LIKE` **416/5000 → ~4.1 ms**（FTS 列 `LIKE` 5.8 ms，**因此兜底必须走 content 表**） | 已实测 |
| PERF-04 | FTS 索引体积（5000 条 CJK，trigram） | ≤6.5 MiB | L1 `PRAGMA`/文件大小；实测 6.0 MiB | 已实测 |
| PERF-05 | no-change 轮：请求数 / 字节 | ≤2 请求、总传输 ≤2 KiB、manifest 返回 304 | L3 `STATS`；PROBE:`precondition-412-plumbing` 证明 304/412 可作为状态被引擎重规划 | 待建基线（机制已实测）  **已实测**（`notera-host/tests/sync_cost.rs`：两台设备互相收敛之后，空轮的断言是上界 —— 请求 ≤2、总传输 ≤2 KiB、`index.json` 必为 304、PROPFIND ≤1；纯拉侧曾因不记 etag 而每轮整份重下，已由 `sync_cost` 与修复一并钉住） |
| PERF-06 | 单轮变更同步字节（200 条变更窗口） | ≤8 KiB gzip | L3 `STATS`；实测 **16 KiB raw / 7.8 KiB gzip** | 已实测 |
| PERF-07 | 全量 manifest（5000 条 / 20000 条） | 5000：≤190 KiB gzip；20000：暂定 ≤800 KiB gzip | L3 `STATS`；实测 **89 B/条**、5000 条 **438.7 KiB raw / 186 KiB gzip**、2000 条 seg **175.6 KiB raw / 74.7 KiB gzip**（→ 按 seg 分片外推 20000） | 5000 已实测 / 20000 待建基线 |
| PERF-08 | 变更在设备间传播（桌面） | ≤30 s（设计周期 25 s） | L3 端到端时间戳；L5 真机复核 | 已定阈值，待实测 |
| PERF-09 | argon2id（m=19 MiB / t=2 / p=1） | 单次 ~370–400 ms，且 **UI 线程零占用** | L1：主线程在 KDF 期间每帧阻塞 ≤16 ms（PROBE:`kdf-argon2id` = 19MiB/2it/1p → 400 ms，deterministic=true） | 已实测 |
| PERF-10 | 内存上限（桌面，20000 条 + 100 附件索引） | 暂定 RSS ≤450 MiB；单轮同步峰值增量 ≤50 MiB。**这一档还欠一个量：附件轮开工前的磁盘体检**（`attachment_repair_candidates` 一次查询 + 每条候选一次 `stat`）在 100 / 1000 附件下的耗时与是否可察觉 —— 2026-09-27 加自愈时只写了成本推理（每候选一次 stat，哈希只在长度不符时才算），**没有实测**，按 §40 记在此处。**2026-09-28 进展**：那一格每轮的量现在**有上界**了（候选 `LIMIT` + 一次批量写事务，§48 G4 已解除，门禁 FT-ATT-21/21s），而"有界"那一半**当天晚些时候也量出来了**（`--test attachment_gc_scale::disk_sweep_per_round_cost_at_100_and_1000_attachment_library`，`#[ignore]`，debug 构建，独占机器）：**100 份 × 64 KiB** ⇒ 快路一轮 11 ms（最坏 13）、慢路一轮 315 ms、回填后紧接着的下一轮又回到 11 ms、爆发一轮（100 条降级 = 一次写事务）12 ms、全库 1 轮扫完；**1000 份 × 1 KiB** ⇒ 快路一轮 24 ms（最坏 29）、慢路一轮 323 ms、回填后 23 ms、爆发一轮（200 条）25 ms、全库 5 轮扫完。**可察觉吗**：常驻循环每 20 s 一轮，快路占的是 24/20000 = **0.12%**，量级上是"没有"。顺带钉出来两条口径：① **每轮上界 200 意味着全库覆盖时间是 `ceil(N/200)` 轮** —— 一千份附件的机器被删空之后，最后一张图要到 **100 s** 之后才被记成 missing 并进下载队列（这是产品口径，不是缺陷，但台账以前没写过）；② **慢路那一轮的代价大头不是读字节也不是哈希**（64 KiB×100 与 1 KiB×200 两档都≈320 ms，IO 差了 30 倍而耗时一样），而是**每条候选一次写事务**（夹具里同形状的数：100 次提交 106 ms、1000 次 1151 ms ⇒ ~1.1 ms/次）—— 也就是说 G4 把**降级**批量了，**尺寸回填**还是逐行提交，同一类成本漏在那一半。已按 §40 记成待办（回填批量成一次写事务），判据沿用 FT-ATT-21s 的形状 | L5 采样（30 min 使用 + 5 轮同步），增长趋势 ≤5%/轮以排除泄漏 | **规模侧已实测**（`scripts/verify-perf.mjs` 用系统 `Get-Process.WorkingSet64` 读，不是前端自报）：空库 31.7~32.3 MiB、**20000 条 69.1 MiB**、滚完 807 行后 70.1 MiB（**+1.0 MiB**，没有可见的滚动/分页泄漏）；5000 条那一档 46.2 → 46.7 MiB。**未采**：① ~~100 个附件的索引规模~~ **采了**（2026-09-28 深夜，`ATTACH=100 NOTES=20000 node scripts/verify-perf.mjs` —— 附件是经产品那条 `attach_file` 挂上去的，夹具收尾用 `stats` 核对过 `attachments=100`，不然"100 附件那一档"会悄悄退成"只有笔记"）：**20000 条 + 100 份 200 KiB 附件索引 ⇒ RSS 起步 69.7 MiB**（上一批不带附件那档是 69.1 MiB，但那是 **0.0.21 的壳** —— 跨壳比差值只能当参考，本批能同场对比的是这次跑内部的 69.7 → 69.9），滚完 323 行之后 **69.9 MiB**（+0.2 MiB，没有可见的滚动泄漏），该档冷启动 1399 ms（预算 ≤1500 ✓）。上限那句到此**两半都有数**（69.7 ≪ 450）。<br>**② 30 min × 5 轮的趋势线已经量了**（2026-09-28 深夜，`node scripts/measure-leak-trend.mjs`，30 分钟、59 个 RSS 点按 PID 外部采集，Rust 侧 `--test leak_trend` 跑的是**生产那两条常驻循环**：文本 25 s / 附件 20 s，每 30 s 一轮"新建 → 读回 → 改 → 列表 → 搜索"）：工作集 **23.8 → 25.4 MiB**，五个窗口的均值 24.5 / 24.9 / 25.2 / 25.5 / 26.0 ⇒ **末窗减首窗 1.5 MiB**，最小二乘斜率 **0.063 MiB/min**，判据（≤8 MiB）**PASS**；账上同期 笔记 221 → 280、待发操作 241 → 0（归零 = 循环在追平而不是攒着）、库字节 753664 → 1048576。**两形对着看才敢下结论**（这是补跑第二形的理由）：`NOTERA_LEAK_IDLE=1` 那一跑**不做任何用户动作** —— 笔记数 60 拍都钉在 220、库字节只涨 49 KB（附件把 20 份字节传上去那部分），五窗均值 24.1 / 24.4 / 24.5 / 24.5 / 24.5 ⇒ **末窗减首窗 0.4 MiB、全程斜率 0.015 MiB/min、末 20 点斜率 0（走平）**。合起来能说的那句：**这两条常驻循环本身在 30 分钟（约 90 轮附件轮 + 72 轮文本轮）里没有"每轮留一点"的迹象**；上面那一形那 1.5 MiB 与内容增长（+59 条笔记 / +295 KB）同时发生，末段不降是因为库一直在长。**仍然不写成"证明不会泄漏"**：a) 长跑只有 30 分钟这一档；b) 判据阈值（8 MiB / 30 min）是**我今天定的暂定值**，不是用户拍过的口径；c) 量具自己的红也验过 —— `--self-test` 拿合成序列跑：平的该绿（实际 0 MiB）、每分钟漏 1.5 MiB 的该红（实际 36 MiB）、样本不够的该红；"收没收敛"那一条也两向能红（预热后走平的序列末段斜率 0 显著小于全程 0.132，一直线性涨的末段 0.06 与全程同量级）。<br>**仍未覆盖**：WebView2 那一块（要按 `verify-perf` 的壳口径量）与真实 WebDAV 服务器的行为差异（G7/G8）。上限那半句算已过（69 MiB ≪ 450）。<br>**2026-09-28 补：体检与 GC 那一格的每轮代价已经量了**（`--test attachment_gc_scale`，`#[ignore]`，debug 构建，2000 份 1 KiB blob / 1500 条笔记被永久删除）：**空转一轮 1 ms**（回收 + 销毁都无事可做 —— 这才是常驻循环每 20 s 真付的钱）；体检候选查询 9 ms、GC 候选查询 15 ms（cap=200）；**爆发那一形**：一轮 200 次挪进隔离区 = 1160 ms 平均每轮、8 轮把 1500 条搬完，销毁侧 8 轮共 4141 ms（平均每轮 517 ms）；搬完与销完之后各查一遍，仍被引用的 500 份字节一份不少。<br>三点解读要照实写：① 数字是 **debug 构建**，release 只会更快，所以这是上界不是期望值；② 那 1.2 s 几乎全是**文件系统 rename**（元数据操作，与文件大小基本无关），而且发生在**写事务之外** —— 它不占写锁，用户那次保存不会排在它后面，这正是 G4 当初要的形状；③ 它是**同步阻塞**调用，占的是附件轮那个任务所在的 tokio worker 约 1 s —— 这一类阻塞在这一格里**本来就有**（上传分支一次 `std::fs::read` 一个 ≤32 MiB 的 blob 比它贵得多），所以 GC 没有引入新的类别，但也没有把它消掉：**这条 measured 的结论是"爆发可容忍、空转可忽略"，不是"附件轮里没有同步 IO"**。仍然没量的：真实大文件（几 MiB 级）下的 rename/remove 代价。<br>**2026-09-28 深夜追记（0.0.30）**：体检这一档量完之后立刻掉出一处真缺陷 —— **慢路那一轮的钱花在逐行写提交上，不在哈希上**（100 份 × 64 KiB 与 1000 份 × 1 KiB 的慢路都是 ~320 ms，而 IO 差 30 倍；夹具里同样条数的写提交 ≈1.1 ms/次）。G4 当年批量了降级、漏了尺寸回填，这一格改成一次写事务（`set_attachment_sizes`）之后同一段量具重跑：**1000 份那一档 323 ms → 67 ms**，**100 份 × 64 KiB 那一档 315 ms → 199 ms**（剩下的是真读 6.25 MiB + 算哈希的部分，符合预期）；快路两档不动（11 ms / 24 ms）。判据 FT-ATT-40（存储侧批量语义）+ FT-ATT-20（回填发生在"这一轮一条都没降级"时，钉的就是那个早退顺序），变异 **M63 / M64 / M64b** 逐条打过红。 |
| PERF-11 | 附件不阻塞文本 | 附件在飞时文本轮 ≤25 s 到达对端；`STATS` 证明文本轮不等待 `attachments/**` | L3 + `FAIL(latency,target=attachments/**,ms=…)`；PROBE:`timeout-and-cancel` 证明取消语义真实可用 | 待建基线（机制已实测） |
| PERF-12 | 自动保存落盘频率 | ≤3 次 / 20 次快速输入；输入到落盘 ≤1500 ms | L1 fake clock；暂定 | 待建基线 **已实现并通过**（`notera-host/../stores/editor.spec.ts` 的「自动保存的节拍有上界（PERF-12）」：假时钟下连打 30 下、每 50ms 一次，`edit_note` 次数必须 >0 且 ≤3，且末次正文完整 —— 不是"理论上有防抖"）|
| PERF-13 | 列表滚动 / 大数据量渲染 | 20000 条列表滚动无 >50 ms 掉帧（暂定），首屏 ≤1 s | L5 采集（CDP trace） | **已实测（20000 条那一档，2026-09-27 本机 release 壳）**：300 次真滚轮共 **1120 帧，p95 = 17 ms、最坏一帧也是 17 ms**（预算 ≤50 ms）；首屏（第一行可见）最好 1322 ms。覆盖范围写清楚：这一滚跨过了 **807 行**、滚到列表深度的 **92%**（125171 / 136800 px），虚拟化列表 DOM 常驻只有 16-17 行 —— 所以它量到的是"滚 + 分页取数 + 重排"这一段，不是"一次性渲染 2 万行"（那本来就不是本设计）。判据里还有一条自检：**滚不过一页（<300 行）就直接红**，免得"浅滚"冒充大列表基线。<br>**2026-09-28 在本批新编的 0.0.21 release 壳上重跑（`RUN_TAG=g20k NOTES=20000 REPS=2`）**：首屏最好 **1455 ms**（上一档是 1322 ms；预算 ≤1500 仍过，但余量只剩 45 ms —— 这一格以后红不红取决于机器状态，不当作稳态结论）、RSS 起步 **69.3 MiB** 滚完 **69.4 MiB**（与 27 号那档的 69.1 对得上，没有可见增长）、滚动 **442 帧 p95 17 ms / 最坏 17 ms**，跨过 323 行、滚到深度 50091/60800（82%）—— 覆盖比上次浅一点（同样 120 次滚轮，上次数到 807 行），因为这一档的列表总深度不同；同一次跑里"空库"那一档 1450 ms（见 PERF-01 的 G13）。顺带量到一个之前没记过的数：**灌 20000 条走产品写入路径要 294.4 s**（≈68 条/秒，含每次保存的 FTS 与 revision），这是"导入/迁移要多久"的真实尺度，比任何推理都有用 |
| PERF-14 | 网络层出站请求数（一轮） | 与 `DUMP` 差分一致：无多余 PROPFIND（每轮 ≤1） | L3 `STATS`；**已实测**（`sync_cost` 断言 PROPFIND ≤1/轮，且空轮总请求 ≤2） |
| PERF-15 | Android 冷启动（arm64-v8a，中端机） | 暂定 ≤2500 ms | L6 + 真机；无法本机测量 | 待建基线（本机 **BLOCKED**，见 Q2） |

## 兼容性矩阵

### 客户端平台

| ID | 平台 / 版本 | 必测项 | 判据 | 状态 |
| --- | --- | --- | --- | --- |
| CM-PL-01 | Windows 10 x64（LTSC 最近的两个 GA 版本） | 安装 → 首启 → 建库 → 一轮 sync → 升级安装 → 卸载后数据保留策略 | L6 全通；升级后条目数与 hash 不变 | 待建基线（本机为 Win11） |
| CM-PL-02 | Windows 11 x64（10.0.26200，本机） | 同上 + GNU toolchain 构建（PROBE 记录 MSVC 链接失败：`PROBE-FAIL-001`） | L0–L5 全通 | 本机可执行（MSVC 通道 **BLOCKED**，见 Q3） |
| CM-PL-03 | WebView2 Evergreen（当前稳定版） | 记录 `RuntimeVersion`；富文本 12 类节点渲染 | 截图基线 diff ≤1%；无节点丢失 | 待建基线 |
| CM-PL-04 | WebView2 版本较旧 / Runtime 缺失或被禁用 | 安装器行为 + 降级路径 | 不得白屏：可见错误文案 + 引导；崩溃率不因此上升（无 panic 日志） | 待建基线 |
| CM-PL-05 | macOS 13+ arm64 | 构建 → 起窗 → 沙盒内 SQLite 写入 → 一轮 sync → 重启 → 退出后后台任务不残留 | L6 全通 | 本机无 macOS → **BLOCKED**（见 Q2） |
| CM-PL-06 | 系统字体差异（Windows YaHei / macOS PingFang SC / Android Noto Sans CJK） | 富文本与列表排版 | 无文本截断（每类节点各 1 条）；CJK trigram 搜索在三种字体下结果数一致（字体不影响检索正确性） | 待建基线 |
| CM-PL-07 | Android 8.0（API 26）/ 12（API 31）/ 15（API 35），arm64-v8a | 安装 → 首启 → 离线全功能 → 一轮 sync → 进程被系统回收后再启 → 返回键行为 → 字体缩放 200% → 深色跟随 | 每项均有可见结果；回收后未保存内容按 FT-SAVE-02 判据；`targetSdk` 相关权限提示不阻塞功能 | 本机无 Android 环境 → **BLOCKED**（见 Q2） |
| CM-PL-08 | Android 厂商后台限制（Doze / 省电白名单） | 后台同步窗口 | 明确断言**只保证**"启动时同步 + OS 调度任务"；调度未执行时不丢变更（outbox 保留）；可见的"上次同步于…"时间戳 | 待建基线 |
| CM-PL-09 | iOS 15+（架构预留，Phase 1 不构建） | 仅验证架构：`notera-core`/`-store`/`-crypto`/`-sync` 不依赖桌面专属 API；SQLite 路径通过注入；无 `std::env::current_exe` 类假设 | 静态可测部分：在 `aarch64-apple-ios` target 下 `cargo check` 通过（需要 macOS/Xcode）；运行态测试全部 **BLOCKED** | **BLOCKED**（见 Q2、Q4） |
| CM-PL-10 | 深色 / 浅色 + 跟随系统 | 见 FT-THEME-01..03 | 同 FT-THEME 判据 | 待建基线 |

### 后端（WebDAV 服务器）兼容

| ID | 目标 | 必测项 | 判据 | 状态 |
| --- | --- | --- | --- | --- |
| CM-SV-01 | `notera-test-webdav`（`mem` 模式） | 全部 L1–L4：动词 GET/HEAD/PUT/DELETE/MOVE/COPY/PROPFIND/PROPPATCH/OPTIONS/LOCK/UNLOCK、`Depth`、`Destination`、`Overwrite`、ETag、`If-Match`/`If-None-Match`（412/304）、chunked body、`/_control/*`、`/_fs/dump` | 服务端自测与 `notera-webdav` 客户端测试分离编写且全绿（否则整个 L1–L4 结论无效） | 本机可执行（待实现） |
| CM-SV-02 | `notera-test-webdav`（`fs` 模式） | 上述全部 + `RESTART` 后状态一致 | 同 CM-SV-01 + SY-INT-07 全绿 | 本机可执行（待实现） |
| CM-SV-03 | 真实 WebDAV 服务器清单（需用户提供，见 Q5） | 逐项记录：PROPFIND `Depth:1/0/infinity` 语义、MOVE 是否原子、是否支持 `If-Match`、是否支持 ETag、chunked 接受度、目录自动创建、路径中文/大小写折叠、LOCK 需求与否、`Overwrite:T/F` 行为、深度限制、每请求最大体积 | 差异写入兼容性表；任一项不支持 ⇒ 明确记录降级策略并由测试断言"降级后 INV-01/09 仍成立"，否则该服务器列为不支持 | **BLOCKED**（见 Q5） |
| CM-SV-04 | 自签 CA / 企业 TLS 终止代理 | `notera-net` 加载自定义 CA 完成握手（PROBE:`https-tls-handshake`、`proxy-config-actually-honored` 已证握手与代理生效链路可用） | 成功：真实 TLS 握手 + 一轮 sync；失败：错误分类为 TLS 而非 401；**不降级明文** | 本机可执行 |
| CM-SV-05 | 路径前缀与命名 | `.notes/` 下含中文实体名/大小写不敏感文件系统（Windows 侧） | 同一 id 在大小写折叠文件系统上不产生两个对象；`records/<kind>/<id>.json` 路径安全（PROBE:`uuidv7-monotonic-pathsafe`） | 待建基线 |
| CM-SV-06 | `aws-lc-rs` + `rustls` 于 `x86_64-pc-windows-gnu` | 构建 + 真实 TLS | 编译通过并握手成功（已实测） | 已实测 |
| CM-SV-07 | §5 能力探测（`notera-webdav/tests/probe.rs`） | 五项各自的"缺失"被单独认出（`FAIL(status=…)` 逐项注入）且不误伤别项；服务器不可达 ⇒ `RemoteError::Offline` 而**不是**"全 false 的结论"；探测对象只待在 `probe/` 且用完清空，绝不碰 records/manifest/attachments | 位图 → 写入策略映射与 §5 表逐字一致（有条件写⇒S1，什么都没有⇒S3）；`caps_bits_map_exactly_as_the_table_says` | 已实测（5 例） |
| CM-SV-08 | §5 探测的**接线位置**（`notera-host/tests/sync_once.rs`） | 启动路径先探后装：装出来的适配器带的就是实测位图（注入"条件写无效"→ 该会话应选 S2 而非默认 S1）；当天不重复探测；探测未完成时 `cap_mask` 保持 NULL 且发出可见提示；随后一轮同步把本地笔记推到真服务器、第二台设备原样拉回 | 请求日志里所有探测请求的 seq 都早于任何真实读写；`one_round_carries_a_local_note_to_a_second_device` 标题逐字一致 | 已实测（3 例） |
| CM-SV-09 | §11.4 租约的适配器侧（`notera-webdav/tests/lease.rs`） | 贴上/他人可见/release；自己的旧租约必须被覆盖而不是"别人占着"；过期即失效；`PROPFIND` 坏掉时仍能靠"从清单学到的对手"让路；不可达时发布报错但**读**不许挡住同步 | 6 例全绿；写租约只碰 `locks/`，不碰 records/manifest/attachments | 已实测（6 例） |
| CM-SV-10 | §11.4 租约的决策与端到端（`notera-sync/tests/engine.rs`、`notera-host/tests/sync_once.rs`） | 开关只看 §5 结果（S3 或无强 ETag 才开）；轮次开始贴自己的；**写清单之前**别人新鲜 → 不提交清单、改动保持 dirty、状态显示 `sync.leaseHeld`；过期/自己的/读不出来 都不挡公告；贴不上不中止本轮 | 弱服务器上第二台设备让路（清单 sha 不变、记录仍上传、`dirty_notes==1`），强 ETag 服务器上一个 `/locks/` 请求都不发 | 已实测（引擎 7 例 + 端到端 2 例） |
| CM-SV-11 | outbox 结清的两套 kind 词汇（`notera-store/tests/sync_surface.rs`、`notera-host/tests/sync_once.rs`） | 结清按 `(账户, entity_type, entity_id, payload_rev)` 精确命中；rev 对不上、账户对不上、kind 对不上都返回 false 而**不随便结一行**；已 done 的行不被第二次结清改写。端到端跑真的一轮：待发队列必须归零、`done` 行留在表里、哨兵账户的留痕行不许被顺手改掉 | 变异测过两条：把 `entity_type` 换成线上短标记（`n`）→ store 单测与端到端同时变红；把"待同步"计数的 `enabled=1` 过滤去掉 → 端到端 `left: 2` 变红 | 已实测（store 1 例 + 端到端 1 例） |
| CM-SV-12 | 命令面 `stats` 的键名 == 界面读的键名（`notera-host` 内 `stats_command_emits_…`、`scripts/verify-app.mjs`、`scripts/verify-tauri-window.mjs`、arch-check `edge:stats-dto-covers-ui-reads` + `edge:command-wire-is-camelCase`） | 走真 `dispatch`/真 `invoke` 拿到的对象，键集合必须恰为契约那 8 个（camelCase）；浏览器里设置页五行全是数字而非 `—`；架构门禁扫前端每一处 `settings.stats.X` 与 `StoreStats` 声明，并通用地要求每个命令出参类型显式声明 camelCase | 变异测过四条：删掉 `#[serde(rename_all)]` → Rust 断言与 arch-check 双双变红；改任一个 DTO 字段名 → arch-check 指名那个键与那个文件；删掉 `BackupInfo` 的该属性 → 门禁指名 `backup_db → BackupInfo`；把窗口门禁的类型判断改坏 → 该步 FAIL | 已实测 |

## 质量闸门

### 门定义

| 门 | 必须运行的层 / 行 | 时长目标 | 失败处置 | 本机可执行性 |
| --- | --- | --- | --- | --- |
| 预提交（本地，可选） | L0 + L1（受影响 crate） | ≤3 min | 本地拦下 | 是 |
| PR 门 | L0 + L1 + L2 全量 + L3 smoke（SY-CONV-01/02/04、SY-INT-02、MD-01 缩样）+ L5 smoke 10 条（FT-NOTE-01/02/03、FT-SRCH-01/08、FT-RT-12、FT-OFF-01、FT-THEME-01、FWD-01、FWD-03） | ≤15 min | 阻断合并 | **BLOCKED**（GitHub 不可达，见 Q1） |
| main 门（每次合并主干） | PR 门 + L3 全量 + L4 关键 5 点（CP-02/03/04/05/07） | ≤30 min | 回滚或修复 | **BLOCKED**（Q1） |
| nightly | L0–L4 全量 + L5 全量（可分片）+ INV-* 属性测试（每次 ≥10000 次生成） | ≤90 min | 次日 P1 | **BLOCKED**（Q1） |
| 发布候选门 | 全部：L0–L6 + 性能预算全表 + 兼容矩阵内首发三平台 + 崩溃点矩阵全量 | ≤4 h | 阻断发布 | 部分（Windows-gnu 可，macOS/Android/iOS BLOCKED） |

### 覆盖率

| 目标 | 值 | 说明 |
| --- | --- | --- |
| 核心 crate 行覆盖 | ≥80% | `notera-core`、`notera-store`、`notera-webdav`、`notera-net`、`notera-importer`、`notera-host` |
| 关键正确性 crate 行覆盖 | ≥90% | `notera-sync`、`notera-richtext`、`notera-crypto` |
| 分支覆盖（同上三 crate） | 暂定 ≥75%（待基线） | 冲突/失败分支必须有测试命中 |
| 不变式覆盖 | 100% | INV-01..15 每条至少 1 个属性测试 + 1 个 E2E/崩溃行；缺任一即视为门未达 |
| 覆盖率工具 | 暂定 `cargo llvm-cov`（待确认，见 Q6） | 阈值以工具实测数为准，禁止手工豁免 |

### flaky 处理政策

1. 检测到不稳定 → 立即打 `flaky` 标签 + 建立 owner 明确的 P2 缺陷 + 迁移到 nightly 的隔离 job（**不从 PR 门删除，只是移出**）。
2. 隔离期上限 14 天；到期未修复则该测试转为常驻阻断（因为它覆盖的是数据安全类断言时不允许长期出圈）。
3. **禁止**：删除断言、放宽阈值让 flaky 消失、加 `sleep` 掩盖竞态、`#[ignore]`/`skip`、mock 掉 HTTP/WebDAV/SQLite 来"稳定"、把 L3 降级为只断言不 panic。任何一项出现在 diff 里，按绕过闸门处理（review 必须驳回）。
4. 竞态类 flaky 优先用 fake clock / 显式 barrier / `/_control/inspect` 的顺序断言消根，而不是重跑。
5. 重跑策略：门允许**整 job 重跑 1 次**，但重跑通过而首跑失败必须留 issue；同一测试 30 天内 2 次重跑通过 = 升级为 P1 缺陷。
6. 阈值修改必须附带新的实测证据行（如 PROBE 记录），且在 `## 性能与容量预算` 中留痕。

### 新增测试要求（与变更绑定）

| 变更类型 | 同批必须交付 |
| --- | --- |
| 改协议（envelope / `protocol.json` / manifest / `records` / `attachments` 布局、`rev`/`base`/`hash` 语义） | ① 更新 `tests/fixtures/protocol/**` golden；② L2 **双向兼容**用例（旧 fixture 由新客户端读、新 fixture 由旧客户端读）；③ 涉及删除/冲突语义时新增对应 INV 断言行；④ `protocol` 版本号变更 + FWD-05 用例。缺一不合并 |
| 改富文本 schema | 新增 FWD-01/02 风格的未知节点保留用例 + 往返零漂移 fixture（FT-RT-12） |
| 改 SQL migration | 上一版本库 → 新版本的升级用例 + CP-08 崩溃用例（每个新版本各 1 条） |
| 改同步状态机 | L1 状态机全覆盖路径 + L3 至少 1 条端到端场景 + 相关 INV 属性测试 |
| 修 bug | **先写一条能复现该 bug 的失败测试**（P0/P1 无例外），再修复；测试名保留缺陷号 |
| 新增平台/后端兼容项 | 在 §兼容性矩阵 增行，且不允许以"手工验证"结束（必须脚本化） |
| 性能相关改动 | §性能与容量预算 对应行的前后对比数字（同机同数据集） |

### P0 定义（发布阻断）

内容静默消失或静默覆盖；已提交 rev 被更旧 rev 覆盖（INV-05 破坏）；tombstone 自动 GC 或本地误删（INV-02/03/11）；校验失败数据进入权威表（INV-08）；加密/AEAD 校验可被绕过或降级；迁移导致不可恢复的库损坏；旧客户端销毁新内容（INV-10）；崩溃后不可启动。

## 阶段出口条件

阶段范围为本文件的推定划分（见 Q7）。每阶段出口 = "必须全绿集合" 全部 `PASS` 且无 P0/P1 未闭环，且**无 BLOCKED 覆盖该阶段核心目标**（若有，须由人工签署例外）。

| 阶段 | 范围（推定） | 必须全绿 | 额外出口条件 | 不允许的遗留 |
| --- | --- | --- | --- | --- |
| P0 | 架构与可行性 | L2 golden 骨架、`notera-test-webdav` 自测 CM-SV-01/02、协议契约文档化 | 本计划所有 ID 有落点 crate；PROBE 16 项全绿（现有） | 无真实 socket 的测试基础设施 |
| P1 | 领域模型 + 本地存储（core/store/crypto/migration） | L0 全量、L1（store：CRUD/migration/事务/tombstone）、FT-NOTE-01/02/03/04、FT-BOOT-01、CP-08（全部迁移版本）、INV-04/06/08/11 | 三 crate 覆盖率达 ≥80%；migration 升级用例逐版本 | 无崩溃安全迁移 |
| P2 | 富文本 + 功能全集（无同步） | FT-NOTE-*、FT-FLDR-*、FT-MOVE-01、FT-PIN-01、FT-CHK-01/03、FT-RT-01..13、FWD-01..03、FT-SAVE-01 | L5 smoke 通过；canonical JSON 零漂移 | 未知节点丢失 |
| P3 | 检索 / 外观 / 稳定性打磨 | FT-SRCH-01..11、FT-THEME-01..03、FT-PIN-03、FT-SAVE-03、PERF-02/03/04 | 1–2 字 CJK 搜索必须非 0（硬条件）；无 >1 次 flaky 记录 | 用 `LIKE` 兜底遮蔽 FTS 回归 |
| P4 | WebDAV 客户端 + 单向同步 + 离线 | SY-CONV-01/02/04、SY-FAULT-05/11/12、SY-INT-06/07/08、SY-ID-05/06、FT-OFF-01/02、CP-01/02/03/07/10、FT-SETUP-01 | manifest 为唯一提交点被 L4 证明；离线全功能可用 | 任何依赖"远端成功"才允许的本地写 |
| P5 | 双向同步 + 冲突策略 | SY-CONV-03/05/06、SY-CONF-01..09、SY-FAULT-01/02/03/04/06/07/09/10、SY-INT-01/02/03/05、SY-ID-01/02/03/04、FT-MOVE-02、FT-PIN-02、FT-CHK-02、FT-OFF-03、CP-04/05、MD-01/02/03/06、全部 INV 属性测试 | **冲突不丢内容 0 例外**；PERF-08 首轮实测；`notera-sync` 覆盖率 ≥90% | 未断言 keep-both 的冲突用例 |
| P6 | 附件 + 导入导出备份恢复 | FT-ATT-01..05、FT-IO-01..06、SY-FAULT-08、SY-INT-04、CP-06/09、MD-04 | 附件不阻塞文本（INV-13）实测；导入可中断且无损 | 任何"恢复破坏原库"的路径 |
| P7 | 规模 / 性能 / 崩溃加固 + 三平台构建 | CP 全量、MD-05、SY-INT-09/10、PERF-01/05/06/07/10/12/13/14、L6 三 target（Windows-gnu 可执行；macOS/Android 见 Q2） | 全表性能预算有实测值并固化为基线；崩溃矩阵 0 未覆盖点 | 未固化的"暂定"阈值 |
| P8 | 发布候选 | L0–L6 全量 + 兼容矩阵内可执行行全绿 + 无 P0/P1 + flaky 隔离清单为空 | 三平台真机 UAT 清单脚本化完成；发布说明含已知不支持的 WebDAV 服务器（依赖 Q5） | 任何以"仅手工测试"计为 PASS 的行 |

## 待人工确认

只列真正缺条件的事项；本文件未对其做任何默认决定。

| 编号 | 事项 | 原因 | 影响 | 所需条件 |
| --- | --- | --- | --- | --- |
| Q1 | CI 全部不可验证：PR 门 / main 门 / nightly 的执行结果 | 本机 github.com 与 api.github.com 不可达（HTTP 000），CI 文件可写但无法触发或查看 | 所有标注"进 PR 门"的行为**门定义**而非**门结果**；P1 之后每个阶段出口都缺一份"CI 已绿"证据 | 可达 GitHub 的网络，或改用可达的 CI 平台（自建/镜像） |
| Q2 | 非 Windows 平台的 L5/L6/兼容性行：macOS 13+ arm64、Android 8/12/15 arm64-v8a、iOS | 本机仅 Windows 11 x64，无 macOS/Xcode、无 Android SDK 与真机/模拟器 | CM-PL-05/07/08/09、PERF-15、L6 三分之二的行无法执行；首发三平台只有一平台可闭环 | macOS（含 Xcode + aarch64-apple-ios target 用于 CM-PL-09 静态验证）、Android SDK + 至少 1 台 API 26 与 1 台 API 35 真机 |
| Q3 | Windows 发行通道（MSVC vs GNU toolchain、安装包格式与签名） | PROBE 记录 `PROBE-FAIL-001`：本机无 MSVC Build Tools，且 Git 的 coreutils `link(1)` 遮蔽 rustup msvc host 的 `link.exe`，MSVC 目标构建失败；Tauri 打包/签名通常以 MSVC 为主 | L6 Windows 行、升级安装（CM-PL-01）与代码签名/SmartScreen 行为无法在本机验证；"Windows x64 首发"缺少可发布的构建通道结论 | 决定 Windows 发行 target 与打包链路；若必须 MSVC，需要一台装好 MSVC Build Tools 的机器（或修复本机 PATH 遮蔽） |
| Q4 | iOS "架构预留"的验收定义 | 已给事实仅为"不构建、需架构就绪"，未给可观测判据 | CM-PL-09 无法从 BLOCKED 转为 PASS/FAIL；架构违规（如桌面专属 API 泄漏）可能到 Phase 8 才发现 | 明确预留判据（例：`cargo check -p notera-core -p notera-store -p notera-crypto -p notera-sync --target aarch64-apple-ios` 纳入 main 门；IO 路径全量注入化） |
| Q5 | 真实 WebDAV 服务器清单（CM-SV-03） | 只有 `notera-test-webdav` 可控；真实服务器（各家对 MOVE 原子性、`If-Match`、ETag、chunked、大小写折叠、目录自动创建的差异）无可用测试端点 | 产品定位是"用户自有 WebDAV"，却只对自研服务器验证过协议；上线后最可能的 P0 来源 | 用户提供至少 2 个可读写、可反复建删 `.notes/` 前缀的真实 WebDAV 端点（含凭据），并允许在其中注入并发/权限差异 |
| Q6 | 覆盖率与测试基础设施的具体工具链 | 未决定：`cargo llvm-cov` / `tarpaulin`、vitest / 其他、Playwright 驱动 Tauri WebView 的接线方式（Windows 上依赖 WebView2 调试端口是否可用）、属性测试框架（quickcheck / proptest）、测试数据集生成脚本 | §质量闸门 的覆盖率阈值暂无法自动执行；L5 在 Windows 上能否真实驱动 WebView 尚未证实 | 确认工具选型并允许在 Phase 0 末做一次端到端 spike（含 WebView2 远程调试与 `/_fs/dump` 断言闭环） |
| Q7 | Phase 1–8 的官方范围定义 | 已给信息仅到"Phase 0 是架构"和"iOS 不在 Phase 1 构建"，未给 P1..P8 划分 | §阶段出口条件 的 P1–P8 范围为本文件按能力递进的推定；各矩阵"所属阶段"列需据此重排 | 提供路线图各阶段目标，或直接确认本文件的推定划分 |
| Q8 | 若干性能阈值为暂定值 | **2026-09-27 已把桌面侧三档量出来了**（`scripts/verify-perf.mjs`，真实 release 壳）：冷启动 空库 907~1002 ms / 5000 条 1022 ms / 20000 条 1322 ms、RSS 31.7→69.1 MiB、列表滚动 p95 17 ms（1120 帧）；加上更早的搜索、manifest 体积、AEAD/argon2、TLS、超时、空轮字节（PERF-05/14 由 `sync_cost` 钉）、自动保存节拍（PERF-12）。**仍无实测支撑**：Android / macOS 冷启动（没有那些设备，见 B1）、100 附件索引那一档、30 min × 5 轮的泄漏趋势 | 桌面侧三档现在可以当依据引用（数字就在 PERF-01/10/13 三行里）；未测的那几项仍然不能（行里已写清"未采"） | 已按本行第 4 列回填；剩下的三项要等真机与长时间挂跑，不靠"暂定值"糊过去 |
| Q9 | 未同步/冲突之外的删除保留策略与 purge 语义 | 已给事实为"tombstone 永不自动 GC"，但"用户主动清空回收站 / 永久删除后远端对象与附件何时真正回收"未定 | FT-NOTE-05/06、FT-ATT-05 的断言只能停在"purged 置位 + tombstone 保留"，无法验证存储回收 | 决定 purge 的触发者与安全性条件（是否需全设备确认、是否有安全窗口），再补对应不变式 |
| Q10 | 附件与记录的加密边界 | 已给事实为"记录信封 aes-256-gcm-siv"；附件（内容寻址、immutable）是否加密、加密是否影响 sha256 去重与跨设备复用 | FT-ATT-01/02、CM-SV-03 无法断言"附件在远端为密文"这一层；可能存在"内容 hash 泄露元数据"的安全疑问 | 明确附件是否加密、hash 取明文还是密文、以及信封字段（`enc`/`ct`）对附件的取值 |
| Q11 | `notera-test-webdav` 控制 API 的字段名与语义细节 | 已给事实只到 `/_control/*` 支持 start/stop/reset/inspect/inject-failure 与 `/_fs/dump` | §记法 中的 `FAIL(kind,target,times)` 为**测试计划统一记法**，与实现签名可能有差异；L3/L4 断言写法需按实现校正 | 在 P0 末冻结控制 API schema（并把本表与实现逐条对齐） |

## 崩溃注入矩阵（§20 · 进程真的被杀死）

实现：`notera_core::crash_point(点名)` + `NOTERA_CRASH_AT`，只在 debug 构建有效
（正式产物不留"一个环境变量就能让应用自杀"的开关）。用 `process::exit(77)` 而不是
`panic`：panic 会展开栈、跑析构、还可能被上层接住，那测的就不是"写一半时断电"。
注入点名单一处在 `CRASH_POINTS`，插桩、文档、测试共用一份，避免"点名拼错却照样绿"。

跑法：`crates/notera-host/tests/crash_recovery.rs` 为每个点 **spawn 一个子进程**
（写一条带附件的笔记 → 正文轮 → 附件轮），要求它以 77 死在这一点上，然后父进程
重启同一目录、按生产调度跑正文轮 + 附件轮至收敛。

| 编号 | 注入点 | 必须成立 | 层 |
|---|---|---|---|
| CI-CRASH-01 | `after_local_write`（`write_tx` 唯一写入口，commit 之后） | 该条改动仍在，重启后一轮即公告 | L3 |
| CI-CRASH-02/03 | `before_records_push` / `after_records_push` | 记录不半截；已推未公告的下轮补公告，不产生副本 | L3 |
| CI-CRASH-04/05 | `before_apply` / `after_apply` | 拉下来的内容要么完整生效要么没有，不留半条 revision | L3 |
| CI-CRASH-06/07 | `before_attachment_upload` / `after_attachment_upload` | 字节在不在服务器与账上状态一致；不重复传也不谎报已传 | L4 |
| CI-CRASH-08/09 | `before_manifest_commit` / `after_manifest_commit` | 公告失败 → 改动保持脏；**公告成功但本地未结清 → 下一轮必须把它结清**（今天就是这里坏过：P7 判 NoOp 后再没人回头，"待同步"计数永久挂着） | L3 |
| CI-CRASH-10 | `after_segment_write`（压实：分段已落盘、索引尚未提交） | 大库夹具（240 条 > `WINDOW_MAX`）真死一次 → 重启同一座库 → 跑到结清 → 另一台设备追平。**必须成立**：崩完之后索引引用的每个分段都在盘上（INV-09）、窗口没超过上限、本机笔记数一条不少、两台设备标题+内容哈希逐条一致、待办归零。这一格由 `tests/compaction_crash.rs` 覆盖，**不在 9 点小库矩阵里**（那套夹具只写 1 条笔记，走不到压实）；两边的名单由 `CRASH_POINTS_NEED_LARGE_LIBRARY` 做减法拼回全表，`crash_point_lists_partition_registry` 保证"新加了点却两边都没盖"直接红。变异自证：摘掉注入点 → 报"没让进程死在那里"；让分段不落盘就换索引 → 报"索引引用了不存在的分段 seg-0000" | L4 |
| CI-CRASH-11 | `after_quarantine_move`（GC：**字节已挪进隔离区、账还没写**那一瞬） | 需要**零引用夹具**（那条笔记得已被永久删除，否则 GC 不认领 —— 与 `after_segment_write` 同一类"点名了但走不到"的坑，所以它单独一张表跑）。真死一次之后：字节整份还在盘上（只是换了位置）、账上仍是那条假账 `available`；重启后**一轮附件轮自己收回来**（磁盘体检把假账降级 → 下载那一轮先看隔离区 → 本地挪回，**为该 sha 的请求数不变**），再一轮 GC 才把它正常认领并写上账 —— 也就是**收敛到设计里的状态，而不是在"补/收"之间来回摆**。门禁 `attachment_gc.rs::a_crash_between_the_move_and_the_ledger_heals_itself_without_one_request`；变异自证 **M43** 摘掉那句 `crash_point` → 红在"注入点没有让进程死在那里（退出码 Some(0)）"，**M41** 短路掉下载轮的本地恢复 → 红在"本机隔离区里就有这一份，自愈却去服务器要了一遍" | L5 |
| CI-CRASH-12 | `after_corrupt_park`（体检：**坏字节已挪成 `.corrupt`、账还没降级**那一瞬） | 要"本机那份被截断（长度与登记不符）+ 服务器上是好的"这一形 —— 矩阵夹具写的字节永远是好的，体检只会 `continue`，走不到这一步（同长度位腐它本来就不管，那是 FT-ATT-12b 读侧那一格）。真死一次之后：正式位置空、**`.corrupt` 里那份逐字节等于挪开前的坏字节**（断电不许变成销毁）、账上仍是 `available` 那条假账、正文照旧可用；重启后一轮附件轮必须自己捡回来（体检看见"available 而正式位置没有"→ 降级 → 下载 → 拿到好字节之后才清 `.corrupt`），且**这一格请求数要增加**（本机没有好的一份，不去要才是假绿）；补回来后再一轮必须 `(0,0,0)` 收手。证据：`attachment_faults.rs::a_crash_right_after_parking_the_corrupt_copy_still_gets_a_clean_replacement`；变异 **M44**（摘掉那句 `crash_point`）红在"注入点没有让进程死在那里（退出码 Some(0)）"、**M45**（"文件不在就不降级"那一支）红在"重启后的第一轮要把这一行捡回来：(0, 0, 0)" | L5 |
| CI-CRASH-ALL | 小库矩阵 8 个点连跑（`after_segment_write` 要大库夹具、`after_quarantine_move` 与 `after_corrupt_park` 要专门的附件夹具，各自单独跑，见 CI-CRASH-10/11/12） | 崩完重启后两台设备笔记**逐条一致**（不多不少）、`dirty_notes=0`、`outbox_pending=0`、FTS 行数 == 笔记数（I5） | L3 |

门禁自身的反空转证明：把 P7 的结清分支改回"永远跳过"，CI-CRASH-09 立刻红并打出
卡住的那一行（`outbox pending=1；脏笔记 [rev=3 sync_rev=0]；待办行 [note rev=3 op=Upsert]`）。

## 无障碍（§26）

| ID | 场景 | 注入 | 操作 | 预期 + 不变式 | 层 | 阶段 |
|---|---|---|---|---|---|---|
| A11Y-01 | 每个交互控件都念得出名字 | 静态扫 `apps/desktop/src/**/*.vue` 模板（含 `div role="button"` 这类自定义控件；先抹掉注释 / `<script>` / `<style>` 免得把注释里的 `<input type=file>` 当控件） | `node scripts/arch-check.mjs` | 每个 `button/input/select/textarea/a` 与带交互 role 的元素，必须有 aria-label / aria-labelledby / title / `<label for>` / 包裹式 `<label>` / 可见文字之一。**已实现并通过**（第 25 条门禁）。第一次跑抓到编辑区文本块 `role="textbox"` 只有 `data-placeholder`（那不是名字来源）→ 现在由 `v-editable` 写 `aria-label`。变异自证：摘掉块把手的 `aria-label`+`title` → 红；摘掉文本块的名字 → 红 | L1（静态） | P5 |
| A11Y-02 | 块型文案键真的登记过 | `editor/labels.spec.ts` 逐种块型核对 `t(key) !== key` | `pnpm --dir apps/desktop test` | 未知块型返回原始类型名而不是假装"正文"。**已实现并通过**。存在的理由：`t()` 查不到键时**原样返回键名**，拼错的键一声不响 —— 这张表原来用模板串拼键时 7 种里 5 种显示成 `editor.blockCodeBlock` | L0 | P5 |
| A11Y-03 | 读屏名字跟着块型走 | `editableDirective.spec.ts` 在 `mounted`/`updated` 上断言 | 同上 | 正文→「正文」、代码→「代码」、标题带层级；**切换块型而文本没变时名字也要更新**（`updated` 会因签名相同跳过覆写 DOM，名字必须在短路之前就写好 —— 这条测试第一次跑就抓到我这么写漏了） | L0 | P5 |
| A11Y-04 | 键盘可达 + 触摸目标 ≥44pt | 真浏览器 | `verify-blackbox.mjs` 第 9 步 / `verify-app.mjs` 移动端视口步 | Esc 与 Tab 不失控、焦点始终在界面里；3 个移动入口全部 ≥44px。**已实现并通过**（沿用），对比度仍只有设计期 token 证据 | L4 | P5 |

## 纯黑盒 UAT（§23：只许界面动作）

`scripts/verify-blackbox.mjs` —— 10 步，全脚本**没有一次** `/cmd/*` 调用、没有任何数据库读取；
断言只读屏幕上可见的文字与几何。它和 `verify-app.mjs` 是两件事：后者用本地桥复核"库里到底有没有"，
因此不算黑盒，但那条一致性黑盒给不了。两道都得跑。

| 编号 | 动作（只有点击/输入/键盘/文件选择器/刷新） | 必须看得见 | 层 |
|---|---|---|---|
| BB-01 | 打开应用 | 首帧就是笔记列表（可见文字 > 20 字），不是白屏也不是加载圈 | L5 |
| BB-02 | 点「新建笔记」→ 敲一行 | 回「全部笔记」后列表里能看到刚敲的那条 | L5 |
| BB-03 | 侧栏点「＋」→ 命名 → 回车 | 侧栏出现这个子文件夹 | L5 |
| BB-04 | 打开笔记 → 「移动到」选它 → 点进子文件夹 | 下拉里有该路径；进去后这条笔记在列表里 | L5 |
| BB-05 | 搜索框输入关键词 | 恰好 1 行命中；清空后恢复 | L5 |
| BB-06 | 删除 → 回收站 → 恢复 | 回收站里看得到、恢复后回到列表 | L5 |
| BB-07 | 刷新（等价重启 App） | 列表仍在；点开正文仍是那段字 | L5 |
| BB-08 | Esc、Tab×2 | 焦点始终落在可交互元素上（不跑到 body） | L5 |
| BB-09 | 全程 | console error 0 条、HTTP 4xx/5xx 0 次 | L5 |
| BB-10 | 打开笔记 → 走那个文件选择器插一张真 PNG | 屏幕上这张 `<img>` **真的解出像素**（`naturalWidth > 0`，只有 `src` 属性不算），刷新重开之后仍解得出来 —— 全程不读库、不调命令 | L5 |

BB-02 抓到过一个只有黑盒才能抓到的缺陷（详见 CHANGELOG）：**空库里点「新建笔记」，
光标本来在正文里，此时点到最后一行下面的空白就把焦点丢给了 `body`**，紧接着敲的头
几个字直接消失 → 界面上永远是一条「无标题」。修法是在编辑区空白处的点击里把光标送
到最后一个文本块的末尾，并且仍走 `focusBlock` 那一条唯一的路径（不在这里另写一套选区
逻辑）。这条判据是屏幕事实，白盒那一层看不到 —— 它只看库里存了什么，而库里存的确实是
"那条空笔记"。

反空转证据：把回收站"恢复"按钮的 `v-if` 改成 `false`，9/9 立刻掉到 6/9。
这条门禁第一遍也抓到过自己的假绿 —— 收起状态的 `<select>` 上点 `option` 元素不触发
`change`，必须 `selectOption`（当时那一步"过了"，实际什么都没移动）。

## 平台能力（§15 · 托盘与全局快捷键，2026-09-27 落地）

| ID | 判据 | 落在哪 | 层级 | 优先级 | 状态 |
|---|---|---|---|---|---|
| CF-13 | P11（删除 vs 修改）：冲突卡片要读得出**对面那一版**的正文，而不是只给一串哈希 | 存储侧 `notera-store/tests/conflict_payload.rs`（5 条：只挂到面板真正显示的那条未裁决行 / 同笔记多条登记时挂最新 / 已收卡既不暴露也不接受补写 / 账户之间不串 / 没取回来时载荷为 None 且冲突照旧在册）；端到端 `notera-host/tests/conflict_payload_e2e.rs`（两台真设备 + 真 WebDAV：A 删、B 在删后又改 → B 卡片 `remote_preview` 含 A 那版文字、不含 B 本机文字，且引擎没替用户改本机正文）；**真浏览器** `scripts/verify-p11-panel.mjs`（10 步，夹具是同一份两台真设备现场留在盘上）：右栏渲染的就是对面那一版、且不是本机那份的复制；载荷缺失时屏幕上是"没能取回那一版"而不是空白也不是本机内容 | L3+L4 | P0 | **已实现并通过**。这条门禁的自证不是造反例，是**它第一次跑就抓红了我自己刚写错的设计**：第一版把读回接在 `preview_text(id, remoteRev)` 的回落上，而 rev 是各设备自己的编号 → 右栏读回来的是本机那一版，测试当场红（断言两条：必须含对面文字、必须不含本机文字）。修法是让载荷只由卡片带（`ConflictDto.remote_preview`），并按 §6.1 在面板里禁掉右栏那次按号查本机历史。**第二次是浏览器 lane 抓的**：那个禁令只写在"卡片带了载荷"的分支上，没载荷时仍会去查本机历史 —— 而单测把 `preview_text` 桩成了抛错，于是"查失败所以空着"看着是对的，真界面里它查得回来（本机历史上确实有那个号），"没取回来"这句话永远说不出口。现已改成一条规则：右栏**一律**只读卡片自带的 `remote_preview`；`update_update` 采纳的那一版由引擎把同一份字节也登记到冲突行上（`notera-sync` 的 `ConflictPayload`），所以两类冲突同一口径，界面侧不需要知道冲突是哪类。变异自证：把那条 `continue` 改回旧条件 → `conflicts.spec.ts` 当场红 |
| CF-14 | §5.2：远端**永久删除**撞上本机**从未上传**的编辑 —— 传播不许吃掉那一段文字 | `notera-host/tests/conflict_payload_e2e.rs::a_remote_purge_never_eats_an_edit_that_never_left_the_device`（两台真设备 + 真 WebDAV：A 建 → B 拉到 → B 改但一轮都不跑 → A 彻底删除并公告 → B 才同步；断言 ① 未裁决冲突里有这条 ② 那段文字仍读得回来：正文或卡片副本） | L3 | P0 | **已实现并通过**。之前这条只有存储层"purged 笔记不上行"的断言，**没有任何**证据说明撞车时本机内容被留住 —— 而 `plan.rs` 表头写着"永久删除传播优先于一切编辑"，光读代码会以为 §5.2 被推翻。变异自证：把 P13 的 `r.purged && !local_changed` 改成无条件 `r.purged` → 当场红在"静默丢失"那句；改回即绿 |
| CF-15 | §5.1 第 4 步：用户还没选之前，冲突笔记要**留在正常列表里**并在行上看得出"有分歧" | `scripts/verify-p11-panel.mjs` 第 9 步（真浏览器 + 两台真设备留下的现场：回到"全部笔记"，那条笔记仍在、标题仍是本机那一版的文字、⚠ 只落在冲突在册的行上）；`conflicts.spec.ts` 断言 `contended` 集合来自未裁决卡片 | L4 | P2 | **已实现并通过**。这条先是**文档有、代码没有**（`NoteList.vue` 里连一次 "conflict" 都没出现过）。判据故意写成两条：有标记 **且** 标记数严格小于行数 —— 只写"标记 > 0"的话，"每行无条件画个 ⚠"的坏实现照样通过。变异自证两个方向：改成 `v-if="true"` → 红在"3 行里 3 行都带标记"；摘掉渲染 → 红在"列表上没有任何标记" |
| CF-16 | §5.1 第 3 步：用户在 P11 卡片上接受对面那条删除之后，删除必须**真的发生并传播**，且不许在对面那台复活 | `notera-host/tests/conflict_payload_e2e.rs::accepting_the_remote_delete_actually_deletes_and_propagates_it`（两台真设备 + 真 WebDAV：A 删 → B 改而不上行 → 冲突 → B 按 `replaceWithRemote` → 断言 B 没有这条正常笔记；两边再各追平若干轮，断言 A 那台仍是已删除） | L3 | P0 | **写出来时是红的**（卡在第一个断言：卡片被关掉而笔记照常活着，下一轮本机的脏 head 会把对面的删除覆盖回来 —— 按了按钮等于什么都没发生，还替别人撤销了删除）。修法是 `App::resolve_conflict` 在 remote 这一支按冲突行的 `remote_wire` 判定并落地删除；没带载荷就不动内容、只关卡片（由 `conflict.remoteNotFetched` 那句说明）。改完该文件 4/4、全量 516/0 |
| PLAT-01 | 托盘菜单除"显示/隐藏""退出"两条独有项之外，每一项都必须是应用菜单里已有的 id（不许长出第二条实现） | `notera-host::platform::tests::tray_items_other_than_the_trays_own_reuse_the_menu_routes` | L1 | P2 | **已实现并通过**（变异：塞一个 `tray.backup` → 红） |
| PLAT-02 | 系统级快捷键**不得**复用任何一条应用菜单 accel，且必须带两个修饰键（否则就是在别的应用里劫持 `Ctrl+S`/`Ctrl+F`） | `global_shortcuts_never_reuse_a_bare_app_menu_accelerator` | L1 | P1 | **已实现并通过** |
| PLAT-03 | 设置页显示的全局快捷键（行 + 组合键字面）与实际注册的那批一模一样 | `the_settings_page_shows_exactly_the_registered_global_shortcuts`（Rust 读 `platform/caps.ts`） | L1（跨语言契约） | P1 | **已实现并通过**（变异：`Alt` 偷改成 `Shift` → 红并打出该行原文） |
| PLAT-04 | 「关窗收进托盘」需要**开关为真且托盘真的挂上**两个条件；默认关 | `apps/desktop/src/platform/caps.spec.ts`（4 条） | L1 | P2 | **已实现并通过** |
| PLAT-05 | 能力声明由**注册结果**写：`tray` / `globalShortcuts` 只有在壳真挂上之后才为 true | `scripts/verify-tauri-window.mjs` 第 3 步（真壳里经真 `invoke` 读 `platform_caps`） | L4（真窗口运行期） | P1 | **已实现并通过**（变异：摘掉 `report_native_cap(Tray, …)` 重新构建真壳 → 8/9 并点名 `tray 不是 true`） |
| PLAT-06 | 托盘/快捷键注册失败要**可见**：Toast `platform.caps_degraded`，且设置页那几行随之消失 | 判据在 PLAT-05 的同一处（能力为 false → `shortcutsFor` 过滤掉）；界面侧**未做真机失败注入** | — | P2 | **BLOCKED**：要让托盘注册失败得先把系统托盘弄坏（`explorer` 重启 / 键位被占），本机没有可重复的注入手段 → 记为待人工确认，不当作已验 |

## 导入：`.enex`（Evernote，2026-09-27 落地）

| ID | 判据 | 落在哪 | 层级 | 优先级 | 状态 |
|---|---|---|---|---|---|
| IMP-01 | 一份 `.enex` 出 N 条笔记；CDATA 里的 ENML 必须被当 XML 解析（正文不许留 `<div>` 字样） | `notera-importer::enex::tests`（`one_enex_file_...`、`cdata_payload_is_parsed_as_xml_...`） | L1 | P1 | **已实现并通过**（这条抓到了第一版把 `Event::CData` 静静吞掉的实现 —— 只测"笔记条数"是过不了的：笔记数来自 `<note>`，与正文无关） |
| IMP-02 | 附件按 **sha256** 内容寻址；`<en-media hash>`（MD5）只用于配对；"只挂不嵌"的资源也要有块 | `resources_become_blocks_the_storage_layer_can_read` | L1 | P1 | **已实现并通过** |
| IMP-03 | 导入造的块，存储层必须真读得出附件（字段名一漂，用户看到的就是一辈子停在占位的图片） | 同上（断言打在 `notera_richtext::attachments(&doc)` 上）+ `notera-host/tests/enex_import.rs` 断言字节真的落在数据目录 | L1+L3 | P0 | **已实现并通过** |
| IMP-04 | 本库表达不了的（`<tag>` / 时间戳 / 表格结构 / 悬空引用 / 非 base64 资源）必须**点名**，一路到命令回报与界面 | `what_the_schema_cannot_hold_is_named_not_dropped`、`non_base64_resource_is_named_...`、`a_media_reference_with_no_resource_is_reported`、`import_files` 的 `notices` | L1+L3 | P1 | **已实现并通过** |
| IMP-05 | 幂等：同一份 `.enex` 再导一次一条都不新建；重复数按条目报 | `notera-importer/tests/importer.rs` + `enex_import.rs`（重放断言 `created == 0 && duplicates == 2`） | L3 | P0 | **已实现并通过** |
| IMP-06 | 新格式不许绕开任何一道入口闸门（8 MiB 体积上限对 `.enex` 同样生效） | `enex_goes_through_the_same_size_gate_as_other_sources` | L3 | P1 | **已实现并通过**。⚠️ 代价：真实带图导出可能超限 → 表现为一条看得见的失败（不是静默截断）；要支持大文件得改成按 `<note>` 流式读盘，动 `ImportSource` 形状 → §9 |
| IMP-07 | 从**命令面**进来（`import_files`）而不是只有库内 API；空路径列表要报错而不是"成功导入 0 条" | `notera-host/tests/enex_import.rs::import_files_command_...` | L3 | P1 | **已实现并通过** |
| IMP-08 | 浏览器里点得到、看得见 notices | `scripts/verify-app.mjs` 第 35 步（重载到桌面视口 → 点侧栏「设置」→ 填 `.enex` 路径 → 点「导入这个文件」→ 读屏幕上的报告与逐条说明 → 回列表确认两条笔记真在） | L4 | P1 | **已实现并通过**（36/36）。变异自证：把 dispatch 里的命令名改成 `import_files_MUTATED` 重新起桥 → 这一步红，并且"零失败请求"那步点名 `POST /cmd/import_files → HTTP 400`。**踩到的坑记一笔**：清空数据目录后必须等桥 `{"ok":true}` 再跑，用固定 `sleep 8` 会出现 4 条与改动无关的红（"刷新后笔记消失"那种），差点被误判成产品回归 |

## XML 解析行为锚（quick-xml 0.37.5 → 0.42，2026-09-29）

这三族锚不是为了覆盖功能，是为了**换 XML 解析库版本时能看见语义漂移**。
0.42 的实测漂移不是编译错误而是**静默少字符**：解析器不再顺手反转义文本，
`&amp;` / `&#65;` 单独成 `Event::GeneralRef`。只接 `Event::Text` 的话，
导入的正文变成"粗  斜  尖"、租约路径变成 `/x/locks/ab.json`（别人的租约就此看不见）。

| ID | 判据 | 落在哪 | 层级 | 优先级 | 状态 |
|---|---|---|---|---|---|
| XML-01 | `.enex` 两遍解析里，实体引用都必须解成字符：标题（第一遍）与 CDATA 内 ENML（第二遍）各自钉住，且不许解过了头 | `notera-importer::enex::tests::entity_references_are_decoded_in_both_parse_passes` | L1 | P1 | **已实现并通过**。升级前先在原版本上跑绿，升级后仍绿。变异自证两个方向各一次：摘掉第二遍那一格 → 红在 `粗  斜  尖`；摘掉第一遍那一格 → 红在标题那句（M66 / M67）。这两条**都是红的样子不同**，所以两遍各自有独立证据 |
| XML-02 | 一个没分号的裸 `&`（`Tom & Jerry` 这种手写/第三方导出）既不许让整份 `.enex` 导入失败，也不许把那个文本节点变成空串 | `notera-importer::enex::tests::a_lone_ampersand_survives_without_failing_the_import`（断言 2 条笔记都在、标题原样、正文原样） | L1 | P1 | **已实现并通过**。这条钉的是 `allow_dangling_amp = true` 那一格 + 一条**旧行为缺陷**：旧代码遇到解不开的引用是 `unescape().unwrap_or_default()` ⇒ 整个节点变空（0.37.5 上这条是红的）。变异自证 M68：摘掉那一行 → 红，且其余 55 条不受影响（判据是特异的） |
| XML-03 | PROPFIND 里的 `href` 必须①按局部名认（换前缀/大小写都算）②引用解成字符③**遇到解不开的引用也要留在列表里** | `notera-webdav::lease::parse_pins` 三条（`hrefs_are_matched_by_local_name_not_by_prefix`、`entity_references_inside_href_are_decoded`、`an_unrecognized_reference_keeps_the_lease`） | L1 | P0 | **已实现并通过**。第三条钉的是方向：这一层"少看一条租约"的后果是两台设备同时写同一条笔记，所以宁可路径里留着 `&nbsp;`。变异自证：摘掉引用那一格 → 两条红（路径被拆成 `/x/locks/ab.json`）；把"解不开就丢"（旧行为）写回去 → 第三条红在 `["", ""]`；摘掉 `allow_dangling_amp` → 第三条红在"第二条租约整个消失"（M69 / M70 / M71） |
| XML-04 | 测试服务器读 PROPPATCH 属性值必须是**原文**（不替调用方反转义），且一个裸 `&` 不许让整条请求解析失败 | `notera-test-webdav::xml::tests::property_values_are_read_verbatim_and_a_lone_amp_does_not_kill_the_request` | L1 | P2 | **已实现并通过**。两个方向都验过会红：摘掉 `allow_dangling_amp` → 值变空串；顺手把值反转义 → `x &amp; y` 变 `x & y`（M73 / M74b）。**两条踩坑记录**：第一次变异改的是 PROPFIND 那个 reader，测试照常绿 —— 改错了地方，不算证据；而反转义那一版第一版夹具里同时带着裸 `&`，`unescape` 对它报错后回落到原文，把 `&amp;` 那半也一起放过了 —— 判据被自己的夹具糊过去，补了一条不含裸 `&` 的属性才有区分度 |

## 文件夹树的环（§4 `parent_id`，2026-09-29 落地）

这一族抓的是**两台设备各做一次"单独看合法"的移动**。本机那一支（`Store::move_folder`）一直有环检测，
而远端那一支（`apply_folder`）过去只查"父存在不存在" —— 于是 甲的父=乙 与 乙的父=甲 能被同步写进同一台库。
后果都不报错：① 这两个夹子从根走不到了（侧栏里没有，里面的笔记也看不见）；② 用户下一次移动任何文件夹
都要跑那条递归 CTE，**有环时它永不返回**（实测：把那一句改回 `UNION ALL` 之后 60 s 都没回来）。

| ID | 判据 | 落在哪 | 层级 | 优先级 | 状态 |
|---|---|---|---|---|---|
| CY-01 | 两台真设备 + 真 TCP WebDAV：A 把甲移到乙下面、B 把乙移到甲下面，各自追平之后 —— **两台盘上的父子关系都不许成环**；子树查询必须能返回且把两个子夹都算进来；追平之后本机移动文件夹仍要可用 | `notera-host/tests/folder_cycle.rs::two_devices_moving_two_folders_into_each_other_never_create_a_cycle`（环的判定不借道被测的那条 CTE，而是自己在内存里沿 parent 走并记 visited —— 否则"读侧也一起坏了"会被当成通过） | L3 | P0 | **先写出来是红的**：未修时第一次跑就红在"A 这台设备被同步写出了一个文件夹环"（0.3 s，不靠超时）。修法是 `apply_folder` 认对面那一版的**其余字段与 rev**、唯独**不写那个会成环的父**（保留本机现在的父），于是这一行照样 settle、不每轮重推。变异自证 **M75**：把那一支守卫短路掉 → 同一条红回原处。判据故意含"能返回"那一句：只断言"没有环"的话，读侧那条不收敛的 CTE 就没人守 |
| CY-02 | 库上**已经**有环（老版本写坏的现场）时，子树查询不许卡死，也不许把错抛给界面 | `notera-store/tests/folders_and_fk.rs::a_folder_cycle_already_on_disk_never_eats_the_subtree_query` —— 环是直接改表搓出来的（产品写入路径现在已经会拒绝，模拟的正是旧版本留下的库），查询放进**线程 + 5 s 超时**里收 | L1 | P0 | **已实现并通过**。钉的是 `descendant_ids` 用 `UNION`（按 id 去重）而不是 `UNION ALL`。变异自证 **M76**：改回 `UNION ALL` → 这条在 5.11 s 处**红**（红话就是"这就是界面上的'点了没反应'"）；**为什么必须放线程里收**：直接在测试线程调，卡死的实现会把整个测试进程一起挂住 —— 那就不是判据而是超时。还原 → 该文件 8/8 全绿 |

## 同内容收敛（§1.2，2026-09-29）

| ID | 判据 | 落在哪 | 层级 | 优先级 | 状态 |
|---|---|---|---|---|---|
| SY-CONV-01 | 两台真设备把同一篇笔记各自改成**同一份内容**（A 连改两次 rev 3、B 改一次 rev 2）→ 不许发冲突卡片，且**追平几轮之后本机必须没有脏行、没有待发操作**（`rev == sync_rev`），否则设置页那句"待处理任务"永远不掉 | `notera-host/tests/same_content_convergence.rs::identical_final_content_on_both_sides_converges_and_leaves_nothing_pending`；规划层另两条：`plan.rs::p7_identical_content_is_not_a_conflict`（本地低 → Pull、本地高 → Push、rev 相等 → NoOp 三种形状都有数）、`p7_converges_with_the_hash_shapes_production_actually_uses` | L3+L1 | P1 | **先写出来是红的**：未修时六轮之后仍 `dirty_notes=1 / outbox_pending=1`（这条是拿真设备**问系统**问出来的，不是从代码推的 —— 清扫报告给的是推断，采信之前先做实验）。修法是 P7 按 rev 高低退回 Push/Pull，外加"对面那一版落地后结掉本机未走完的上写待办"。变异自证 **M77**（P7 退回 `NoOp` → 红在 `dirty=1 outbox=1`）与 **M78**（去掉那一支结清 → 红在 `dirty=0 outbox=1`）：**两种红话不同，所以两半各自有独立证据**。失败消息故意把 outbox 按状态拆开发（`Pending=… Inflight=… Failed=…`）—— 就是这么发现我自己写的 `op = 'up'`（真值 `'upsert'`）匹配了 0 行 |



