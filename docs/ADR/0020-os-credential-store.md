# ADR-0020: OS 凭据库接入（Windows Credential Manager；非 Windows 明确记为未接）

## 状态
日期：2026-09-28
状态：Accepted（Windows 侧已实现并验通；macOS / Linux / Android 见"后果"一节，按 BLOCKED 记）

## 背景

`AccountConfig.credential_ref` 从 Phase 0 起语义就是**"指向 OS 钥匙串的引用"**（DATA-MODEL §6
凭据行、PROXY.md §凭据），配置里从来不放明文。实现里这一格一直是空的：

* `configure_account` 收到 `draft.password` 之后**把它丢掉**，只把 `credential_ref` 写成 `keychain:{id}`；
* 解析点 `App::secret_for` 在 release 构建里只认一个开发用环境变量 `NOTERA_DEV_WEBDAV_SECRET`，
  而那个变量在用户的机器上不存在。

于是两件事同时成立，而且都是静默的：

```text
用户在设置页填了 WebDAV 口令
  → 配置写 credential_ref = "keychain:{id}"（背后什么都没有）
  → 界面按 hasCredential = !credential_ref.is_empty() 显示"口令已设置"   ← 指示说谎
  → 发布版 secret_for() 拿不到凭据 → 徽标停在"需要凭据"，永远同步不了    ← 核心卖点失效
```

第二行不是边角：整份方案是"数据在自己服务器上 + 自动同步"，发布版拿不到凭据就等于这个能力没有交付。
`PlatformCaps.keychain` 在 Windows 上也一直报 `"none"` —— 那是**唯一一处诚实的信号**，但它没有配套的行为。

## 决策

1. **口令只进系统凭据库，配置只留引用**（维持既有不变式，不新造一套存储）。
   Windows 用凭据管理器的 **generic credential**：目标名 `notera:webdav:<账户 id>`，
   `UserName` 存用户名、`CredentialBlob` 存口令的 **UTF-16 字节**。
   配置里的引用写成 `keychain:<账户 id>`，**其含义收紧为"系统里真有一条"** —— 只有 `put` 成功才写它。
   代理凭据同理共用这一条通道：目标名 `notera:proxy:<账户 id>`（`net_proxy` 以前一看到引用就报
   `proxy_credentials_pending`，于是"填了代理口令"在出口层永远不成立；0.0.31 把这个码改名为
   `proxy_credential_missing` 并重写了文案 —— 0.0.29 起代理口令是真能存的，"当前版本还不能安全地保存它们"
   那句已经说反，留着只是叫用户去查一件不存在的事）。
2. **顺序是"先存凭据，再落配置"，失败要回滚**。反过来会留下"配置说有条凭据、系统里什么都没有"的
   账户 —— 那正是本次要消灭的形状。落配置的任何一步失败，就把刚存进去的那几条抹掉，
   不留"没人引用、也没界面能再删掉"的口令。
3. **不截断**。generic credential 的 blob 上限是 **512 字节**，按 UTF-16 算是 **256 个单元**。
   超限返回具名错误 `credential_too_long`（带上到底多少个单元），什么都不写。
   截断存进去等于给用户一个"配好了但永远 401"的账户 —— 那是把可见失败换成不可见失败。
4. **解析顺序是"系统凭据优先，环境变量只作 debug 兜底"**。各条 lane 仍用 `NOTERA_DEV_WEBDAV_SECRET`
   喂凭据（用户名-only 的草稿现在 `hasCredential=false`，因为确实没存东西），
   但**发布版那条路只能走系统凭据**；顺序反过来的话，一条环境里的旧值会掩盖界面上刚改的口令。
5. **删账户连带删凭据**（`remove` 幂等，本来没有也算成功）。
6. **非 Windows 不假装**：`credential_store::available() == false`，`put/get/remove` 一律返回
   `credential_unavailable`，行为退回今天的样子（徽标停在"需要凭据"）。
   **这句从 0.0.31 起才有能照着办事的文案**：0.0.29 那批三条凭据码漏登记（原因见第 7 条），
   那之前非 Windows 配账号看到的是通用兜底那句。
   PROXY.md §凭据里那句"钥匙串不可用时降级为本地加密文件（argon2id + AES-256-GCM-SIV）"**尚未实现**，
   在该文件里按 BLOCKED 标注，不当已完成。

7. **错误码必须写成 `CmdError::of("字面量")`，不许从函数算出来**（0.0.31 补，起因是一条门禁盲区）。
   当时 `App::secret_err` 写的是 `CmdError::of(e.code())`：码从枚举算出来，而门禁
   `hygiene:rust-error-codes-registered` 只认 `CmdError::of("…")` 那个形状 —— 于是"漏登记文案"既不编译报错、
   也不测试失败，界面安静退成通用兜底，没人知道少了哪一句能照着办事的说明。`credential_too_long` /
   `credential_unavailable` / `credential_store_failed` 三条就是这么漏在表外的（非 Windows 配账号必踩；
   Windows 上口令超过 256 个 UTF-16 单元也踩）。
   **修法**：三个臂各写自己的字面量，`SecretError::code()` 那个逃逸口一并删掉；四条码（含改名后的
   `proxy_credential_missing`）补进 `i18n.ts` 与 `i18n.spec.ts` 的 `COMMAND_CODES`；新增门禁
   `hygiene:error-code-must-be-literal`（arch-check 第 30 条）把"算出来的码"这一形直接判红 ——
   变异自证 **M65**：把一个臂换成 `let code = "…"; CmdError::of(code, …)` → 29/30 红在那一行
   （打印出 `crates/notera-host/src/lib.rs → CmdError::of(code)`），还原 → 30/30。
   判据登记在 TEST-PLAN 的 **FT-CRED-07**；那条也照实写着"没测过非 Windows 真机上这句话长什么样"——
   本机没有那个平台，能证的只有"码有专属文案"与"这个形状不再被漏"。

## 后果

**正面**：发布版第一次真的能同步；`hasCredential` 这个指示从"用户填过没有"变成"系统里有一条"，
不再是谎；代理口令这条边也通了。

**代价与边界**：

* 口令变成**按用户**可见（凭据管理器里明文可列 `cmdkey /list`），这是这一格的固有性质，
  与"配置里放明文"的区别是：它不进备份包、不进日志、不进 SQLite、不跟着 ZIP 跑到别的机器上。
* 换机器 / 重装系统时凭据不会跟着走 —— 用户要重新填一次口令。这是有意的：
  备份包（`export_data`）里不含凭据，若含就等于把口令发到云上。
* Windows 上的这条边是**真系统状态**：门禁会真往当前用户的凭据库里写一条带 uuid 的目标名并在
  收尾抹掉（`Guard` 的 `Drop`）。所以 `--test credentials` 不与别的套件叠着跑。
* **非 Windows 未实现**（§46 那种"本机没环境"不掩盖）：原因 = 没接 Keychain / Secret Service，
  且这台机器上无法验证；影响 = macOS / Linux 上发布版仍不能同步、`PlatformCaps.keychain` 报 `none`
  （这一处至少是诚实的）；解除条件 = 各平台的凭据后端实现 + 真机验证，
  或者按 PROXY.md 那条降级成"本地加密文件 + UI 明说"（依赖已在工作区里：`argon2`、`aes-gcm-siv`）。

**判据**（TEST-PLAN FT-CRED-01..06）与变异自证 **M55..M61**：见 CHANGELOG 0.0.29 那一段。
