# IMPLEMENTATION-STATUS

> 总指令 §44 要求的那份验收状态表。**状态只能取**
> `PLANNED / IMPLEMENTING / VERIFIED / BLOCKED / DONE`；
> `VERIFIED` 一律带**可重放的证据出处**，没有证据就往下写。
> `SKIP / TODO / 理论通过 / 应该没问题 / 本地没环境所以跳过` 在这份表里不算 PASS。
>
> 更新时间：2026-09-27 · 本轮新增门禁：千库规模追平（`big_library`，SY-INT-14） · 本机工具链 `stable-x86_64-pc-windows-gnu`

## 门禁总览（今天实测）

| 门禁 | 结果 | 出处 |
|---|---|---|
| `cargo test --workspace` | 487 通过 / 0 失败 / 0 ignored（58 个测试二进制） | 本机 |
| `cargo clippy --workspace --all-targets -- -D warnings` | 0 / 0 | 本机（CI-CD 原样命令） |
| `cargo fmt --all --check` | 退出码 0（`rustfmt` 组件已装；全仓已格式化） | 本机 |
| `node scripts/arch-check.mjs` | 26/26（第 26 条 = 版本单源，两处变异验过） | 本机 |
| 版本单源 | 权威 = 根 Cargo.toml，基线 0.0.0；`check-versions` 一致 | 本机 |
| `node scripts/verify-diagram.mjs` | 59/59，交互后无运行时错误 | 本机 |
| `node scripts/verify-app.mjs` | 35/35（真 Rust 核心，非 mock） | 本机 |
| `node scripts/verify-tauri-window.mjs` | 8/8（真 `invoke`，控制台 0 error） | 本机 |
| `cargo fmt --check` | **已解除**（组件已装）：第一次跑就发现 92 个文件漂移，已纯格式化提交并复验 487/0 + clippy 0/0 | 本机 |

## 分领域状态

列含义：架构 = 设计是否已定义并有 ADR；单测 = L0/L1；集成 = L2/L3/L4；E2E = 浏览器/真窗口；平台 = 该平台是否真的能跑。

| 领域 | 架构 | 实现 | 单测 | 集成 | E2E | 平台 | 文档 | 状态 | 证据与缺口 |
|---|---|---|---|---|---|---|---|---|---|
| 不变量与分层（I1–I15、只向下依赖） | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | **DONE** | `arch-check` 24 条机器可判定，含"扫到 0 个文件即判失败"的防空转 |
| 富文本模型（normalize/parse/canonical/plain/三方合并/未知节点保留） | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | **DONE** | `notera-richtext` 47 条 + 编辑器端到端 9 步；零宽字符夹具已改显式转义并加"夹具自己必须带上被测字符"的防空转断言 |
| 存储与迁移（FTS5、tombstone、outbox、附件寻址） | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | **DONE** | `migrations_and_pragmas.rs`：空库、重复打开、**真 v2 旧库升级并留备份**、更高版本只读打开、§12 pragmas |
| 搜索（2 字中文 LIKE 兜底 + 较长词 FTS5） | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | **DONE** | PROBE 实测驱动；端到端"搜索命中这条笔记"在跑 |
| 同步引擎与清单（P1..P18、两段式清单、CAS、退避） | ✅ | ✅ | ✅ | ✅ | ⬜ | ⬜ | ✅ | **VERIFIED** | 引擎 10 条集成跑真 `run_round`；E2E 列未覆盖（浏览器层不驱动同步全过程，由 L3 承担） |
| WebDAV 适配与 §5 能力探测（S1/S2/S3、诚实降级） | ✅ | ✅ | ✅ | ✅ | ✅ | ⬜ | ✅ | **VERIFIED** | `notera-webdav` 33 条 + `notera-test-webdav` 真 TCP 注入；设置页能力块 3 条端到端。真实公网服务器矩阵 = **BLOCKED**（无外部服务器） |
| 崩溃恢复（§20 九个提交点） | ✅ | ✅ | ✅ | ✅ | ⬜ | ⬜ | ✅ | **VERIFIED** | `crash_recovery.rs` 真 spawn 子进程并以 77 死在每一点，崩完重启两台逐条一致、待办归零；反空转已证（废掉 P7 结清即红） |
| 冲突系统（update/update 三方） | ✅ | ✅ | ✅ | ✅ | ✅ | ⬜ | ✅ | **VERIFIED** | 真双设备分叉：一张卡、两版都在、`rev==sync_rev`、"用我这一版"真交换并传播 |
| P11（删除 vs 修改）远端那一版的可见性 | ✅ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ✅ | **IMPLEMENTING**（载荷链路已通，缺一条两设备断言） | 冲突卡右栏只有哈希 —— 用户被要求"二选一"却看不到另一版的正文，**看不见的选择不是选择**。**产品语义已拍板**（2026-09-27：按现在最佳体验、不承诺历史兼容 → 要展示被删那一版的正文），剩下的不再是决定而是实现：切片与门禁逐条写在 `CONFLICT-RESOLUTION.md §5.1.1`（引擎取料一次 → 迁移 0008 存 `remote_doc` → `conflicts` DTO 出 `remotePreview` → 卡片渲染 → 两台真设备断言 + 两条变异自证）。取料失败**必须**仍登记冲突并具名说明，不许让冲突消失 |
| 删除生命周期与防复活（回收站/恢复/永久删除/404 不删本地） | ✅ | ✅ | ✅ | ✅ | ✅ | ⬜ | ✅ | **DONE** | 端到端一整条边（删除→回收站→恢复→再删→彻底删除）+ P3/P12 判定测试 |
| 附件（sha256 寻址、上传/下载、**Range 续传**、去重、校验、缺失修复） | ✅ | ✅ | ✅ | ✅ | ✅ | ⬜ | ✅ | **VERIFIED** | 跨设备真传、包内字节一致、引用计数、下载队列都验过；**`resume` 已落地**：每轮推进一个 4 MiB 窗口，半截写在 `<2hex>/<sha>.part`，进程重启后从断点续（`attachment_resume.rs`：第一轮恰好停在窗口边界、重启后一次 206 补齐、字节全等、`.part` 清掉）。响应解释只认一条判据——**没有 206 就是整份**，于是偏移 0 走覆盖、绝不服软追加（第二条测试用 `FAIL(ignore-range)` 专门让服务器"探测时老实、正式请求没理我们的 Range"）。变异验证两处：摘掉 `want_range` → 第一条红；拆掉"非 206 报偏移 0" → 第二条与 4 条偏移判定一起红。上传侧仍是整块 PUT（分片上传未做，见下方缺口）|
| 代理与 TLS（direct/HTTP/SOCKS5/5h、自定义 CA、超时/退避/取消） | ✅ | ✅ | ✅ | ✅ | ✅ | ⬜ | ✅ | **VERIFIED** | `notera-net` 唯一出口 + 注入测试；`RouteProof` 脱敏审计 |
| 备份 / 恢复 / 导出 / 导入（含按文件夹子树） | ✅ | ✅ | ✅ | ✅ | ✅ | ⬜ | ✅ | **DONE** | `VACUUM INTO` + 三道闸门；ZIP 自描述包；子树/祖先范围两条 Rust 测试 + 两条端到端 |
| 自动保存与后台自动同步（不阻塞本地、单实例轮次） | ✅ | ✅ | ✅ | ✅ | ✅ | ⬜ | ✅ | **VERIFIED** | debounce→事务→rev→outbox→后台轮；端到端"输入即落库、刷新仍在"；链路抖动三种坏法已进门禁（SY-INT-11）；**"还在追平"这件事用户看得见**（SY-INT-13：被预算截断的轮次徽标报「正在同步」而不是「已同步」，调度器对 `Partial` 立刻续跑）；**大库换设备已修并进门禁（SY-INT-12：260 条 > 窗口上限，空库设备完整收敛、标题+内容哈希逐条一致，变异验证过：拆掉"已持有就跳过请求"这一判据 → 停在 197/260 立刻红）**。仍未验的是 macOS/Android/iOS 的后台调度 |
| UI：桌面工作区 / 编辑器原生感（AppFlowy 参照） | ✅ | ✅ | ✅ | ⬜ | ✅ | ✅ | ✅ | **VERIFIED** | 三栏、块把手、浮动工具条、`/` 面板、暗色、token 对比度契约测试 |
| UI：无障碍（可识别名、label、焦点、44pt、Esc/Enter） | ✅ | ✅ | ✅ | ⬜ | ✅ | ⬜ | ✅ | **VERIFIED** | 触摸目标与对比度有契约测试；本轮修掉"删除按钮念成删除文件夹"。**屏幕阅读器真机 = BLOCKED**（需 NVDA / VoiceOver 人工） |
| 平台能力：原生菜单 | ✅ | ✅ | ✅ | ✅ | ⬜ | ✅ | ✅ | **VERIFIED** | 三级菜单（笔记 / 同步 / 前往）在窗口显示前挂上，点击 → `notera://menu` → 前端路由；id 集合由一条 **Rust 读前端路由表**的契约测试守双向漂移（已做变异验证：改一个 id 即红并打出两侧集合）。release 产物 8/8 说明挂载在真壳上成功 |
| 平台能力：系统通知 | ✅ | ✅ | ✅ | ⬜ | ⬜ | ⬜ | ✅ | **VERIFIED**（判定） | 判定是纯函数 `notice_for`：只有"一条冲突"与"带原因的同步失败"会响，进度/Toast/notes-changed 一律安静（有测试）。**真机上是否弹出受系统权限影响 → 用户侧验** |
| 平台能力：托盘 | ✅ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ✅ | **IMPLEMENTING**（Windows 侧 VERIFIED） | 已开 `tauri` 的 `tray-icon` 特性并真挂上：左键显示/隐藏、右键托盘菜单（显示/隐藏、新建笔记、立即同步、退出）。**关窗是否收进托盘仍由设置页那条偏好决定，默认关**（判据 `shouldHideOnClose`：偏好为真 **且** 托盘真的挂上，两条缺一不可）。能力声明改口：`tray` 只在 `attach_tray` 成功后由 `report_native_cap` 翻 true。运行期证据 = 真窗口 lane 第 3 步（已做变异验证：摘掉那行上报 → 8/9 并点名 `tray 不是 true`）。代码路径桌面三端共用（`cfg!(any(...))`），但**只有 Windows 真机验过** → macOS/Linux 保持 ⬜（B1） |
| 平台能力：全局快捷键 | ✅ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ✅ | **IMPLEMENTING**（Windows 侧 VERIFIED） | 加了 `tauri-plugin-global-shortcut`（用户 2026-09-27 批准动依赖图）。两条：`Ctrl+Alt+N` 新建、`Ctrl+Alt+I` 显示/隐藏（mac `⌘⌥…`），唯一来源 `global_shortcut_plan()`；**刻意不复用应用菜单 accel**（把 `Ctrl+S`/`Ctrl+F` 注册成系统级 = 劫持别的应用）。只从 Rust 侧注册，前端不碰该插件 IPC，因此无需 capability。设置页那两行与真实注册的组合键由一条 Rust 读 TS 的契约测试对账（变异验证：把 `Alt` 改成 `Shift` → 红）。此前 macOS/Linux 报 `global_shortcuts: true` 而壳里没有任何注册 —— 那条假声明已随 as-built 改口修掉 |
| 平台能力：OS 钥匙存放凭据（`credential_ref`） | ✅ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ✅ | **BLOCKED** | 今天凭据只认 debug 环境变量；口令与 `credential_ref` 明确不下发界面。需要 §9 评审 + 各平台真机验证 |
| 移动端（Android） | ✅ | ⬜ | ✅ | ⬜ | ⬜ | ⬜ | ✅ | **BLOCKED** | 前端 390×844 视口与 44pt 已验；APK 构建/真机 = 本机无 NDK/JDK 与设备。原因/影响/解除条件见 CI-CD §L6 |
| 移动端（iOS / iPadOS） | ✅ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ✅ | **BLOCKED** | 无 macOS/Xcode 与真机（本机 Windows）。总指令 §46 明确这种情况要写 BLOCKED 而非伪装已验 |
| 发布构建与安装包（`.msi` / `.exe`） | ✅ | ⬜ | ✅ | ✅ | ⬜ | ⬜ | ✅ | **IMPLEMENTING** | **release 产物本身已验通**（`cargo build --release` + 关掉 vite 跑 `verify-tauri-window` 8/8，页面 `http://tauri.localhost/`）；修掉的是"缺 `[features] custom-protocol` → 正式构建开空白窗"这条 P0。**仍未产出安装器**：`@tauri-apps/cli` 不在依赖里 → 跑不了 `pnpm tauri build`（`.msi`/`.exe` 打包、图标嵌入、Updater 骨架都在它身上）。解除条件 = 许可装 `@tauri-apps/cli`（动依赖图，需 §9 点头）或改由 CI 的 Windows lane 出包 |
| 黑盒 UAT（§23：只许点击/输入/键盘/拖放） | ✅ | ✅ | ⬜ | ⬜ | ✅ | ⬜ | ✅ | **VERIFIED** | `scripts/verify-blackbox.mjs` 10/10：零 `/cmd/*` 调用、断言只看屏幕可见文字（含"插图后 `<img>` 真解出像素、刷新后仍在"）；已做反空转（废掉"恢复"按钮即 6/9）。与 `verify-app.mjs`（复核库内状态，非黑盒）并存。这一层抓到过一条白盒抓不到的：点在最后一行下面的空白会把焦点丢给 `body`，接着敲的字直接消失 |
| CI/CD 工作流落地 | ✅ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ✅ | **BLOCKED** | 仓库里没有 workflow 文件；推送通道未定（ARCHITECTURE-REVIEW §14 D1–D10）。无 Actions 运行证据时不得声称 CI 已过 |
| `.enex` 结构化导入 | ✅ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ✅ | **VERIFIED**（Windows，带一条限制） | 依赖已批准（`quick-xml` + `base64`，都在 workspace 单一版本源里）。两遍解析（信封 + CDATA 里的 ENML）；一条 `.enex` → N 条笔记；附件按 **sha256** 内容寻址（Evernote 的 `<en-media hash>` 是 MD5，只用来配对），`image`/`attachment` 块 + `attach_blob` 建 `note_attachments` 链接；`<tag>`/时间戳/表格这类本库表达不了的进 `notices` 并在设置页显示，不静默丢。入口：新命令 `import_files` + 设置页按钮（与"整库还原"的 `import_data` 两条语义）。**限制**：仍受 `MAX_SOURCE_BYTES = 8 MiB` 闸门约束（真实带图导出可能超，表现是一条看得见的失败而非静默截断）→ 流式读盘要动 `ImportSource` 形状，按 §9 走评审。证据：13 单测 + 2 集成 + 1 命令面端到端（真 Store、附件字节真落盘、重放 0 新建） |
| 文档 = 代码 = 协议 = 数据模型 = 测试 | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | **VERIFIED** | 本轮同步了 CHANGELOG / ARCHITECTURE-MAP / DATA-MODEL / TEST-PLAN（含新增崩溃注入矩阵）；`arch-check` 会盯漂移 |

## 距离 §47「READY FOR PRODUCTION」还缺什么

> 完整判定与逐条实测台账见 `docs/PRODUCTION-READINESS.md`（§52）。本节只留"还缺什么"。

按优先级（数据安全 > 同步正确性 > 稳定性 > 用户体验 > 原生体验 > 性能 > 可维护性 > 新功能）：

1. **【数据安全档 · 已修并验】新建笔记后立刻打字偶发丢正文、界面却写「已保存」**。成因是两段链：① autosave 的**在飞回包**代表过去那一版，回来却清 `dirty`、写 `lastSavedAt`，使更新的正文失去"未保存"身份；② `hydrate` 无条件覆盖 blocks。两段都修了（签名比对后保持 dirty 并另起一轮 / 同一笔记的旧快照回读不动本地输入），各有回归测试并做过变异。验证：纯黑盒 UAT **连跑五轮 5/5**（修前 3 轮 2 红、只修②那一版仍 4 轮 3 红）、端到端 35/35、前端 186 通过、`vue-tsc` 0 错。剩一个小项（不丢数据）：点击编辑区空白会先落进一条换行 `[{"text":"
"}]`，可能污染首行标题派生
2. **附件分片上传**（§8 里 `upload` 那一侧仍是一次 PUT）。下载续传已完成；上传大附件时中断仍要整份重传。
3. **安装器**（`.msi`/`.exe`）—— release 产物本身已验通（见上表），但打包要靠 `@tauri-apps/cli`，它不在依赖里。
4. **托盘 / 原生菜单 / 全局快捷键 / 真通知 / OS 钥匙串**（§15、§47"平台能力完成"）—— 需要 §9 评审点头。
5. **CI 工作流**：`.github/workflows/ci.yml` 已写好（远端 `github.com/fwd001/Noto`）。**首次运行结果未知** —— 本机到 GitHub 不可达，按 §40 记为 BLOCKED：需要有人把第一次跑的日志回贴。里面刻意用 `continue-on-error` 放轻了两步（`cargo fmt`、契约图），因为本机无从验证它们的 CI 环境；浏览器 lane 与真窗口 lane 显式排除并写明原因（CI 无 WebView2）。另：仓库现在同时有 `package-lock.json` 与 `pnpm-lock.yaml` 两份锁文件，需收敛到一个包管理器（未拍板前 CI 跟本机一致用 npm）
6. **追平过程中的基线分段整份重下（性能，已试修并回退）**。千条库追平 5 轮、每轮 `GET seg-0000.json` 一次（末版 86 KiB）；判据（`hash12` 比对）与表都齐，缺的是**可持久的远端视图**：`HostLocalPort::cached_remote()` 读的 `Mutex` 从未被写入、`sync_remote_index`（DDL 注释就写着"让每轮同步免于全量下载"）在生产路径零调用者、且缺引擎需要的 `deleted_at`。只接缓存的实测后果是第二台设备停在 198/1000 并报 `NoOp` —— 已按 §40 回退，补丁与判据在 `patches/`。解除要先走 §9/§51 的迁移决定。**不丢数据、不错报状态**（空轮实测 1 请求 / 304 / 0 字节，由 `big_library` 钉住）。
7. ~~**P11 远端版本可见性**的产品决定（§51）~~ —— **已拍板**（2026-09-27：按最佳体验，展示被删那一版的正文，不做历史兼容）。落地切片与门禁见 `CONFLICT-RESOLUTION.md §5.1.1`，剩下的工作是实现而不是决定。
8. 真实公网 WebDAV 服务器矩阵、真 Android/iOS/macOS 设备 —— 这些只能交给用户（§49）。
