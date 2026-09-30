# PROXY

Notera 的 App 级网络与代理架构。规范性文档。

关联：[SYNC-PROTOCOL.md](./SYNC-PROTOCOL.md) §12 错误分类 · [ARCHITECTURE-MAP.md](./ARCHITECTURE-MAP.md) · ADR-0010

---

## 0. 定位

代理是 **App 级**的：只影响 Notera 自己发出的 WebDAV 请求，**绝不修改操作系统网络设置**，**绝不**做全局透明代理，**绝不**代理除 WebDAV 之外的任何流量。

需要它的真实原因（内网办公 + 家里/移动双环境）：

* 办公网只能经 HTTP/SOCKS 代理出到部署 WebDAV 的区域；
* 家里网络能直连同一地址，走代理反而失败；
* 内网 WebDAV 常是自签证书，公网是正规证书 —— 同一份配置不能两边都严格校验。

因此代理与 TLS 策略都必须**按账户**（per-account）可配置，而不是全局一个开关。

---

## 1. 唯一出口闸门

```text
所有 HTTP 请求  ──►  notera-net::HttpClient  ──►  reqwest  ──►  socket
                     （代理解析 / TLS / 超时 / 重试 / 审计）
```

**铁律**：除 `notera-webdav` 经 `notera-net` 发出的请求外，任何 crate 不得构造 HTTP 客户端。

强制手段（三重，不依赖自觉）：

1. `notera-net` 不 `pub` 出底层客户端类型，只暴露 `send(RequestSpec) -> Response`；
2. `notera-webdav` 的依赖声明里只有 `notera-net`，没有 `reqwest`（Cargo 依赖图约束）；
3. CI 架构测试：全仓 grep `reqwest::Client`、`hyper::`、`rustls::ClientConfig`，命中 `notera-net` 之外即失败。

这条闸门直接服务于需求里那句"不能出现某些请求走代理、某些绕过代理而自己不知道"。

---

## 2. 模式与实测能力

| 模式 | 配置 | 实测状态 | 说明 |
|---|---|---|---|
| `Direct` | 无 | ✅ | 显式禁用系统代理（不是"没配"） |
| `System` | 跟随 OS | ⚠ 需分平台实现 | 见 §4 |
| `Http` | host/port/用户/密码 | ✅ | 明文 CONNECT 或 HTTP 转发；HTTPS 目标走 CONNECT |
| `Https` | host/port/… | ⚠ 待测 | 与代理服务器建立 TLS 后再 CONNECT |
| `Socks5` | host/port/用户/密码 | ✅ **支持认证** | `reqwest 0.13.5` 解析 `socks5://user:pass@host` 并在连接层消费（`proxy.rs:744`、`connect.rs:1956`） |
| `Socks5h` | 同上 | ✅ | 远端 DNS 解析（见 §5） |

> SOCKS5 带用户名/密码在早期 reqwest 是已知缺口，**本版本已支持**，因此不需要自造 loopback 转发器。这一结论来自读源码而非推测，实现时仍需用 `net probe` 在真实代理上复验。

`socks4` 不支持认证且无法远端解析域名，v1 不提供。

---

## 3. 配置模型

```rust
ProxyProfile {
  mode: ProxyMode,              // Direct | System | Http | Https | Socks5 | Socks5h
  host: Option<String>,
  port: Option<u16>,
  username_ref: Option<CredentialRef>,   // 只存引用，值在钥匙串
  password_ref: Option<CredentialRef>,
  bypass: Vec<String>,          // "10.0.0.0/8"、"*.corp.internal"、"webdav.local"、"192.168.1.20:5005"
  resolve_system_when_off: bool // System 模式下是否允许"无系统代理=直连"
}
```

* 凭据**永不**进 SQLite、不进日志、不进 URL 明文（`notera-net` 在写审计日志前统一脱敏，见 §9）。
* 存储：配置只存 profile 本体 + 一个引用。as-built（0.0.29，ADR-0020）：`credential_ref` = `keychain:<账户 id>`，系统侧的条目名是 `notera:webdav:<账户 id>`；代理凭据同一条通道，条目名 `notera:proxy:<账户 id>`（`UserName` 装用户名、blob 装口令的 UTF-16 字节）。引用**只在真的存进去之后**才写 —— 它是"系统里有一条"的证据，不是"用户填过"的证据。
* 上限：generic credential 的 blob 是 **512 字节**，按 UTF-16 算是 **256 个单元**。超限返回具名错误 `credential_too_long`（带上实际单元数），**什么都不写、也不截断** —— 截断存进去等于交给用户一个"配好了但永远 401"的账户。
* **钥匙串不可用时的降级（本地加密文件：argon2id 派生 + AES-256-GCM-SIV，并在 UI 明说"凭据保护强度下降"）尚未实现**。今天的行为是：`credential_store::available() == false` → `put/get/remove` 一律 `credential_unavailable`，发布版徽标停在"需要凭据"（`caps.keychain` 报 `none`，这一处至少是诚实的）。按 §40 记：原因 = 各平台的凭据后端没接、这台机器上也无法验证；影响 = macOS / Linux / Android 上发布版仍不能同步；解除条件 = 各平台后端 + 真机验证，或者实现这条降级路径（`argon2` 与 `aes-gcm-siv` 已在工作区依赖里）。**不静默降级为明文**这一条始终成立。

---

## 4. `System` 模式的分平台实现

| 平台 | 来源 | 实现 | PAC |
|---|---|---|---|
| Windows | WinHTTP 配置 | `WinHttpGetIEProxyConfigForCurrentUser`（用户手工设置）+ `WinHttpGetProxyForUrl`（自动配置脚本） | v1 只解析 `autoconfig_url` 存在与否并**提示**，不执行 JS PAC |
| macOS / iOS | `SystemConfiguration` | `SCDynamicStoreCopyProxies` → `HTTPEnable/HTTPProxy`、`SOCKSEnable/…` | 同上 |
| Android | `java.net.ProxySelector` | 经 Tauri 插件回调取得（Android 系统代理常由 Wi-Fi 设置或 `global_http_proxy` 提供） | 不支持 |
| Linux（开发用） | `http_proxy` / `https_proxy` / `no_proxy` 环境变量 | 直接读取 | 不支持 |

**PAC 明确不在 v1 范围**：执行 PAC 需要 JS 引擎，且企业 PAC 常依赖内网 AD 域身份。若用户的实际环境只有 PAC（很常见于公安网/企业网），则需要引入 `hyper-proxy` + 受限 JS 解释器或要求用户手工填代理地址 —— 记入 §10 U1，需人工确认环境后再决定是否升级。

`System` 模式的失败语义：读取系统配置失败 ≠ 无代理。必须区分并记入审计，否则会误判为"直连可用"。

---

## 5. DNS 语义

| 模式 | 解析方 | 后果 |
|---|---|---|
| Direct / Http / Https | 本地解析后连接代理或目标 | 内网域名在公网机器上解析不到（正常，应配 bypass） |
| `Socks5` | 本地解析，把 IP 交给代理 | 本地无该域名解析能力时失败；且**DNS 查询泄露**在本地网络 |
| `Socks5h` | 代理侧解析 | 规避上述两点；**内网场景推荐 SOCKS5h** |

规则：UI 提供 SOCKS5 时同时提供"由代理解析域名"开关（即 `socks5` vs `socks5h`），默认开（更稳、更少泄露）。

bypass 命中判定需要目标主机信息：对 `*.domain` 形式的 bypass 采用**不解析**的字符串匹配（避免为判 bypass 反而先发 DNS）。

---

## 6. TLS 与证书策略（内网自签是主场景）

`TlsPolicy` 按账户配置，四档：

| 档 | 含义 | 允许场景 |
|---|---|---|
| `strict` | 系统信任库校验（默认） | 公网正规证书 |
| `ca_bundle` | 追加/替换为指定 CA（PEM，内网根证书） | **内网自签主路径**，推荐 |
| `pin` | 校验叶/中间证书指纹白名单 | 无可用 CA、又要求强校验 |
| `insecure_local` | 跳过校验 | **仅** localhost 与显式勾选的内网 HTTP；UI 必须红字告警 |

实现要点：

* 根证书优先使用**系统信任库**（`rustls-platform-verifier`，实测在 windows-gnu 可编译），这样企业把内网根 CA 装进系统后 Notera 自动可信 —— 与公安网/企业网运维现实一致。
* `ca_bundle` 通过 `rustls` 的根存储追加实现，PEM 内容存 `settings`，不落明文路径引用。
* **纯 HTTP（非 TLS）默认拒绝**，仅当主机是 loopback 或用户显式选择 `insecure_local` 时允许，并在每次同步完成后在状态区保留"未加密传输"标记（不弹窗骚扰，但不隐藏）。
* 证书错误必须与"网络不可达"给出不同文案 —— 用户看到"连不上"会去查网线，看到"证书不受信任"才会去找运维。

---

## 7. 超时、重试、退避

分层超时（`Timeouts`，规范默认值）：

```text
connect        8 s      （代理握手也计入）
read          20 s
write         30 s      （附件上传单块）
per_request   45 s      总预算
round         20 s      一轮同步预算，超时则本轮收敛为部分完成
```

* 实测：400 ms 预算下停滞服务器请求在 402–416 ms 内返回，说明取消路径真实生效（不是"等它回来再看"）。**2026-09-30 这句从"某次手工 PROBE 的抄录"变成会红的门禁**：`notera-net/tests/cancel_within_budget.rs` 三判据（900 ms 预算实测 901–904 ms、且源站 `counters.hung` 数到那一次挂起），带下界防止"秒失败"冒充"超时"。
* 退避：`min(15min, 2s × 1.85^n) × (1±0.2 jitter)`，`Retry-After` 优先。
* 重试只作用于幂等请求（GET/HEAD/PROPFIND）与可判定重放的写（靠 `dedupe_key` + 内容哈希复验，见 SYNC-PROTOCOL §11.1）。
* 一轮内重试预算 3 次，超出即让位给下一轮，避免"一轮卡死 5 分钟"。

> **上面这四行是规范，不是 shipped 现状 —— 缺口 G34（2026-09-30 量出来的）。**
> `WebDavRemote::new` 把政策硬编成 `RetryPolicy::deterministic(40, 1)`：base **40 ms**、预算 **1**、
> jitter **0**。`RetryPolicy::default()`（也就是这里写的 2 s × 1.85^n、预算 3、±0.2 抖动）在**产品路径上没人用**，
> `with_retry` 这个覆盖口子全仓唯一的调用点是测试自己给的。读数：`notera-host/tests/retry_in_round.rs`
> 里"一轮连坏两次 503"整批跑完 **0.49 s**（真按本节的退避该是十几秒），一次挡得住、两次挡不住。
>
> 已经证明为真的两半：退避**算法**与预算**语义**在 `notera-net`（含 §7"只重试幂等"那道护栏，
> `tests/retry_idempotency.rs`：同注入同预算只变 `idempotent`，MOVE 一条都不许多发）；
> 调用点选择（条件写 / MOVE / DELETE 压根不进重试层）在 `notera-webdav/tests/retry_backoff.rs`。
> **没证明为真的**：本节这四行所描述的**时长与次数**在产品里生效。
>
> 为什么 G34 现在只记不改：把那一行换成 `default()` 会把"一轮失败"的耗时从 40 ms 抬到十几秒，
> 界面反馈时延、离线类测试与 `round 20 s` 预算都会被它拽动 —— 属 §9 那类要先量的取舍。
> 无抖动这一半单独看就更像缺陷（多台设备同一台 WebDAV 会齐步走重试），若要动，两条一起拍。
> 解除条件：用户拍"失败要更扛但报得更慢"还是"现状"，然后本节的四行与被硬编的那一行对成一句话。

---

## 8. 失败与降级矩阵

| 故障 | 出口行为 | 本地影响 | 用户可见 |
|---|---|---|---|
| 代理不可达 | `Connect` 分类，退避 | **无**（照常读写） | `! 同步失败` |
| 代理认证失败(407) | 停轮、这一轮不写入：`map_status(407) → RemoteError::Auth →` 错误码 `sync_auth_failed`；待办留在 outbox（`Pending`/`Failed` 两态，**没有** "blocked" 这个状态），改对凭据后重试即可 | **无**（照常读写） | `! 服务器或代理不认这组凭据…`（文案键 `sync.auth_failed` 与 `error.sync_auth_failed`）。**0.0.46 之前这一格是假的**：`negotiate` 把所有远端错误折成 `sync.protocol_unreadable`（"暂时读不到…请稍后重试"），凭据问题被说成"等一会儿" —— 缺口 G33，现在由 `notera-host/tests/proxy_account_407.rs` 两条门禁钉住 |
| 代理中途挂 | 请求超时 → 重试 | 无 | `↻ 正在同步`→`!` |
| DNS 失败 | `Dns` 分类（与 `Connect` 区分） | 无 | `! 无法解析服务器地址` |
| TLS 校验失败 | `Tls` 分类，**不降级重试** | 无 | `! 证书不受信任` + 指向 §6 配置。**2026-09-30 第一次有门禁**（`notera-net/tests/tls_policies.rs`，新工装 `TlsOrigin`：每次运行现造一张自签 CA + 叶证书，盘上不落密钥）：自签链在 `Strict` 下必须拒且**一个字节的应用数据都不交换**（源站两本账 `accepted≥1 / handled==0`）；`Tls` 不算可重试形态 —— 配预算 3 也只上一次连接（`accepted` 数出来的）。变异 M13（`InsecureLocal` 放行任意主机）、M14（把 `Tls` 当可重试）各红在对应的门禁上。**同批量出缺口 G35**：`CaBundle`（本节写的"内网自签主路径"）在 Windows 上走不通，见 §6 的说明 |
| 从 System 切到 Direct 后恢复 | 立即触发一轮（网络变化事件） | 无 | `↻` |
| 配置写坏（非法 host/port） | 保存前校验拒绝；已存在的坏配置 → 回退 Direct 并告警 | 无 | `! 代理配置无效` |

**核心不变式**：任何网络/代理故障都不得阻塞本地笔记的创建、编辑、删除、搜索、移动（I8）。代理是同步的传输细节，不是应用可用性的前置条件。

---

## 9. 可观测与可证伪

每个请求写一条审计（内存环形缓冲 + 落盘日志，**脱敏后**）：

```text
NetAudit { ts, method, host, path, proxy_mode, proxy_endpoint, bypassed,
           dns_ms, connect_ms, tls_ms, total_ms, status, bytes_out, bytes_in,
           error_kind?, credential_sent: bool }
```

脱敏规则：`Authorization` / `Proxy-Authorization` 头一律替换为 `<redacted:basic>`；URL 中的 `user:pass@` 去除；日志文件权限跟随应用数据目录。

### 怎么证明"真的走了代理"（需求 §19 要求可测试）

三条互补证据，缺一不可。**今天（2026-09-27）起证据 1 与 2 已经是自动门禁**，不再只是手工 PROBE：
`cargo test -p notera-webdav --test proxy_routing`（`crates/notera-webdav/tests/proxy_routing.rs`，3 条）。

1. **差分测试**：同一目标，`proxy=死代理` 必须失败、`no_proxy` 必须成功。若死代理下仍成功 → 代理被静默忽略 → 缺陷。
   已进门禁的形态：死代理端口（`127.0.0.1:1`）、代理主机名解析不出来（`no-such-proxy-host.invalid`）、
   `bypass` 命中目标时照常可用、`bypass` 列表里只有一条不相干主机名时**不许**顺手放行、
   撤掉代理换回直连立刻可读（"恢复"那一腿）。此前只有 `dead-proxy→error=true; no_proxy→HTTP 200` 的一次性探针记录。
   **变异自证**：把 `notera-net::configure()` 里那条 `b.proxy(p)` 换成 `b.no_proxy()`（即"配了代理等于没配"这类缺陷）
   → 两条测试同时红，分别报 `配了 HTTP 代理却被服务器拒掉：403` 与 `指向死代理的请求居然成功了`。
2. **代理独占**：`notera-test-webdav` 支持"仅接受经前置代理到达的连接"模式 —— 直连一律 403。于是"同步成功"本身就构成"确实走了代理"的证明，无需信任客户端自述。
   已进门禁：配 HTTP 代理写 + 读回必须 2xx，直连必须 403，且要读服务器自己的 `rejections_403_not_proxied` 计数
   （不看客户端自述，也不假设"没被挡"）。
   **这一条的边界要说清**：工装里那个"代理"**就是源站本身** —— 它认识 CONNECT 与绝对形式请求，收下之后由自己的存储应答。
   因此这套门禁证的是"我们的出口确实按代理的方式发了请求"，**不证**"请求被转发到了另一台独立源站"。
   **2026-09-30 补上了那两个独立组件**：`Socks5Forwarder`（真 RFC 1928 握手 + 按需 RFC 1929）与
   `HttpForwardProxy`（真绝对形式解析 + origin-form 转发 + 可选 RFC 7235 `407`），代理与源站分开，各自进门禁
   （见 TEST-PLAN §28 那两行）。补的时候顺带量出一件以前没说清的事：**真转发代理交给源站的是 origin-form，
   源站分不出"经代理"与"直连"** —— 所以 `require_proxy`（上面第 2 条那个"代理独占"模式）**只在"源站扮演代理"
   的老夹具里成立，与真转发代理不能同机使用**；真代理那两格的牙齿换成"代理自己的计数 vs 源站收到的请求数"两边对照。
   还缺的一块是**能出示证书的源站**（真 TLS 握手失败 / `CaBundle` / `Pin` 的实测），按 §40 记在 TEST-PLAN §28 与下面 U2/U3。
   逐条缺口见 `docs/TEST-PLAN.md`「§28 十二条 —— 实证账」。
3. **RouteProof 回传**：每次同步的 `RoundStats` 带实际出口（`proxy_endpoint`），`notera-cli net probe` 打印三者一致性。

`net probe` 输出示例（人读 + `--json`）：目标解析、实际连接地址、CONNECT 是否成功、TLS 链、服务端证书指纹、往返分段耗时。

---

## 10. 移动端注意

* Android/iOS 上系统代理读取路径与桌面不同，且切 Wi-Fi 后配置会变 → 网络状态事件必须触发**重新解析代理**，不能复用旧连接池配置。
* 移动后台任务时间片短：代理握手 + TLS 全量协商可能吃掉几秒 → 保持连接池存活（`pool_idle` 60 s），前台时预热。
* 不实现"检测到代理失败就自动尝试直连"的静默回退：内网环境下直连必然失败但会耗时，且静默回退会让用户以为配置没生效。失败要如实报。

---

## 11. 未决与需要人工确认

| # | 事项 | 状态 | 需要什么 |
|---|---|---|---|
| U1 | 用户环境是否只有 PAC（无手工代理地址） | **待人工确认** | 若"是"，v1 需升级到执行 PAC（引入 JS 引擎），影响依赖与工期 |
| U2 | 内网 WebDAV 的实际 TLS 形态（自签 / 企业内网 CA / 纯 HTTP） | **BLOCKED** | 需一个可访问的真实端点做兼容实测 |
| U3 | `Https` 模式（与代理建立 TLS）实测 | 待测 | 需要一个 TLS 代理端点 |
| U4 | Linux 无 Secret Service 时的降级体验 | 待设计 | 影响开发机与 CI 桌面冒烟 |
| U5 | 是否需要"按账户不同代理"同时生效（多账户不同网络） | 未决 | 当前模型支持，UI 未设计 |
