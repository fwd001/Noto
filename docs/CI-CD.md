# Notera CI/CD 设计（Phase 0 · 仅设计）

> 本文是 **设计规格**，不是实施记录。Phase 1 据此落地 workflow 骨架，Phase 8 补全全量门禁。
> 文中所有 YAML 均为「待落地蓝图」，仓库中 **尚不存在** `.github/workflows/` 任何文件。
> 测试分层（L0—L6）的定义与用例设计见 `docs/TEST-PLAN.md`（并行编写中），本文只规定 **CI 如何门禁**，
> 不重述测试内容。

## 0. 标记约定与证据基线

| 标记 | 含义 |
| --- | --- |
| `[实测]` | 来自开发者本机探针输出，见 `docs/evidence/probe-windows-gnu.txt`、`tools/feasibility-probe/`。仅证明本机 GNU 工具链行为，**不证明 CI、不证明 MSVC 路径、不证明 Tauri** |
| `[上游]` | 来自 Tauri v2 / GitHub Actions / Android 官方前置条件陈述，本文按原文尊重，未在本机复现 |
| `[假设]` | 无法从已知事实推出，落地前必须 spike 或在 CI 首次运行中确认 |
| `[BLOCKED]` | 本机环境或网络导致 **当前无法验证**，需按 §12 交接协议由人工回贴证据后才能升级为事实 |

基线事实（`[实测]`，除标注外）：

| 项 | 值 |
| --- | --- |
| OS | Windows 11 x64 `10.0.26200` |
| rustc / cargo | 1.98.1（`rustup` 在位） |
| 已安装 target | `x86_64-pc-windows-msvc`（默认 host）、`x86_64-pc-windows-gnu`（Phase 0 期间补装） |
| MSVC Build Tools / Visual Studio | **未安装**；无 `cl.exe` |
| msvc host 可编译性 | **不可**：链接调用被 Git Bash 的 `/usr/bin/link.exe`（GNU coreutils `link`，非 MSVC linker）截获，报 `link: extra operand`；连 proc-macro 构建脚本（`quote`/`proc-macro2`/`getrandom`）都无法编出 → 见探针 PROBE-FAIL-001 |
| GNU host 工具链 | MinGW-W64 `x86_64-ucrt-posix-seh` gcc 16.1.0；`windres`/`dllwrap` 在位 |
| GNU host 实测可编译并运行 | `rusqlite` 0.40.2（bundled SQLite 3.53.2 + FTS5）、`reqwest` 0.13.5（rustls / `aws-lc-rs` 1.18.1）、`aes-gcm-siv` 0.12.1、`argon2` 0.6.0、`uuid` 1.26.1 v7：探针 16 项全 PASS，`exit=0` |
| C/C++ 工具 | 无 `nasm`、无 graphviz `dot`、无 `sqlite3` CLI |
| Node / pnpm / npm | v24.16.0（fnm）/ 12.5.1 / 11.13.0 |
| Python / git | 3.12.10 / 2.41.0 |
| WebView2 运行时 | **缺失**（既无 `Microsoft\Edge\Application`，也无 `EdgeCore`）→ 本机无法启动 Tauri 桌面窗口 |
| Java / Android SDK / NDK / adb | **全部缺失** → 本机无法构建 Android |
| iOS | Windows 上根本无法构建（需 macOS + Xcode）`[上游]` |
| 出网 | `crates.io` / `static.crates.io` / `index.crates.io` / `registry.npmjs.org` 可达（HTTP 200） |
| 出网（GitHub） | `github.com`、`api.github.com` **不可达**（curl `000`）→ 本文所有 CI 相关结论均为 `[BLOCKED]` |

## 目标与非目标

### 目标（CI 必须保证）

| # | 保证 | 门禁位置 |
| --- | --- | --- |
| G1 | 任何合入 `main` 的提交：前端测试 + Rust 测试 + 集成 + WebDAV E2E + 三平台构建 **全绿**，缺一不可 | §2 全链 |
| G2 | 首发三产物可由 tag 一键复现：Windows x64 安装包、macOS **arm64** 包、Android **arm64-v8a** APK | `build-*` + `release` |
| G3 | 数据库升级路径永不破坏用户数据（只前进、可校验、拒绝未知未来版本） | §8 |
| G4 | 同步协议版本与产品版本 **不静默漂移**；老客户端遇新协议自动降级为只读 | §7 |
| G5 | 依赖可复现、许可证与安全告警可见、密钥不入仓 | §10 |
| G6 | 崩溃/杀进程后状态可恢复（L4），且这类改动不可能绕过门禁 | `crash` |
| G7 | 架构文档与代码保持一致（`ARCHITECTURE-MAP` 一致性） | `docs` |

### 非目标（CI 明确不做）

| 不做 | 原因 |
| --- | --- |
| **不保存任何用户数据**：不落库、不缓存笔记正文、不上传用户文件为 artifact；测试库仅用 CI 生成的合成数据并在 job 结束销毁 | local-first 产品的数据外流是最高级事故 |
| **不接触真实 WebDAV 账号**：L3 一律打 `notera-test-webdav`（127.0.0.1 真实 HTTP）；禁止把任何真实服务器/账号写进 workflow 或 secrets 供测试使用 | 真实账号凭据一旦入 CI 即不可召回 |
| **不跑需要付费证书/账号的步骤**：Apple 开发者签名与公证、Android release 签名、代码签名服务、更新服务器，**仅当对应 secrets 已配置才执行**；否则该步 `skip` 且在 job summary 显式标注 `SKIPPED-BY-MISSING-SECRETS`，**不得伪装为通过** | secrets 决策未定（§13） |
| 不在 CI 中做真机（物理手机 / 真 Mac 硬件）测试 | 需要自托管 runner，首发不做 |
| 不做 HTTP 层的 mock | 项目硬约束：L3 必须真 HTTP；`cargo test` 里出现 mock HTTP 视为门禁失败 |
| 不做「自动合并 PR」 | 需人工评审 |
| 不在 CI 里发布到任何应用商店 | 上架流程独立、人工 |

## 流水线分层

| 作业 | 触发条件 | 内容 | 时长预算 | 阻塞合并 |
| --- | --- | --- | --- | --- |
| `pr` | `pull_request`（main / release/*）、`push` 到 feature 分支 | `cargo fmt --check`；`clippy -D warnings`（全 workspace，`--all-targets`）；`eslint`；`vue-tsc --noEmit`；**L0** 单测（`cargo nextest run --workspace` + `vitest run`）；**L2** 协议契约（含 `protocol.json` 与 §7 校验） | ≤ 8 min | 是 |
| `integration` | 依赖 `pr` 成功（`needs: pr`） | **L1** 组件（core↔store↔richtext 组装）；**L2** 契约回归；**L3** 多客户端 E2E：起 `notera-test-webdav`，≥2 个 `notera-host` 实例经 127.0.0.1 真实 HTTP 收敛 | ≤ 12 min | 是 |
| `crash` | `needs: pr`，且在 `crates/notera-{store,sync,crypto}/**`、`migrations/**` 有变更时必跑（其余 PR 也跑，但允许 `continue-on-error: false`） | **L4** 写入中途 `kill -9` / `taskkill /F` / `abort()`，重启后校验 WAL 恢复、FTS 索引与 `user_version` 一致 | ≤ 10 min | 是 |
| `e2e-desktop` | 仅 `windows-2022`（预装 WebView2 `[上游/假设]`）+ `pull_request`，`workflow_dispatch` 可手动 | **L5** 黑盒 UAT：Playwright 驱动 Tauri WebView（经 `devtools`/远程调试端口，接线方式 Phase 1 spike） | ≤ 15 min | 是（release 必过；PR 若 runner 无 WebView2 必须显式 fail，不允许 skip） |
| `audit` | `pull_request` + `schedule` 每日 + tag | `cargo-audit`（RustSEC）、`cargo-deny`（licenses/advisories/bans/duplicates）、`npm audit --audit-level=high`、`gitleaks detect` | ≤ 5 min | 是（高危项；`audit` 的 advisory 允许带到期日的 `waiver` 列表，见 §10） |
| `build-windows` | `pull_request`（标签 `build:win` 或改 `apps/desktop/**`）+ tag | `cargo build --release --target x86_64-pc-windows-msvc` + `pnpm tauri build --bundles msi`（`nsis` 是否同时出：待决策 §13） | ≤ 25 min | 是（release 前置） |
| `build-macos-android` | 同上（矩阵两个 job） | macOS：`macos-14` + `tauri build --target aarch64-apple-darwin` → `.dmg` + `.app.zip`；Android：`ubuntu-22.04` + JDK17 + SDK/NDK → `arm64-v8a` APK | ≤ 30 min | 是（release 前置） |
| `release` | **仅** `push` tag `v*` | `needs: [pr, integration, crash, e2e-desktop, audit, build-windows, build-macos-android]` → 汇总产物、生成 `checksums.txt` 与 CHANGELOG、`gh release create` | ≤ 10 min | 自身即终态 |
| `nightly-soak` | `schedule: cron '0 18 * * *'`（UTC，≈ 北京 02:00）+ tag 前手动触发 | 长时/大数据：10 万条笔记、FTS 索引重建、72h（预算截为 3h 采样）多客户端反复同步、内存/FD 泄漏曲线、崩溃重放 1000 次 | ≤ 3 h | 否；但 **release candidate 必须引用最近一次绿过的 nightly run id** |
| `docs` | `pull_request`，paths: `docs/**`、`*.md`、`crates/**`、`apps/**` | `markdownlint-cli2`；架构图渲染（graphviz `dot` / mermaid，本机无 `dot` → 只在 CI 出图）；**`ARCHITECTURE-MAP` 一致性检查**：文档声明的 crate ↔ 目录 ↔ 依赖边 必须与 `cargo metadata` 实际图一致，漂移即 fail | ≤ 4 min | 是（改文档/改 crate 结构的 PR） |

### 发布门禁的硬规则

| 规则 | 表达 |
| --- | --- |
| **绝不允许仅凭 `cargo test` 放行发布** | `release` 的 `needs:` 必须列出全部 7 个 gate job；缺 `e2e-desktop` 或 `build-*` 即无法运行 |
| 五件套全绿才可发布 | 前端测试 ∧ Rust 测试 ∧ 集成（L1/L2）∧ WebDAV E2E（L3）∧ 平台构建（L6） |
| skip 不等于通过 | 任何 `SKIPPED-BY-MISSING-SECRETS` 必须在 release body 顶部列出，并由人工在 §13 决策后才能勾选 `confirm`（`workflow_dispatch` 二次输入） |
| L6 平台构建冒烟 | 产物必须被「安装/解包 + 启动到能读 SQLite」冒烟检查一次；Windows 冒烟依赖 WebView2，Android 冒烟首发只做 `apkanalyzer`/`aapt dump` 结构校验（真机冒烟 `[BLOCKED]`） |

## Runner 矩阵

| Runner 镜像 | 目标三元组 / 产物 | 预装（`[上游/假设]`） | 必须显式 setup 的步骤 | 承接作业 |
| --- | --- | --- | --- | --- |
| `ubuntu-22.04` | `x86_64-unknown-linux-gnu`（仅开发校验，不发布 Linux 产物） | gcc、pkg-config、libwebkit2gtk（Tauri Linux 依赖 `[假设]`） | `dtolnay/rust-toolchain`、`pnpm/action-setup` | `pr`、`integration`、`crash`、`docs`、`audit`、Android 构建 |
| `windows-2022` | `x86_64-pc-windows-msvc` → `.msi` | VS 2022 + C++ Build Tools（→ 天然规避本机 PROBE-FAIL-001）、MSYS/Git Bash、Edge/WebView2 | `dtolnay/rust-toolchain --targets x86_64-pc-windows-msvc`、`pnpm/action-setup`、（若需）NuGet/Windows SDK | `build-windows`、`e2e-desktop`、以及 Windows 侧 `pr`/`crash` |
| `macos-14` | `aarch64-apple-darwin` → `.dmg`/`.app.zip` | **原生 Apple Silicon**、Xcode + CLT、`codesign`/`xcrun notarytool` | `dtolnay/rust-toolchain --targets aarch64-apple-darwin`、`pnpm/action-setup` | `build-macos-android`(macos leg) |
| `ubuntu-22.04` + Android | `aarch64-linux-android` → `arm64-v8a` APK | 无 Android SDK/NDK | `actions/setup-java@v4`（`temurin`,`17`）、`android-actions/setup-android@v3`、NDK/Build-Tools via `sdkmanager`、`rustup target add aarch64-linux-android armv7-linux-androideabi` | `build-macos-android`(android leg) |

> `windows-2022` 与 `macos-14` 的具体镜像内容本文 **无法在本机验证**（GitHub 不可达）→ `[BLOCKED]`，
> 首次流水线绿灯时逐项确认，并把结论回写本表（含 run URL）。

### 固定版本（写入 workflow，禁止 `@latest`）

| Action | 固定 | 用途 | 漂移风险 |
| --- | --- | --- | --- |
| `actions/checkout` | `@v4` | 全部 job | 低；升 v5 需单独 PR |
| `actions/setup-java` | `@v4` + `distribution: temurin`, `java-version: 17` | Android | 中：AGP 要求 JDK 与 `compileSdk` 匹配 `[假设]` |
| `android-actions/setup-android` | `@v3` | Android SDK/命令行工具 | 高：镜像内 SDK 版本会变，必须显式 `sdkmanager` 固定 `platforms;android-35`、`build-tools;35.0.0`、`ndk;<pin>` `[假设]` |
| `dtolnay/rust-toolchain` | `@stable`（但仓库有 `rust-toolchain.toml` 精确 pin 1.98.1，见 §10） | 全部 Rust job | 中 |
| `Swatinem/rust-cache` | `@v2` | `target/` + `~/.cargo` | 低 |
| `pnpm/action-setup` | `@v4`（版本由 `package.json` 的 `packageManager` 字段决定） | 前端 | 中 |
| `actions/upload-artifact` / `download-artifact` | `@v4` | 产物 | 高：v3→v4 行为不兼容（合并名、通配符），必须一次性统一到 v4 |
| `actions/cache` | `@v4` | pnpm store、gradle | 低 |
| `gitleaks/gitleaks-action` | `@v2` | secret 扫描 | 高：付费/免费边界需 §13 确认 `[假设]` |

### 缓存键设计

| 缓存 | 路径 | key（自顶向下即降级顺序） | 规则 |
| --- | --- | --- | --- |
| crates registry | `~/.cargo/registry`、`~/.cargo/.crates2.json` | `cargo-${{ runner.os }}-${{ hashFiles('**/Cargo.lock') }}` | **跨 job、跨 target 共享**（下载产物与工具链无关）；restore-keys 允许前缀命中 |
| `target/` | workspace 各 `target/` | `${{ runner.os }}-${{ matrix.rust_target }}-${{ matrix.toolspace }}-cargo-${{ hashFiles('**/Cargo.lock') }}` | **按 job/按 toolspace 隔离**：`msvc` 与 `gnu` 的对象文件绝不混用（混用是静默错乱的温床）；`toolspace` 显式取自 `rustc -vV` 的 host |
| pnpm store | `$(pnpm store path)` | `pnpm-${{ runner.os }}-${{ hashFiles('**/pnpm-lock.yaml') }}` | 只缓存 store，不缓存 `node_modules`（store→`pnpm install --offline` 重建） |
| Gradle / Android | `~/.gradle/caches`、`~/.gradle/wrapper` | `gradle-${{ runner.os }}-${{ hashFiles('**/*.gradle.kts','**/gradle-wrapper.properties','**/libs.versions.toml') }}` | 与 SDK/NDK 版本解耦；SDK 变更不使此缓存失效（可能反而导致陈旧构建，故 Android job 每周清理一次：`nightly` 里加 `--refresh-dependencies`） |
| Playwright | `~/.cache/ms-playwright` | `pw-${{ runner.os }}-${{ hashFiles('**/pnpm-lock.yaml') }}` | 版本与 `pnpm-lock` 绑定 |
| **永不缓存** | keystore、`.p12`、`~/.cache/keyring`、Apple 专用密钥、`.env` | — | 见 §10 密钥规则；`docs/evidence/` 不入缓存 |

```yaml
# 设计片段：.github/workflows/pr.yml（Phase 1 骨架即为此形状）
name: pr
on:
  pull_request:
    branches: [main]
  push:
    branches: [main]
concurrency:                       # 同分支只留最新，取消在跑的旧 run
  group: pr-${{ github.ref }}
  cancel-in-progress: true
permissions:
  contents: read                   # 最小权限：pr 流水线不需要写
env:
  CARGO_TERM_COLOR: always
  CARGO_INCREMENTAL: "0"           # 缓存命中优先于增量
  RUST_BACKTRACE: "1"
jobs:
  static:
    strategy:
      fail-fast: false
      matrix:
        include:
          - os: ubuntu-22.04
            target: x86_64-unknown-linux-gnu
          - os: windows-2022       # 唯一允许 msvc host 的地方（镜像自带 VS）
            target: x86_64-pc-windows-msvc
    runs-on: ${{ matrix.os }}
    timeout-minutes: 15
    steps:
      - uses: actions/checkout@v4
      - uses: pnpm/action-setup@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.target }}
          components: rustfmt, clippy
      - uses: Swatinem/rust-cache@v2
        with:
          key: ${{ matrix.target }}
      - name: fmt
        run: cargo fmt --all --check
      - name: clippy (warnings are errors)
        run: cargo clippy --workspace --all-targets -- -D warnings
      - name: eslint + typecheck
        run: pnpm -w lint && pnpm -w typecheck
      - name: unit L0 (rust)
        run: cargo nextest run --workspace --all-features
      - name: unit L0 (frontend)
        run: pnpm -w test:unit -- --run
      - name: contract L2 (protocol.json vs SYNC_PROTOCOL_VERSION)
        run: pnpm -w exec tsx scripts/check-versions.ts --contract-only
```

> **禁止**在任何 job 里出现：`-A` 全量 `git add`、`--no-verify`、`#[allow]` 批量压制、把 clippy
> 降级为 `--cap-lints allow`、`continue-on-error: true` 挂在 gate job 上。这三者一旦出现在 diff 中，
> `audit` 的 grep 门直接 fail（见 §10 表「CI 自审」行）。

## Windows 工具链决策

三条路线的对照（左列 = 本机事实，非推测）：

| 维度 | A. 本机装 VS Build Tools，全程 MSVC | B. 本机 `x86_64-pc-windows-gnu` 开发+出包 | C. 本机不出包，Windows 构建只在 CI |
| --- | --- | --- | --- |
| 上游支持 | ✅ Tauri v2 前置条件明写「Microsoft C++ Build Tools」，只点名 `x86_64-pc-windows-msvc` / `i686-pc-windows-msvc` / `aarch64-pc-windows-msvc` `[上游]` | ❌ 官方 **未把 GNU/MinGW 列为受支持**（`[上游]`）→ 属未验证假设 | ✅ 与 A 同源，本机不承担构建 |
| 本机现状 | ⛔ VS/Build Tools 未装，需人工安装（数 GB） | ✅ 已装并可用：MinGW-W64 gcc 16.1.0 + `stable-x86_64-pc-windows-gnu` | ✅ 立即可用 |
| 纯 Rust crate（无 GUI） | 未知（未实测）`[假设]` | ✅ 实测：rusqlite(bundled 3.53.2+FTS5)、reqwest 0.13.5+rustls/aws-lc-rs、aes-gcm-siv、argon2、uuid v7 均可编译可运行 | ✅ 走 B 的 GNU 工具链跑测试 |
| 与 CI 一致性 | ✅ 与 `windows-2022` 镜像同一 host，本地/CI 行为同构，缓存与告警可复现 | ❌ **异构**：CI 用 msvc，本地用 gnu → 本地绿不代表 CI 绿，问题只在 CI 暴露（反馈变慢，且掩盖 ABI/链接差异） | ⚠️ 本机不做构建，异构风险收敛到 CI |
| 桌面窗口 / WebView2 | ⛔ 本机仍缺 WebView2 运行时，装完 Build Tools 后 **还需单独装 WebView2 Evergreen** 才能启动窗口 | ⛔ 同样缺 WebView2（与工具链无关） | ⛔ 同左；L5 只能 CI |
| 已知技术风险 | 无额外（MSVC 为默认路径） | ⚠️ ①`aws-lc-rs`/`ring` 的汇编与 `nasm` 依赖（本机 **无 nasm**，aws-lc-rs 侥幸通过，`ring` 未测）；②WebView2/COM 绑定在 gnu 的 `windows`/`webview2-com` crate 上是否完整生成 **未知**；③panic/unwind 模型与 SEH 差异影响 L4 崩溃测试语义；④`notera-richtext`/`notera-host` 若用到 C++/WinRT 或 `mt.exe` 资源嵌入则无解 | ⚠️ Windows 产物只能由 CI 生成 → GitHub 不可达期间 **无任何 Windows 产物**（§12） |
| 结论 | ✅ **主线（推荐）**：一次性安装 VS 2022 Build Tools「使用 C++ 的桌面开发」负载 + WebView2 Evergreen | 🟡 **仅允许作为过渡/降级**，且在 spike 通过前 **不得写入任何构建脚本、不得作为发布路线** | ✅ **A 落地前的临时约束**，同时作为 CI 的兜底纪律 |

**推荐（分层，不冲突）**：

1. **立即执行 C 的纪律**：本机不再声称能出任何安装包；本地只跑「无 GUI 的 crate 测试」，且必须显式
   `cargo +stable-x86_64-pc-windows-gnu` 而不是依赖默认 host（默认 host 现在连 proc-macro 都编不过）。
2. **人工完成 A**：装 VS 2022 Build Tools（C++ 桌面负载 + Windows 11 SDK）+ WebView2 Evergreen 运行时；
   装完后 `rustup default stable-x86_64-pc-windows-msvc`，并把 `git` 的 `/usr/bin/link.exe` 遮蔽问题作为
   验收项（从 PowerShell/`cmd` 侧构建，或把 MSVC `link.exe` 排到 PATH 之前）。A 落地即成为唯一开发+CI 同构路线。
3. **B 需先做 spike 且不得先信**：`tools/windows-gnu-spike/`（Phase 1 建），通过前不进任何门禁脚本。

### Spike 验收证据（B 路线唯一的解禁条件）

| 步 | 命令/动作 | 通过判据 | 证据落盘 |
| --- | --- | --- | --- |
| 1 | `cargo check --workspace --all-targets` | exit 0，且 **未** 触发 `link: extra operand` | 日志 → `docs/evidence/` |
| 2 | `cargo test --workspace` | 全绿；并确认 L4 崩溃用例在 gnu 下语义不变（`abort` / SEH 行为差异需记录） | 日志 + 差异说明 |
| 3 | `pnpm tauri build`（gnu host） | 产出 `.msi`/`.exe`，文件存在且 > 阈值大小；`bundle` 阶段无 `mt.exe`/`rc.exe` 缺失告警 | 产物 hash + 日志 |
| 4 | **启动窗口成功** | 双击可打开主窗口、前端渲染出 UI、能建库并写入/读回一条笔记（需先补装 WebView2）；截图 + 进程存活 ≥ 30s | 截图 + `run.log` |

四项 **全部** 有盘上证据，才允许把 windows-gnu 从 `[假设]` 升级为「可依赖」；任一失败即回退到路线 A，
并把失败原文留档（不留失败日志等于没做过 spike）。

### 目标机上的 WebView2（发布物必须处理，与本机无关）

| 事项 | 规定 |
| --- | --- |
| 打包方式 | Windows bundle 使用 **`offlineInstaller`** 模式内嵌 WebView2 固定版本安装器（`downloadBootstrapper` 要求目标机可联网，内网/离线环境不可用；`embedBootstrapper` 仍需在线下载安装器）`[上游]`，具体字段名 Phase 1 以官方 schema 校验 `[假设]` |
| 前置条件声明 | `docs/` 安装页 + release notes 明写「需 Windows 10 1809+ 与 Microsoft Edge WebView2 Evergreen 运行时」；企业内网分发提供独立离线安装器下载链接 |
| 运行时检测（应用侧硬要求） | 启动最早期探测 WebView2 运行时（HKLM/HKCU EdgeUpdate Clients 下 `{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}` 版本键 `[假设]` 待实测）；**缺失时必须弹原生对话框/命令行提示**，文案含「请安装 WebView2 运行时 + 下载 URL + 错误码」，**绝不允许白屏/静默退出** |
| 失败模式测试 | `e2e-desktop` 增加一条「运行时缺失」用例：在无 WebView2 的隔离环境（容器或新建 VM）里启动，断言退出码非 0 且 stderr/日志含指定错误码 → 本机 `[BLOCKED]`（无 `dot`、无虚拟化验证通道），Phase 8 落地 |
| 崩溃兜底 | L4 用例必须覆盖「WebView2 进程被杀」这一类失败，行为定义为可恢复 + 数据不丢 |

## Android 构建

本机状态：**`[BLOCKED]` — 无 JDK、无 Android SDK/NDK、无 `adb`**，因此以下步骤 **全部未在本机执行过**，
属设计规格，首次执行必须在 CI（或人工装齐工具链后）完成并按 §12 回贴证据。

### 构建步骤（`build-android` job）

| 步 | 命令 | 说明 / 待确认 |
| --- | --- | --- |
| 1 | `uses: actions/setup-java@v4` `distribution: temurin` `java-version: "17"` | Tauri 2 + AGP 需 JDK 17 `[上游]`，具体 AGP 最低 JDK 待首次 run 确认 `[假设]` |
| 2 | `uses: android-actions/setup-android@v3` | 提供 `sdkmanager` 与 command-line tools |
| 3 | `sdkmanager --install "platforms;android-35" "platform-tools" "build-tools;35.0.0"` | `compileSdk/targetSdk` 提案见下表；`android-35` 镜像内可用性 `[假设]` |
| 4 | `sdkmanager --install "ndk;27.2.12479018"`（**精确 patch 号必须 pin**） | Tauri 要求 NDK，官方仅写「NDK」→ 版本需 spike 后固化 `[假设]` |
| 5 | `export ANDROID_HOME=$RUNNER_WORKSPACE/Android/sdk`（由 setup-android 注入 PATH，仍显式导出）<br>`export NDK_HOME=$ANDROID_HOME/ndk/27.2.12479018` | 两者缺一即构建失败；必须 `echo` 到 job summary 便于回贴证据时核对 |
| 6 | `rustup target add aarch64-linux-android armv7-linux-androideabi` | 首发只需 `aarch64`；`armv7` 仅为「同一命令序列可扩」保留，产物矩阵首发不启用（见下） |
| 7 | `pnpm tauri android init` | 生成 `apps/desktop/src-tauri/gen/android/`（已在 `.gitignore` 中忽略 `build/`）；`init` 是否可重复执行/需 `--force` `[假设]` |
| 8 | `pnpm tauri android build --apk --target aarch64` | 只出 `arm64-v8a`；若改为通用 AAB 上架需单独 job（首发不做） |
| 9 | `apkanalyzer` / `aapt dump badging` 冒烟：断言 `native-code: arm64-v8a`、`minSdkVersion`、包名、`versionName == §7 版本` | L6 的 Android 冒烟 = 结构校验，**不装真机** |

| 参数 | 提案值 | 理由 | 状态 |
| --- | --- | --- | --- |
| `minSdk` | 24（Android 7.0） | 覆盖 WebView2/系统 WebView 的现代 API，且 sqlite FTS5/trigram 由 bundled rusqlite 提供，不受平台 sqlite 版本约束（`[实测]` bundled 3.53.2 与平台无关） | `[假设]` 待产品确认机型下限 |
| `targetSdk`/`compileSdk` | 35 | Play 商店政策每年上抬 `[假设]` | 待决策 §13 |
| ABI | 仅 `arm64-v8a`（`--target aarch64`） | 首发要求；同时上 `armv7` 会使构建时长×2、产物校验面翻倍 | 明确 |
| `abis` 拆分 | 通过 `tauri.android` 配置 + Gradle `splits.abi`，**产物文件名必须含 ABI** | 见 §9 命名 | Phase 1 落地 |

### 签名

| 情形 | 规则 |
| --- | --- |
| Release keystore | `Notera-release.keystore` 以 **base64 存 secrets**（`ANDROID_KEYSTORE_B64`），配 `KEYSTORE_PASSWORD` / `KEY_ALIAS` / `KEY_PASSWORD`（变量名 Phase 1 固化）；secrets 缺失 → 该 job 产 **debug** 包并 `SKIPPED-BY-MISSING-SECRETS`，**不得**命名为 release |
| keystore 保管 | 谁生成、备份在哪、丢失后能否升级已装用户（签名密钥不可换 → 已装用户无法覆盖升级）= **人工决策 §13**，本设计只留槽位 |
| Debug/未签名产物 | 文件名必须含 `-debug` 或 `-unsigned`，release job 断言 `if [[ $file == *debug* || $file == *unsigned* ]]; then exit 1; fi`，**永不** 作为 release 资产发布 |
| 密钥入仓 | `.gitignore` 已排除 `*.keystore`/`*.jks`/`*.p12`/`*.key`/`*.pem`/`.env*`；`gitleaks` 为 PR 门禁（§10） |
| CI 日志 | 严禁回显任何凭据（构建命令前 `set +x`，密码通过文件写入而非命令行参数，避免泄漏到 process list） |

## macOS 构建

本机事实：**Windows 上无法构建 macOS 产物**，且 `macos-14` runner 行为不可观测 → 全节 `[BLOCKED]`，
只能作为设计规格。

| 事项 | 规定 |
| --- | --- |
| Runner | `macos-14`（原生 Apple Silicon，**不需要** 交叉编译）；只出 `aarch64-apple-darwin` |
| 工具链 | `rustup target add aarch64-apple-darwin`；`xcode-select` / CLT 由镜像提供 `[上游]`，`xcodebuild -version` 输出必须打到 job summary 以固定「本次发布用的 Xcode 版本」 |
| 构建 | `pnpm tauri build --target aarch64-apple-darwin`（`--bundles dmg app`） |
| 产物 | `Notera-<ver>-macos-arm64.dmg` + `Notera-<ver>-macos-arm64.app.zip`（zip 用于 updater 与「右键打开」文档指引；dmg 用于人肉安装） |
| Universal binary | 首发 **不做** `x86_64-apple-darwin`；架构上保留 `--target` 数组扩展位（Intel 用户走 `[假设]` 无需求，待决策） |
| 签名 + 公证 | **需要 Apple Developer ID Application 证书 + `notarytool` 凭据（`APPLE_ID`/`APPLE_APP_PASSWORD`/`APPLE_TEAM_ID` 或 App Store Connect API key）→ 无证书即 `[BLOCKED]` / 待决策 §13**。未配置 secrets 时：跳过 `codesign`+`notarytool`，产物打 `-adhoc` 或 `-unsigned` 后缀，**不发布为正式 release** |
| Ad-hoc 签名后果（必须在文档中如实写） | Gatekeeper 拦截「无法打开，因为无法验证开发者」/「已损坏」；**每次新构建都需重新右键打开一次**；`xattr -dr com.apple.quarantine /Applications/Notera.app` 是权宜手段；跨小版本升级可能因签名不一致失败；Tauri updater 的 `.app.zip` 在无签名/无更新签名密钥时不可用 |
| 文档义务 | `docs/` 必须有《未签名 macOS 构建的打开方法》：①优先下载已公证版本；②`dmg` 拖入 `/Applications`；③Finder 中 **右键 → 打开**（或「系统设置 → 隐私与安全性 → 仍要打开」）；④命令行 `xattr -dr com.apple.quarantine`；⑤明确风险提示 + 校验 `checksums.txt` 的 SHA-256 步骤；不得只写「忽略警告即可」 |
| Tauri updater 签名 | 需 `TAURI_SIGNING_PRIVATE_KEY`(+password) 生成 `.sig`；密钥生成/保管为人工决策 §13；secrets 缺失则 release notes 明写「本版本不提供自动更新通道」 |
| 冒烟（L6 macos） | CI 内在 runner 上 `open` 一次并检查进程/窗口存在性 `[假设]`（runner 无显示器，可能需 headless 处理），Phase 8 明确 |

## 版本与单一版本源

| 项 | 规定 |
| --- | --- |
| **权威版本位置** | 根 `Cargo.toml` 的 `[workspace.package] version` —— 这是唯一可手改的版本号 |
| 派生位置 | `apps/desktop/package.json` `version`、`apps/desktop/src-tauri/tauri.conf.json` `version`、`crates/*/Cargo.toml`（一律 `version.workspace = true`）、`protocol.json` 的 `app_version` 字段 |
| **独立的第二权威** | `SYNC_PROTOCOL_VERSION`（Rust `notera-sync` 中的 `u16` 常量）与产品版本 **不同轴**：产品发 0.1.1 可以不改协议，协议变更必须同时改 `protocol.json` 与本常量 |
| 一致性检查 | `scripts/check-versions`（Rust 或 tsx 实现，CI 与本地 pre-commit 同一入口）：读取权威值并逐处比对，**任一漂移即 exit 1**；`pr` 与 `release` 都跑 |
| 允许的写入口 | 只有 `scripts/bump-version <x.y.z>` 可以批量改派生位置（禁止「顺手编辑其中一个」）；该脚本改动必须与 bump 同 commit |
| SemVer 计划 | `0.1.0` 首发三产物（内部可用/可复现安装）→ `0.2.0` 功能补全（同步/WebDAV/导入完整）→ `1.0.0` 承诺数据格式与协议兼容策略 |

```yaml
# 设计片段：scripts/check-versions 的失败面（伪 YAML，Phase 1 实现为真实脚本 + 单测）
checks:
  - id: V1-single-source
    desc: 除权威位置外，禁止在任何 Cargo.toml 写死 version
    fail: grep -R -n '^version = "' crates/*/Cargo.toml
  - id: V2-lockstep
    desc: workspace version == package.json version == tauri.conf.json version == protocol.json.app_version
    fail: 打印 diff（左权威 右实际）后 exit 1
  - id: V3-protocol-declared
    desc: protocol.json 必须含 protocol_version / min_protocol / max_protocol / breaking_since 且为正整数
  - id: V4-no-silent-break
    desc: git diff 中 protocol_version 增大且 major.minor 未变 → exit 1（0.x 阶段破坏性变更必须升 minor）
  - id: V5-artifact-name-matches
    desc: 产物文件名里的版本必须等于权威版本（防发布错版本）
```

### 同步协议变更闸门

| 规则 | 强制性 | CI 表达 |
| --- | --- | --- |
| 破坏性变更（旧客户端读不懂 / 会读坏）→ **必须升 minor**；`0.x` 阶段 **不得静默**（禁止只改代码不改 `protocol.json`） | 硬 | `V3`/`V4`；并加 `P1`：`protocol.json` 未在 diff 中但 `SYNC_PROTOCOL_VERSION` 变了 → fail |
| `protocol.json` 记录 `min_protocol` / `max_protocol`，握手时 **双端协商**（各自取交集，无交集则明确降级） | 硬 | L2 契约测试：穷举 `{min,max}` 组合与「无交集」分支（用例见 TEST-PLAN，本文不重述） |
| **老客户端遇到新协议 → 必须只读，不得写回**（不得产生任何 WAL 写入、不得写 sync 游标、不得改 `user_version`） | 硬（发布门禁） | L2/L3 用例：以 `protocol_version = N+1` 的服务端跑旧客户端实例，断言 ①无写请求发出（`notera-test-webdav` 记录 PROPPATCH/PUT/MOVE 计数 == 0）②UI 明示「数据库由更新版本创建，当前为只读」③重启后仍保持只读；此用例失败 = PR 不可合 |
| 协议版本必须落入数据库/导出文件元数据，便于取证 | 软→Phase 8 转硬 | 迁移与导出格式契约测试 |

## Migration 契约

| 项 | 规定 |
| --- | --- |
| 形态 | `migrations/NNNN_snake_name.sql`，**编号严格递增、只前进（forward-only）**；不存在 down 脚本、不存在「条件回滚」 |
| 版本载体 | SQLite `PRAGMA user_version`（`[实测]` 探针 `migration-user_version` 验证 `applied=[1,2,3] version=3` 路径可行） |
| 文件不可变 | 已发布过的迁移文件 **禁止修改**（`audit` 里对 `migrations/**` 的 diff 做守卫：若文件名已存在于上一个 tag 而内容变化 → fail） |
| 事务性 | 每个迁移文件在单事务内执行，失败即整体回滚且不推进 `user_version` |

CI 必须运行的四类检查（映射到 job）：

| # | 检查 | 实现 | job | 失败判据 |
| --- | --- | --- | --- | --- |
| M1 | 空库 → 最新 | 新建临时库，`migrate(None→HEAD)` | `pr` | 任一 SQL 错误 / `user_version != HEAD` |
| M2 | 每个历史版本 → 最新（升级路径矩阵） | 对 `migrations/` 里每个前缀长度 `k`：构建 v_k 库（用 **该版本当时的代码**，通过 git worktree checkout tag 或保留 fixture `.db` 二进制）再升到 HEAD | `integration` | 任一路径失败；矩阵项数 < `count(migrations)` 也失败（防「悄悄少测一个」） |
| M3 | 升级后数据完整性 | 迁移前灌入确定性合成数据并记录：每表 **行数** + 规范化后的 **SHA-256 校验和**（列按稳定顺序序列化，NULL/文本/时间戳规范化）；升级后逐表比对，允许的差异必须写进 `expected_diffs/<N>.json` 白名单 | `integration` | 校验和漂移且不在白名单 |
| M4 | 拒绝未知未来版本 | 打开 `user_version = HEAD+1` 的库：必须进入 **只读模式**、给出可操作错误文案、不执行任何迁移、不改文件（比对 `st_mtime`/mtime_ns 与哈希） | `crash` | 有任何写行为或 `user_version` 变化 |
| M5 | 禁止运行期 DDL | CI grep 测试（见下） | `pr` | 命中且不在 allowlist |

```yaml
# 设计片段：M5 —— 运行期 ALTER TABLE 守卫（放在 pr 的静态检查步骤）
- name: no runtime DDL outside migrations/
  shell: bash
  run: |
    set -euo pipefail
    # 允许 DDL 的地方只有 migrations/*.sql 与 tests/**/fixtures
    hits=$(grep -RnE --include='*.rs' \
      'ALTER[[:space:]]+TABLE|CREATE[[:space:]]+TABLE|DROP[[:space:]]+TABLE|PRAGMA[[:space:]]+user_version[[:space:]]*=' \
      crates/ apps/ || true)
    if [ -n "$hits" ]; then
      echo "$hits"; echo "DDL 只能出现在 migrations/ —— 见 docs/CI-CD.md §Migration 契约"; exit 1
    fi
```

> 上面这条 grep 对 `PRAGMA user_version` 的赋值也拦（迁移执行器自身所在文件需显式 allowlist，
> 且 allowlist 变更必须两人评审）。grep 是 **下限保障**，不替代 M1—M4 的真实执行。

## 产物与发布

### 命名约定：`Notera-<version>-<platform>-<arch|triple>[.<ext>]`

| 产物 | 文件名 | 内部必须可查到的元数据 |
| --- | --- | --- |
| Windows x64 安装包 | `Notera-0.1.0-windows-x64.msi`（`x86_64-pc-windows-msvc`） | FileVersion/ProductVersion = `0.1.0`+commit sha（资源段）`[假设]` Tauri 是否写此字段待验 |
| macOS arm64 | `Notera-0.1.0-macos-arm64.dmg`、`Notera-0.1.0-macos-arm64.app.zip`（`aarch64-apple-darwin`） | `Info.plist` `CFBundleShortVersionString`；`.app/Contents/MacOS` 里嵌 `GIT_SHA` |
| Android arm64 | `Notera-0.1.0-android-arm64-v8a.apk`（`aarch64-linux-android`） | `versionName=0.1.0`、`versionCode` 递增、`native-code: arm64-v8a`；未签名必须 `-unsigned`/`-debug` 后缀 |
| CLI / 探针（可选资产） | `Notera-0.1.0-cli-windows-x64.exe` 等 | `notera-cli --version` 必须输出 `0.1.0+<sha>` |
| SBOM | `Notera-0.1.0-sbom.cdx.json` | CycloneDX |
| 校验和 | `checksums.txt`：`sha256  <filename>`，**每行两空格分隔**（与 `sha256sum -c` 兼容） | 生成后由另一个 job 独立复算验证 |
| 变更日志 | `CHANGELOG-0.1.0.md`（从 conventional commits 生成 + 人工补「已知问题/未签名说明」） | — |

规则：**任何产物文件名不含 version 与 sha 即视为发布失败**；workflow 上传前对每个文件跑
`test -s`（非零）+ `file` 类型断言 + 体积阈值（`[假设]` 阈值数值待首个产物定）。commit sha 取
`git rev-parse --short=7 ${{ github.sha }}`（tag 触发时即 tag 指向的提交，不取工作区脏值）。

### `release` job（只响应 tag `v*`）

```yaml
# 设计片段：.github/workflows/release.yml
name: release
on:
  push:
    tags: ["v*"]
permissions:
  contents: write                  # 仅 release 需要写
jobs:
  gates:                           # 汇总节点：把 7 个 gate 变成 release 的前置
    runs-on: ubuntu-22.04
    needs: [pr, integration, crash, e2e-desktop, audit, build-windows, build-macos-android]
    if: startsWith(github.ref, 'refs/tags/v')
    steps:
      - run: echo "all release gates green"
  publish:
    runs-on: ubuntu-22.04
    needs: gates
    environment: release           # 可挂人工审批 + 记录谁批的
    steps:
      - uses: actions/checkout@v4
      - uses: actions/download-artifact@v4
        with: { name: windows-msi, path: dist }
      - uses: actions/download-artifact@v4
        with: { name: macos-arm64, path: dist }
      - uses: actions/download-artifact@v4
        with: { name: android-arm64-v8a, path: dist }
      - name: refuse debug/unsigned artifacts in a release
        run: |
          set -euo pipefail
          if ls dist | grep -qE '(debug|unsigned|adhoc)'; then
            echo "release 资产含未签名/调试产物，拒绝发布"; ls dist; exit 1; fi
      - name: checksums (recomputed independently)
        run: |
          set -euo pipefail
          (cd dist && sha256sum $(ls | grep -v checksums.txt) > checksums.txt && sha256sum -c checksums.txt)
      - name: version must match tag
        run: test "$(grep -m1 '^version' Cargo.toml | cut -d'\"' -f2)" = "${GITHUB_REF_NAME#v}"
      - name: publish
        env: { GH_TOKEN: "${{ secrets.GITHUB_TOKEN }}" }
        run: |
          gh release create "$GITHUB_REF_NAME" \
            --title "Notera ${GITHUB_REF_NAME#v}" \
            --notes-file dist/CHANGELOG-${GITHUB_REF_NAME#v}.md \
            dist/Notera-*.{msi,dmg,zip,apk} dist/checksums.txt
```

| 事项 | 规定 | 状态 |
| --- | --- | --- |
| Artifact 保留 | Actions artifacts 默认保留期需在仓库设置里显式配置（设计目标 **90 天**；nightly-soak 的日志 30 天） | `[假设]` 默认值不可依赖，必须核对仓库设置 `[BLOCKED]` |
| `checksums.txt` 分发 | 与产物同 release；release body 顶部固定写「安装前先 `sha256sum -c`」+ 未签名平台提示 | 明确 |
| 发布前置 secrets | `APPLE_*`、keystore、`TAURI_SIGNING_PRIVATE_KEY`；缺任一 → 该资产后缀化且 release body 列 `NOT-PUBLISHED-BECAUSE:` 清单 | 待 §13 |
| 幂等 | 重跑同一 tag 不得产生重复资产（`gh release create --verify-tag` + 先删同名资产）；`[假设]` 具体 flag 组合待首次验证 | `[BLOCKED]` |
| iOS | 架构预留：`platform/ios/` + `tauri ios` 的 job 以 `workflow_dispatch` + `runs-on: macos-14` 写好形状但 `if: false` 冻结，首发不产 iOS 资产 | 明确 |

## 可复现性与依赖治理

| 项 | 规定 | 门禁 |
| --- | --- | --- |
| 锁文件 | `Cargo.lock`（含 workspace 与 `tools/feasibility-probe`）与 `pnpm-lock.yaml` **必须入库**；`pr` 跑 `--locked`（`cargo build/test --locked`、`cargo install --locked`），锁文件与 `Cargo.toml` 不匹配即 fail | `pr` |
| 工具链 pin | 根 `rust-toolchain.toml`：`channel = "1.98.1"`（精确到 patch，`[实测]` 本机即该版本）+ `components = [rustfmt, clippy]` + `targets = [...]`（按平台列 msvc / aarch64-apple-darwin / aarch64-linux-android）+ `profile = "minimal"` | `docs`（校验文件存在且 channel 与 CI 实际 `rustc -vV` 一致） |
| 许可证/告警 | `deny.toml`：`licenses.allow` 白名单（MIT/Apache-2.0/Unicode-DFS/BSD-3/OpenSSL 等，`aws-lc-rs`、`ring`、`sqlite3` 绑定需单独确认）、`yank = deny`、`unknown-registry = deny`、`bans`（重复 crate 版本上限，`[实测]` 探针已引入 `cmake`/`cpufeatures` 等，需 `duplicates.allow` 精确列出） | `audit` |
| 安全告警 | `cargo audit --deny warnings`；`cargo update` 单独 PR（不允许与功能混提）；豁免必须写 `audit.toml` + `expires` + 理由 + issue 链接，**无到期日的豁免 = fail** | `audit` |
| 前端依赖 | `npm audit --audit-level=high`（`[实测]` npm 11.13.0 / pnpm 12.5.1）；`packageManager` 字段锁定 pnpm 版本；postinstall 脚本需 allowlist（`pnpm.onlyBuiltDependencies`） | `audit` |
| SBOM | `cargo sbom`（CycloneDX）或 `syft dir:. -o cyclonedx-json`，作为 release 资产 `Notera-<v>-sbom.cdx.json`；两条路径哪条为准 Phase 1 试一次 `[假设]` | `release` |
| 二进制供应链 | 禁止 `curl \| sh`、禁止从非 crates.io/npmjs 源拉构建脚本；所有第三方 Action 用 **commit SHA 或精确 tag**（禁 `@main`/`@latest`） | `audit`（grep `uses:` 行） |
| CI 自审（防门禁被悄悄拆） | grep 全仓 `.github/`：`--no-verify`、`continue-on-error: true`（在 gate job 上）、`--cap-lints`、`allow(warnings)` 扩散、`-D warnings` 被移除、`needs:` 被缩短、`if: false` 新增 | `audit` |
| secret 扫描 | `gitleaks detect`（全历史 + PR diff）+ 提交前本地钩子；**禁止 `--no-verify`**，`.gitignore` 已排 `*.pem *.key *.p12 *.keystore *.jks .env*` | `audit`（PR 阻塞） |
| 更新节奏 | `nightly-soak` 附带「依赖新鲜度报表」（`cargo outdated` + `pnpm outdated`），只出报告不 fail；安全类例外走 `audit` | `nightly` |
| 缓存投毒/回滚不可见 | 缓存 key 含 `Cargo.lock`/`pnpm-lock.yaml` 哈希（见 §3），换依赖必然失效；**禁止用 `restore-keys` 让 `target/` 跨 toolspace 兜底** | `pr` 配置评审 |

## 环境与网络限制（本机现状）

| 能力 | 状态 | 证据 | 解锁条件（唯一路径） |
| --- | --- | --- | --- |
| Rust 无 GUI crate 构建（`notera-core/store/crypto/sync/net/webdav/config/importer`） | 可用 | `[实测]` GNU host 探针 16/16 PASS，含 rusqlite bundled 3.53.2 + FTS5、reqwest 0.13.5 + aws-lc-rs、aes-gcm-siv、argon2、uuid v7 | 已解锁（必须显式 `cargo +stable-x86_64-pc-windows-gnu`） |
| 默认 msvc host 构建 | **缺失** | `[实测]` `link: extra operand`（Git 的 coreutils `link.exe` 遮蔽 MSVC linker），proc-macro 都编不出 | 安装 VS 2022 Build Tools「C++ 桌面负载」+ 修正 PATH 顺序 |
| `cargo fmt` / `clippy` 本地跑 | 可用（GNU host 下） | `[假设]` 探针未跑 clippy/fmt，仅推断 rustup 组件可用 → Phase 1 首次本地执行时确认 | 已解锁（结论待实测） |
| Tauri 桌面窗口启动（本机 L5、L6-win 冒烟） | **缺失** | `[实测]` 无 WebView2（`Microsoft\Edge\Application` 与 `EdgeCore` 均无） | 安装 WebView2 **Evergreen 运行时** |
| 架构图渲染（`docs` 的本地等价检查） | **缺失** | `[实测]` 无 graphviz `dot` | 装 graphviz 或把该检查只留在 CI（推荐后者） |
| SQLite CLI 交叉验证 | **缺失** | `[实测]` 无 `sqlite3` CLI | 不需要（一律用 rusqlite 走代码断言），文档需说明 |
| Android 构建 | **BLOCKED** | `[实测]` 无 JDK / 无 SDK / 无 NDK / 无 `adb` | 本机装 JDK17 + Android SDK/NDK；或只在 `ubuntu-22.04` CI 出包 |
| iOS 构建 | **BLOCKED** | 平台事实：Windows 不能构建 iOS | macOS 14 + Xcode（本机永久不可行） |
| macOS 产物构建 | **BLOCKED** | 同上 | `macos-14` runner 或借用人手 Mac |
| 观察 GitHub Actions 运行 | **BLOCKED** | `[实测]` `github.com` / `api.github.com` curl `000` | **可访问 GitHub 的推送通道**（人工 push，回贴 Actions URL + 日志） |
| crates.io / npm 依赖拉取 | 可用 | `[实测]` 四个域 HTTP 200（探针 `https-tls-handshake` PASS） | 已解锁 |
| 真实 WebDAV 服务器兼容性测试（Nextcloud/坚果云/阿里 OSS WebDAV 等） | **BLOCKED** | 设计只有 `notera-test-webdav`（127.0.0.1）；真实服务器凭据按 §1 非目标禁止入 CI | 人工提供 **可访问的真实 WebDAV 服务器**，且只在 `workflow_dispatch` 的独立 job 跑（不进 PR 门禁，不写日志凭据） |
| Apple 签名/公证、Android release keystore、updater 签名密钥 | **BLOCKED** | 无证书/无私钥/secrets 未配置（`[实测]` 本机无相关凭据文件） | 见 §13，secrets 落地后才能验证 |
| Windows CI 首次绿灯、artifact 下载、release 上传、缓存命中、runner 矩阵行为、Actions 版本漂移 | **BLOCKED（全部）** | `[实测]` GitHub 不可达 → 本文任何 workflow 均 **未被执行过一次** | §12 交接协议首轮闭环 |

> **不得声称已验证**：截至本文写就，**没有一次流水线运行被任何人观察到**。`[假设]` 条目在 Phase 1
> 首次真跑时必须逐条转为「通过 / 失败」并附 run URL，否则视为未闭环。

## 交接协议

前提：开发机无法访问 `github.com`/`api.github.com`，因此 **agent 永远看不到 Actions 结果**。

| 步 | 谁 | 动作 | 产出 |
| --- | --- | --- | --- |
| 1 | agent | 写/改 workflow 与 `scripts/`，在本地跑 **能跑的** 部分（GNU host 的 `cargo check/test/clippy/fmt`、`pnpm` 检查），并列出「本机不可验证清单」 | commit（**不 push、不 `git` 由 agent 执行**，仓库所有者提交） |
| 2 | 人工 | 在可访问 GitHub 的网络下 `git push` 并按需打 tag | push 结果 / PR 链接 |
| 3 | 人工 | 打开 Actions 页面，回贴：**run URL + run id + commit sha + 失败 job 名 + 该 job 完整原始日志（或 `gh run view --log --job=<id>` 输出）** | 日志文本，存入 `docs/evidence/ci-<run-id>.md` |
| 4 | agent | **以该日志为唯一证据源**定位（不许凭想象改 YAML），产出最小修复 commit + 更新本文 §3/§11 的状态列（`[假设]`→`[实测]`/失败） | commit + 文档回写 |
| 5 | 人工 | 再 push，再回贴 | 循环，直到绿灯 |

| 规定 | 内容 |
| --- | --- |
| 证据文件最小字段 | `# ci-<run-id>` / `run_url:` / `commit:` / `started_at:` / `conclusion:` / `jobs:`（每个 job 的 结果 + 耗时 + 缓存命中/miss 实测值） / `raw_log:`（粘贴失败段原文） |
| 禁止的表述 | 拿到 §12 步骤 3 的证据前，任何文档、PR 描述、commit message、ADR 中 **不得出现**「CI 已通过 / 流水线绿灯 / 构建成功 / 缓存已验证」；只能写「设计完成，等待首次运行证据（BLOCKED）」 |
| 禁止的替代 | 不得用「本地跑通了」推定 CI 跑通（本机 msvc 编不过 + runner 镜像不同构，两个方向都推不出来） |
| 失败优先级 | 先修 `pr`→再 `integration`/`crash`→再 `build-*`→再 `e2e-desktop`→最后 `release`；每层绿灯前不启动下一层的排错 |
| 版本漂移观测 | 每次 run 必须记录 runner 报告的 Node/Action 版本与 `rustc -vV`，写进证据文件——这是发现「镜像悄悄升级」的唯一手段 |
| 缓存可观测 | 每个 Rust job 打印缓存命中/miss（`Swatinem/rust-cache` 的 `cache-hit` 输出 + 首步耗时对比）；预算表（§2）以实测均值替换设计值 |

## 待人工决策

| # | 决策 | 选项 | 影响 | 阻塞什么 |
| --- | --- | --- | --- | --- |
| D1 | **本机路线：MSVC vs GNU** | ①装 VS 2022 Build Tools + WebView2（与 CI 同构，上游支持，推荐）；②继续 windows-gnu（今天能跑，但 Tauri/COM/`aws-lc-rs`/`ring`/unwind 风险未评估，上游不支持）；③本机不出包（纯 C 纪律） | 决定开发者体验、反馈延迟、以及 L4 崩溃测试语义是否跨工具链一致 | 阻塞 `build-windows` 的本地预验证；阻塞是否投入 spike |
| D2 | **Apple 证书** | ①买 Apple Developer ID（¥688/年量级）+ notarytool → 正式公证发布；②**自签/不签**：只发 `-adhoc` 包 + 文档写清「右键打开/`xattr` 去隔离」并明示风险 | macOS 用户首启体验、自动更新（updater 需签名密钥）能否成立 | 阻塞 `build-macos-android`(macos leg) 的签名步与 `release` 的 mac 资产 |
| D3 | **Android release keystore 由谁保管** | ①人工保管、agent 永不见明文，CI 只读 secrets；②团队密钥库/密码管理器；③首发先出 `-unsigned`，正式版后补 | **签名密钥不可更换**：一旦丢失，已装用户无法覆盖升级 | 阻塞 `release` 的 android 资产与升级策略 |
| D4 | **GitHub 推送通道** | ①人工 push（本协议）；②给 agent 一个可达 GitHub 的网络/代理；③自建 CI（Gitea/Forgejo）以绕开不可达 | 决定本文全部 `[BLOCKED]` 何时能清；也决定 §2 预算表能否回填实测值 | 阻塞 **所有** CI 验证，是当前第一优先级 |
| D5 | **真实 WebDAV 兼容测试由谁提供服务器** | ①人工提供一台可公网访问的测试专用实例（Nextcloud/坚果云等），仅 `workflow_dispatch`；②本地容器化多个上游服务器（`notera-test-webdav` 加方言模式）覆盖兼容性；③暂不做真实兼容 | 决定 §1 非目标「不接触真实账号」的边界如何表述；L3 之外是否需要 L3b | 阻塞兼容性结论与首发「能同步」的可信度 |
| D6 | **制品是否需内网镜像分发** | ①公开 GitHub Releases；②额外提供内网镜像/网盘 + 独立 WebView2 offlineInstaller 下载；③只走内网（则需私有 registry 镜像 crates/npm，CI 需自建） | 影响 WebView2 打包模式选择（禁 `downloadBootstrapper`）、依赖拉取源、以及 `deny.toml` 的 `unknown-registry` 策略 | 阻塞 `release` 的目标平台与镜像配置 |
| D7 | Windows 安装包格式 | ①`.msi`（企业分发/静默安装友好）；②`.exe`(nsis) 单文件（个人下载友好）；③两者都出（构建时长 +约 40%） | 产物矩阵、`checksums.txt` 行数、文档安装指引 | 阻塞 §9 资产清单 |
| D8 | `minSdk` / `targetSdk` 数值确认 | 见 §5 提案（24 / 35） | 覆盖机型范围、上架政策合规 | 阻塞 Android 构建参数固化 |
| D9 | nightly 时长与硬件预算 | 3h 采样 vs 真 72h；是否需要 self-hosted runner 控成本 | 分钟数配额、泄漏曲线可信度 | 阻塞 `nightly-soak` 的 cron 设计 |
| D10 | 豁免策略归属 | `cargo audit` 豁免、`deny` 的 `duplicates.allow` 由谁批准、issue 跟踪在哪 | 安全门禁是否会被静默拆掉 | 阻塞 `audit` 落地为硬门禁 |

## 附录：Phase 1 骨架 / Phase 8 全量的边界

| Phase 1（骨架，本次设计完成后第一件事） | Phase 8（全量） |
| --- | --- |
| `.github/workflows/{pr,integration,build,release}.yml` 四个文件成形；`rust-toolchain.toml`；`deny.toml` 最小可用；`scripts/check-versions`、`scripts/no-runtime-ddl`；`migrations` 的 M1/M5；`build-windows`(msvc) + `build-macos-android` 各出一条可下载产物（允许 `-adhoc`/`-unsigned` 后缀）；`docs` 的 markdownlint + `ARCHITECTURE-MAP` 检查 | `crash`(L4)、`e2e-desktop`(L5)、`audit` 全项、`nightly-soak`、M2/M3/M4 升级路径矩阵与校验和白名单、SBOM、`checksums.txt` 复算、三平台签名链、无 WebView2 场景断言、门禁自审 grep、预算表用实测值替换 |
