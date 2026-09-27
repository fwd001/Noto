# PLATFORM

Notera 的四端平台策略。规范性文档。

关联：[ARCHITECTURE.md](./ARCHITECTURE.md) · [SYNC-PROTOCOL.md](./SYNC-PROTOCOL.md) §14 · [PROXY.md](./PROXY.md) §10 · ADR-0011

---

## 0. 原则

> **共享核心逻辑 + 平台原生能力**，而不是"把所有平台做成完全一样"。

三条判据：

1. 数据模型、同步、冲突、搜索、加密、任务队列 → **一份 Rust 实现**，四端逐字节一致；
2. 窗口、菜单、托盘、通知、分享、后台任务、返回手势、键盘、安全区 → **各端原生**，用能力声明而非机型判断；
3. 任何"因为 iPhone 所以…"的分支都是设计失误 —— 正确写法是"因为 `caps.tray == false` 所以…"。

技术路线（已定，见 ADR-0001）：**Tauri 2 一体化**，桌面与移动共用同一 Vue 3 前端 + 同一 Rust 核心（移动端经 `cargo-ndk` / Xcode 集成）。前端一份实现，原生差异收敛在 `platform/*` 与能力抽象里。

---

## 1. 能力抽象

```rust
trait Lifecycle      { fn events(&self) -> Stream<LifecycleEvent>; fn flush_budget(&self) -> Duration; }
trait BackgroundTask { fn schedule(&self, req: BackgroundTaskRequest) -> Result<BgGrant>; fn cancel(&self, id); }
trait Notifications  { fn post(&self, req: NotificationRequest) -> Result<()>; }
trait Tray           { fn set(&self, spec: TraySpec) -> Result<()>; }        // 仅桌面
trait GlobalShortcut { fn register(&self, combo, action) -> Result<()>; }
trait FilePicker     { fn open(&self, filter) -> Result<Vec<LocalPath>>; }
trait Share          { fn receive(&self) -> Stream<ShareInbound>; fn send(&self, payload) -> Result<()>; }
trait NetworkMonitor { fn state(&self) -> Stream<NetworkState>; }
trait Secrets        { fn write(&self, CredentialRef, &[u8]) -> Result<()>; fn read(..); fn delete(..); }
trait SystemTheme    { fn current(&self) -> Theme; fn changes(&self) -> Stream<Theme>; }
```

`notera-host` 只依赖这些 trait；具体实现按 target 条件编译注入。**能力缺失必须显式降级**，例如 `bg_task == None` 时 UI 不显示"后台自动同步"开关，而不是显示一个不工作的开关。

---

## 2. 平台能力矩阵

| 能力 | Windows 10/11 x64 | macOS (Apple Silicon) | Android arm64 | iOS / iPadOS |
|---|---|---|---|---|
| 窗口 | 多窗口，自绘标题栏 | 统一工具栏 + 隐藏标题栏 | 单 Activity | 单 Scene + 分栏 |
| 托盘 | ✅ | ❌（不做 Dock 常驻） | ❌ | ❌ |
| 全局快捷键 | ✅ | ✅（应用内为主） | ❌ |  |
| 系统菜单 | 应用内菜单栏 | 顶部原生菜单 | ❌ | ❌ |
| 通知 | ✅ WinRT | ✅ UNUserNotificationCenter | ✅ 通知渠道 | ✅（需授权） |
| 系统文件选择 | ✅ | ✅ | ✅ SAF | ✅ UIDocumentPicker |
| 分享收件 | ❌ | ❌ | ✅ Intent Filter | ✅ Share Sheet Extension |
| 分享发件 | ❌ | ✅ 服务菜单 | ✅ | ✅ |
| 后台任务 | ✅ 定时器/服务 | ✅ 定时器 | ⚠ WorkManager（≥15 min，系统可延后） | ⚠ BGAppRefreshTask（系统调度，不保证） |
| 返回手势 | Alt+← | ⌘[ | ✅ 系统返回 | ✅ 边缘滑动 |
| 安全区 | N/A | N/A | ✅ insets | ✅ safeAreaInsets |
| 动态字体 | ⚠ WebView 缩放 | ⚠ | ✅ | ✅ |
| 系统字体 | Segoe UI Variable | SF Pro | Roboto | SF Pro |
| 凭据存储 | Credential Manager | Keychain | Keystore | Keychain |
| 生物识别解锁 | ⚠ Windows Hello（需原生插件） | ✅ LocalAuthentication | ✅ BiometricPrompt | ✅ Face/Touch ID |
| 键盘避让 | N/A | N/A | ✅ `adjustResize` | ✅ |
| 键盘外接快捷键 | ✅ | ✅ | ✅ | ✅ |
| 全屏沉浸写作 | ✅ F11 | ✅ | ✅ | ✅ |
| 平板分栏 | ✅ | ✅ | ✅ | ✅ |
| 拖拽文件入窗 | ✅ | ✅ | ❌ | ✅（iPad） |

⚠ = 有实现路径但受平台约束限制，不能按桌面直觉承诺。

---

## 3. 启动路径（严格时序，规范）

```text
1. 进程启动
2. 打开 SQLite（WAL 已存在则直接读，不做网络动作）        ≤ 60 ms
3. 迁移检查（user_version 闸门；需要迁移 → 备份 + 执行）
4. 读最近列表（v_note_list，不 SELECT doc）                ≤ 40 ms
5. 渲染首帧（列表 + 编辑器骨架，可立即输入）               ≤ 400 ms 目标
6. 后台并行启动：
     ├─ 网络监听注册
     ├─ 代理解析（不阻塞）
     └─ SyncEngine 启动 → 第一轮同步（Trigger::AppLaunch）
7. 同步结果以事件回流，UI 就地更新
```

**绝对禁止**：第 5 步之前出现任何网络等待。冷启动时服务器不可达，用户看到的必须是完整可用的本地库（顶多徽标是 `○ 离线`）。

启动自检（与第 3 步并行，不阻塞首帧）：outbox `inflight → pending` 归位、FTS 与权威表一致性抽样、清单 checksum 校验、崩溃标记检测（上次非优雅退出 → 记诊断并做一次远端复验）。

---

## 4. 前后台与生命周期

| 事件 | 桌面动作 | 移动动作 |
|---|---|---|
| 启动 | 首帧 → 后台同步 | 同 |
| 进入前台 | 立即一轮（若距上次 > debounce） | 立即一轮 |
| 进入后台 | 继续跑完当前轮 | **停止发起新轮**；当前轮在 5 s 内收尾，未完成留下轮 |
| 窗口失焦 | 提交未保存编辑（自动保存） | 同 |
| 退出请求 | `TerminateRequest` → 1.5 s flush 预算内提交本地 + 尽力上传 | 系统杀进程（无退出通知）→ 依赖本地事务已提交 |
| 系统休眠 | 唤醒后触发一轮 | 解锁后触发一轮 |
| 网络恢复 | 立即一轮 | 立即一轮（若在前台） |

要点：**本地写入永远在事件循环内即时提交**（debounce 1.2 s 或失焦即提交），因此进程被杀不会丢字。上传失败与进程被杀都不影响本地已提交状态 —— 这是 Local-first 的实际含义，不是口号。

---

## 5. 后台同步与诚实预算

### 5.1 桌面

`debounce 2.5 s` + `周期 25 s` + 事件触发（网络恢复 / 前台回归 / 保存 / 手动）。

周期取 25 s 而非 30 s：给一轮同步留出 5 s 执行余量，才能兑现"正常网络下 ≤30 s 到达另一台设备"。空轮成本 = 1 请求 0 字节（ETag 304），因此 25 s 轮询在内网与移动热点下都不构成负担（SYNC-PROTOCOL §6.3）。

### 5.2 Android

```text
主路径：WorkManager 周期任务  period ≈ 15 min（系统最小约束），flex 5 min，
        约束 requiresNetwork=true
加速：  打开即同步、前台回归、网络可用回调（ConnectivityManager）
可选：  同步进行中短暂前台服务（带通知），不用于长期常驻
```

### 5.3 iOS

```text
主路径：BGAppRefreshTask —— 由系统按使用模式调度，通常 ≥15 min 且可能整小时不给
加速：  打开即同步、sceneDidBecomeActive、registerForRemoteNotifications 不可用
        （WebDAV 无推送通道 → 无法服务端唤醒，这是协议事实而非实现缺陷）
预算：  后台时间片约 30 s 量级 → 一轮必须能在 30 s 内完成，因此：
        优先 ETag 304 快路径；大 relist / 大附件一律不在后台发起
```

### 5.4 必须承认的限制

> 移动端**不承诺** 30 秒到达。真实承诺是：**"你拿起设备打开 Notera 时，看到的基本已经是最新的"**，加上桌面端之间满足 ≤30 s。

任何声称能在 iOS 后台稳定 30 s 同步的设计都是把系统调度当成自己的线程。产品文案、测试矩阵、验收标准都必须按此口径写（TEST-PLAN 已按此拆分桌面/移动两类目标）。

---

## 6. Windows

* **标题栏**：自绘 + 原生拖拽/贴靠（Tauri `decorations` + 自定义 `data-tauri-drag-region`），保留系统贴靠布局与 Snap Assist。
* **观感**：Mica/Acrylic 仅在系统支持（Win11）时启用，Win10 回退纯色；**必须**同时提供"关闭透明效果"开关（性能与无障碍）。
* **托盘**：最小化到托盘可选，默认关闭（笔记软件常驻托盘对多数用户是噪音）；关闭按钮行为可选"最小化/退出"。
  *as-built（2026-09-27）*：托盘图标在桌面三端启动时挂上（左键 = 显示/隐藏窗口，右键 = 托盘菜单：显示/隐藏、新建笔记、立即同步、退出）；**关窗是否收进托盘由设置页那个开关决定，默认关**（开关 = `UiPrefs.trayHint`，判据是 `platform/caps.ts::shouldHideOnClose`，要"开关为真 **且** 托盘真的挂上"两个条件同时成立 —— 只判断前者会得到一个关不掉也找不回的进程）。托盘菜单里的"新建笔记/立即同步"与原生菜单、快捷键共用同一批 id，经 `notera://menu` 一条路进前端。
* **全局快捷键**：*as-built（2026-09-27）* 两条，`Ctrl+Alt+N`（任何应用里新建笔记）与 `Ctrl+Alt+I`（显示/隐藏窗口），mac 为 `⌘⌥N` / `⌘⌥I`；定义在 `notera_host::platform::global_shortcut_plan()`（唯一来源）。**刻意不复用应用菜单上的 accel**（`Ctrl+S`、`Ctrl+F` 那批）：把它们注册成系统级快捷键就是劫持别的应用的按键。注册只发生在 Rust 侧，前端不碰这个插件的 IPC，因此不需要给前端开 capability；成没成经 `report_native_cap` 写回能力声明，设置页里那两行快捷键（`requires: 'globalShortcuts'`）随之出现或消失，实际注册的组合键与界面上显示的字面由 `the_settings_page_shows_exactly_the_registered_global_shortcuts` 对账。**P6（托盘/后台常驻的默认取向）仍未拍板**，所以"常驻"这条路按默认关实现。
* **WebView2**：Tauri 在 Windows 的硬依赖。开发机实测**WebView2 运行时已装（150.0.4078.105）**（无 `Edge\Application` 也无 `EdgeCore`）→ 安装包必须内置离线安装器引导，且启动时检测缺失要给出可操作提示，而不是白屏。
* **凭据**：Windows Credential Manager（`keyring` crate）。
* **文件**：`IFileOpenDialog`（经 Tauri dialog 插件），拖拽入窗支持图片/文件。
* **通知**：WinRT 通知，需 AppUserModelID 与开始菜单快捷方式（安装器负责）。

---

## 7. macOS

* **统一工具栏**：`titleBarStyle: overlay` + 隐藏标题栏，搜索框进工具栏（Spotlight 式），三栏可折叠。
* **菜单**：原生主菜单（`应用/文件/编辑/格式/显示/窗口/帮助`），菜单项由 Rust 侧定义、动作经命令派发 —— 保证 ⌘ 快捷键在系统层面可被用户改。
* **窗口**：支持多窗口（每条笔记可弹出独立窗口编辑）→ 需要 `sync_rev` 乐观并发保护，跨窗口编辑同一条走 §冲突路径。
* **观感**：跟随系统外观 + 强调色；字体走系统 SF Pro； vibrancy 谨慎使用（正文可读性优先）。
* **凭据**：Keychain，`kSecUseDataProtectionKeychain`。
* **签名与公证**：无开发者证书时产物只能 ad-hoc 签名 → Gatekeeper 会拦。已在 CI-CD.md 列为待决策。

---

## 8. iOS / iPadOS

架构预留（v1 不出包，但必须能低成本接入）：

* `NavigationStack` 式单列 → 详情推进，iPad 横屏自动三栏分栏；
* 边缘右滑返回（WebView 内需保持原生手感 → 由 Tauri iOS 壳处理，前端不得自绘返回拦截）；
* Safe Area 与 `keyboardWillShow` 避让，编辑器焦点滚动到可视区；
* Dynamic Type：字号跟随系统，布局用相对间距而非像素；
* Share Sheet 收件（Extension target）→ 写入待办笔记并入 outbox；
* 长按 Context Menu（固定/移动/删除/分享）；
* 生物识别应用锁（本地能力，不参与同步）。

预留的具体形式：`platform/ios/` 目录、`PlatformCaps` 的 iOS 实现位、`sync_state` 的账户无关主键、富文本模型的触摸命中尺寸（≥44 pt）—— 这些在 Phase 1 就按 iOS 约束实现，避免后期返工。

---

## 9. Android

* 返回手势：系统返回优先关闭键盘 → 再退列表 → 再退侧栏 → 最后退应用；
* Material 兼容交互（涟漪、底部弹层、Snackbar 提示同步状态）；
* 键盘：`adjustResize` + 编辑器光标保持可见；外接键盘快捷键支持；
* 分享收件：`ACTION_SEND` / `ACTION_SEND_MULTIPLE` Intent Filter；
* 文件：SAF（`ACTION_OPEN_DOCUMENT`），不申请全存储权限；
* 通知渠道：`sync`（低优先级，可静默）与 `error`（需要处理时）；
* 目标 ABI：`arm64-v8a` 首发（`armeabi-v7a` 与 `x86_64` 由构建开关决定）。

---

## 10. 快捷键（跨端语义一致，键位各端原生）

| 动作 | Windows/Linux | macOS | 移动 |
|---|---|---|---|
| 新建笔记 | `Ctrl+N` | `⌘N` | 工具栏 |
| 搜索 | `Ctrl+F`（列表内）/ `Ctrl+K`（全局） | `⌘F` / `⌘K` | 搜索框 |
| 折叠侧栏 | `Ctrl+\` | `⌘\` | 手势 |
| 删除 | `Del` / `Ctrl+Backspace` | `⌘⌫` | 菜单 |
| 永久删除 | `Shift+Del` | `⇧⌘` | 菜单 |
| 固定 | `Ctrl+P` | `⌘P` | 长按菜单 |
| 加粗/斜体/下划线 | `Ctrl+B/I/U` | `⌘B/I/U` | 工具条 |
| 标题 1/2/3 | `Ctrl+1/2/3` | `⌘1/2/3` | 工具条 |
| 清单切换 | `Ctrl+Enter` | `⌘Enter` | 工具条 |
| 插入附件 | `Ctrl+Shift+F` | `⌘⇧F` | 工具条 |
| 立即同步 | `F5` | `⌘R` | 下拉刷新 |
| 冲突收件箱 | `Ctrl+Shift+C` | `⌘⇧C` | 提示条 |
| 命令面板 | `Ctrl+Shift+P` | `⌘⇧P` | — |

规则：键位**不跨端强行统一**（`Ctrl` vs `⌘` 各随其主），但**语义与可达性统一**（每个快捷键动作都必须有等价的鼠标/触摸路径）。

---

## 11. 主题与字体

* 全部颜色/间距/圆角/阴影走 design token，禁止组件内硬编码；
* 深色/浅色跟随系统，且应用内可强制；两套都要满足正文对比度 ≥ 7:1（AAA）；
* 字体：系统字体栈（不内置中文字体，体积代价 > 收益，且系统字体才是"原生感"的来源）；
* 字号缩放：0.85–1.6 档，缩放后布局不得出现横向滚动（测试矩阵有对应用例）；
* 减少动效：尊重系统 `prefers-reduced-motion`，同步动画降级为静态指示。

---

## 12. 本机/CI 验证限制（如实记录）

| 能力 | 本机 | CI | 影响 |
|---|---|---|---|
| Rust 无 GUI crate 测试 | ✅（GNU host） | ✅ | Phase 1–3 可在本机完整推进 |
| Windows 桌面出包 | ❌ 无 MSVC 链接器 | ✅ `windows-2022` | 安装包只能在 CI 验证 |
| 桌面窗口运行 | ❌ WebView2 运行时已装（150.0.4078.105） | ✅ | UAT（L5）本机不可跑 |
| macOS 出包 | ❌ 物理不可能 | ✅ `macos-14` | 只能 CI |
| Android 出包 | ❌ 无 JDK/SDK/NDK | ✅ | 只能 CI |
| iOS 出包 | ❌ | ✅（需证书） | 证书待决策 |
| GitHub Actions 观察 | ❌ 不可达 | — | **BLOCKED**，见 CI-CD.md 交接协议 |

结论：**Phase 1（Local Core）、Phase 2（Sync）、Phase 3（Proxy）的全部验证都不依赖 GUI**，可在本机完成；GUI 与安装包验证从 Phase 4 起依赖 CI 通道。这个划分让本机环境限制不阻塞前三阶段。

---

## 13. 未决与需要人工确认

| # | 事项 | 状态 |
|---|---|---|
| P1 | Windows 是否必须支持 Win10 1809 以下（WebView2 需随包分发） | 待确认目标机最低版本 |
| P2 | 目标机能否安装 WebView2 Runtime（内网无外网 → 需离线安装包随产品分发） | **BLOCKED**，需环境信息 |
| P3 | 是否需要生物识别应用锁（四端实现代价不均等） | 待产品确认 |
| P4 | macOS 多窗口编辑同一条笔记是否 v1 提供 | 未决（会放大冲突面） |
| P5 | iOS 接入时点与是否复用同一 Tauri 前端 | Phase 5 前重新评审 |
| P6 | 托盘/后台常驻的默认取向（省电 vs 及时性） | 待产品确认，默认关 |
