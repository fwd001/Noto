//! HTTP Basic 凭据。
//!
//! 头由本模块自己拼、自己塞进 [`notera_net::RequestSpec`]（不依赖任何"环境凭据"），
//! 因此每条请求走哪个身份是显式可审的。脱敏靠 `RequestSpec::redacted_headers`
//! （`Authorization` → `<redacted:basic>`），本模块另外保证 [`Credentials`] 自身的
//! `Debug`/`Display` 也不带出口令 —— 凭据一旦进过 `Debug` 就再也收不回来了（PROXY.md §3/§9）。

use base64::Engine as _;

use crate::error::WebDavError;

/// 一个 WebDAV 账户的用户名/口令。
#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    user: String,
    secret: String,
}

impl Credentials {
    /// 构造凭据。RFC 7617 规定 userid 不得含 `:`，含了就没法编码进 `user:pass` ——
    /// 这里直接拒绝，而不是偷偷抹掉一个字符（那会让用户永远登录不上且查不出原因）。
    pub fn new(user: impl Into<String>, secret: impl Into<String>) -> Result<Credentials, WebDavError> {
        let user = user.into();
        let secret = secret.into();
        if user.is_empty() {
            return Err(WebDavError::Config("WebDAV 用户名不能为空".into()));
        }
        if user.contains(':') {
            return Err(WebDavError::Config("WebDAV 用户名不得含冒号（RFC 7617）".into()));
        }
        if user.chars().any(|c| c.is_control()) || secret.chars().any(|c| c.is_control()) {
            return Err(WebDavError::Config("凭据含控制字符".into()));
        }
        Ok(Credentials { user, secret })
    }

    pub fn user(&self) -> &str {
        &self.user
    }

    /// `Authorization: Basic <base64(user:secret)>` 的头值（不含头名）。
    pub fn basic_header(&self) -> String {
        let pair = format!("{}:{}", self.user, self.secret);
        format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(pair.as_bytes()))
    }
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials").field("user", &self.user).field("secret", &notera_net::REDACTED).finish()
    }
}

impl std::fmt::Display for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "basic:{}:{}", self.user, notera_net::REDACTED)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_value_and_redaction() {
        let c = Credentials::new("u1", "p@ss").expect("合法凭据");
        assert_eq!(c.basic_header(), format!("Basic {}", base64::engine::general_purpose::STANDARD.encode("u1:p@ss")));
        let d = format!("{c:?}");
        assert!(!d.contains("p@ss"), "{d}");
        assert!(!d.contains(&base64::engine::general_purpose::STANDARD.encode("u1:p@ss")), "Debug 不得泄露 base64 形态");
        assert!(!format!("{c}").contains("p@ss"));
        let spec = notera_net::RequestSpec::new(notera_net::HttpMethod::Get, "http://127.0.0.1/x")
            .with_header("authorization", c.basic_header());
        let red = spec.redacted_headers();
        assert_eq!(red[0].1, notera_net::REDACTED_BASIC, "{red:?}");
    }

    #[test]
    fn colon_in_user_is_rejected_not_silently_stripped() {
        assert!(Credentials::new("a:b", "p").is_err());
        assert!(Credentials::new("", "p").is_err());
    }
}
