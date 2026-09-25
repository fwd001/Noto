//! 出口的类型词汇：方法、请求、响应、路由证据、超时分层。

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::NetError;
use crate::proxy::ProxyMode;

/// 需要发出去的动词集合。
///
/// 自定义动词（`PROPFIND`/`MOVE`…）经 `http::Method::from_bytes` 构造 ——
/// `Method::from_static` 在 http 1.5 不是公开常量入口，`from_bytes` 才稳
/// （实测 PROPFIND 207 / MOVE 201 / If-Match 412 均可通）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HttpMethod {
    Get,
    Head,
    Put,
    Delete,
    Propfind,
    Move,
    Mkcol,
    Options,
    Copy,
    Patch,
    Post,
}

impl HttpMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            HttpMethod::Get => "GET",
            HttpMethod::Head => "HEAD",
            HttpMethod::Put => "PUT",
            HttpMethod::Delete => "DELETE",
            HttpMethod::Propfind => "PROPFIND",
            HttpMethod::Move => "MOVE",
            HttpMethod::Mkcol => "MKCOL",
            HttpMethod::Options => "OPTIONS",
            HttpMethod::Copy => "COPY",
            HttpMethod::Patch => "PATCH",
            HttpMethod::Post => "POST",
        }
    }

    pub fn to_reqwest(self) -> Result<reqwest::Method, NetError> {
        reqwest::Method::from_bytes(self.as_str().as_bytes())
            .map_err(|e| NetError::Protocol(format!("方法名非法 {:?}: {e}", self.as_str())))
    }

    /// 协议意义上的幂等（重试只作用于这些 + 显式标记 `idempotent` 的写）。
    pub fn is_inherently_idempotent(self) -> bool {
        matches!(
            self,
            HttpMethod::Get
                | HttpMethod::Head
                | HttpMethod::Options
                | HttpMethod::Propfind
                | HttpMethod::Put
                | HttpMethod::Delete
                | HttpMethod::Mkcol
        )
    }
}

impl std::fmt::Display for HttpMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 分层超时（PROXY.md §7 规范默认值）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timeouts {
    /// 建连（含代理握手）。
    pub connect: Duration,
    /// 单次读。
    pub read: Duration,
    /// 单次写（附件上传单块）。
    pub write: Duration,
    /// 一个请求的总预算。
    pub per_request: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Timeouts {
            connect: Duration::from_secs(8),
            read: Duration::from_secs(20),
            write: Duration::from_secs(30),
            per_request: Duration::from_secs(45),
        }
    }
}

impl Timeouts {
    /// 测试用：把全部预算压到同一个短值。
    pub fn short(budget: Duration) -> Timeouts {
        Timeouts {
            connect: budget,
            read: budget,
            write: budget,
            per_request: budget,
        }
    }
}

/// 一次请求的完整描述。
#[derive(Clone, PartialEq, Eq)]
pub struct RequestSpec {
    pub method: HttpMethod,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    pub if_match: Option<String>,
    pub if_none_match: Option<String>,
    /// 内容哈希复验可判定重放的写也标 true（SYNC-PROTOCOL §11.1）。
    pub idempotent: bool,
    /// 本次请求的预算覆盖（<= `Timeouts::per_request`）。
    pub timeout: Option<Duration>,
}

impl RequestSpec {
    pub fn new(method: HttpMethod, url: impl Into<String>) -> RequestSpec {
        RequestSpec {
            method,
            url: url.into(),
            headers: Vec::new(),
            body: None,
            if_match: None,
            if_none_match: None,
            idempotent: method.is_inherently_idempotent(),
            timeout: None,
        }
    }

    pub fn with_header(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.headers.push((k.into(), v.into()));
        self
    }
    pub fn with_body(mut self, b: Vec<u8>) -> Self {
        self.body = Some(b);
        self
    }
    pub fn with_if_match(mut self, etag: impl Into<String>) -> Self {
        self.if_match = Some(etag.into());
        self
    }
    pub fn with_if_none_match(mut self, etag: impl Into<String>) -> Self {
        self.if_none_match = Some(etag.into());
        self
    }

    /// 是否携带凭据（决定跨源重定向能否跟随）。
    pub fn carries_credentials(&self) -> bool {
        let header_hit = self.headers.iter().any(|(k, _)| {
            let k = k.to_ascii_lowercase();
            k == "authorization" || k == "proxy-authorization" || k == "cookie"
        });
        header_hit || url_has_userinfo(&self.url)
    }

    /// **脱敏**后的头部副本：`Authorization`/`Proxy-Authorization` 值换成 `<redacted:basic>`。
    pub fn redacted_headers(&self) -> Vec<(String, String)> {
        self.headers
            .iter()
            .map(|(k, v)| {
                let lk = k.to_ascii_lowercase();
                if lk == "authorization" || lk == "proxy-authorization" || lk == "cookie" {
                    let scheme = v
                        .split_whitespace()
                        .next()
                        .map(|s| s.to_ascii_lowercase())
                        .unwrap_or_default();
                    let tag = if scheme == "basic" {
                        REDACTED_BASIC
                    } else {
                        REDACTED
                    };
                    (k.clone(), tag.into())
                } else {
                    (k.clone(), v.clone())
                }
            })
            .collect()
    }

    /// 脱敏 URL（去掉 `user:pass@`）。
    pub fn redacted_url(&self) -> String {
        strip_userinfo(&self.url)
    }
}

/// 手工 `Debug`：任何日志/panic 输出都不得带出凭据。
impl std::fmt::Debug for RequestSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RequestSpec")
            .field("method", &self.method)
            .field("url", &self.redacted_url())
            .field("headers", &self.redacted_headers())
            .field("body_bytes", &self.body.as_ref().map(|b| b.len()))
            .field("if_match", &self.if_match)
            .field("if_none_match", &self.if_none_match)
            .field("idempotent", &self.idempotent)
            .field("timeout", &self.timeout)
            .finish()
    }
}

/// 凭据脱敏标记。
pub const REDACTED: &str = "<redacted>";
pub const REDACTED_BASIC: &str = "<redacted:basic>";

pub(crate) fn url_has_userinfo(url: &str) -> bool {
    extract_userinfo(url).is_some()
}

pub(crate) fn extract_userinfo(url: &str) -> Option<(String, Option<String>)> {
    let after = url.find("//")?;
    let rest = &url[after + 2..];
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let at = authority.rfind('@')?;
    let userinfo = &authority[..at];
    let (u, p) = match userinfo.split_once(':') {
        Some((u, p)) => (u.to_string(), Some(p.to_string())),
        None => (userinfo.to_string(), None),
    };
    Some((u, p))
}

pub(crate) fn strip_userinfo(url: &str) -> String {
    let Some(after) = url.find("//") else {
        return url.to_string();
    };
    let (scheme, rest) = url.split_at(after + 2);
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(authority_end);
    match authority.rfind('@') {
        Some(at) => format!("{}{}{}", scheme, &authority[at + 1..], tail),
        None => url.to_string(),
    }
}

/// 实际出口证据（PROXY.md §9 证据链③）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteProof {
    /// 建客户端时配置的出口模式。
    pub mode: ProxyMode,
    /// 实际出口端点：代理模式是代理 `host:port`；直连是目标 `host:port`。
    pub endpoint: String,
    /// 命中 bypass 名单（字符串匹配判定，**不**依赖 DNS）。
    pub bypassed: bool,
    /// 人类可读细节（含 TLS 档位与是否真的挂了代理）。
    pub detail: String,
}

impl RouteProof {
    pub fn one_line(&self) -> String {
        format!(
            "mode={} endpoint={} bypassed={} {}",
            self.mode, self.endpoint, self.bypassed, self.detail
        )
    }
}

/// 响应。
#[derive(Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub etag: Option<String>,
    pub elapsed: Duration,
    pub route: RouteProof,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        let n = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(k, _)| k.to_ascii_lowercase() == n)
            .map(|(_, v)| v.as_str())
    }
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
    pub fn body_len(&self) -> usize {
        self.body.len()
    }
    pub fn body_string(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

impl std::fmt::Debug for Response {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Response")
            .field("status", &self.status)
            .field("headers", &"<见 redacted_headers>")
            .field("body_len", &self.body.len())
            .field("etag", &self.etag)
            .field("elapsed", &self.elapsed)
            .field("route", &self.route)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_verbs_map_to_http_methods() {
        assert_eq!(HttpMethod::Propfind.to_reqwest().unwrap().as_str(), "PROPFIND");
        assert_eq!(HttpMethod::Move.to_reqwest().unwrap().as_str(), "MOVE");
        assert!(HttpMethod::Propfind.is_inherently_idempotent());
        assert!(!HttpMethod::Post.is_inherently_idempotent());
    }

    #[test]
    fn userinfo_is_stripped_everywhere() {
        let spec = RequestSpec::new(
            HttpMethod::Get,
            "http://secret-user:secret-pass@webdav.local:5005/.notes",
        )
        .with_header("authorization", "Basic U2VjcmV0OtKu")
        .with_if_match("\"abc\"");
        let d = format!("{spec:?}");
        assert!(!d.contains("secret-pass"), "{d}");
        assert!(!d.contains("secret-user@"), "{d}");
        assert!(!d.contains("U2VjcmV0"), "{d}");
        assert!(d.contains("<redacted:basic>"), "{d}");
        assert_eq!(
            spec.redacted_url(),
            "http://webdav.local:5005/.notes",
            "URL userinfo 必须去除"
        );
        assert!(spec.carries_credentials());
    }

    #[test]
    fn no_credentials_when_plain() {
        let spec = RequestSpec::new(HttpMethod::Get, "http://127.0.0.1:1/x");
        assert!(!spec.carries_credentials());
        assert_eq!(spec.redacted_url(), "http://127.0.0.1:1/x");
    }
}
