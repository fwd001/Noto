//! notera-config —— 账户、代理与偏好配置。
//!
//! ## 为什么账户配置落在 `config.json` 而不是 SQLite `settings`
//!
//! 三条硬理由（与 docs/DATA-MODEL.md §6 的分类一致，只是把 scope=account 的载体换成文件）：
//! 1. **启动顺序**：`notera-host` 必须在打开 SQLite 之前就决定连哪个服务器（PLATFORM.md §3），
//!    配置若在库里就成了"要先开库才知道开哪个库"的循环。
//! 2. **备份/恢复语义**：库备份是**用户内容**。把服务器地址与凭据引用卷进内容备份，
//!    恢复时会把一台机器的端点配置带到另一台，属于事故而非特性。
//! 3. **凭据边界**：密码/token 永不入库（只存引用），配置里也只有引用。
//!
//! UI / 设备级偏好（主题、字号、侧栏折叠、每设备排序）仍在 SQLite `settings` 表，
//! 由 `notera-store` 负责；本 crate 只提供其类型与校验，不直连 SQLite，
//! 以免与存储层实现互相阻塞。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const CONFIG_FILE: &str = "config.json";
pub const CONFIG_VERSION: u32 = 1;

// ---------------------------------------------------------------- 错误 ---

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum ConfigError {
    #[error("字段 {field} 无效: {why}")]
    Invalid { field: &'static str, why: String },
    #[error("账户 {0} 不存在")]
    UnknownAccount(String),
    #[error("配置文件读写失败: {0}")]
    Io(String),
    #[error("配置文件版本 {found} 高于本程序支持的 {supported}，已拒绝写入")]
    TooNew { found: u32, supported: u32 },
    #[error("凭据不可用: {0}")]
    CredentialMissing(String),
}

// ---------------------------------------------------------------- 类型 ---

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TlsPolicyKind {
    /// 系统信任库严格校验（公网默认）
    Strict,
    /// 追加/替换为给定 PEM 根证书（内网自签主路径）
    CaBundle,
    /// 校验叶/中间证书指纹白名单
    Pin,
    /// 跳过校验 —— 仅允许 loopback 或用户显式勾选，UI 必须红字告警
    InsecureLocal,
}

impl Default for TlsPolicyKind {
    fn default() -> Self {
        TlsPolicyKind::Strict
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ProxyMode {
    #[default]
    Direct,
    System,
    Http,
    Https,
    Socks5,
}

/// 代理参数。凭据只存引用，值在系统钥匙串（PROXY.md §3）。
///
/// 注意：**不能** `#[derive(Default)]` —— 那会让 `resolve_remote_dns` 取 bool 的
/// 默认 false，与 serde 侧的 `default = "default_true"` 不一致，导致
/// "代码里构造的 profile 走本地 DNS、从文件读出的走远端 DNS" 这种隐蔽分叉。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyProfile {
    pub mode: ProxyMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// 钥匙串引用（service/account），**不是**明文用户名。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_ref: Option<String>,
    /// host / CIDR / `*.domain` / `host:port`。匹配为字符串比较，不发 DNS（PROXY.md §5）。
    #[serde(default)]
    pub bypass: Vec<String>,
    /// SOCKS5 是否让代理侧解析域名（socks5h）。默认开：更少泄露、内网域名可解析。
    #[serde(default = "default_true")]
    pub resolve_remote_dns: bool,
}

fn default_true() -> bool {
    true
}

impl Default for ProxyProfile {
    fn default() -> Self {
        Self {
            mode: ProxyMode::default(),
            host: None,
            port: None,
            username_ref: None,
            password_ref: None,
            bypass: Vec::new(),
            resolve_remote_dns: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AuthKind {
    #[default]
    Basic,
    Token,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountConfig {
    pub id: String,
    pub label: String,
    pub base_url: String,
    pub root_prefix: String,
    pub auth_kind: AuthKind,
    /// Basic 的用户名 / Token 的主体名。**用户名不是秘密**，可以进配置；
    /// 秘密（口令）只以 `credential_ref` 指向钥匙串。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// 凭据引用；配置里永不含明文。
    pub credential_ref: String,
    pub tls_policy: TlsPolicyKind,
    /// 内网自签场景的 PEM 根证书（可选，明文可存 —— 公钥不是秘密）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca_pem: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned_sha256: Option<Vec<String>>,
    pub proxy: ProxyProfile,
    pub enabled: bool,
}

impl Default for AccountConfig {
    fn default() -> Self {
        Self {
            id: String::new(),
            label: "WebDAV".into(),
            base_url: String::new(),
            root_prefix: "/.notes".into(),
            auth_kind: AuthKind::Basic,
            username: None,
            credential_ref: String::new(),
            tls_policy: TlsPolicyKind::default(),
            ca_pem: None,
            pinned_sha256: None,
            proxy: ProxyProfile::default(),
            enabled: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncBudget {
    /// 桌面周期。取 25s 给一轮留 5s 余量以兑现"≤30s 到达"（SYNC-PROTOCOL §14）。
    pub period_secs: u32,
    pub debounce_ms: u64,
    pub mobile_period_secs: u32,
    pub max_concurrent_rounds: u8,
    pub pull_concurrency: u8,
    pub round_request_cap: u16,
    pub round_byte_cap: u32,
}

impl Default for SyncBudget {
    fn default() -> Self {
        Self {
            period_secs: 25,
            debounce_ms: 2500,
            mobile_period_secs: 900,
            max_concurrent_rounds: 1,
            pull_concurrency: 8,
            round_request_cap: 200,
            round_byte_cap: 8 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AttachPrefs {
    pub auto_upload: bool,
    pub wifi_only: bool,
    pub max_inline_bytes: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppConfig {
    pub version: u32,
    pub device_id: String,
    pub accounts: Vec<AccountConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_account: Option<String>,
    pub sync: SyncBudget,
    pub attach: AttachPrefs,
    /// 只放"必须在开库前知道"的东西。UI/设备偏好走 SQLite settings。
    #[serde(default)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            device_id: String::new(),
            accounts: Vec::new(),
            active_account: None,
            sync: SyncBudget::default(),
            attach: AttachPrefs::default(),
            extra: BTreeMap::new(),
        }
    }
}

// ---------------------------------------------------------------- 校验 ---

/// 校验一个账户草案。**这里挡住的是会被拿去发请求的输入**，属于系统边界。
pub fn validate_account(a: &AccountConfig) -> Result<(), ConfigError> {
    if a.label.trim().is_empty() {
        return Err(ConfigError::Invalid {
            field: "label",
            why: "不能为空".into(),
        });
    }
    validate_base_url(&a.base_url)?;
    if a.root_prefix.is_empty() || !a.root_prefix.starts_with('/') {
        return Err(ConfigError::Invalid {
            field: "root_prefix",
            why: "必须以 / 开头".into(),
        });
    }
    if a.root_prefix.contains("..") || a.root_prefix.ends_with('/') && a.root_prefix != "/" {
        return Err(ConfigError::Invalid {
            field: "root_prefix",
            why: "不得包含 .. 且不应以 / 结尾".into(),
        });
    }
    match a.tls_policy {
        TlsPolicyKind::CaBundle => {
            if a.ca_pem.as_deref().unwrap_or("").trim().is_empty() {
                return Err(ConfigError::Invalid {
                    field: "ca_pem",
                    why: "选择「自定义 CA」时必须提供 PEM".into(),
                });
            }
        }
        TlsPolicyKind::Pin => {
            let pins = a.pinned_sha256.clone().unwrap_or_default();
            if pins.is_empty() {
                return Err(ConfigError::Invalid {
                    field: "pinned_sha256",
                    why: "选择「指纹锁定」时至少需要一个指纹".into(),
                });
            }
            for p in pins {
                if p.len() != 64 || !p.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return Err(ConfigError::Invalid {
                        field: "pinned_sha256",
                        why: format!("指纹不是 64 位十六进制: {p}"),
                    });
                }
            }
        }
        TlsPolicyKind::InsecureLocal => {
            // 明文传输只在显式确认下允许；这里挡住"顺手勾上"的公网目标。
            if !is_local_host(&a.base_url) {
                return Err(ConfigError::Invalid {
                    field: "tls_policy",
                    why: "insecure_local 仅允许 localhost/内网地址".into(),
                });
            }
        }
        TlsPolicyKind::Strict => {}
    }
    validate_proxy(&a.proxy)?;
    Ok(())
}

pub fn validate_base_url(raw: &str) -> Result<(), ConfigError> {
    let bad = |why: String| ConfigError::Invalid { field: "base_url", why };
    let u = url2(raw).ok_or_else(|| bad("格式无法解析".into()))?;
    match u.scheme {
        "https" => {}
        "http" => {
            if !is_local_host(raw) {
                return Err(bad("明文 http 仅允许 localhost；内网请装证书或显式选择 insecure_local".into()));
            }
        }
        other => return Err(bad(format!("不支持的协议 {other}"))),
    }
    if u.host.is_empty() {
        return Err(bad("缺少主机名".into()));
    }
    if !u.userinfo.is_empty() {
        return Err(bad("凭据不得写在 URL 里".into()));
    }
    if u.path.chars().filter(|&c| c == '.').count() > 0 && u.path.contains("..") {
        return Err(bad("路径包含目录穿越".into()));
    }
    Ok(())
}

pub fn validate_proxy(p: &ProxyProfile) -> Result<(), ConfigError> {
    let bad = |why: String| ConfigError::Invalid { field: "proxy", why };
    match p.mode {
        ProxyMode::Direct | ProxyMode::System => return Ok(()),
        _ => {}
    }
    let host = p.host.clone().unwrap_or_default();
    if host.trim().is_empty() {
        return Err(bad("该代理模式需要主机地址".into()));
    }
    if host.contains("://") || host.contains('/') || host.contains('@') || host.contains("..") {
        return Err(bad("主机地址不应含协议、路径或凭据".into()));
    }
    let port = p.port.unwrap_or(0);
    if port == 0 {
        return Err(bad("该代理模式需要端口".into()));
    }
    if (p.username_ref.is_some()) != (p.password_ref.is_some()) {
        return Err(bad("代理用户名与密码引用必须成对出现".into()));
    }
    for b in &p.bypass {
        let t = b.trim();
        if t.is_empty() || t.len() > 253 || t.contains(' ') || t.contains("://") {
            return Err(bad(format!("bypass 条目无效: {b}")));
        }
    }
    Ok(())
}

/// bypass 匹配：**纯字符串比较**，绝不为判定而先发 DNS（PROXY.md §5）。
/// 支持 `*`、`example.com`、`*.example.com`、`10.0.0.0/8`、`host:port`。
pub fn bypass_matches(host: &str, rules: &[String]) -> bool {
    let h = host.trim_end_matches('.').to_ascii_lowercase();
    // 允许传 "host:port"，判定按主机部分。
    let hp = h.split_once(':').map(|(a, _)| a.to_string()).unwrap_or_else(|| h.clone());
    for raw in rules {
        let r = raw.trim().to_ascii_lowercase();
        if r.is_empty() {
            continue;
        }
        if r == "*" {
            return true;
        }
        // CIDR：只在规则本身含 / 且两侧都是 IPv4 时判定
        if r.contains('/') {
            let probe = hp.split_once(':').map(|(a, _)| a.to_string()).unwrap_or(hp.clone());
            if ip_in_cidr(&probe, &r) {
                return true;
            }
            continue;
        }
        let r_host = r.split_once(':').map(|(a, _)| a.to_string()).unwrap_or(r.clone());
        if let Some(suffix) = r_host.strip_prefix("*.") {
            // `*.example.com` 匹配 example.com 自身与任意子域，但**不**匹配 evilexample.com
            if hp == suffix || hp.ends_with(&format!(".{suffix}")) {
                return true;
            }
            continue;
        }
        if r_host == hp || r == h {
            return true;
        }
    }
    false
}

fn is_local_host(raw: &str) -> bool {
    let u = match url2(raw) {
        Some(u) => u,
        None => return false,
    };
    let raw = u.host.to_ascii_lowercase();
    // 主机部分常带端口（"127.0.0.1:5005"），不剥掉就会把本机误判成公网。
    let h = raw
        .strip_prefix('[')
        .map(|x| x.trim_end_matches(']').to_string())
        .filter(|x| x.contains(':'))
        .unwrap_or_else(|| raw.split_once(':').map(|(a, _)| a.to_string()).unwrap_or(raw.clone()));
    h == "localhost" || h == "127.0.0.1" || h == "::1" || h.starts_with("192.168.") || h.starts_with("10.")
}

/// 极简 URL 拆分（避免为一个字段拉一个依赖）。只用于校验，不用于请求构造。
struct Bits<'a> {
    scheme: &'a str,
    userinfo: &'a str,
    host: &'a str,
    path: &'a str,
}

fn url2(raw: &str) -> Option<Bits<'_>> {
    let (scheme, rest) = raw.split_once("://")?;
    let authority = rest.split('/').next().unwrap_or("");
    let path = &rest[authority.len()..];
    let (userinfo, host) = match authority.rsplit_once('@') {
        Some((u, h)) => (u, h),
        None => ("", authority),
    };
    if scheme.is_empty() || host.is_empty() {
        return None;
    }
    Some(Bits { scheme, userinfo, host, path })
}

fn ip_in_cidr(ip: &str, cidr: &str) -> bool {
    let (net, bits) = match cidr.split_once('/') {
        Some(v) => v,
        None => return false,
    };
    let bits: u32 = match bits.parse() {
        Ok(b) if b <= 32 => b,
        _ => return false,
    };
    let a = match octets(ip) {
        Some(v) => v,
        None => return false,
    };
    let b = match octets(net) {
        Some(v) => v,
        None => return false,
    };
    let mask: u32 = if bits == 0 { 0 } else { u32::MAX << (32 - bits) };
    (a & mask) == (b & mask)
}

fn octets(s: &str) -> Option<u32> {
    let mut acc = 0u32;
    for part in s.split('.') {
        let n: u32 = part.parse().ok()?;
        if n > 255 {
            return None;
        }
        acc = (acc << 8) | n;
    }
    if s.split('.').count() != 4 {
        return None;
    }
    Some(acc)
}

// ---------------------------------------------------------------- 仓库 ---

/// 文件型配置仓库。原子写（tmp + rename），损坏时拒绝覆盖用户配置。
#[derive(Clone, Debug)]
pub struct ConfigRepository {
    path: PathBuf,
}

impl ConfigRepository {
    pub fn new(data_dir: &Path) -> Self {
        Self { path: data_dir.join(CONFIG_FILE) }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 读不到文件 ≠ 错误：返回默认配置（首次启动）。
    /// 但文件**存在却解析失败**必须报错并保留原文件 —— 静默重置会把用户的服务器地址抹掉。
    pub fn load(&self) -> Result<AppConfig, ConfigError> {
        match std::fs::read_to_string(&self.path) {
            Ok(s) => {
                let cfg: AppConfig = serde_json::from_str(&s)
                    .map_err(|e| ConfigError::Io(format!("配置文件已损坏，未覆盖：{e}")))?;
                if cfg.version > CONFIG_VERSION {
                    return Err(ConfigError::TooNew { found: cfg.version, supported: CONFIG_VERSION });
                }
                Ok(cfg)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(AppConfig::default()),
            Err(e) => Err(ConfigError::Io(e.to_string())),
        }
    }

    pub fn save(&self, cfg: &AppConfig) -> Result<(), ConfigError> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| ConfigError::Io(e.to_string()))?;
        }
        for a in &cfg.accounts {
            validate_account(a)?;
        }
        let body = serde_json::to_string_pretty(cfg).map_err(|e| ConfigError::Io(e.to_string()))?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, body.as_bytes()).map_err(|e| ConfigError::Io(e.to_string()))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| ConfigError::Io(e.to_string()))?;
        Ok(())
    }

    pub fn upsert_account(&self, cfg: &mut AppConfig, a: AccountConfig) -> Result<(), ConfigError> {
        validate_account(&a)?;
        match cfg.accounts.iter_mut().find(|x| x.id == a.id) {
            Some(slot) => *slot = a,
            None => {
                let mut a = a;
                if a.id.is_empty() {
                    a.id = new_id();
                }
                if cfg.active_account.is_none() {
                    cfg.active_account = Some(a.id.clone());
                }
                cfg.accounts.push(a);
            }
        }
        Ok(())
    }

    pub fn remove_account(&self, cfg: &mut AppConfig, id: &str) -> Result<(), ConfigError> {
        let before = cfg.accounts.len();
        cfg.accounts.retain(|x| x.id != id);
        if cfg.accounts.len() == before {
            return Err(ConfigError::UnknownAccount(id.to_string()));
        }
        if cfg.active_account.as_deref() == Some(id) {
            cfg.active_account = cfg.accounts.first().map(|x| x.id.clone());
        }
        Ok(())
    }

    pub fn active(cfg: &AppConfig) -> Option<&AccountConfig> {
        cfg.accounts.iter().find(|a| Some(&a.id) == cfg.active_account.as_ref() && a.enabled)
    }
}

fn new_id() -> String {
    notera_core::EntityId::new().to_string()
}

// ---------------------------------------------------------------- 测试 ---

#[cfg(test)]
mod tests {
    use super::*;

    fn acct(base: &str) -> AccountConfig {
        AccountConfig {
            id: "a1".into(),
            label: "家里".into(),
            base_url: base.into(),
            ..Default::default()
        }
    }

    #[test]
    fn https_ok_http_public_rejected() {
        assert!(validate_account(&acct("https://dav.example.com/dav")).is_ok());
        let e = validate_account(&acct("http://dav.example.com/dav")).unwrap_err();
        assert!(format!("{e}").contains("http"), "公网明文必须被拒: {e}");
    }

    #[test]
    fn http_localhost_allowed() {
        assert!(validate_account(&acct("http://127.0.0.1:5005/.notes")).is_ok());
    }

    #[test]
    fn credentials_in_url_rejected() {
        let e = validate_account(&acct("https://user:pw@dav.example.com")).unwrap_err();
        assert!(format!("{e}").contains("URL"), "{e}");
    }

    #[test]
    fn root_prefix_rejects_traversal() {
        let mut a = acct("https://d.example.com");
        a.root_prefix = "/.notes/../../etc".into();
        assert!(validate_account(&a).is_err());
    }

    #[test]
    fn proxy_requires_host_and_port_and_paired_creds() {
        let mut a = acct("https://d.example.com");
        a.proxy.mode = ProxyMode::Socks5;
        assert!(validate_account(&a).is_err(), "缺 host/port 必须拒");
        a.proxy.host = Some("127.0.0.1".into());
        a.proxy.port = Some(1080);
        assert!(validate_account(&a).is_ok());
        a.proxy.username_ref = Some("u".into());
        assert!(validate_account(&a).is_err(), "只有用户名引用、无密码引用必须拒");
        a.proxy.password_ref = Some("p".into());
        assert!(validate_account(&a).is_ok());
    }

    #[test]
    fn insecure_local_blocked_for_public() {
        let mut a = acct("https://d.example.com");
        a.tls_policy = TlsPolicyKind::InsecureLocal;
        assert!(validate_account(&a).is_err());
        let mut b = acct("http://192.168.1.20:5005/.notes");
        b.tls_policy = TlsPolicyKind::InsecureLocal;
        assert!(validate_account(&b).is_ok());
    }

    #[test]
    fn ca_bundle_needs_pem_and_pin_needs_hex() {
        let mut a = acct("https://d.example.com");
        a.tls_policy = TlsPolicyKind::CaBundle;
        assert!(validate_account(&a).is_err());
        a.ca_pem = Some("-----BEGIN CERTIFICATE-----\nxx\n-----END CERTIFICATE-----".into());
        assert!(validate_account(&a).is_ok());
        a.tls_policy = TlsPolicyKind::Pin;
        assert!(validate_account(&a).is_err());
        a.pinned_sha256 = Some(vec!["zz".into()]);
        assert!(validate_account(&a).is_err());
        a.pinned_sha256 = Some(vec!["ab".repeat(32)]);
        assert!(validate_account(&a).is_ok());
    }

    #[test]
    fn bypass_matches_without_dns() {
        let rules = vec!["*.corp.internal".to_string(), "10.0.0.0/8".to_string(), "localhost".to_string()];
        assert!(bypass_matches("dav.corp.internal", &rules));
        assert!(bypass_matches("corp.internal", &rules));
        assert!(!bypass_matches("evilcorp.internal", &rules), "后缀匹配必须看点号边界");
        assert!(bypass_matches("10.20.30.40", &rules));
        assert!(!bypass_matches("11.20.30.40", &rules));
        assert!(bypass_matches("localhost:5005", &rules));
    }

    #[test]
    fn cidr_boundaries() {
        assert!(ip_in_cidr("10.0.0.1", "10.0.0.0/8"));
        assert!(!ip_in_cidr("9.255.255.255", "10.0.0.0/8"));
        assert!(ip_in_cidr("192.168.1.7", "192.168.1.0/24"));
        assert!(!ip_in_cidr("192.168.2.7", "192.168.1.0/24"));
        assert!(!ip_in_cidr("not-an-ip", "10.0.0.0/8"));
    }

    #[test]
    fn repo_roundtrip_and_corrupt_file_is_never_reset() {
        let dir = std::env::temp_dir().join(format!("notera-cfg-{}", new_id()));
        let repo = ConfigRepository::new(&dir);
        // 不存在 → 默认值（首次启动）
        let mut cfg = repo.load().unwrap();
        assert!(cfg.accounts.is_empty());
        repo.upsert_account(&mut cfg, acct("https://d.example.com")).unwrap();
        cfg.device_id = new_id();
        repo.save(&cfg).unwrap();
        let back = repo.load().unwrap();
        assert_eq!(back, cfg, "配置必须无损往返");
        // 存在但损坏 → 报错且**不覆盖**（否则用户服务器地址被抹掉）
        std::fs::write(repo.path(), b"{ this is not json").unwrap();
        assert!(repo.load().is_err());
        assert_eq!(std::fs::read_to_string(repo.path()).unwrap(), "{ this is not json");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn too_new_config_refuses_to_run() {
        let dir = std::env::temp_dir().join(format!("notera-cfg2-{}", new_id()));
        let repo = ConfigRepository::new(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut cfg = AppConfig::default();
        cfg.version = CONFIG_VERSION + 5;
        std::fs::write(repo.path(), serde_json::to_string(&cfg).unwrap()).unwrap();
        match repo.load() {
            Err(ConfigError::TooNew { .. }) => {}
            other => panic!("应拒绝比本程序更新的配置，实际 {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_validates_every_account() {
        let dir = std::env::temp_dir().join(format!("notera-cfg3-{}", new_id()));
        let repo = ConfigRepository::new(&dir);
        let mut cfg = AppConfig::default();
        cfg.accounts.push(acct("ftp://bad.example.com"));
        assert!(repo.save(&cfg).is_err(), "非法账户不得落盘");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn removing_active_account_repoints_active() {
        let dir = std::env::temp_dir().join(format!("notera-cfg4-{}", new_id()));
        let repo = ConfigRepository::new(&dir);
        let mut cfg = AppConfig::default();
        repo.upsert_account(&mut cfg, acct("https://a.example.com")).unwrap();
        let mut b = acct("https://b.example.com");
        b.id = "a2".into();
        repo.upsert_account(&mut cfg, b).unwrap();
        assert_eq!(cfg.active_account.as_deref(), Some("a1"));
        repo.remove_account(&mut cfg, "a1").unwrap();
        assert_eq!(cfg.active_account.as_deref(), Some("a2"));
        assert!(repo.remove_account(&mut cfg, "ghost").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn defaults_match_protocol_spec() {
        // SYNC-PROTOCOL §14 / PROXY.md §7 的规范默认值，漂了就会悄悄改变同步行为。
        let s = SyncBudget::default();
        assert_eq!(s.period_secs, 25);
        assert_eq!(s.debounce_ms, 2500);
        assert_eq!(s.mobile_period_secs, 900);
        assert_eq!(s.max_concurrent_rounds, 1);
        assert_eq!(AccountConfig::default().root_prefix, "/.notes");
        assert!(ProxyProfile::default().resolve_remote_dns);
    }
}
