# ARCHITECTURE

Notera 系统架构。Phase 0 产物。规范性文档。

配套：[ARCHITECTURE-MAP.md](./ARCHITECTURE-MAP.md)（长期架构记忆与改动路由）· [DATA-MODEL.md](./DATA-MODEL.md) · [SYNC-PROTOCOL.md](./SYNC-PROTOCOL.md) · [CONFLICT-RESOLUTION.md](./CONFLICT-RESOLUTION.md) · [PROXY.md](./PROXY.md) · [PLATFORM.md](./PLATFORM.md) · [TEST-PLAN.md](./TEST-PLAN.md) · [CI-CD.md](./CI-CD.md) · [ADR/](./ADR/) · [交互契约图](./diagram/architecture.html)

---

## 1. 产品定位

Local-first 跨平台个人笔记软件，同步后端为用户自有的 WebDAV 服务器，支持 App 级代理，四端原生体验，数据安全优先。

优先级顺序（任何决策冲突时按此裁决，不得重排）：

```text
数据安全 > 同步正确性 > 稳定性 > 用户体验 > 原生平台体验 > 性能 > 可维护性 > 新功能
```

用户不应感知同步系统的存在：只看到 `✓ 已同步 / ↻ 正在同步 / ○ 离线 / ! 同步失败` 四格同步结果，外加一格静止的 `· 未配置同步 / 同步已关闭`（**没在同步**不等于"同步坏了"，更不等于"正在忙"），`WebDAV / Revision / Manifest / Tombstone / ETag / Pull / Push / Merge / Conflict` 全部是实现细节。

---

## 2. 技术栈

| 层 | 选型 | 版本（实测解析） | 理由 |
|---|---|---|---|
| 应用外壳 | Tauri 2 | v2 系 | 桌面 + 移动同一壳；Rust 核心可直接调用，无 FFI 层 |
| 核心语言 | Rust | 1.98.1 | 全部业务与同步逻辑 |
| 异步运行时 | Tokio | 1.53.1 | 后台同步、网络、队列 |
| 前端 | Vue 3 + TypeScript + Vite | pnpm 12.5.1 | 单份 UI 实现，四端复用 |
| 存储 | SQLite（`rusqlite` bundled） | rusqlite 0.40.2 / SQLite 3.53.2 | 事务、WAL、FTS5、JSON1 |
| 检索 | FTS5 `trigram` 分词 | 随 SQLite | 中文子串检索（实测决定，见 §6） |
| HTTP | `reqwest` | 0.13.5（rustls + aws-lc-rs） | 唯一网络出口；支持 socks 与 system-proxy |
| 加密 | `aes-gcm-siv` 0.12.1 / `argon2` 0.6.0 / `sha2` 0.11.0 | 实测通过 | 记录信封与内容寻址 |
| ID | `uuid` 1.26.1（v7） | 实测单调、路径安全 | 跨设备无需协调 |
| 凭据 | 系统钥匙串（Credential Manager / Keychain / Keystore） | — | 密码不落库 |

**明确不使用**：把 SQLite 数据库文件当同步载体；用 mtime 判定新旧；在 WebView 里实现同步协议；WebDAV 库的"现成同步语义"（多数实现不满足崩溃一致性要求，故只用语义原语自建协议）。

---

## 3. 分层与依赖方向

```text
┌──────────────────────────────────────────────────────────────────┐
│ L0 用户与平台   apps/desktop/src (Vue3+TS)      platform/{win,mac,android,ios}
│                 三栏信息架构 · 编辑器 View · 同步徽标（4 结果 + 1 静止）· 原生窗口/托盘/菜单/分享
└───────────────────────────────┬──────────────────────────────────┘
                                │ 命令 DTO ↓        ↑ ViewModel / 事件
┌───────────────────────────────┴──────────────────────────────────┐
│ L1 应用与装配     notera-host          notera-cli（诊断/E2E 驱动）
│                  用例编排 · 先出 UI 再启同步 · 调度 · 事件总线 · Tauri Commands
└──────┬───────────────┬──────────────────┬────────────────┬───────┘
       ↓               ↓                  ↓                ↓
┌──────────────────────────────────────────────────────────────────┐
│ L2 领域服务   notera-sync   notera-richtext   notera-importer   notera-config
│              状态机/计划/冲突 · 文档模型/合并 · 备份恢复 · 设置与代理配置
└──────┬──────────────────────────┬─────────────────────────────────┘
       ↓                          ↓
┌──────────────────────────────────────────────────────────────────┐
│ L3 基础设施   notera-store   notera-crypto   notera-webdav   notera-net
│              SQLite/FTS/迁移 · 信封/摘要/KDF · DAV 语义/原子写 · 代理/TLS/超时退避
└───────────────────────────────┬──────────────────────────────────┘
                                ↓
┌───────────────────────────────┴──────────────────────────────────┐
│ L4 内核   notera-core    类型 · ID · 时钟 · 错误词表 · 不变式断言
│                          零 I/O · 零 async · 零跨 crate 依赖
└──────────────────────────────────────────────────────────────────┘

L5 外部世界   用户自有 WebDAV 服务器 · 系统钥匙串 · 应用数据目录
测试专用      notera-test-webdav（真实 HTTP + 故障注入，仅 dev/test 依赖）
```

**依赖方向唯一**：上层依赖下层，禁止反向，禁止跨层回调。唯一"向上"的通道是 `notera-host` 的事件总线（订阅式，非调用）。

三条结构性铁律：

1. **前端不碰同步**：`apps/desktop/src` 里不出现 WebDAV/协议词汇，只发命令 DTO、只收 ViewModel/事件（CI grep 闸门）。
2. **网络出口唯一**：所有 HTTP 必经 `notera-net`；其他 crate 的 `Cargo.toml` 里没有 `reqwest`（依赖图 + grep 双闸门）。
3. **写入入口唯一**：所有权威表写入必经 `notera-store` 的 `commit_*`，派生列与 FTS 在同一事务内刷新。

---

## 4. 数据流

### 4.1 本地写入（离线也必须成立）

```text
用户输入
 → UI 乐观更新（立即，不等任何 IO）
 → Command DTO → notera-host（校验 + 乐观并发 expectedRev）
 → notera-richtext（normalize → validate → canonical → 派生 title/plain_text）
 → notera-store 单事务：notes + note_revisions + 派生列 + notes_fts + sync_operations(outbox)
 → 提交完成（≈1.2 s debounce 后）→ 通知 notera-host "有脏数据"
 → host 调度器 debounce 2.5 s → SyncEngine 后台起一轮
```

网络完全不在这条路径上。断网、代理挂、服务器 500、Token 过期 —— 用户写字的手感不变。

### 4.2 同步一轮（详见 SYNC-PROTOCOL §6）

```text
Trigger → 读 manifest/index.json (If-None-Match)
        → 304 且无脏 → 结束（1 请求 0 字节）
        → 200 → 校验 checksum → 生成 SyncPlan（P1..P18 判定表）
        → push 实体记录（tmp → 校验 → MOVE，按 cap_mask 选 S1/S2/S3）
        → pull 变更实体（并发 GET ≤ 8）
        → 冲突交 notera-richtext 三方合并；失败则保留双方 + 冲突副本
        → 本地 apply（单事务）
        → CAS 提交 manifest（seq+1，重写 index.json，窗口 7.8 KiB 量级）
        → RoundStats → 事件回流 UI
```

### 4.3 附件（独立队列，永不阻塞文本）

```text
插入 → 读字节 → sha256 → tmp+rename 落盘（内容寻址）→ 同事务写引用 + outbox(upload)
后台 upload 队列：清单已含该 sha → 跳过（去重）；否则流式 PUT → Range 复验 → present
download 队列：缺失且远端有 → GET → 校验 → 原子 rename
```

---

## 5. 同步模型要点（为什么这样设计）

| 决策 | 理由 | 出处 |
|---|---|---|
| 记录文件是正确性来源，清单只是索引 | 清单损坏/落后不丢数据，可扫描重建 | SYNC-PROTOCOL R1 |
| 每实体一个可变 JSON 文件 + 先实体后清单 | 少请求、原子、崩溃可判 | ADR-0003 |
| `rev = max(local, remote) + 1`，不用向量时钟 | 单调整数 + 共同祖先点足以判真并发，向量时钟带来的复杂度无收益 | ADR-0005 |
| 清单两段式（基线分段 ⊕ 变更窗口） | 每轮成本与库规模解耦：窗口 200 条 ≈ 7.8 KiB gzip，全量 5000 条 ≈ 186 KiB | 实测 |
| Tombstone 不自动 GC | 自动 GC 必然留下"离线超期设备复活已删笔记"的窗口 | ADR-0006 |
| 文件夹删除不级联删笔记 | 误触不摧毁内容；避免 N 条墓碑放大 | ADR-0006 |
| 冲突默认保留双方（块级可无损合并则自动合） | 替用户二选一 = 静默丢数据 | ADR-0007 |
| 富文本独立模型，编辑器只是 View | 换编辑器不触碰同步层 | ADR-0008 |
| 信封 v1 即带 `enc` 字段（`alg=none`） | 日后启用 E2EE 是次版本，不是全量重写 | ADR-0002 |
| 附件按 `sha256` 内容寻址 | 天然去重 + 半上传可判 | ADR-0009 |
| 移动端不承诺 30 s 后台到达 | iOS/Android 系统调度不由应用决定，承诺即撒谎 | ADR-0011 |

---

## 6. 实测驱动的设计（不是偏好）

Phase 0 用 `tools/feasibility-probe`（16 项，全通过，输出见 `docs/evidence/probe-windows-gnu.txt`）替代推测：

| 结论 | 实测证据 | 影响 |
|---|---|---|
| **FTS5 trigram 对 <3 字符中文查询静默返回 0 行** | 2 字 `MATCH` → 0 行 / 22 µs；`LIKE` → 416/5000 正确 | 检索层强制双路径：≥3 字 MATCH，≤2 字 LIKE |
| ≥3 字检索性能足够 | 5000 条中文笔记 p50=32 µs，p95=1.4 ms，索引 6.0 MiB | 满足 §17 目标，不引入外部分词器 |
| AES-256-GCM-SIV 标签恒 16 B、nonce 12 B、篡改必拒 | `envelope-*` 两项 | 信封开销可精确预算；E2EE 预留可行 |
| argon2id 19 MiB/2 it/1 p ≈ 400 ms | `kdf-argon2id` | KDF 只能后台，UI 线程禁用 |
| `aws-lc-rs` + `rustls` 在 `x86_64-pc-windows-gnu` 可编译并完成真实 TLS | `https-tls-handshake → 200` | GNU 开发路线成立（但正式构建仍走 CI 的 MSVC） |
| PROPFIND/MOVE 自定义动词 + Depth + 412 条件请求可用 | `webdav-custom-verbs`、`precondition-412-plumbing` | 自建协议所需 HTTP 原语齐备 |
| 代理配置真实生效（差分可证） | `proxy-config-actually-honored` | §19 的"可测试"要求有实现路径 |
| 超时取消真实生效 | 400 ms 预算 → 402–416 ms 返回 | 后台轮次可被取消，不挂死 |
| 清单条目 89 B，全量 5000 条 gzip 186 KiB | 尺寸测量脚本 | 否决"每轮重写全量清单"，采用两段式 |
| 本机无 MSVC 链接器、无 JDK/SDK（WebView2 实测在位）、GitHub 不可达 | 环境探测 | 见 §8 与 CI-CD.md 的 BLOCKED 清单 |

---

## 7. 仓库结构

```text
Notera/
├── Cargo.toml  rust-toolchain.toml  deny.toml  rustfmt.toml  clippy.toml
├── package.json  pnpm-workspace.yaml  tsconfig.json
├── crates/
│   ├── notera-core/       notera-richtext/   notera-crypto/
│   ├── notera-store/      notera-net/        notera-webdav/
│   ├── notera-sync/       notera-config/     notera-importer/
│   ├── notera-host/       notera-cli/        notera-test-webdav/
├── apps/desktop/            # Tauri 2：src/ (Vue3+TS) + src-tauri/ (薄壳) + gen/{android,ios}
├── platform/{windows,macos,android,ios}/
├── migrations/              # 0001_init.sql …  forward-only
├── tests/{contract,e2e,crash,uat,scenarios}/
├── fixtures/                # 金样本：信封、清单、富文本文档
├── scripts/                 # 校验、版本一致性、架构图验证
├── tools/feasibility-probe/ # Phase 0 可行性探针（保留为证据，非产品代码）
└── docs/                    # 本目录：长期工程记忆
```

---

## 8. 已知限制与阻塞项（不隐藏）

| # | 事项 | 状态 | 解除条件 |
|---|---|---|---|
| B1 | 本机无 MSVC 链接器（PATH 中 `link.exe` 被 Git coreutils 顶替），默认 host 连 proc-macro 都编不过 | 已绕行 | 装 VS Build Tools "C++ 生成工具"，或长期用 GNU host（Tauri 官方未列 GNU → 需先做 spike） |
| B2 | 本机已装独立 WebView2 运行时（`Microsoft\EdgeWebView\Applicationh.0.4078.105`，含 `msedgewebview2.exe`）→ 桌面窗口可本地启动 | **BLOCKED** | 安装 WebView2 Evergreen（离线包） |
| B3 | 本机无 JDK/Android SDK/NDK → 无法本地出 APK | **BLOCKED** | 装工具链，或全部交 CI |
| B4 | `github.com` / `api.github.com` 本机不可达（000）→ Actions 无法观察 | **BLOCKED** | 提供可访问 GitHub 的推送通道；在此之前禁止任何"CI 已通过"表述 |
| B5 | 无真实 WebDAV 服务器可测（只有自建 test-webdav） | **BLOCKED** | 用户提供内网 + 公网各一个端点 |
| B6 | iOS 出包需 macOS + Xcode + 证书 | **BLOCKED** | Phase 5 + 证书决策 |
| B7 | 产品名 `Notera` 未做商标/域名核查 | 待核 | 品牌定名（改名只影响 brand 层，不影响架构） |

---

## 9. 明确不做（Phase 0 及近期）

账号体系 · 协作与共享 · 服务端搜索 · 版本历史上传（历史只留本地）· SQLCipher 本地静态加密（待决策）· PAC 自动代理脚本执行 · WebDAV `LOCK` 作为正确性依赖 · 表格单元格级合并 · 实时多端光标协同 · 插件系统

---

## 10. 阶段闸门

| Phase | 目标 | 出口条件（详见 TEST-PLAN §阶段出口条件） |
|---|---|---|
| **0 架构** | 本套文档 + 契约图 + 探针 | **人工审核通过**（当前所处阶段，未通过前不得进入 Phase 1） |
| 1 Local Core | SQLite/迁移/CRUD/富文本/搜索/自动保存 | 功能矩阵全绿，离线 100% 可用，无 GUI 依赖 |
| 2 Sync Engine | manifest/rev/pull/push/tombstone/冲突/重试 | 同步矩阵 + 多设备 E2E + 崩溃点全绿（对 test-webdav） |
| 3 Proxy | Direct/System/HTTP/SOCKS5/bypass/TLS | 代理差分证明 + 失败不阻塞本地 |
| 4 Desktop | Windows + macOS 原生体验 | 三平台构建产物 + UAT 桌面用例 |
| 5 Mobile | Android + iOS 生命周期/后台/分享 | Android 出包 + 移动同步目标达成；iOS 至少架构验证 |
| 6 Attachments | 内容寻址/去重/双队列 | 大附件不阻塞文本（专项证明） |
| 7 Hardening | 崩溃/断网/迁移/恢复/大数据/长稳 | 加固矩阵 + 24 h 长稳 |
| 8 CI/Release | 三平台自动出包 | 三产物 + 全闸门绿灯（依赖 B4 解除） |
