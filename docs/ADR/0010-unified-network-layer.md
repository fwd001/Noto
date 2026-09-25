# ADR-0010: 统一网络层与 App 级代理

## 状态
日期：2026-09-25
状态：Accepted

## 背景
需求要求 App 级代理（不改系统设置），并要求能证明"不存在某些请求走代理、某些绕过代理而自己不知道"。这类命题无法靠"看起来都走同一处"来证明，必须让旁路在编译期与 CI 期都不存在，并让每个请求自带出口证据。

## 决策
- 所有 HTTP 必经 `notera-net`：不 `pub` 底层客户端类型，只暴露 `send(RequestSpec) -> Response`；`notera-webdav` 的依赖声明里没有 `reqwest`。CI 用 Cargo 依赖图 + grep 双闸门。
- 代理模式：`Direct | System | Http | Https | Socks5 | Socks5h` + `bypass`（host / CIDR / `*.domain`）；配置按账户（per-account），凭据只存钥匙串引用。
- TLS 四档：`strict | ca_bundle | pin | insecure_local`；根证书优先用系统信任库（`rustls-platform-verifier`，实测 GNU 可编译）；企业把内网根 CA 装进系统后自动可信。
- 纯 HTTP（非 TLS）默认拒绝，仅 loopback 或用户显式选 `insecure_local` 时允许，并在同步完成后保留"未加密传输"标记。
- 每个请求产出 `RouteProof` 与脱敏审计日志（`NetAudit`：method/host/path/proxy_mode/proxy_endpoint/bypassed/分段耗时/status/bytes/`credential_sent`）。

实测（`docs/evidence/probe-windows-gnu.txt`）：
- `socks5://user:pass@host` 的凭据在 reqwest 0.13.5 中被解析（`proxy.rs:744`）并在连接层消费（`connect.rs:1956`），因此**无需自造 loopback 转发器**；`socks5h` 支持远端 DNS。
- 代理差分：`dead-proxy->error=true; no_proxy->HTTP 200`。

## 备选方案与被否决的原因
- 用系统全局代理设置：需求要求 App 级，且不能改系统；同一台机器上其他应用不应被笔记软件影响。
- 各模块各自发请求（webdav / 更新检查 / 导入器各建客户端）：违反 N1，无法证明一致性，代理与超时策略会逐处漂移。
- 执行 PAC 脚本：需要 JS 引擎，v1 不做（延后），列为待人工确认 U1——若用户环境只有 PAC 则必须重新评估。

## 后果
正面：代理策略、TLS 策略、退避与审计集中一处；"是否真走代理"有可证伪的判定手段而不是自述。
代价：
- PAC 环境可能不可用，用户需手工填代理地址（企业网/公安网常见，见 U1）。
- `System` 模式需四端分别实现（WinHTTP / SystemConfiguration / ProxySelector / 环境变量），且切 Wi-Fi 后要重新解析、不能复用旧连接池配置。
- 唯一出口意味着任何新增 HTTP 用途（更新检查、分享拉取）都要经过 `notera-net` 的接口扩展，不能就近建客户端。

## 验证方式
可证伪方法三条，缺一不可，全部进 CI：
1. 差分测试：同一目标，`proxy=死代理` 必须失败、`no_proxy` 必须成功；死代理下仍成功 = 代理被静默忽略 = 缺陷。
2. 代理独占：`notera-test-webdav` 的"仅接受经代理到达的连接"模式（直连一律 403）——同步成功本身即构成"确实走了代理"的证明。
3. `RouteProof` 回传实际出口（`proxy_endpoint`），与前两者的观测一致。
- `notera-cli net probe` 打印三者一致（人读 + `--json`），作为第三方裁判。
- 依赖闸门：`cargo tree -e normal -p notera-webdav` 无 reqwest；CI grep `reqwest::Client|hyper::|rustls::ClientConfig` 命中 `notera-net` 之外即 fail。
- 代理失败用例：`FAIL(status=407)` / 死代理下断言本地读写（新建、编辑、删除、搜索）不受影响，outbox 内容不变（N2）。

## 关联
- PROXY.md 全文（§1 唯一出口闸门、§2 模式与实测能力、§4 分平台 System 模式、§6 TLS 四档、§9 可证伪三证据、§11 U1）
- ARCHITECTURE-MAP.md §3 N1/N2、§5 禁止模式（`notera-net` 之外的 HTTP 客户端）
- SYNC-PROTOCOL.md §12 错误分类与退避
- 实测：`docs/evidence/probe-windows-gnu.txt` 的 `proxy-config-actually-honored`、`https-tls-handshake`、`timeout-and-cancel`
- ADR-0013（代理独占模式由 test-webdav 提供）· ADR-0001（网络唯一出口的架构位置）
