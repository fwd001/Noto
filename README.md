# Notera

> 工作代号。商标与域名核查未完成，改名只影响 brand 层，不影响架构（见 ARCHITECTURE.md §8 B7）。

**Local-first 跨平台个人笔记软件**：数据由用户自己掌控，同步后端是用户自有的 WebDAV 服务器，支持 App 级代理，四端原生体验，数据安全优先。

用户体验目标只有一个动作链：

```text
安装 → 打开 → 直接写笔记 → 自动保存 →（配好同步之后才）自动同步
```

同步机制（WebDAV / Revision / Manifest / Tombstone / ETag / Pull / Push / Merge / Conflict）属于内部实现，用户只看到五格：`✓ 已同步` `↻ 正在同步` `○ 离线` `! 同步失败`，外加一格静止的 `· 未配置同步 / 同步已关闭`。

**装完是干净的**：没配账户、或把「启用同步」关掉，就不该有任何一轮同步在跑，也不该有那颗转着的圈在暗示"有东西正在上传"（一切以用户配置的设置为主）。

---

## 项目状态

**Phase 0（架构）已完成，等待人工审核。审核通过前不写业务代码。**

当前仓库里只有：架构文档、可交互契约图、技术可行性探针。**没有**产品代码 —— 这是刻意的。

优先级顺序（决策冲突时按此裁决）：

```text
数据安全 > 同步正确性 > 稳定性 > 用户体验 > 原生平台体验 > 性能 > 可维护性 > 新功能
```

---

## 技术栈

Tauri 2 · Rust · Vue 3 + TypeScript + Vite · SQLite（rusqlite bundled，FTS5）· Tokio · reqwest

平台：Windows 10/11 x64 · macOS Apple Silicon · Android arm64-v8a · iOS/iPadOS（架构预留）

---

## 文档地图

| 文档 | 作用 |
|---|---|
| [ARCHITECTURE-MAP.md](docs/ARCHITECTURE-MAP.md) | **长期架构记忆**：模块注册表、不变式、改动路由、禁止模式。每次开工先读这个 |
| [ARCHITECTURE.md](docs/ARCHITECTURE.md) | 分层、依赖方向、数据流、实测结论、已知限制 |
| [DATA-MODEL.md](docs/DATA-MODEL.md) | SQLite 表与字段语义、revision 模型、富文本 schema |
| [SYNC-PROTOCOL.md](docs/SYNC-PROTOCOL.md) | WebDAV 布局、清单结构、同步状态机、崩溃恢复、错误分类 |
| [CONFLICT-RESOLUTION.md](docs/CONFLICT-RESOLUTION.md) | 冲突检测、块级三方合并、删除与反复活语义 |
| [PROXY.md](docs/PROXY.md) | App 级代理、TLS/证书、超时退避、可证伪测试 |
| [PLATFORM.md](docs/PLATFORM.md) | 四端能力矩阵、启动时序、后台同步诚实预算 |
| [TEST-PLAN.md](docs/TEST-PLAN.md) | 行为测试矩阵、测试分层、不变式、阶段出口条件 |
| [CI-CD.md](docs/CI-CD.md) | 构建矩阵、版本策略、迁移契约、交接协议 |
| [ADR/](docs/ADR/) | 每条重大架构决策一份记录 |
| [diagram/architecture.html](docs/diagram/architecture.html) | **可交互契约图**：16 模块 × 输入/输出/数据结构，点击穿透 |
| [evidence/](docs/evidence/) | 实测证据（探针输出、图渲染截图） |

---

## 契约图

```bash
# 直接在浏览器打开即可，单文件、零外部依赖、可离线
start docs/diagram/architecture.html      # 或双击文件
node scripts/verify-diagram.mjs           # 59 项浏览器实跑断言
```

---

## 技术可行性探针

Phase 0 不写"理论上可行"，只写实测结论。探针覆盖 16 项，全部通过：

```bash
# 本机默认 host 是 x86_64-pc-windows-msvc 但没有 MSVC 链接器，因此显式走 GNU host
cargo +stable-x86_64-pc-windows-gnu run -q --manifest-path tools/feasibility-probe/Cargo.toml
```

关键发现（完整输出见 `docs/evidence/probe-windows-gnu.txt`）：

* **FTS5 trigram 对 2 字中文查询静默返回 0 行**（实测 0 行 / 22 µs），而 `LIKE` 正确返回 416/5000 条（≈4.1 ms）→ 检索层必须是双路径，这是硬约束不是优化选项。
* ≥3 字检索：5000 条中文笔记 p50 = 32 µs、p95 = 1.4 ms、索引 6.0 MiB → 不需要外部分词器。
* 清单尺寸：条目 89 B；全量 5000 条 gzip 186 KiB；变更窗口 200 条 gzip 7.8 KiB → 否决"每轮重写全量清单"，采用两段式清单。
* AES-256-GCM-SIV：nonce 12 B、认证标签恒 16 B、篡改必拒；argon2id(19 MiB/2/1) ≈ 400 ms → KDF 禁止在 UI 线程。
* `aws-lc-rs` + `rustls` 在 `x86_64-pc-windows-gnu` 下可编译并完成真实 TLS 握手。
* `reqwest 0.13.5` 支持 SOCKS5 用户名/密码认证与 `socks5h` 远端 DNS（读源码确认，非推测）。

---

## 本机环境限制（如实记录）

| 能力 | 状态 | 说明 |
|---|---|---|
| Rust 无 GUI crate 开发 | ✅ | 走 `stable-x86_64-pc-windows-gnu` |
| MSVC 链接器 | ❌ | 未装 VS Build Tools；且 PATH 中 `link.exe` 被 Git coreutils 版顶替 |
| WebView2 运行时 | ✅ | 独立运行时 150.0.4078.105 已安装（此前误判为缺失：只查了 Edge 浏览器目录） |
| JDK / Android SDK / NDK | ❌ | 无法本地出 APK |
| GitHub 可达性 | ❌ | Actions 无法观察，CI 验证标 BLOCKED |
| crates.io / npm registry | ✅ | 依赖可解析 |

Phase 1–3（Local Core / Sync / Proxy）的全部验证不依赖 GUI，因此不受上述限制阻塞；GUI 与安装包验证从 Phase 4 起依赖 CI 通道。

---

## 开发纪律

1. 开工前读 `ARCHITECTURE-MAP.md` §0 清单。
2. 需求与架构冲突时：**停下**，新建 ADR 写明冲突与影响，等人工决定。不自行改架构、不猜。
3. 不"顺便"：不顺手重构、不顺手升依赖、不顺手换组件。
4. 测试失败时定位并修复，禁止删测试、降断言、`#[ignore]`、mock 掉核心逻辑。
5. 不可验证的项写 `BLOCKED` + 原因 + 影响 + 解除条件；`SKIP` / `TODO` / "理论通过" 不算通过。
6. 拿到 Actions 绿灯证据前，任何提交与文档不得出现"CI 已通过"。
