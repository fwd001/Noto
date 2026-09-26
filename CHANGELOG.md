# Changelog

遵循 [SemVer](https://semver.org/lang/zh-CN/)。同步协议发生破坏性变更时，`SYNC_PROTOCOL_VERSION` 与版本号同步升级（见 CI-CD.md §版本与单一版本源）。

## 0.1.0 — Phase 1–4（实现进行中）· 未发布

当前测试基线（2026-09-26 本机 GNU 工具链实测）：

| 门禁 | 结果 |
|---|---|
| `cargo test --workspace` | 441 通过 / 0 失败 / 0 ignored（50 个测试二进制） |
| 前端 | 162 通过（15 文件）；`vue-tsc --noEmit` 无错误；构建 206 KB → gzip 70 KB |
| `scripts/arch-check.mjs` | 23/23 |
| `scripts/verify-diagram.mjs` | 59/59，交互后无运行时错误 |
| `scripts/verify-app.mjs`（浏览器端到端，真 Rust 核心） | 32/32 |
| `scripts/verify-tauri-window.mjs`（真窗口，走真 `invoke`） | 8/8，控制台 0 error |

复现命令见 `docs/ARCHITECTURE-MAP.md` §8。

### 新增

- **存储层** `notera-store`：11 张表 + FTS5（trigram）迁移 0001…0006、写连接单写者 + 只读连接池、tombstone、outbox、冲突收件箱、附件内容寻址、偏好读写与记录 wire 出口
- **同步引擎** `notera-sync`：清单两段式解析与压实、`P1..P18` 判定表、退避重试、CAS 提交与恢复阶梯；端口化（`LocalPort`/`RemotePort`），10 个引擎级集成测试跑真 `run_round`
- **网络出口** `notera-net`：全系统唯一 HTTP 出口，代理四档、TLS 策略、分层超时、退避、`RouteProof` 脱敏审计（13 测试）
- **测试基建** `notera-test-webdav`：真 TCP/HTTP 的 WebDAV 子集 + `/_control/*` 能力开关与故障注入 + `/_fs/dump`
- **前端** `apps/desktop`：三栏 UI、独立富文本模型映射、四态同步徽标、design token 与对比度契约测试、Tauri 单命令通道及其契约测试
- **WebDAV 适配器** `notera-webdav`：`RemotePort` 的生产实现，S1/S2/S3 写入策略与中途降级、清单 CAS（tmp → 让位 → 落地 → 复算）、目录穿越白名单闸门；33 个测试里有两条是**跨设备真同步**与**重启后仍在**
- **导入器** `notera-importer`：Markdown/纯文本 → 富文本，无损优先（不认识的标记按字面保留）、三道入口闸门、按内容哈希幂等；64 个测试
- **诊断入口** `notera-cli`：`serve`（dev 桥，落真实 Store）、`verify`、`conflicts`、`sync-once`、`dav-probe`、`net-probe`、`export`、`backup`；退出码 0=PASS / 1=ASSERT_FAIL / 2=BLOCKED。`sync-once` 走的是 `App::sync_once` —— 与产品调度器**同一条**代码路径，否则这里的绿灯和产品无关；`export` 写完立刻回读校验包内容，"文件存在"不当作"数据可恢复"
- **§5 能力探测接线**：`App::remote_for_sync()` = 先按需探测、再装适配器，探测结果因此对**本次会话**的写入策略生效（先装后探就要等下次启动）；`cap_mask IS NULL`（从未探测，用保守默认）与 `= 0`（实测全不支持，落到 S3）严格分开；探测未完成时不写位图、只提示 `sync.probeDeferred`，同步照常 —— 猜低的代价是盲写覆盖，猜高最坏只是 412 后就地降级
- **ADR-0018**：单一活跃同步账户约束（多服务器推迟到"按账户确认点"）
- **备份 / 恢复**（DATA-MODEL §15）：`VACUUM INTO` 一致快照 + `sha256`/`integrity_check`/`user_version` 三道闸门 + 替换前留一份当前库 + 下次启动落地（不在进程内换库）。零新依赖
- **导出 / 导入**：自描述 ZIP（`manifest.json` + `folders.json` + `notes/<id>.json` + `tombstones.json` + `attachments/<sha>`）。记录由**与上传同一套**信封构造函数产出，导入因此直接喂 `apply_remote` —— 没有第二条写入路径，"防复活"就没有第二个会漏的地方
- **编辑器原生感**（对齐 AppFlowy）：Markdown 输入缩写、`/` 命令面板、块把手（拖拽重排 + 下方插入 + Alt+↑↓）、选中文字浮出小工具条
- **ADR-0019**：Joplin 同步与 AppFlowy 编辑交互的逐条采纳/拒绝及其理由

- **设置页显示 §5 的判定**：`AccountDto` 增加 `capMask`/`writeStrategy`/`capsProbedAt`，设置页"服务器能力"块分三种说法 —— 还没探过 / 有并发保护（S1·S2）/ **没有并发保护（S3，建议多设备串行编辑）**，外加五项能力的 ✓✕ 芯片。策略由核心算好下发，前端不许自己从位图反推（`sync/serverCaps.ts` + 7 条测试）
- **§11.4 尽力而为租约**：规范里有第三层并发保护（`locks/<device>.json`）却从没写过细则、也没人实现。先补规范（谁写、什么时候让路、TTL、读不到怎么办、以及它明确**防不住**什么），再接端口与实现。开关只看 §5 的结果：**S3 或探不到强 ETag 才开**，CAS 可信的服务器一个多余请求都不发。引擎在轮次开始贴自己那份，在**提交清单之前**看别人新不新鲜：新鲜就不公告，改动保持 dirty、状态显示"另一台设备正在写入"，下一轮自动重来。`sync_state.lease_token/lease_expires_at` 两列自 0002 起第一次真正被写入

### 修掉的静默错误（都是"看着在用、其实没接线"）

- **清单公告失败的那一轮，改动已经被标成已同步了**：引擎在每条记录 PUT 成功后立刻 `MarkSynced` + outbox `Done`，可清单要等本轮最后一步才提交。CAS 失败、网络在最后一步断掉、或（新增的）租约让路 —— 这批实体就"本地已同步、服务器公告板没有"，别的设备**永远**看不见它们，而崩溃恢复矩阵 §11.3 C4 写的恰恰是"清单重放"。现在标 synced 与结清 outbox 都挪到清单提交成功之后，让路/失败的轮次一律保持 dirty 等下一轮重发。这条是写租约时顺带发现的真丢数据路径，比租约本身更要紧
- **设置页的"保存服务器"从来没成功过**：前端把草案包成 `{draft:{…}}` 而核心的命令参数是**平铺**的，`tlsPolicy` 发的是 `{kind:'caBundle'}` 对象而核心要 `"ca_bundle"` 字符串，回填又按 `account.proxy.host` 读嵌套而核心发的是 `proxyHost`。三处不一致叠在一起：点保存回一句 `bad_args`，而且即便存成功，改过的 TLS/代理设置也会在下次打开页面时静默变回默认。现在整条边集中到 `sync/accountWire.ts` 一份翻译 + 11 条线格式契约测试；顺带让核心把 `username` 回发（它不是秘密，而不回发就意味着改一次设置要重填用户名，漏填还会把配置静默退回"需要凭据"）。端到端新增一步真的走"填表→保存→读回→清理"
- **架构门禁里有 8 条一直在空转**：`scripts/arch-check.mjs` 的 `sources()` 用 `statSafe(dir)` 当入口守卫，而 `statSafe(p)` 默认判的是"这不是目录" —— 于是每次遍历都在第一行返回空表，`layer:sql-literal`（host/UI 不得写 SQL）、`layer:ui-protocol-vocab`（前端不得出现协议词汇）、`egress:raw-socket`、`edge:webdav-uses-only-ports`、`hygiene:ui-no-node-apis` 等 8 条**全绿但什么都没看**。修好之后立刻炸出两条真实越界：`notera-webdav` re-export 了 `SyncEngine`（引擎入口该只有 sync 一处，已删），以及核心错误词表有 8 个 messageKey 前端没登记。这条是"绿灯不等于检查过"的最坏样子
- **门禁自己不会说"我没检查到东西"**：修好 `sources()` 之后加了两条自我约束 —— `hygiene:no-vacuous-source-scan`（任何源码扫描扫到 0 个文件即判失败）与"按名字取源码目录"（桌面壳在 `apps/desktop/src-tauri/src`，不在 `crates/` 下）。后者一上来就抓到 `egress:raw-socket` 从来没扫过壳代码：它扫的是不存在的 `crates/notera-desktop/src`
- **核心错误词表里 8 个键没登记，提示全部退化成"操作没有成功"**：`sync.forbidden / sync.precondition / sync.unsupported / sync.divergence / sync.cancelled / app.db_too_new / attach.missing / proxy.cert_untrusted` —— 都是会直接讲给用户的话（"服务器拒绝了这次写入"和"操作没成功，稍后再试"完全不是一回事）。现在补进 `i18n.ts`，并新增门禁 `hygiene:rust-message-keys-registered` 把这条边钉住（双向变异测过：改 Rust 侧键名或改登记表都会变红）
- **不带 id 的账户草案会配出"永不出站"的账户**：`upsert_account` 遇到空 id 会自己生成一个存进配置，host 却继续拿**空串**去 `register_account` —— 于是 `sync_accounts` 里那行的键和配置里的键根本不是同一个，outbox 按账户扇出时找不到目标，用户看到"已配置、已同步"而一个字节都没出去。是 §5 的端到端测试第一次跑起来时以"账户不存在: <uuid>"炸出来的；现在 id 在装配前就定下来，两处用的是同一个值
- **动态拼出来的文案键把键名直接印到界面上**：`MessageKey` 只是 `string` 别名，`t()` 查不到就原样返回 —— 工具条上写着 `editor.blockCodeBlock`，设置页写着 `settings.rootPrefix`。改成查表，并加两层门禁（扫源码的键登记测试 + 端到端看渲染文本）
- **两条自动保存并发出发，自己造出一条假"在别处被改动了"**：`save()` 无互斥，防抖那一发和 blur 的 flush 带着同一个 `expectedRev` 同时上路，先回来的把 rev 推进、后回来的被核心判成 stale_edit。核心没错，错在前端让自己的两个保存互相打架
- **校验备份这一步破坏了被校验的备份**：`pool::open_readonly_conn` 其实不是只读，会跑 `PRAGMA journal_mode=WAL` —— 把待校验的文件就地改写并留下 `-wal` 边车，于是 sha256 永远对不上、恢复必然失败
- **导出会正好盖掉刚才的备份**：备份产物被回填进唯一那个路径框，下一次"导出"就写在那份 `.sqlite` 上。现在输出/输入两个框分开，且导出遇已存在文件一律不覆盖
- **`caps` 报的是"这个平台原则上能做到什么"而不是"壳里做了什么"**：Windows 下 `tray/global_shortcuts/native_menu/notifications` 全报 true、`keychain` 报 `credential_manager`，而壳里一行相关代码都没有 —— 设置页因此摆出"关闭窗口时留在系统托盘"这种存了没人读的开关，并让用户误信口令已进钥匙串。现按 as-built 报 false/none
- **端到端门禁漏掉整类 4xx**：只盯 `requestfailed`，而本地桥把业务拒绝映射成 HTTP 400 → 真错误（上面那条 stale_edit）被当成功放过
- **待办结清用错了 kind 词汇，每一行都永远停在 inflight**：引擎交给适配器的 `kind` 是**线上短标记**（`n/f/a`，它同时是远端路径 `/.notes/n/<id>.json` 的一段），而 `sync_operations.entity_type` 存的是长标记（`note/folder/attachment`）。`outbox_settle` 拿短标记去 UPDATE：匹配 0 行、返回 `Ok(())`、留一条 warn，于是设置页的"待处理任务"永远不掉、`sync_operations` 只增不清、崩溃恢复也判断不了从哪重放。修法是把参数类型从 `&str` 改成 `EntityKind` —— 词汇翻译从此是**编译期**的事，store 内部只认一种词汇，host 适配器用 `EntityKind::from_tag` 显式翻并认不出就放弃（绝不"猜一行"标完成）。**store 的单测当时是绿的**，因为它自己传的就是长标记：单测用错词汇不会失败，只有真跑一轮才会。顺带把"待同步"的口径定清 —— 只统计**启用中的账户**，本地哨兵账户（`enabled=0`）是"提交即入 outbox"的留痕账、引擎永不消费它，把它算进来这个数就永远归不了零，长得像同步卡死（本地有没有未上传改动由 `dirty_notes` 表达，两者不混）
- **设置页的"最近删除 / 本地占用 / 待处理任务"恒为占位符 `—`，侧栏回收站恒为 0**：`App::stats` 是命令面里唯一没走 DTO 的一条 —— 它把 `notera_store::StoreStats` 原样序列化，wire 上是 snake_case 的 `notes_trash / fts_rows / outbox_pending`，而契约图与前端读的是 `notesInTrash / ftsEntries / inflightOps`，于是全是 `undefined`。TypeScript 的类型是断言不是校验，`formatNumber(undefined)` 很诚实地给出 `—`；而 `app.spec.ts` 喂的 mock 恰好是 camelCase —— 假数据把这个洞完整盖住了。现在命令面补 `StatsDto`（8 个契约字段 + `From<StoreStats>` 一处翻译），三处一起钉住：Rust 用真 `dispatch("stats")` 断言键集合、arch-check 新增 `edge:stats-dto-covers-ui-reads`（扫前端每一处 `settings.stats.X` 与 `StoreStats` 声明，核心发不出就判红；删掉 `#[serde(rename_all)]` 或改任一个字段名都会变红，两条都实测过）、端到端新增一步在浏览器里断言这五行全是数字
- **通用门禁 `edge:command-wire-is-camelCase`**：上一条是逐键核对，这一条把整类挡掉 —— 命令面每一个 `j(app.x()?)` 出参的结构体都必须**显式**声明 `#[serde(rename_all = "camelCase")]`，否则判红并指名是哪个命令、哪个类型。审计顺带查了另一处同源风险（`backup_db`/`list_backups` 发的是 store 的 `BackupInfo`），它本来就带这个属性所以界面没坏；门禁的作用是不许下一个人在没注意的时候漏掉它（变异测过：删掉那个属性 → `backup_db → BackupInfo（缺 #[serde(rename_all = "camelCase")]）` 立刻变红）
- **导出说"含附件"，包里一个附件都没有**：附件按 DATA-MODEL §5.1 落在 `<attachments>/<2hex>/<sha>` 两层分片目录里，而导出用的是"读一层目录、按 64hex 筛文件名"—— 那一层里只有 2 字符的分片目录名，所以过滤器**永远筛不到任何东西**。后果不是报错而是安心感被偷走：`include_attachments: true`（设置页写死 true）、`notera-cli export`、以及备份语义都指向"这份包里有我的文件"，实际交出去的是一个只有文字的空壳，而且回读校验只检查"包能不能打开"，于是 `附件 0` 也照样算通过。改成以库为准（新增 `Store::local_attachment_shas()`），并补三处判定：命令面测试真挂一个 blob 并断言包里的条目数/字节（变异测过：换回旧的扫目录写法 → `left: Some(0) right: Some(1)`）、CLI 导出后**比对数量**而不只是打开、端到端在统计卡与包之间对账。顺带发现：编辑器"插入图片/附件"这条 UI 入口至今没有文件选择器接上（`attach_file` 需要 `localPath`/`mediaType`，前端只发 `{noteId, blockId, role}` → 必然 `bad_args`），失败路径本身是干净的（占位块会撤掉并提示），但功能确实不可用 —— 见 §尚未做
- **编辑器的"插入图片 / 附件"接通了**（D11 拍板走 ③：前端 `<input type=file>` 读字节 → base64 交给 `attach_file`）。这条边整条是断的：核心要 `localPath` + `mediaType`，前端只发 `{noteId, blockId, role}` → 点一下必然 `bad_args`，而且没有任何测试走过它。现在形状集中在 `editor/attachmentWire.ts`（一处 + 11 条契约测试：载荷必须平铺 camelCase、超限在**读字节之前**就拒、base64 与 RFC 4648 已知答案逐字符对齐、1 MB 编码 < 1.5 s），核心仍然是唯一的写入口（sha256、落盘、`attachments`/`note_attachments`、上传队列）。显示用的 data URL 只活在内存表里，**绝不写进块属性** —— 那等于把每个附件在正文里再存一份 base64 并跟着每次编辑同步走（端到端有断言盯着）。新增命令 `attachment_data`（按 sha 取回字节）；`sha256` 参数先校验形态再用，因为它会被拼进 blob 路径，不校验就是给 `../../` 开门。零新依赖（`base64` 早已在 workspace 单一版本源里，经 `notera-crypto::b64` 用）
- **插一张图，却被告知"这条笔记在别处被改动了"**：核心首次挂附件会翻转派生列 `has_attachment`，而那是**在同一事务里推进笔记 rev** 的动作；编辑器排队的自动保存还带着旧 rev 出发，于是被判定 `stale_edit` —— 界面切走、本地版本进 draft，用户完全看不出是自己干的。这就是之前那条"偶发一次、连跑三次全绿"的端到端红灯：给门禁补上"失败的 4xx 发生在哪一步"的归位信息后，新的附件步骤一复现就是它（`expected 7, actual 8`）。修法是把顺序钉死并在命令面回带新 rev：先落自己的编辑 → 核心写附件 → 接住 `attach_file` 的 `rev` → 才把附件块写进正文；失败则把占位块撤干净且**不**多存一版。核心侧与前端侧各一条测试锁住这个顺序
- **另外 5 个命令错误码没有登记文案**（`no_default_folder` / `bad_action` / `sync_refused` / `sync_busy` / `unknown_account`）：新门禁 `hygiene:rust-error-codes-registered` 从 Rust 侧扫 `CmdError::of("…")` 与 `error.*` 表比对，一上来就炸出这五个 —— 它们此前全体退化成"操作没有成功，可以稍后再试"。这条门禁是从**源头**扫的，不再依赖前端那张手抄的对照表（`read_failed` / `attachment_missing` 也正是手抄漏掉的）
- `WorkspaceView` 不跟随 `selectedId` 打开编辑器（选中了却一片空白）、`create()` 不打开新笔记、冲突动词表三处不一致、`create_note` 拒绝 `folderId: null`、`/favicon.ico` 404
- **架构适应度检查** `scripts/arch-check.mjs`：23 条机器可判定的层次约束（依赖边、唯一出口、SQL 只出现在 store、前端无协议词汇、端口边越界引用、命令面 DTO 覆盖界面读的每一个键…）
- **端到端等价** `scripts/verify-app.mjs`：Playwright 驱动同一份前端 + 同一份 Rust 核心的 32 步 UAT

### 修复（都是会静默丢数据或静默错的那些，不是整理）

- 同步轮次在"本地有改动 + 远端 304"路径上不公告变更 → 改动永不上传
- `dedupe_key` 不含账户，导致本地编辑只同步到其中一台服务器
- 只读连接借出前不确认 autocommit，WAL 下把旧快照钉住（同一 `COUNT(*)` 两次结果不同）
- 附件 `CHECK (sha256 GLOB '[0-9a-f][0-9a-f]')` 把长度限成 2 字符，任何真实哈希都插不进
- `ProxyProfile` 的 `#[derive(Default)]` 与 serde 默认值不一致（`resolve_remote_dns`）
- dev 桥的 Origin 校验用前缀匹配，`http://127.0.0.1.evil.example` 可通过
- `Tauri` 壳配置里 `bundle.targets` 含协议外的取值，构建脚本直接失败

### 已知限制（明确记为 BLOCKED / 待决，不当作已完成）

- devserver 本地 HTTP 桥尚未关进 `debug_assertions`（`arch-check` 现在诚实报红）
- 两台服务器同时启用不支持，见 ADR-0018
- macOS/Android/iOS 产物、签名与真机后台同步预算未在本机验证（`[BLOCKED]` 需要对应硬件与证书）
- `ARCHITECTURE-REVIEW.md` §14 的 D1–D10 仍待人工决定


## 0.0.0 — Phase 0（架构）· 未发布

本阶段**不产出可运行软件**，只产出未来不易被推翻的工程蓝图。

### 新增

- **架构文档集**：`ARCHITECTURE.md`、`ARCHITECTURE-MAP.md`（长期架构记忆）、`DATA-MODEL.md`、`SYNC-PROTOCOL.md`、`CONFLICT-RESOLUTION.md`、`PROXY.md`、`PLATFORM.md`、`TEST-PLAN.md`、`CI-CD.md`
- **ADR-0001…0017**：重大架构决策逐条留痕（0016/0017 为 Proposed，待人工决定）
- **可交互架构契约图** `docs/diagram/architecture.html`：16 个模块 × 输入/输出/数据结构三栏契约，支持点击穿透、四条链路筛选、契约总表、跨命名风格字段搜索；单文件零外部依赖，可离线打开
- **技术可行性探针** `tools/feasibility-probe`：16 项实测全部通过，输出归档于 `docs/evidence/`
- **图验证脚本** `scripts/verify-diagram.mjs`：59 项浏览器实跑断言（几何、标签重叠、连线穿越、对比度、命中区、移动端溢出、筛选态边数与数据模型一致性）
- 仓库骨架：`.gitignore`、`.gitattributes`（仓库内强制 LF）、`.editorconfig`、目录结构

### 关键设计决策（摘要）

- 记录文件是同步正确性的来源，清单（manifest）只是索引与公告；写入顺序固定为 实体 → 附件 → 清单
- 清单采用两段式（基线分段 ⊕ 变更窗口），使每轮同步成本与笔记库总规模解耦（实测窗口 200 条 gzip 7.8 KiB vs 全量 5000 条 186 KiB）
- Revision 采用 Lamport 式单调整数 + 内容哈希 + `sync_rev` 共同祖先点，不使用向量时钟，禁止以 mtime 判定新旧
- Tombstone 不自动 GC，且文件夹删除不级联删除笔记 —— 从协议层排除"已删笔记复活"
- 冲突先做块级三方合并，失败则保留双方并生成冲突副本，永不静默覆盖
- 富文本为编辑器无关的独立模型，未知节点 preserve-unknown，版本超前则只读
- 记录信封 v1 即预留 E2EE 结构（`enc.alg="none"`），未来启用加密是次版本而非数据重写
- 检索双路径：≥3 字符走 FTS5 trigram `MATCH`，≤2 字符走 `LIKE`（实测短查询 MATCH 静默返回 0 行）
- 所有 HTTP 出口唯一收敛于 `notera-net`，App 级代理，配三条可证伪证据链
- 移动端不承诺 30 秒后台到达，真实承诺为"打开设备时已同步"

### 已知阻塞（不隐藏）

| # | 项 | 解除条件 |
|---|---|---|
| B1 | 本机无 MSVC 链接器（`link.exe` 被 Git coreutils 顶替） | 装 VS Build Tools，或长期采用 GNU host（需先做 Tauri-on-GNU spike） |
| B2 | ~~本机无 WebView2~~ **已纠正**：独立运行时装在 `Microsoft\EdgeWebView\Applicationh.0.4078.105` | 无需解除，桌面可本地验证 |
| B3 | 本机无 JDK / Android SDK / NDK | 安装工具链或全部交 CI 构建 |
| B4 | `github.com` 本机不可达，Actions 无法观察 | 提供可访问 GitHub 的推送通道 |
| B5 | 无真实 WebDAV 端点可做兼容矩阵 | 用户提供内网 + 公网各一个 |
| B6 | iOS 出包需 macOS + Xcode + 证书 | Phase 5 + 证书决策 |
| B7 | 代号 `Notera` 未做商标/域名核查 | 品牌定名 |

### 下一步

Phase 0 输出 Architecture Review 并**停止**，等待人工审核。审核通过前不进入 Phase 1（Local Core）。
