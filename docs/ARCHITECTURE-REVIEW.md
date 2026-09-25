# Architecture Review

Notera · Phase 0 交付评审件 · 2026-09-25
评审人：待填 ｜ 结论：☐ 通过 ☐ 有条件通过 ☐ 驳回

本文件可独立阅读并据以拍板，无需先读其它文档。细节规范在括号内给出出处。

---

## 1. Product Goal

做一款 **local-first 跨平台个人笔记软件**：体验接近 Apple Notes，但数据存在用户自有的 WebDAV 服务器上，并支持 App 级代理。

用户动作链只有五步：安装 → 打开 → 直接写 → 自动保存 → 自动同步。同步机制（WebDAV/Revision/Manifest/Tombstone/ETag/Pull/Push/Merge/Conflict）全部是实现细节，用户只看到 `✓已同步 / ↻正在同步 / ○离线 / !同步失败` 四态。

裁决优先级（不可重排）：`数据安全 > 同步正确性 > 稳定性 > 用户体验 > 原生平台体验 > 性能 > 可维护性 > 新功能`。

首发平台：Windows 10/11 x64、macOS Apple Silicon、Android arm64-v8a；iOS/iPadOS 架构预留并保留原生能力接入位。

**Phase 0 明确不交付**：任何业务代码、UI、编辑器、同步实现、附件、账号体系、E2EE 实装。

---

## 2. Architecture

```text
L0 用户与平台   apps/desktop/src (Vue3+TS)      platform/{windows,macos,android,ios}
L1 应用与装配   notera-host（用例/调度/事件/Commands）   notera-cli（诊断与 E2E 驱动）
L2 领域服务     notera-sync  notera-richtext  notera-importer  notera-config
L3 基础设施     notera-store  notera-crypto  notera-webdav  notera-net
L4 内核         notera-core（类型/ID/时钟/错误/不变式，零 I/O 零 async）
L5 外部世界     用户自有 WebDAV · 系统钥匙串 · 应用数据目录
测试专用        notera-test-webdav（真实 HTTP + 故障注入）
```

依赖方向只允许向下；唯一"向上"通道是 `notera-host` 的事件总线（订阅，非调用）。

三条结构性铁律（各有 CI 闸门，不靠自觉）：

1. **前端不碰同步** —— UI 层不出现协议词汇，只发命令 DTO、只收 ViewModel/事件。
2. **网络出口唯一** —— 所有 HTTP 必经 `notera-net`；`notera-webdav` 的依赖里没有 `reqwest`（Cargo 依赖图 + grep 双闸门）。
3. **写入入口唯一** —— 权威表写入必经 `notera-store::commit_*`，派生列与 FTS 在同一事务内刷新。

技术栈：Tauri 2 + Rust 1.98 + Tokio + rusqlite(bundled, SQLite 3.53.2) + reqwest 0.13.5 + Vue 3/TS/Vite。

核心数据流（写入路径上**没有网络**）：

```text
输入 → UI 乐观更新 → Command → richtext(normalize/validate/canonical/派生)
     → store 单事务(notes + revisions + 派生列 + FTS + outbox) → 提交
     → 调度器 debounce 2.5s → 后台同步引擎起一轮
```

（详见 ARCHITECTURE.md §3–§4）

---

## 3. Architecture Diagram

交付物：`docs/diagram/architecture.html` —— 单文件、内联 CSS/JS、**零 CDN**（开发机不可达外网，必须离线可开）。

内容：16 个模块，每个模块三栏契约（输入：来源·数据·类型 / 输出：去向·数据·类型 / 关键数据结构字段清单）+ 内部职责穿透 + 禁止事项；33 条依赖边。

交互：默认只画 12 条主干边（一次画全会糊成一团）；点模块进入穿透模式（只显邻边、展开职责、无关模块淡出）；四条链路筛选（本地写入 / 同步穿透 / 事件回流 / 网络出口）；契约总表视图；字段搜索跨命名风格（`content_hash` 与 `contentHash` 互搜得到）。

**已实跑验证**：`scripts/verify-diagram.mjs` 用盘上 chromium 跑 **59 项断言全部通过** —— 无 JS 错误、连线不穿无关模块、边标签两两不重叠、不压层标题、各筛选态边数与 EDGES 数据模型精确相等、对比度最低 8.63:1（正文 16.36:1）、命中区 ≥44px、420px 宽无整页横向溢出。渲染证据在 `docs/evidence/canvas-*.png`。

验证过程中修掉 4 个真实缺陷，其中两个只有看截图才发现（DOM 断言漏掉）：同层卡片 flex 换行会摧毁"层间隙"路由模型；边标签压在层标题文字上。

---

## 4. Data Model

11 张表 + 1 个 FTS5 虚表：`folders` `notes` `note_revisions` `attachments` `note_attachments` `tombstones` `sync_accounts` `sync_state` `sync_remote_index` `sync_operations` `sync_conflicts` `settings` `meta` `notes_fts`。

**Revision 模型（本评审最需要看清的一处）**：

```text
rev          本地/远端已知最新修订号        rev = max(rev, remote_rev) + 1
sync_rev     双端最后一次确认一致的点       ← 即三方合并的 base
remote_rev   服务器当前 rev
content_hash 当前内容 sha256
```

推演过程中把早期草案的 `base_rev` 与 `synced_rev` **合并为一列**：在 push 成功、pull 生效、本地脏、远端领先、冲突五种可达状态下两者恒等，少一列即少一类不一致 bug。**这是需要评审确认的设计收敛**（ADR-0005）。

不用向量时钟：`sync_rev` 是双方都确认过的内容点，base 文档可从本地 `note_revisions` 精确取回，三方齐备即可判真并发。代价：`note_revisions` 中 `rev >= sync_rev` 的行**永不 GC**（硬约束，否则冲突时找不回 base）。

**富文本**：`{v, content:[Block]}`，块含稳定 `id`（三方合并的锚点）；不是 HTML；未知节点/属性 preserve-unknown；`doc.v` 高于客户端支持 → 该笔记**只读**。canonical JSON（键按码点升序、整数不写成浮点）用于哈希，实测同一文档不同插入顺序输出一致。

**检索（实测驱动，非偏好）**：

| 查询 | 路径 | 实测 |
|---|---|---|
| ≥3 字符 | FTS5 trigram `MATCH` | p50 32 µs / p95 1.4 ms（5000 条中文笔记，索引 6.0 MiB） |
| ≤2 字符 | content 表 `LIKE` | 正确 416/5000，≈4.1 ms |
| ≤2 字符走 `MATCH` | **静默返回 0 行** | 22 µs —— 会让用户以为"没有结果"而数据其实在 |

这条实测结论直接否决了"统一走全文索引"的朴素实现，是必须写进代码的硬规则。

**删除**：两级（回收站 30 天 → 永久删除），见 §7。

（详见 DATA-MODEL.md）

---

## 5. Sync Protocol

**三条不可违反的规则**：

* **R1** 记录文件是正确性来源，清单只是索引与公告 → 清单损坏/落后不丢数据。
* **R2** 写入顺序固定：实体记录 → 附件 → 清单 → 清单落后是正常且安全的。
* **R3** 判定只用 `rev` + `content_hash`，**禁止** mtime / 服务器时间 / 墙上时钟。

远端布局：

```text
.notes/protocol.json                       根自描述 + 版本协商
      /manifest/{index.json, index.json.prev, seg-*.json}
      /records/<kind>/<id>.json            每实体一个信封文件
      /attachments/<2hex>/<sha256>         内容寻址，不可变
      /tmp/  /locks/
```

**清单两段式（基线分段 ⊕ 变更窗口）** —— 让每轮成本与库规模解耦。实测尺寸：

| 形态 | 裸 | gzip |
|---|---|---|
| 全量 5000 条 | 438.7 KiB | **186 KiB** ❌ 不可作每轮载体 |
| 全量 20000 条 | 1 754 KiB | 742 KiB ❌ |
| 窗口 200 条 | 16 KiB | **7.8 KiB** ✅ 每轮实际重写量 |
| 分段 2000 条 | 175.6 KiB | 74.7 KiB ✅ 仅压实/落后过多时拉 |

每轮预算（规范上限）：空轮 **1 请求 0 字节**（ETag 304）；本地改 1 条 ≤4 请求 / 上行 ≈9 KiB；单轮硬上限 200 请求 / 8 MiB。

清单损坏恢复阶梯：D1 回退 `index.json.prev` → D2 PROPFIND 扫描从记录重建（R1 保证信息无损）→ **D3 若与本地缓存背离 >30% 且 >50 条，拒绝据此删除本地，要求人工确认**（防"运维误删/挂错目录/配额清空"被翻译成"用户删了 3000 条笔记"）→ D4 单分段丢弃。

服务器能力差异由 `cap_mask` 吸收，写入策略自动选：S1 条件 PUT（If-Match）/ S2 tmp+MOVE（Overwrite:F）/ S3 盲写后 GET 复验。S3 下强制启用租约并在 UI 标注"服务器不支持并发保护"—— 诚实降级，不静默承担风险。

崩溃点矩阵 C1–C10 逐点定义恢复动作；幂等靠 `dedupe_key` + 内容哈希复验（"至少一次"投递不会变成"重复覆盖"）。

（详见 SYNC-PROTOCOL.md）

---

## 6. Conflict Strategy

**唯一不可协商的目标**：用户输入过的内容不会因同步、冲突、崩溃或另一台设备的操作而静默消失。冲突解决"不够聪明"可接受，"聪明到替用户决定并丢掉一份内容"不可接受。

判定式：`L.rev≠sync_rev ∧ R.rev≠sync_rev ∧ H(local)≠H(remote)`。

流水线：信封级校验 → 内容相同即收敛（P7，避免假冲突）→ **块级三方合并**（按块 `id` 对齐，不同块改动无损自动合，静默完成只记审计）→ 标量属性合并（降级为审计+提示，不进收件箱）→ 仍无法判定 → **保留双方**。

自动合并降级顺序 M1(仅格式差)→M2(严格包含)→M3(纯追加)→M4(checklist)→M5(判冲突)，每级都要求"能证明无损"。合并产物必须重新 validate，不通过则丢弃合并、退回保留双方。

保留双方 = 原笔记取远端为正文 + 本地完整内容生成**冲突副本**（普通笔记，非隐藏状态，参与同步），并写入冲突收件箱。默认取向可回调（见 §14 D3）。

明确否决的方案：字符级三方 diff（段落漂移导致把可合并场景误判为冲突）、CRDT（需常驻合并状态，与"离线任意久后汇合"及 WebDAV 文件形态不匹配）。

**测试断言强度要求**：冲突用例的通过标准是"两份内容都能逐字节找回"，不是"没有报错"或"条数正确"。

（详见 CONFLICT-RESOLUTION.md）

---

## 7. Delete/Tombstone Strategy

三态：正常 → 回收站（`deleted_at`，30 天）→ 永久删除（`purged`）。

删除是**记录内容**（信封多一个字段），不是"文件消失" → 与编辑共用同一条已验证路径，不需要独立协议。

需求明令禁止的复活路径：`A 删除 → 远端文件消失 → B 发现"本地有服务器没有" → 重新上传 → 笔记复活`。协议层用四条互相独立的机制排除它：

1. 远端记录不删除，只带 `deleted_at`；
2. `tombstones` 表**不自动 GC**（ADR-0006）；
3. `missing_remote`（远端 404）**永不**导致本地删除，只导致补传（C2）；
4. 导出文件默认携带删除事实（防"导出→清空→导入"复活）。

**Tombstone 不自动 GC 是刻意的代价交换**：任何 TTL（30/90 天）都必然留下"离线超过 TTL 的设备复活已删笔记"的窗口，而这被明令禁止。代价：墓碑永久累积，每条约 100–200 B，1 万条删除 ≈ 2 MB，另提供显式"压缩远端"维护动作作为出口。

**文件夹删除不级联删除笔记**（子文件夹上移、笔记移入默认本）。理由：① 一次误触不应连带摧毁内容；② 级联会产生 N 条墓碑 + N 次远端写，把删除变成最重的操作。

删除 vs 修改（P11）必进收件箱，本地内容先完整保留，用户明确选择"仍要删除"才传播。

---

## 8. Proxy Strategy

**App 级**：只影响 Notera 自己的 WebDAV 请求，绝不修改操作系统网络设置，绝不代理其它流量。按**账户**配置（内网办公与家里网络的两套参数可以共存）。

模式：`Direct / System / Http / Https / Socks5 / Socks5h` + bypass（host / CIDR / `*.domain`）。

出口唯一收敛于 `notera-net`，三重闸门强制（不 pub 客户端类型 + Cargo 依赖图约束 + CI grep）。这直接服务于需求那句"不能出现某些请求走代理、某些绕过代理而自己不知道"。

**已读源码确认**（非推测）：`reqwest 0.13.5` 支持 SOCKS5 用户名/密码认证（`proxy.rs:744` 解析、`connect.rs:1956` 消费）与 `socks5h` 远端 DNS → **不需要自造 loopback 转发器**。探针亦实测"死代理必失败 + 直连必成功"的差分。

TLS 四档：`strict` / `ca_bundle`（内网自签主路径）/ `pin`（指纹白名单）/ `insecure_local`（仅 loopback 或显式勾选，UI 红字告警）。根证书优先用系统信任库（`rustls-platform-verifier`，实测 GNU 可编译）→ 企业把内网根 CA 装进系统后 Notera 自动可信。纯 HTTP 默认拒绝。

**可证伪性（需求 §19 要求）三条证据链，缺一不可**：① 差分测试（死代理失败 vs 直连成功）；② test-webdav 支持"仅接受经代理到达的连接"，同步成功本身即证明；③ 每轮回传 `RouteProof` 实际出口 + `notera-cli net probe` 校验三者一致。

任何网络/代理/证书故障都不得阻塞本地任何操作，也不改变 outbox 内容（N2）。

**已知缺口**：不执行 PAC 脚本（需 JS 引擎）。若用户环境只有 PAC 而无手工代理地址，需要升级 —— 见 §14 D1。

（详见 PROXY.md）

---

## 9. Platform Strategy

共享核心逻辑 + 平台原生能力。判据：数据/同步/冲突/搜索/加密/队列一份 Rust 实现；窗口/菜单/托盘/通知/分享/后台任务/返回手势/键盘/安全区各端原生。差异用 `PlatformCaps` 能力声明表达，**禁止**按机型分支（`caps.tray==false` 而非 `is_iphone`）。

启动时序为规范（P1）：开库 → 迁移闸门 → 读最近列表（不 SELECT doc）→ **首帧可输入 ≤400 ms** → 后台才启同步引擎。首帧路径上不存在任何网络等待；服务器不可达时用户看到的是完整可用的本地库 + `○离线` 徽标。

**后台同步的诚实结论**：

| 端 | 承诺 |
|---|---|
| 桌面 | debounce 2.5 s + 周期 **25 s**（留 5 s 执行余量）+ 事件触发 → 兑现"正常网络 ≤30 s 到达" |
| Android | WorkManager 周期 ≥15 min（系统最小约束，可被延后）+ 打开即同步 + 网络回调 |
| iOS | BGAppRefreshTask 由系统调度（常 ≥15 min 且不保证）+ 打开即同步；WebDAV 无推送通道 → **无法服务端唤醒** |

> **移动端不承诺 30 秒到达。** 真实承诺是"拿起设备打开 Notera 时看到的基本已经是最新的"。任何声称能在 iOS 后台稳定 30 s 同步的设计都是把系统调度当成自己的线程。这会改变产品文案与验收口径，需要评审确认（§14 D2）。

本机环境限制如实记录：无 MSVC 链接器（PATH 中 `link.exe` 被 Git coreutils 顶替，默认 host 连 proc-macro 都编不过）、无 JDK/SDK/NDK、GitHub 不可达。WebView2 实测**已安装**（独立运行时 150.0.4078.105，含 `msedgewebview2.exe`）——本文早期版本曾误判为缺失，原因是只检查了 Edge 浏览器目录 `Microsoft\Edge\Application`，未检查 `Microsoft\EdgeWebView\Application`。**关键结论：Phase 1–3 的全部验证不依赖 GUI，可在本机完成**；GUI 与安装包验证从 Phase 4 起依赖 CI。这个划分让环境限制不阻塞前三阶段。

（详见 PLATFORM.md §2 能力矩阵、§12 限制表）

---

## 10. Test Strategy

黑盒/行为驱动：测试只关心"用户能不能完成目标"，不关心代码怎么写。DB 检查仅作辅助证据。

七层：L0 单元（Rust+TS）→ L1 组件（store/sync 状态机 + 假时钟 + 进程内真 socket）→ L2 协议契约（金样本双向兼容）→ L3 多客户端 E2E（`notera-cli sync-once` 驱动 A/B/C 对 test-webdav）→ L4 崩溃（在每个提交点 kill）→ L5 黑盒 UAT（Playwright 驱动 WebView，只点击/输入/重启/断网）→ L6 平台构建冒烟。

覆盖：功能矩阵（CRUD/恢复/永久删/文件夹/移动/搜索含**两字中文**/固定/重命名/附件/图片/清单/富文本各节点往返/深色/重启/离线全功能）、同步矩阵（含 create+create、update+update、delete+update、update+delete、move+update、离线转在线、重试、超时、服务不可用、认证失败、部分上传、清单损坏）、多设备 A/B/C、崩溃点 C1–C10。

15 条全局不变式（INV-01..15），每条配一个属性测试并指明落点 crate。内容寻址于 §3 的 I1–I8 / R1–R3 / C1–C4 / N1–N2 / P1–P2。

质量闸门：核心 crate 行覆盖 ≥80%，`notera-sync`/`notera-richtext`/`notera-crypto` ≥90%；flaky 隔离修复，**禁止**删测试/降断言/`#[ignore]`/mock 核心逻辑；改协议必须同批改契约测试。

**验收口径**：`SKIP` / `TODO` / "暂时忽略" / "理论通过" / "仅手工测试" 一律不算 PASS；确实无法测试的写 `BLOCKED` + 原因 + 影响 + 解除条件。

（详见 TEST-PLAN.md，454 行）

---

## 11. WebDAV Test Server

自建 `notera-test-webdav`（Rust + Tokio）。原因：需求明令禁止用假 API 测同步，而现成服务器（Apache/Nextcloud）不具备故障注入能力。

真 TCP HTTP/1.1，实现 `GET/HEAD/PUT/DELETE/MOVE/COPY/PROPFIND/PROPPATCH/OPTIONS/LOCK/UNLOCK`，支持 `Depth` / `Destination` / `Overwrite` / ETag / `If-Match` / `If-None-Match`(412/304) / Range / chunked。

故障注入：延迟、断网、超时、401、403、404、409、412、500、507、**半上传**（chunked 中途断连）、清单损坏、服务器重启、以及"仅接受经代理到达的连接"模式（用于 §8 的代理证明）。

控制面 `/_control/{reset,inject,stop,restart,log}` 与 `/_fs/dump`（**唯一允许的服务端状态断言手段**）。两种后端：`mem`（快）与 `fs`（跨进程重启保留状态，用于验证清单恢复 D1–D4）。

约束：注入必须确定可复现（同 scenario 两次结果一致），否则测试无价值。

代价如实说明：要自己实现并维护一个 DAV 子集（约 1–2k 行），其正确性本身需要被测试。真实服务器兼容矩阵**不能**由它替代（§14 D5）。

---

## 12. CI/CD

作业：`pr`（fmt + clippy `-D warnings` + eslint + typecheck + 单元 + 契约）→ `integration`（L1/L2/L3 含 test-webdav）→ `crash`（L4）→ `e2e-desktop`（L5，需 WebView2）→ `audit`（cargo-audit/deny + npm audit + 密钥扫描）→ `build-{windows,macos-android}` → `release`（tag `v*`，`needs:` 依赖全部闸门）→ `nightly-soak` → `docs`（含 ARCHITECTURE-MAP 一致性检查）。

**发布不得只靠 `cargo test` 绿灯**：闸门是 前端测试 + Rust 测试 + 集成 + WebDAV E2E + 平台构建 全绿。

三平台产物：`Notera-0.1.0-windows-x64.msi`、`-macos-arm64.dmg`、`-android-arm64-v8a.apk` + SHA-256 `checksums.txt`。Windows 用 `windows-2022`（自带 VS → MSVC）、macOS 用 `macos-14`（原生 arm64）、Android 用 JDK17 + SDK/NDK + `aarch64-linux-android`。

版本单一来源 + `scripts/check-versions` 校验 Cargo / package.json / tauri.conf.json / `SYNC_PROTOCOL_VERSION` 一致，漂移即失败；协议破坏性变更必须升版本并写入 `protocol.json` 协商。

迁移契约 CI 四项：空库→最新、逐版本升级链、升级后行数与哈希校验和一致、未知未来版本只读；grep 闸门禁止迁移目录外的 `ALTER TABLE`。

**GitHub 本机不可达（实测 000）→ 已定交接协议**：agent 写/改 workflow → 人工在可访问网络推送 → 人工回贴 Actions 运行 URL 与失败日志 → agent 定位修复 → 重复。**在拿到绿灯证据前，任何文档/PR/提交信息禁止出现"CI 已通过"**（CI-CD.md §禁止的表述已写死）。

---

## 13. Risks

| # | 风险 | 严重度 | 缓解 | 状态 |
|---|---|---|---|---|
| R1 | Tauri 2 mobile（尤其 iOS）成熟度与插件残缺 | 高 | Phase 5 前保留 uniFFI + 原生壳降级路径（ADR-0001 备选③）；Phase 0 已把核心逻辑做成纯 crate，降级不改协议 | 未验 |
| R2 | 真实 WebDAV 服务器行为差异（无 If-Match / 无 Overwrite:F / 无 Depth:infinity） | 高 | `cap_mask` 探测 + S1/S2/S3 三策略 + 租约；S3 明确标注风险 | **BLOCKED**（无端点可测） |
| R3 | 本机无 MSVC 链接器 → 本地无法出 Windows 包 | 中 | 绕行：GNU host 已实测可编译全部依赖；正式构建以 CI MSVC 为准 | 已绕行 |
| R4 | WebView2 运行时已装（150.0.4078.105） → 桌面窗口无法本地验证 | 中 | 安装包内置离线引导 + 启动检测给可操作提示；L5 UAT 转 CI | **BLOCKED** |
| R5 | GitHub 不可达 → CI 与发布通道未经一次真实绿灯 | 中 | 交接协议 + 禁止"CI 已通过"表述 | **BLOCKED** |
| R6 | 移动端同步时效低于用户预期（15 min 量级 vs 30 s） | 中 | 本文 §9 明示 + 产品文案按此口径 + 打开即同步兜底 | 需确认 |
| R7 | 墓碑永久累积导致远端文件数增长 | 低 | 提供显式"压缩远端"维护动作；量级评估（1 万删 ≈ 2 MB） | 已接受 |
| R8 | 清单并发写覆盖（多设备同时提交） | 中 | CAS + seq + 幂等重放 + 租约；且 R1 使清单覆盖不丢数据 | 设计已覆盖，待 E2E 证 |
| R9 | 块级合并产生冲突副本造成用户困惑 | 低 | 默认取向可回调（D3）+ 收件箱集中处理 + 副本是普通笔记 | 需确认 |
| R10 | 内网合规要求本地静态加密（当前明文，依赖 OS 全盘加密） | 高（若成立） | ADR-0016 已列三选项，**Proposed 待决定**；若采纳需同步改备份/恢复与测试 | 需决定 |

---

## 14. Open Decisions（需要评审拍板）

| # | 决策点 | 我的倾向 | 若不同意的影响 |
|---|---|---|---|
| **D1** | 用户环境是否只有 PAC（无手工代理地址）？ | 假设"有手工地址"，PAC 延后 | 若只有 PAC，v1 必须引入 JS 引擎执行 PAC，工期 +，依赖 + |
| **D2** | 接受"移动端不承诺 30 s，只承诺打开即同步"？ | 接受（这是系统事实） | 若必须 30 s，需引入自建推送/中转服务，与"数据由用户掌控"冲突 |
| **D3** | 冲突默认取向：远端为正文 + 本地为副本 | 保持 | 反转不影响协议，只改 UI 与用户习惯 |
| **D4** | 本地静态加密（ADR-0016，Proposed） | 依赖 OS 全盘加密，除非合规强制 | 选 SQLCipher → 替换 SQLite 依赖、四端交叉编译风险、备份/恢复全改 |
| **D5** | 能否提供真实 WebDAV 端点（内网 + 公网各一）做兼容矩阵？ | 需要 | 不给则 R2 风险一直未验，发布质量凭推断 |
| **D6** | Windows 本机路线：装 VS Build Tools，还是长期 GNU host？ | 装 Build Tools（与官方一致） | 长期 GNU 需先做 Tauri-on-GNU spike 并承担未获官方支持的风险 |
| **D7** | GitHub 推送通道由谁/何时提供？ | 人工推送 + 回贴日志 | 不给则 Phase 8 无法收口 |
| **D8** | Apple 开发者证书与 Android release keystore 归属？ | 待定 | 无证书 → macOS 产物 Gatekeeper 拦截，需文档说明；keystore 不可换，丢失即无法覆盖升级 |
| **D9** | 是否 v1 支持多账户（同设备两个 WebDAV）？ | 数据模型已预留 `account_id`，UI 延后 | 若 v1 要，需补 UI 与切换语义 |
| **D10** | 代号 `Notera` 是否可用（商标/域名核查）？ | 改名只影响 brand 层 | 现在改成本≈0，1.0 后改成本高 |

---

## 15. Recommended Next Phase

**Phase 1 — Local Core**，且**只有在 D1/D2/D4/D5 得到答复后**开始。

范围（严格限定）：Cargo workspace 与 12 个 crate 骨架、`migrations/0001..0005`、`notera-core` 类型与不变式、`notera-richtext` 模型 + normalize/validate/canonical + 派生、`notera-store` 仓储 + 唯一写入口 + FTS 双路径 + 自动保存事务、`notera-config` 骨架、`notera-test-webdav` 最小可用版、L0/L1 测试与 CI 的 `pr` 作业。

**Phase 1 不做**：任何网络代码、任何 UI/编辑器、附件上传、同步引擎、代理、安装包。

选择理由：Phase 1 的验证**完全不依赖 GUI 与网络**，因此不受 B2/B3/B4 阻塞；而 `notera-richtext` 的 canonical/派生/合并与 `notera-store` 的事务边界是后续所有阶段的依赖底座，先做可让协议文档在真实代码上再校验一遍。

Phase 1 出口条件：功能矩阵本地部分全绿、离线 100% 可用、检索双路径用例（含两字中文非空）通过、迁移链四项 CI 通过、覆盖率达标 —— 见 TEST-PLAN.md §阶段出口条件。

---

## 附：本阶段交付物清单

```text
docs/ARCHITECTURE.md            分层、依赖、数据流、实测结论、限制
docs/ARCHITECTURE-MAP.md        长期架构记忆（开工必读 / 改动路由 / 禁止模式）
docs/DATA-MODEL.md              表与字段语义、revision 模型、富文本 schema
docs/SYNC-PROTOCOL.md           远端布局、清单、状态机、崩溃恢复、错误分类
docs/CONFLICT-RESOLUTION.md     冲突检测、块级合并、删除与反复活
docs/PROXY.md                   代理、TLS、超时退避、可证伪测试
docs/PLATFORM.md                四端能力矩阵、启动时序、后台预算
docs/TEST-PLAN.md               测试分层与行为矩阵
docs/CI-CD.md                   构建矩阵、版本策略、迁移契约、交接协议
docs/ADR/0001..0017             17 份决策记录（0016/0017 为 Proposed）
docs/diagram/architecture.html  可交互契约图
docs/evidence/                  探针输出 + 图渲染截图
tools/feasibility-probe/        16 项技术可行性实测（全部通过）
scripts/verify-diagram.mjs      59 项浏览器断言（全部通过）
```
