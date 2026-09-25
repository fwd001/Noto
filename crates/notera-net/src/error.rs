//! 错误分类：把 HTTP 状态码与传输层异常折叠成协议词汇（docs/SYNC-PROTOCOL.md §12）。
//!
//! 分类结果直接决定三件事：是否重试、映射到哪个 [`ErrorCode`] 文案、
//! 以及"是否可能被用户误读为网络问题"。因此 **TLS 与 DNS 必须与 Connect 分开**
//! （证书错误要让人去找运维，而不是去查网线 —— docs/PROXY.md §6）。

use notera_core::error::ErrorCode;

/// 传输/协议层错误。
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum NetError {
    /// 预算内未完成（含连接、读、总预算三种超时）。
    #[error("请求超时")]
    Timeout,
    /// TCP/代理握手失败（不含 DNS）。
    #[error("无法建立连接")]
    Connect,
    /// 域名解析失败 —— 与 `Connect` 区分，文案不同。
    #[error("无法解析服务器地址")]
    Dns,
    /// 证书校验/指纹不匹配。
    #[error("证书不受信任")]
    Tls,
    /// 401 / 407：需要凭据。
    #[error("需要凭据")]
    Auth,
    /// 403：无权限。
    #[error("无写入权限")]
    Forbidden,
    /// 404：远端确实没有（§10 missing_remote 的前提是能与之区分）。
    #[error("远端不存在")]
    NotFound,
    /// 409 / 412：并发前置条件失败，引擎重算 plan。
    #[error("前置条件失败")]
    Precondition,
    /// 507 / 509：空间不足。
    #[error("存储空间不足")]
    Quota,
    /// 5xx（不含已被单列的 507）。
    #[error("服务器不可用")]
    Server,
    /// 405 / 501：服务器不支持所需语义（触发写入策略降级）。
    #[error("服务器不支持该操作")]
    Unsupported,
    /// 跨源重定向且本请求带凭据：**拒绝跟随**。
    #[error("跨源重定向不得携带凭据")]
    RedirectCrossOrigin,
    /// 调用方取消（切后台/退出/新一轮）。不计失败。
    #[error("已取消")]
    Cancelled,
    /// 报文/协议层异常（含响应体校验失败）。
    #[error("协议异常: {0}")]
    Protocol(String),
}

impl NetError {
    /// 状态码 + 可选传输层错误的统一分类入口。
    ///
    /// `status == 0` 表示"没有响应"，此时只看 `err`；两者都缺省按 `Connect` 处理
    /// （宁可判成不可达，也不要当成"远端没有数据"）。
    pub fn classify(status: u16, err: Option<&reqwest::Error>) -> NetError {
        if let Some(e) = err {
            // 传输层优先：状态码此时通常不可信。
            if e.is_timeout() {
                return NetError::Timeout;
            }
            if e.is_dns() {
                return NetError::Dns;
            }
            if e.is_connect() {
                return NetError::Connect;
            }
            if e.is_redirect() {
                return NetError::RedirectCrossOrigin;
            }
            if e.is_body() || e.is_decode() || e.is_request() || e.is_builder() {
                return NetError::Protocol(e.to_string());
            }
            if let Some(st) = e.status() {
                return NetError::classify_status(st.as_u16());
            }
            return NetError::Connect;
        }
        NetError::classify_status(status)
    }

    /// 纯状态码分类（docs/SYNC-PROTOCOL.md §12 的表）。
    pub fn classify_status(status: u16) -> NetError {
        match status {
            0 => NetError::Connect,
            401 | 407 => NetError::Auth,
            403 => NetError::Forbidden,
            404 => NetError::NotFound,
            405 | 501 => NetError::Unsupported,
            408 => NetError::Timeout,
            // 412 = 条件写失败；409 = 锁/冲突；423 = Locked（退避重试）
            409 | 412 | 423 | 428 => NetError::Precondition,
            507 | 509 => NetError::Quota,
            301 | 302 | 303 | 307 | 308 => NetError::RedirectCrossOrigin,
            500..=599 => NetError::Server,
            s if s >= 400 && s < 500 => NetError::Protocol(format!("未预期客户端侧状态 {s}")),
            // 2xx/1xx 不是错误；调用方拿它当"成功"来用。
            s => NetError::Protocol(format!("{s} 不是错误状态")),
        }
    }

    /// 映射到用户词表（`ErrorCode`）—— UI 只认后者。
    pub fn error_code(&self) -> ErrorCode {
        match self {
            NetError::Timeout | NetError::Connect | NetError::Dns | NetError::Cancelled => {
                ErrorCode::Offline
            }
            NetError::Auth => ErrorCode::AuthRequired,
            NetError::Forbidden => ErrorCode::Forbidden,
            NetError::NotFound => ErrorCode::Offline,
            NetError::Precondition => ErrorCode::Precondition,
            NetError::Quota => ErrorCode::QuotaFull,
            NetError::Server => ErrorCode::ServerUnavailable,
            NetError::Unsupported => ErrorCode::Unsupported,
            NetError::RedirectCrossOrigin => ErrorCode::ServerUnavailable,
            // 校验失败 = 脏数据，绝不写库（I6）
            NetError::Protocol(_) => ErrorCode::CorruptRecord,
            NetError::Tls => ErrorCode::CertUntrusted,
        }
    }

    /// 是否值得自动重试。`Tls`/`Auth`/`Forbidden` 明确**不**重试 ——
    /// 证书错误降级重试是安全事故（PROXY.md §8）。
    pub fn retryable(&self) -> bool {
        matches!(
            self,
            NetError::Timeout
                | NetError::Connect
                | NetError::Dns
                | NetError::Server
                | NetError::Precondition
                | NetError::Quota
        )
    }

    /// 诊断细节（进审计日志，不进 UI）。
    pub fn detail(&self) -> &'static str {
        match self {
            NetError::Timeout => "预算内未完成",
            NetError::Connect => "TCP/代理握手失败",
            NetError::Dns => "域名解析失败",
            NetError::Tls => "证书链或指纹校验失败",
            NetError::Auth => "401/407 需要凭据",
            NetError::Forbidden => "403 权限不足",
            NetError::NotFound => "404 远端缺失",
            NetError::Precondition => "409/412/423 前置条件失败",
            NetError::Quota => "507/509 空间不足",
            NetError::Server => "5xx 服务端异常",
            NetError::Unsupported => "405/501 语义不支持",
            NetError::RedirectCrossOrigin => "跨源重定向",
            NetError::Cancelled => "本轮被取消",
            NetError::Protocol(_) => "报文/校验异常",
        }
    }
}

/// 已取消的哨兵：`tokio::select!` 里被外部 token 打断时返回。
pub struct Cancelled;
