# IMPLEMENTATION-STATUS

> 总指令 §44 要求的那份验收状态表。**状态只能取**
> `PLANNED / IMPLEMENTING / VERIFIED / BLOCKED / DONE`；
> `VERIFIED` 一律带**可重放的证据出处**，没有证据就往下写。
> `SKIP / TODO / 理论通过 / 应该没问题 / 本地没环境所以跳过` 在这份表里不算 PASS。
>
> 更新时间：2026-09-26 · 本轮提交链到 `原生菜单/通知` 那一条 · 本机工具链 `stable-x86_64-pc-windows-gnu`

## 门禁总览（今天实测）

| 门禁 | 结果 | 出处 |
|---|---|---|
| `cargo test --workspace` | 475 通过 / 0 失败 / 0 ignored（53 个测试二进制） | 本机 |
| `cargo clippy --workspace --all-targets -- -D warnings` | 0 / 0 | 本机（CI-CD 原样命令） |
| `node scripts/arch-check.mjs` | 24/24 | 本机 |
| `node scripts/verify-diagram.mjs` | 59/59，交互后无运行时错误 | 本机 |
| `node scripts/verify-app.mjs` | 35/35（真 Rust 核心，非 mock） | 本机 |
| `node scripts/verify-tauri-window.mjs` | 8/8（真 `invoke`，控制台 0 error） | 本机 |
| `cargo fmt --check` | **BLOCKED** | 本机工具链没装 `rustfmt` 组件；解除 = `rustup component add --toolchain stable-x86_64-pc-windows-gnu rustfmt`（联网 + 改本机工具链，未擅自执行） |

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
| P11（删除 vs 修改）远端那一版的可见性 | ✅ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ✅ | **BLOCKED** | 冲突卡右栏只有哈希。要拍板的是产品语义（展示已删内容？恢复到哪？）→ §51 属"两个产品语义都合理"，需用户决定 |
| 删除生命周期与防复活（回收站/恢复/永久删除/404 不删本地） | ✅ | ✅ | ✅ | ✅ | ✅ | ⬜ | ✅ | **DONE** | 端到端一整条边（删除→回收站→恢复→再删→彻底删除）+ P3/P12 判定测试 |
| 附件（sha256 寻址、上传/下载、**Range 续传**、去重、校验、缺失修复） | ✅ | ✅ | ✅ | ✅ | ✅ | ⬜ | ✅ | **VERIFIED** | 跨设备真传、包内字节一致、引用计数、下载队列都验过；**`resume` 已落地**：每轮推进一个 4 MiB 窗口，半截写在 `<2hex>/<sha>.part`，进程重启后从断点续（`attachment_resume.rs`：第一轮恰好停在窗口边界、重启后一次 206 补齐、字节全等、`.part` 清掉）。响应解释只认一条判据——**没有 206 就是整份**，于是偏移 0 走覆盖、绝不服软追加（第二条测试用 `FAIL(ignore-range)` 专门让服务器"探测时老实、正式请求没理我们的 Range"）。变异验证两处：摘掉 `want_range` → 第一条红；拆掉"非 206 报偏移 0" → 第二条与 4 条偏移判定一起红。上传侧仍是整块 PUT（分片上传未做，见下方缺口）|
| 代理与 TLS（direct/HTTP/SOCKS5/5h、自定义 CA、超时/退避/取消） | ✅ | ✅ | ✅ | ✅ | ✅ | ⬜ | ✅ | **VERIFIED** | `notera-net` 唯一出口 + 注入测试；`RouteProof` 脱敏审计 |
| 备份 / 恢复 / 导出 / 导入（含按文件夹子树） | ✅ | ✅ | ✅ | ✅ | ✅ | ⬜ | ✅ | **DONE** | `VACUUM INTO` + 三道闸门；ZIP 自描述包；子树/祖先范围两条 Rust 测试 + 两条端到端 |
| 自动保存与后台自动同步（不阻塞本地、单实例轮次） | ✅ | ✅ | ✅ | ⬜ | ✅ | ⬜ | ✅ | **BLOCKED（P1）** | debounce→事务→rev→outbox→后台轮；端到端"输入即落库、刷新仍在"；链路抖动三种坏法已进门禁（SY-INT-11）。**但"新设备追 >200 条变更的大库"追不平且界面无提示**（实测停在 196/260，`dirty_notes/outbox_pending` 都是 0 → 徽标说已同步）。复现、已定位的三处判据缺口、以及被我放弃的半成品补丁都在 CHANGELOG §已知限制第一条与 `docs/evidence/late-device-*`。解除条件：把"本轮是否被请求预算截断"收成引擎唯一落账判据 + `seq_applied` 改成真正读回库值，然后让该复现以 260 条完整收敛进 `cargo test` 并做变异验证。小库（<200 变更）的换设备路径已由 SY-INT-11/两台设备测试证明可用 |
| UI：桌面工作区 / 编辑器原生感（AppFlowy 参照） | ✅ | ✅ | ✅ | ⬜ | ✅ | ✅ | ✅ | **VERIFIED** | 三栏、块把手、浮动工具条、`/` 面板、暗色、token 对比度契约测试 |
| UI：无障碍（可识别名、label、焦点、44pt、Esc/Enter） | ✅ | ✅ | ✅ | ⬜ | ✅ | ⬜ | ✅ | **VERIFIED** | 触摸目标与对比度有契约测试；本轮修掉"删除按钮念成删除文件夹"。**屏幕阅读器真机 = BLOCKED**（需 NVDA / VoiceOver 人工） |
| 平台能力：原生菜单 | ✅ | ✅ | ✅ | ✅ | ⬜ | ✅ | ✅ | **VERIFIED** | 三级菜单（笔记 / 同步 / 前往）在窗口显示前挂上，点击 → `notera://menu` → 前端路由；id 集合由一条 **Rust 读前端路由表**的契约测试守双向漂移（已做变异验证：改一个 id 即红并打出两侧集合）。release 产物 8/8 说明挂载在真壳上成功 |
| 平台能力：系统通知 | ✅ | ✅ | ✅ | ⬜ | ⬜ | ⬜ | ✅ | **VERIFIED**（判定） | 判定是纯函数 `notice_for`：只有"一条冲突"与"带原因的同步失败"会响，进度/Toast/notes-changed 一律安静（有测试）。**真机上是否弹出受系统权限影响 → 用户侧验** |
| 平台能力：托盘 | ✅ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ✅ | **PLANNED** | 仍需要 `tauri` 的 `tray-icon` 特性 + 关窗行为读那条偏好；动了依赖图 → §9 人工评审。能力因此如实报 `tray: false`，设置页不再摆"关闭窗口时留在托盘" |
| 平台能力：全局快捷键 | ✅ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ✅ | **PLANNED** | 需要 `tauri-plugin-global-shortcut`（新依赖）。顺带修了一条假声明：此前 macOS/Linux 报 `global_shortcuts: true` 而壳里没有任何注册 |
| 平台能力：OS 钥匙存放凭据（`credential_ref`） | ✅ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ✅ | **BLOCKED** | 今天凭据只认 debug 环境变量；口令与 `credential_ref` 明确不下发界面。需要 §9 评审 + 各平台真机验证 |
| 移动端（Android） | ✅ | ⬜ | ✅ | ⬜ | ⬜ | ⬜ | ✅ | **BLOCKED** | 前端 390×844 视口与 44pt 已验；APK 构建/真机 = 本机无 NDK/JDK 与设备。原因/影响/解除条件见 CI-CD §L6 |
| 移动端（iOS / iPadOS） | ✅ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ✅ | **BLOCKED** | 无 macOS/Xcode 与真机（本机 Windows）。总指令 §46 明确这种情况要写 BLOCKED 而非伪装已验 |
| 发布构建与安装包（`.msi` / `.exe`） | ✅ | ⬜ | ✅ | ✅ | ⬜ | ⬜ | ✅ | **IMPLEMENTING** | **release 产物本身已验通**（`cargo build --release` + 关掉 vite 跑 `verify-tauri-window` 8/8，页面 `http://tauri.localhost/`）；修掉的是"缺 `[features] custom-protocol` → 正式构建开空白窗"这条 P0。**仍未产出安装器**：`@tauri-apps/cli` 不在依赖里 → 跑不了 `pnpm tauri build`（`.msi`/`.exe` 打包、图标嵌入、Updater 骨架都在它身上）。解除条件 = 许可装 `@tauri-apps/cli`（动依赖图，需 §9 点头）或改由 CI 的 Windows lane 出包 |
| 黑盒 UAT（§23：只许点击/输入/键盘/拖放） | ✅ | ✅ | ⬜ | ⬜ | ✅ | ⬜ | ✅ | **VERIFIED** | `scripts/verify-blackbox.mjs` 10/10：零 `/cmd/*` 调用、断言只看屏幕可见文字（含"插图后 `<img>` 真解出像素、刷新后仍在"）；已做反空转（废掉"恢复"按钮即 6/9）。与 `verify-app.mjs`（复核库内状态，非黑盒）并存。这一层抓到过一条白盒抓不到的：点在最后一行下面的空白会把焦点丢给 `body`，接着敲的字直接消失 |
| CI/CD 工作流落地 | ✅ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ✅ | **BLOCKED** | 仓库里没有 workflow 文件；推送通道未定（ARCHITECTURE-REVIEW §14 D1–D10）。无 Actions 运行证据时不得声称 CI 已过 |
| `.enex` 结构化导入 | ✅ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ✅ | **PLANNED** | 需要 XML 依赖，属"要动依赖图"→ §9 人工评审 |
| 文档 = 代码 = 协议 = 数据模型 = 测试 | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | **VERIFIED** | 本轮同步了 CHANGELOG / ARCHITECTURE-MAP / DATA-MODEL / TEST-PLAN（含新增崩溃注入矩阵）；`arch-check` 会盯漂移 |

## 距离 §47「READY FOR PRODUCTION」还缺什么

按优先级（数据安全 > 同步正确性 > 稳定性 > 用户体验 > 原生体验 > 性能 > 可维护性 > 新功能）：

1. **附件分片上传**（§8 里 `upload` 那一侧仍是一次 PUT）。下载续传已完成；上传大附件时中断仍要整份重传。
2. **安装器**（`.msi`/`.exe`）—— release 产物本身已验通（见上表），但打包要靠 `@tauri-apps/cli`，它不在依赖里。
3. **托盘 / 原生菜单 / 全局快捷键 / 真通知 / OS 钥匙串**（§15、§47"平台能力完成"）—— 需要 §9 评审点头。
4. **CI 工作流**（§42 那套门禁要有地方真的跑）。纯黑盒 UAT lane 已落地：BB-01…10。
5. **P11 远端版本可见性**的产品决定（§51）。
6. 真实公网 WebDAV 服务器矩阵、真 Android/iOS/macOS 设备 —— 这些只能交给用户（§49）。
