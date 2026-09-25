# ADR-0014: 构建矩阵与版本策略

## 状态
日期：2026-09-25
状态：Accepted

## 背景
本机环境实测无法出任何一端的正式包：无 MSVC 链接器（PATH 中 `link.exe` 被 Git coreutils 顶替）、WebView2 运行时已装（150.0.4078.105）、无 JDK/Android SDK/NDK，iOS 在 Windows 上物理不可能；并且 `github.com` / `api.github.com` 本机不可达（curl `000`），因此 CI 结果无法被开发者观察到。发布通道如果默认"本地能跑就等于 CI 能跑"，会立即变成假话。

## 决策
- 版本策略：SemVer。`0.1.0` 首发三产物 → `0.2.0` 功能补全 → `1.0.0` 承诺数据格式与协议兼容。
- 首发三产物：Windows x64 安装包（`x86_64-pc-windows-msvc`）、macOS arm64 包（`aarch64-apple-darwin`）、Android arm64-v8a APK（`aarch64-linux-android`）。iOS 预留不产资产。
- 版本单一来源：根 `Cargo.toml` 的 `[workspace.package] version`；`scripts/check-versions` 校验 Cargo / `package.json` / `tauri.conf.json` / `protocol.json` 一致（V1–V5 失败面）。
- `SYNC_PROTOCOL_VERSION` 是**独立的第二权威**：与产品版本不同轴；协议破坏性变更必须升版本并写入 `protocol.json` 协商（老客户端遇新协议 → 只读，不得写回）。
- release 只在 tag `v*` 触发，且 `needs:` 全部 7 个闸门作业（`pr/integration/crash/e2e-desktop/audit/build-windows/build-macos-android`）。
- 产物带 SHA-256 `checksums.txt`（`sha256sum -c` 兼容格式，由另一 job 独立复算）。
- 本机不可达 GitHub → **交接协议**：agent 写 workflow 并跑本机可跑部分 → 人工 push 并回贴 Actions run URL + 原始日志（存 `docs/evidence/ci-<run-id>.md`）→ agent 以该日志为唯一证据源修复。**拿到绿灯证据前，任何文档/提交禁止写"CI 已通过"**。

## 备选方案与被否决的原因
- 只 `cargo test` 通过即发布：需求明令禁止；本机测试通过既不覆盖 GUI 层，也不覆盖三平台工具链差异。
- 本机出全部包：实测不可行（无 MSVC 链接器、无 JDK/SDK（WebView2 实测在位），iOS 物理不可能）。
- 开发期长期用 `windows-gnu` 出包：**Proposed** —— Tauri 官方只列 MSVC 三元组，GNU 未获支持；需先做 spike（COM / `aws-lc-rs` / `ring` / unwind 行为一致性）再决定。当前 GNU host 仅用于无 GUI 的 crate 测试。

## 后果
正面：版本漂移与协议静默破坏被 CI 阻断（`check-versions`、V3/V4/P1 规则）；发布路径与验证证据绑定。
代价：
- **CI 未验证前发布通道是纸面的**：全部 `build-*` / `release` 结论至今 `[BLOCKED]`，没有一次流水线运行被任何人观察到。
- 依赖人工推送回贴，反馈以小时计；每个 `[假设]` 条目都要在首次真跑时逐条闭环。
- Android release keystore 与 Apple 证书归属**待人工决定**（D2/D3）；缺 secrets 时该步必须标 `SKIPPED-BY-MISSING-SECRETS`，不得伪装通过。

## 验证方式
- `scripts/check-versions` 在 CI 失败即阻断合并与发布（V1 权威唯一、V2 lockstep、V3 协议字段齐备、V4 不静默破坏、V5 产物名含版本）。
- 三平台构建作业各自产物存在且可安装冒烟：`test -s` + `file` 类型断言 + L6"安装/解包 → 起窗 → 打开 SQLite → 完成一轮 sync → 退出 → 再启动"；Windows 冒烟依赖 WebView2，Android 首发只做 `apkanalyzer`/`aapt dump` 结构校验（真机 BLOCKED）。
- 校验和独立复算：`sha256sum -c checksums.txt` 由另一 job 执行；`release` 拒绝含 `debug|unsigned|adhoc` 的资产。
- 文档一致性：BLOCKED 项必须在 ARCHITECTURE.md §8 显式列明（B1–B7），任何"已验证"表述必须附 run URL。
- 本机可跑部分作为前置证据：GNU host 下 `cargo test/clippy/fmt` 通过（但**不得**据此推定 CI 通过）。

## 关联
- CI-CD.md 全文（目标 G1–G7、流水线分层、Runner 矩阵、§版本与单一版本源、§产物与发布、§环境与网络限制、§交接协议、待人工决策 D1–D6）
- ARCHITECTURE.md §8 已知限制与阻塞项（B1–B7）
- SYNC-PROTOCOL.md §15 版本演进规则（`protocol.json` 协商表）
- 实测：`docs/evidence/probe-windows-gnu.txt` 头部环境记录（无 MSVC 链接器、GNU host 可编译并真实握手 TLS）
- ADR-0001（技术栈决定三元组）· ADR-0011（移动端产物与后台约束）· ADR-0012（迁移矩阵是 CI 门禁的一部分）
