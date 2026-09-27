//! TLS 四档策略（docs/PROXY.md §6）。
//!
//! | 档 | 含义 | 允许场景 |
//! |---|---|---|
//! | `Strict` | 系统信任库校验（默认） | 公网正规证书 |
//! | `CaBundle` | 追加指定 CA（PEM） | **内网自签主路径** |
//! | `Pin` | 叶证书 sha256 指纹白名单 | 无可用 CA、又要求强校验 |
//! | `InsecureLocal` | 跳过校验 | **仅** loopback |
//!
//! 实现取向（诚实记录，避免"看起来做到了其实没有"）：
//!
//! * `Strict`：reqwest 的 `rustls` feature 已启用 `rustls-platform-verifier`
//!   （见 `reqwest-0.13.5/Cargo.toml`：`rustls = ["dep:rustls-platform-verifier", ...]`），
//!   因此默认就是**系统信任库**，企业把内网根 CA 装进系统后自动可信。
//!   若将来换掉该 feature，这里必须显式接入 verifier，否则要退回 webpki-roots 并注释说明。
//! * `Pin`：证书链校验交由 rustls，**握手之后再比对叶证书 DER 的 sha256**
//!   （`ClientBuilder::tls_info(true)` + `Response::extensions()` 里的
//!   `reqwest::tls::TlsInfo::peer_certificate()`）。不匹配即 `NetError::Tls`，
//!   响应作废、不交给上层。`RouteProof.detail` 会写明 `pin=checked(sha256:…)`。
//!   这是"连接后比对"而非"握手中拒绝"：中间人无法伪造指纹，但握手阶段本身不做链校验时
//!   仍可能被降级 —— 因此 `Pin` 档**不**关闭链校验，只在其之上叠加指纹白名单。
//! * `InsecureLocal`：只允许 loopback 主机；其它主机直接拒绝建客户端。

/// 证书/传输安全策略。
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum TlsPolicy {
    /// 系统信任库严格校验（默认）。
    #[default]
    Strict,
    /// 在系统信任库之上**追加** PEM 根证书（内网自签根）。
    CaBundle(String),
    /// 叶证书 DER 的 sha256 指纹白名单（64 hex，可带 `sha256:` 前缀）。
    Pin(Vec<String>),
    /// 跳过校验 —— 仅 loopback。
    InsecureLocal,
}

impl TlsPolicy {
    pub fn name(&self) -> &'static str {
        match self {
            TlsPolicy::Strict => "strict",
            TlsPolicy::CaBundle(_) => "ca_bundle",
            TlsPolicy::Pin(_) => "pin",
            TlsPolicy::InsecureLocal => "insecure_local",
        }
    }

    /// 是否跳过链校验（只允许 `InsecureLocal`）。
    pub fn skip_verify(&self) -> bool {
        matches!(self, TlsPolicy::InsecureLocal)
    }

    pub(crate) fn pins(&self) -> &[String] {
        match self {
            TlsPolicy::Pin(v) => v,
            _ => &[],
        }
    }

    /// 规范化指纹：小写纯 hex，容忍 `sha256:` 前缀与 `:`/`-`/空格 分隔写法。
    pub fn normalize_pin(s: &str) -> String {
        let t = s.trim().to_ascii_lowercase();
        let t = t.strip_prefix("sha256:").unwrap_or(&t);
        t.chars()
            .filter(|c| !matches!(c, ':' | '-' | ' '))
            .collect()
    }

    /// 校验目标主机在本档下是否被允许。
    ///
    /// `InsecureLocal` 只放行 loopback；其余档允许任何主机（明文 HTTP 另有闸门，
    /// 见 [`crate::HttpClient`] 的 `allow_plaintext`）。
    pub fn permits_host(&self, host: &str) -> bool {
        match self {
            TlsPolicy::InsecureLocal => is_loopback(host),
            _ => true,
        }
    }
}

/// loopback 判定：纯字符串，不做解析。
pub fn is_loopback(host: &str) -> bool {
    let h = host.trim().trim_start_matches('[').trim_end_matches(']');
    if h.eq_ignore_ascii_case("localhost") {
        return true;
    }
    match h.parse::<std::net::IpAddr>() {
        Ok(ip) => ip.is_loopback(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insecure_local_only_admits_loopback() {
        assert!(TlsPolicy::InsecureLocal.permits_host("localhost"));
        assert!(TlsPolicy::InsecureLocal.permits_host("127.0.0.1"));
        assert!(TlsPolicy::InsecureLocal.permits_host("::1"));
        assert!(!TlsPolicy::InsecureLocal.permits_host("10.0.0.9"));
        assert!(!TlsPolicy::InsecureLocal.permits_host("dav.corp.internal"));
        assert!(TlsPolicy::Strict.permits_host("dav.corp.internal"));
    }

    #[test]
    fn pin_normalization() {
        assert_eq!(TlsPolicy::normalize_pin("SHA256:AA:BB:CC"), "aabbcc");
        assert_eq!(TlsPolicy::normalize_pin(" aa bb "), "aabb");
        assert_eq!(TlsPolicy::normalize_pin("aa-bb-cc"), "aabbcc");
    }
}
