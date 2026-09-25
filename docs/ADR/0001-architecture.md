# ADR-0001: 技术栈与分层

## 状态
日期：2026-09-25
状态：Accepted

## 背景
- 产品要求 local-first：断网时 100% 可用（I8），同步全部发生在后台。
- 目标四端：Windows x64、macOS arm64、Android arm64-v8a、iOS（架构预留，v1 不出包）。
- 硬约束：业务与同步核心逻辑必须在 Rust；前端不得实现同步协议的任何部分。
- 分层与依赖方向必须在写第一行实现之前定死，否则 Phase 2 的协议代码会散落到 UI 与存储层。

## 决策
- 技术栈：Tauri 2 + Rust 1.98.1 + Vue 3 + TypeScript + Vite + SQLite（`rusqlite` 0.40.2 bundled，SQLite 3.53.2）+ Tokio 1.53.1 + reqwest 0.13.5。
- 六层结构：
  - L0 UI/平台：`apps/desktop/src`、`platform/{windows,macos,android,ios}`
  - L1 应用与装配：`notera-host`（用例编排、调度、事件总线、Commands）、`notera-cli`
  - L2 领域服务：`notera-sync` `notera-richtext` `notera-importer` `notera-config`
  - L3 基础设施：`notera-store` `notera-crypto` `notera-webdav` `notera-net`
  - L4 内核：`notera-core`（零 I/O、零 async、零跨 crate 依赖）
  - L5 外部世界：用户自有 WebDAV 服务器、系统钥匙串、应用数据目录
- 依赖方向只允许向下，禁止反向与跨层回调。唯一"向上"通道是 `notera-host` 的事件总线（订阅，非调用）。
- 两个"唯一出口"闸门：前端唯一出口 = Tauri Commands / 事件（不出现协议词汇）；HTTP 唯一出口 = `notera-net`（ADR-0010）。

## 备选方案与被否决的原因
- Electron + Node 核心：无 iOS 路径；内存与安装包体积不可控；核心逻辑无法在移动端复用，等于写两份同步实现。
- Flutter/Dart 核心：与"核心逻辑必须在 Rust"的既定要求冲突；Dart 侧做崩溃一致性与原子写的能力更弱（无 `rusqlite` bundled 同级的可控构建）。
- 桌面 Tauri + 移动端原生壳经 uniFFI 共享 Rust core：功能成立，代价是 UI 三份实现、协议回归三处。保留为 Phase 5 的降级选项；本次经人工确认选择 Tauri 2 mobile 一体化。
- 现成 WebDAV 同步库：其同步语义不满足崩溃一致性（C1–C10 逐点可判）与"删除不复活"要求；只使用 DAV 语义原语，协议自建。

## 后果
正面：一份核心逻辑四端行为一致；Phase 1–3 的验证不依赖 GUI，可在无 WebView 的机器上完成；crate 边界即测试边界。
代价：
- Tauri 2 mobile 成熟度风险：移动端问题可能需要自行绕行，且官方未列 GNU 三元组支持（ADR-0014）。
- Windows 硬依赖 WebView2 运行时；本机缺失（B2），安装包必须内置离线安装器引导。
- iOS 后台能力受系统限制，同步时效真实弱于桌面（ADR-0011），出包依赖 macOS + Xcode + 证书（B6）。
- 六层 + 双闸门需要 CI 架构测试长期维护；违反时修依赖比写功能更耗时。

## 验证方式
- `cargo tree -e normal -p notera-store` 输出中不出现 `notera-sync`；`cargo tree -e normal -i reqwest` 的反向依赖闭包只经 `notera-net`。
- CI grep 闸门：`apps/desktop/src` 内不出现 `WebDAV|ETag|manifest|tombstone|revision|pull|push|merge|conflict`（词汇表见 ARCHITECTURE-MAP §5），命中即 fail。
- CI 架构测试读取 `cargo metadata` 的实际依赖边，与 ARCHITECTURE-MAP §2 注册表逐条比对，漂移即 fail。
- 契约图断言：`node scripts/verify-diagram.mjs` 必须 0 失败（当前 59 项断言通过）。

## 关联
- ARCHITECTURE.md §2 技术栈、§3 分层与依赖方向、§8 阻塞项（B2/B3/B6）
- ARCHITECTURE-MAP.md §1 分层、§2 模块注册表、§5 禁止模式
- PLATFORM.md §0 原则（共享核心逻辑 + 平台原生能力）
- 实测：`docs/evidence/probe-windows-gnu.txt`（16 项全 PASS），`sqlite-bundled-compiled`、`https-tls-handshake` 证明本选型在 windows-gnu 上可编译可运行
- ADR-0010（网络唯一出口）· ADR-0011（平台能力）· ADR-0014（构建矩阵）
