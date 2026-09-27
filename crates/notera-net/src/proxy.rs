//! 代理配置与 bypass 判定（docs/PROXY.md §2/§3/§5）。

use serde::{Deserialize, Serialize};

use crate::error::NetError;

/// 出口模式。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProxyMode {
    /// 显式直连（**不是**"没配代理"：会主动禁用系统代理）。
    #[default]
    Direct,
    /// 跟随操作系统设置（分平台读取，见 PROXY.md §4）。
    System,
    /// 明文 HTTP 代理（对 https 目标走 CONNECT，对 http 目标走绝对形式转发）。
    Http,
    /// 与代理服务器先建 TLS 再 CONNECT（PROXY.md U3：待实测）。
    Https,
    /// SOCKS5；`resolve_remote_dns` 为真时等价 `socks5h`（代理侧解析）。
    Socks5,
}

impl ProxyMode {
    pub fn as_str(self) -> &'static str {
        match self {
            ProxyMode::Direct => "direct",
            ProxyMode::System => "system",
            ProxyMode::Http => "http",
            ProxyMode::Https => "https",
            ProxyMode::Socks5 => "socks5",
        }
    }
    pub fn uses_proxy(self) -> bool {
        !matches!(self, ProxyMode::Direct)
    }
}

impl std::fmt::Display for ProxyMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 按账户可配置的代理档案。
///
/// 凭据在这里是**明文可选字段**：调用方（`notera-config` / 钥匙串适配层）负责取值，
/// 本 crate 保证它**永不**出现在 `Debug`、审计日志或 URL 明文里（PROXY.md §3/§9）。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyProfile {
    pub mode: ProxyMode,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub username: Option<String>,
    pub password: Option<String>,
    /// `"10.0.0.0/8"`、`"*.corp.internal"`、`"webdav.local"`、`"192.168.1.20:5005"`、
    /// `"/_control"`（URL 前缀）。
    pub bypass: Vec<String>,
    /// SOCKS5 是否由代理侧解析域名（`socks5h`，内网推荐且少泄露）。
    pub resolve_remote_dns: bool,
}

impl Default for ProxyProfile {
    fn default() -> Self {
        Self {
            mode: ProxyMode::Direct,
            host: None,
            port: None,
            username: None,
            password: None,
            bypass: Vec::new(),
            resolve_remote_dns: true,
        }
    }
}

impl ProxyProfile {
    pub fn direct() -> ProxyProfile {
        ProxyProfile::default()
    }

    pub fn http(host: impl Into<String>, port: u16) -> ProxyProfile {
        ProxyProfile {
            mode: ProxyMode::Http,
            host: Some(host.into()),
            port: Some(port),
            ..Default::default()
        }
    }

    pub fn socks5(host: impl Into<String>, port: u16) -> ProxyProfile {
        ProxyProfile {
            mode: ProxyMode::Socks5,
            host: Some(host.into()),
            port: Some(port),
            ..Default::default()
        }
    }

    /// `host:port`，用于审计与 [`RouteProof`](crate::RouteProof) 的 endpoint。
    pub fn endpoint(&self) -> Option<String> {
        let h = self.host.clone().filter(|s| !s.is_empty())?;
        Some(match self.port {
            Some(p) => format!("{h}:{p}"),
            None => h,
        })
    }

    /// 配置自检：非法 host/port 一律拒绝（PROXY.md §8"配置写坏"行）。
    pub fn validate(&self) -> Result<(), NetError> {
        if !self.mode.uses_proxy() {
            return Ok(());
        }
        let host = self
            .host
            .clone()
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| NetError::Protocol("代理模式要求 host".into()))?;
        if host.contains(char::is_whitespace)
            || host.contains('/')
            || host.contains('@')
            || host.contains("://")
        {
            return Err(NetError::Protocol(format!("代理 host 非法: {host}")));
        }
        if let Some(p) = self.port {
            if p == 0 {
                return Err(NetError::Protocol("代理 port 不得为 0".into()));
            }
        }
        if self.password.is_some() && self.username.is_none() {
            return Err(NetError::Protocol("带密码必须有用户名".into()));
        }
        Ok(())
    }

    /// 生成给 `reqwest::Proxy::all()` 的 URL。**含凭据，只在内部使用**：
    /// 任何对外展示一律走 [`ProxyProfile::describe`]。
    pub(crate) fn proxy_url(&self) -> Option<String> {
        let authority = self.endpoint()?;
        let auth = match (&self.username, &self.password) {
            (Some(u), Some(p)) => format!("{}:{}@", enc_fragment(u), enc_fragment(p)),
            (Some(u), None) => format!("{}@", enc_fragment(u)),
            _ => String::new(),
        };
        let scheme = match self.mode {
            ProxyMode::Http => "http",
            ProxyMode::Https => "https",
            ProxyMode::Socks5 if self.resolve_remote_dns => "socks5h",
            ProxyMode::Socks5 => "socks5",
            ProxyMode::Direct | ProxyMode::System => return None,
        };
        Some(format!("{scheme}://{auth}{authority}"))
    }

    /// 脱敏后的可读描述（进审计与 `RouteProof.detail`）。
    pub fn describe(&self) -> String {
        match self.mode {
            ProxyMode::Direct => "direct".into(),
            ProxyMode::System => format!(
                "system({})",
                self.endpoint().unwrap_or_else(|| "auto".into())
            ),
            m => format!(
                "{m}{}://{}<credential:{}>",
                if m == ProxyMode::Socks5 && self.resolve_remote_dns {
                    "h"
                } else {
                    ""
                },
                self.endpoint().unwrap_or_else(|| "?".into()),
                self.password.as_deref().map_or("none", |_| "redacted")
            ),
        }
    }

    /// bypass 命中判定：**纯字符串匹配，绝不发 DNS**（PROXY.md §5）。
    pub fn is_bypassed(&self, host: &str) -> bool {
        self.bypass.iter().any(|e| host_matches(e, host))
    }
}

impl std::fmt::Debug for ProxyProfile {
    /// 手工实现：密码永远不进 `Debug`。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProxyProfile")
            .field("mode", &self.mode)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username.as_deref().map(|_| "<redacted>"))
            .field("password", &self.password.as_deref().map(|_| "<redacted>"))
            .field("bypass", &self.bypass)
            .field("resolve_remote_dns", &self.resolve_remote_dns)
            .finish()
    }
}

/// userinfo 里 `:` `@` `/` 必须转义，否则代理 URL 解析错位。
fn enc_fragment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// 单条 bypass 条目对 host 的匹配。
///
/// 支持形式：`*`、`host`、`.domain`/`*.domain`、`host:port`、`IPv4/N`、`/url-prefix`。
/// 注意：CIDR 只对**字面 IP** 生效；对域名不做解析（否则就违反"不发 DNS"）。
pub fn host_matches(entry: &str, target: &str) -> bool {
    let entry = entry.trim();
    let target = target.trim().trim_end_matches('/').to_ascii_lowercase();
    if entry.is_empty() || target.is_empty() {
        return false;
    }
    if entry == "*" || entry == "***" {
        return true;
    }
    let e = entry.to_ascii_lowercase();

    // `/prefix` 形式：对 URL 路径生效（测试控制面用）
    if let Some(pfx) = e.strip_prefix('/') {
        return target.starts_with(&format!("/{pfx}")) || target == format!("/{pfx}");
    }

    // 带端口的条目：与 host[:port] 精确比
    if let Some((h, p)) = e.rsplit_once(':') {
        if !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) {
            if target == e {
                return true;
            }
            if target == h {
                // 目标省略了默认端口时也算命中
                return true;
            }
            continue_check_no_port(&target, h);
            return target == h;
        }
    }

    if target == e {
        return true;
    }
    // `*.domain` / `.domain`
    let bare = e.trim_start_matches('*');
    if bare.starts_with('.') {
        return target.ends_with(bare) || target == bare.trim_start_matches('.');
    }
    // CIDR（仅 IPv4 字面量）
    if let Some((net, bits)) = e.split_once('/') {
        if let (Ok(ip), Ok(b)) = (net.parse::<std::net::Ipv4Addr>(), bits.parse::<u32>()) {
            if let Ok(t) = target.parse::<std::net::Ipv4Addr>() {
                return same_prefix(u32::from(ip), u32::from(t), b);
            }
            return false; // 域名不做解析
        }
    }
    false
}

fn continue_check_no_port(_target: &str, _host: &str) {}

fn same_prefix(a: u32, b: u32, bits: u32) -> bool {
    if bits == 0 {
        return true;
    }
    if bits > 32 {
        return false;
    }
    let mask = u32::MAX << (32 - bits);
    a & mask == b & mask
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bypass_matches_without_dns() {
        let p = ProxyProfile {
            bypass: vec![
                "10.0.0.0/8".into(),
                "*.corp.internal".into(),
                "webdav.local".into(),
            ],
            ..ProxyProfile::default()
        };
        assert!(p.is_bypassed("10.1.2.3"));
        assert!(!p.is_bypassed("11.1.2.3"));
        assert!(p.is_bypassed("dav.corp.internal"));
        assert!(p.is_bypassed("CORP.INTERNAL"));
        assert!(p.is_bypassed("webdav.local"));
        assert!(!p.is_bypassed("evil-corp.internal.example"));
    }

    #[test]
    fn bypass_port_and_prefix_forms() {
        let p = ProxyProfile {
            bypass: vec!["192.168.1.20:5005".into(), "/_control".into()],
            ..Default::default()
        };
        assert!(p.is_bypassed("192.168.1.20:5005"));
        assert!(p.is_bypassed("192.168.1.20"));
        assert!(p.is_bypassed("/_control/inspect"));
        assert!(!p.is_bypassed("/.notes/manifest/index.json"));
    }

    #[test]
    fn proxy_url_carries_credentials_and_scheme() {
        let mut p = ProxyProfile::socks5("proxy.home", 1080);
        p.username = Some("me".into());
        p.password = Some("p@ss:word".into());
        p.resolve_remote_dns = true;
        assert_eq!(
            p.proxy_url().unwrap(),
            "socks5h://me:p%40ss%3Aword@proxy.home:1080"
        );
        p.resolve_remote_dns = false;
        assert!(p.proxy_url().unwrap().starts_with("socks5://"));
        let h = ProxyProfile::http("10.0.0.1", 3128);
        assert_eq!(h.proxy_url().unwrap(), "http://10.0.0.1:3128");
        assert_eq!(ProxyProfile::direct().proxy_url(), None);
        assert_eq!(
            ProxyProfile {
                mode: ProxyMode::System,
                ..Default::default()
            }
            .proxy_url(),
            None
        );
    }

    #[test]
    fn debug_never_leaks_the_password() {
        let mut p = ProxyProfile::http("10.0.0.1", 3128);
        p.username = Some("user1".into());
        p.password = Some("s3cr3t-value".into());
        let d = format!("{p:?}");
        assert!(!d.contains("s3cr3t-value"), "Debug 泄露密码: {d}");
        assert!(!d.contains("%33"), "URL 编码形式也不许出现: {d}");
        assert!(d.contains("<redacted>"), "{d}");
        assert!(!p.describe().contains("s3cr3t-value"));
    }

    #[test]
    fn validate_rejects_broken_config() {
        assert!(ProxyProfile::direct().validate().is_ok());
        assert!(ProxyProfile {
            mode: ProxyMode::Http,
            host: None,
            port: Some(3128),
            ..Default::default()
        }
        .validate()
        .is_err());
        assert!(ProxyProfile {
            mode: ProxyMode::Http,
            host: Some("a b".into()),
            port: Some(1),
            ..Default::default()
        }
        .validate()
        .is_err());
        assert!(ProxyProfile {
            mode: ProxyMode::Http,
            host: Some("x".into()),
            port: Some(0),
            ..Default::default()
        }
        .validate()
        .is_err());
    }
}
