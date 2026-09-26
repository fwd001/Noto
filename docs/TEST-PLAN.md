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
| `OFF` / `ON` | `POST /_control/stop` / `/_control/start`；`OFF(close-listener)` 模拟服务器不可达，`OFF(进程 kill)` 模拟主机消失 |
| `RESTART` | `fs` 模式下重启服务器进程（保留权威状态），用于断言客户端不依赖服务端内存态 |
| `DUMP` | `GET /_fs/dump?prefix=.notes/` → 服务端权威文件清单 + 内容 hash（**唯一允许的服务端状态断言手段**） |
| `STATS` | `GET /_control/inspect` → 请求序列（method/path/请求头/字节数/响应码），用于断言"先写记录再写 manifest"、请求数、字节数、幂等重放次数 |
| `RESET` | `POST /_control/reset`，回到空 root prefix |
| `CRASH(点)` | 令被测进程在指定提交点前 `process::exit(9)`（非优雅），等价 `taskkill /F` |
| `A/B/C sync` | 在隔离数据目录运行的三个 `notera-cli` 实例各执行一次 `sync-once` |

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
| FT-ATT-02 | 已有附件 | 在设备 B 打开同一条笔记 | 附件按内容寻址取回并渲染；本地 sha256 校验通过；不产生第二份副本（同 sha256 只落一个文件） | L3 | P6 |
| FT-ATT-03 | 20 MiB 附件，链路 `FAIL(latency,target=attachments/**)` 限速 1 Mbit | 上传附件的同时编辑并同步另一条纯文本笔记 | 文本笔记在设备 B 于 ≤25 s（设计周期）内可见；附件队列未完成不影响文本轮（`STATS` 显示文本轮请求不含 `attachments/**` 等待）；UI 不出现整体阻塞（输入延迟 <100 ms） | L3,L5 | P6 |
| FT-ATT-04 | 上传中断（`FAIL(abort,target=attachments/**)`） | 下一轮同步 | 附件在后续轮重试成功；服务端仅存在 `.tmp-*` 残留，`DUMP` 中无残缺正式对象；引用该附件的 manifest 不出现（INV-09） | L3,L4 | P6 |
| FT-ATT-05 | 删除含附件笔记 → 清空回收站 | `DUMP` + 检查 tombstone | `purged` 置位；tombstone 不被自动 GC（INV-11） | L3 | P6 |
| FT-CHK-01 | 新笔记 | 建 checklist 3 项，勾选第 1、3 项 | 重开后勾选状态为 `[x][ ][x]`；纯文本抽取输出与该状态一致 | L1,L5 | P2 |
| FT-CHK-02 | 同 checklist | 设备 A 勾第 1 项、设备 B 勾第 2 项（同一 base） → 双向同步 | 结果为两项都勾（块级三方合并不丢勾选）或产生冲突副本且两份内容完整可见；**禁止出现"只剩一项勾选"的静默覆盖**（INV-01/05） | L3 | P5 |
| FT-CHK-03 | checklist 中间项 | 在第 2 项内换行 / 删除整项 | 项序连续无空项；重开后条目数 = 操作后预期数 | L5 | P2 |

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
| FT-IO-01 | 500 条笔记 + 20 附件 | 导出完整包 → 在干净设备导入 | 条目数 500、文件夹树结构一致、每篇正文 canonical JSON hash 一致、20 附件 sha256 一致；固定/回收站状态按导出规格保留 | L1,L5 | P6 |
| FT-IO-02 | 同上 | 导出 → 导入到**已有同名数据**的设备 | 明确策略结果可观测：不覆盖已有内容，冲突条目以副本形式并存（无静默覆盖，INV-01）；导入报告"新增 X / 冲突 Y / 跳过 Z"，X+Y+Z = 500 | L1,L5 | P6 |
| FT-IO-03 | 干净设备 | 导入被截断的包（尾部 1 字节删除）/ hash 不符的包 / 含未知节点类型的包 | 前两者：整体失败或按报告逐条失败，本地不出现半包状态，原库条目数不变（INV-08）；第三者：成功导入且未知节点原样保留（FWD-01 同源） | L1 | P6 |
| FT-IO-04 | 有备份文件 | 备份 → 修改 30 条 → 恢复备份 | 恢复后为备份时点状态（条目数与 3 类 hash 精确匹配）；恢复前自动生成"恢复前快照"并可回退；恢复不改变远端（不自动推送恢复结果，除非用户触发） | L1,L5 | P6 |
| FT-IO-05 | 恢复进行中 | `CRASH`（CP-09） | 重启后**要么**旧库完整可用**要么**新库完整可用，不存在混合状态；原库文件未被破坏（hash 或条目数可核对） | L4 | P6 |
| FT-IO-06 | 从外部格式导入（Markdown/纯文本各 1 组） | 导入含中文文件名、CRLF、BOM、无扩展名的文件 | 每篇正文可读、条目数等于源文件数；文件名冲突时自动改名而非覆盖；0 字节文件按报告跳过 | L1 | P6 |
| FT-SETUP-01 | 全新安装 | 配置 WebDAV URL + 账号密码（含自签 CA） → 测试连接 | 成功时有可见确认；失败时提示区分 DNS / 拒绝连接 / TLS 不受信 / 401 / 403，不得只给"网络错误" | L5 | P4 |
| FT-SETUP-02 | 已配置 | 改为错误密码 → 同步 → 改回 | 错误期间本地不受影响且保留配置；恢复后一轮内追上；失败不删远端任何对象（`DUMP` 前后一致） | L3,L5 | P5 |

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
| SY-INT-10 | 落后设备超过变更窗口 | 预置 A 落后：远端已有 200+ 条变更（实测 200-entry 窗口 ≈16 KiB raw / 7.8 KiB gzip），A 的 last-sync rev 早于窗口起点 | `A sync` | 自动降级为"全量 manifest 拉取"（而非静默丢变更）；同步后 A 条目数与 B 完全一致；`notera-cli diag` 报告窗口溢出原因；无实体被跳过。INV-05,08 | L1,L3 | P7 |

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
| PERF-01 | 冷启动到首帧可用（空库 / 5000 条） | 暂定 ≤800 ms / ≤1500 ms | L5：从进程创建到列表首帧（Playwright 首条列表项可见 + 截图时间戳）；Windows x64 release | 待建基线 |
| PERF-02 | 搜索 p50 / p95（≥3 字 CJK，5000 条） | p50 ≤60 µs、p95 ≤2000 µs | L1 直测；实测基线 **p50=32 µs / p95=1404 µs**，FTS 索引 **6.0 MiB**（PROBE:`fts5-search-5000-notes`） | 已实测（阈值含回归容差） |
| PERF-03 | 1–2 字 CJK 查询延迟 | 暂定 ≤15 ms @5000 条 | L1；实测 content 表 `LIKE` **416/5000 → ~4.1 ms**（FTS 列 `LIKE` 5.8 ms，**因此兜底必须走 content 表**） | 已实测 |
| PERF-04 | FTS 索引体积（5000 条 CJK，trigram） | ≤6.5 MiB | L1 `PRAGMA`/文件大小；实测 6.0 MiB | 已实测 |
| PERF-05 | no-change 轮：请求数 / 字节 | ≤2 请求、总传输 ≤2 KiB、manifest 返回 304 | L3 `STATS`；PROBE:`precondition-412-plumbing` 证明 304/412 可作为状态被引擎重规划 | 待建基线（机制已实测） |
| PERF-06 | 单轮变更同步字节（200 条变更窗口） | ≤8 KiB gzip | L3 `STATS`；实测 **16 KiB raw / 7.8 KiB gzip** | 已实测 |
| PERF-07 | 全量 manifest（5000 条 / 20000 条） | 5000：≤190 KiB gzip；20000：暂定 ≤800 KiB gzip | L3 `STATS`；实测 **89 B/条**、5000 条 **438.7 KiB raw / 186 KiB gzip**、2000 条 seg **175.6 KiB raw / 74.7 KiB gzip**（→ 按 seg 分片外推 20000） | 5000 已实测 / 20000 待建基线 |
| PERF-08 | 变更在设备间传播（桌面） | ≤30 s（设计周期 25 s） | L3 端到端时间戳；L5 真机复核 | 已定阈值，待实测 |
| PERF-09 | argon2id（m=19 MiB / t=2 / p=1） | 单次 ~370–400 ms，且 **UI 线程零占用** | L1：主线程在 KDF 期间每帧阻塞 ≤16 ms（PROBE:`kdf-argon2id` = 19MiB/2it/1p → 400 ms，deterministic=true） | 已实测 |
| PERF-10 | 内存上限（桌面，20000 条 + 100 附件索引） | 暂定 RSS ≤450 MiB；单轮同步峰值增量 ≤50 MiB | L5 采样（30 min 使用 + 5 轮同步），增长趋势 ≤5%/轮以排除泄漏 | 待建基线 |
| PERF-11 | 附件不阻塞文本 | 附件在飞时文本轮 ≤25 s 到达对端；`STATS` 证明文本轮不等待 `attachments/**` | L3 + `FAIL(latency,target=attachments/**,ms=…)`；PROBE:`timeout-and-cancel` 证明取消语义真实可用 | 待建基线（机制已实测） |
| PERF-12 | 自动保存落盘频率 | ≤3 次 / 20 次快速输入；输入到落盘 ≤1500 ms | L1 fake clock；暂定 | 待建基线 |
| PERF-13 | 列表滚动 / 大数据量渲染 | 20000 条列表滚动无 >50 ms 掉帧（暂定），首屏 ≤1 s | L5 采集（CDP trace） | 待建基线 |
| PERF-14 | 网络层出站请求数（一轮） | 与 `DUMP` 差分一致：无多余 PROPFIND（每轮 ≤1） | L3 `STATS` | 待建基线 |
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
| Q8 | 若干性能阈值为暂定值 | 已实测项仅覆盖搜索、manifest 体积、AEAD/argon2、TLS、超时；冷启动、RSS、列表渲染、no-change 轮字节、Android 冷启动无实测支撑 | PERF-01/05/10/12/13/14/15 在建立基线前不能作为放行依据（但不可删除） | 允许在 P3/P7 各跑一次基线采集并回填（含 20000 条规模的真实测量） |
| Q9 | 未同步/冲突之外的删除保留策略与 purge 语义 | 已给事实为"tombstone 永不自动 GC"，但"用户主动清空回收站 / 永久删除后远端对象与附件何时真正回收"未定 | FT-NOTE-05/06、FT-ATT-05 的断言只能停在"purged 置位 + tombstone 保留"，无法验证存储回收 | 决定 purge 的触发者与安全性条件（是否需全设备确认、是否有安全窗口），再补对应不变式 |
| Q10 | 附件与记录的加密边界 | 已给事实为"记录信封 aes-256-gcm-siv"；附件（内容寻址、immutable）是否加密、加密是否影响 sha256 去重与跨设备复用 | FT-ATT-01/02、CM-SV-03 无法断言"附件在远端为密文"这一层；可能存在"内容 hash 泄露元数据"的安全疑问 | 明确附件是否加密、hash 取明文还是密文、以及信封字段（`enc`/`ct`）对附件的取值 |
| Q11 | `notera-test-webdav` 控制 API 的字段名与语义细节 | 已给事实只到 `/_control/*` 支持 start/stop/reset/inspect/inject-failure 与 `/_fs/dump` | §记法 中的 `FAIL(kind,target,times)` 为**测试计划统一记法**，与实现签名可能有差异；L3/L4 断言写法需按实现校正 | 在 P0 末冻结控制 API schema（并把本表与实现逐条对齐） |
