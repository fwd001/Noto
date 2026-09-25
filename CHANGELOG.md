# Changelog

遵循 [SemVer](https://semver.org/lang/zh-CN/)。同步协议发生破坏性变更时，`SYNC_PROTOCOL_VERSION` 与版本号同步升级（见 CI-CD.md §版本与单一版本源）。

## 0.1.0 — Phase 1–4（实现进行中）· 未发布

当前测试基线（本机 GNU 工具链实测）：Rust 268 通过（`notera-store` 58，`core/config/crypto/richtext/sync/net/test-webdav` 合计 210），前端 81 通过 9 个测试文件；`scripts/arch-check.mjs` 18 条中 17 通过；`scripts/verify-diagram.mjs` 59 条全通过；`vue-tsc --noEmit` 无错误，生产包 189 KB（gzip 65 KB）。

### 新增

- **存储层** `notera-store`：11 张表 + FTS5（trigram）迁移 0001…0005、写连接单写者 + 只读连接池、tombstone、outbox、冲突收件箱、附件内容寻址、偏好读写与记录 wire 出口
- **同步引擎** `notera-sync`：清单两段式解析与压实、`P1..P18` 判定表、退避重试、CAS 提交与恢复阶梯；端口化（`LocalPort`/`RemotePort`），10 个引擎级集成测试跑真 `run_round`
- **网络出口** `notera-net`：全系统唯一 HTTP 出口，代理四档、TLS 策略、分层超时、退避、`RouteProof` 脱敏审计（13 测试）
- **测试基建** `notera-test-webdav`：真 TCP/HTTP 的 WebDAV 子集 + `/_control/*` 能力开关与故障注入 + `/_fs/dump`
- **前端** `apps/desktop`：三栏 UI、独立富文本模型映射、四态同步徽标、design token 与对比度契约测试、Tauri 单命令通道及其契约测试
- **诊断入口** `notera-cli`：`serve`（dev 桥，落真实 Store）、`verify`、`conflicts`、`sync-once`、`net-probe`、`export`；退出码 0=PASS / 1=ASSERT_FAIL / 2=BLOCKED
- **ADR-0018**：单一活跃同步账户约束（多服务器推迟到"按账户确认点"）
- **架构适应度检查** `scripts/arch-check.mjs`：18 条机器可判定的层次约束（依赖边、唯一出口、SQL 只出现在 store、前端无协议词汇、端口边越界引用…）
- **端到端等价** `scripts/verify-app.mjs`：Playwright 驱动同一份前端 + 同一份 Rust 核心的 14 步 UAT

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
