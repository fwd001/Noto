//! 错误词表与状态码映射（docs/SYNC-PROTOCOL.md §12）。
//!
//! 映射的**唯一**入口是 [`map_status`] / [`map_net`]，它们薄薄地包一层
//! [`notera_net::NetError::classify_status`] —— 分类只在 `notera-net` 派生一次，
//! 这里只做"传输词汇 → 端口词汇"的换算。重复推导会产生两套漂移的分类表，
//! 那正是 §12 想消灭的东西。

use notera_net::NetError;
use notera_sync::RemoteError;

/// 本适配器自身的构造/输入错误（还没到"远端"就已经不成立）。
#[derive(Debug, thiserror::Error)]
pub enum WebDavError {
    #[error("配置无效: {0}")]
    Config(String),
    #[error("路径不安全: {0}")]
    Path(#[from] PathError),
    #[error("出口构造失败: {0}")]
    Net(#[from] NetError),
}

/// 路径/名字校验失败（§1 的目录穿越闸门）。
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum PathError {
    #[error("base_url 非法: {0}")]
    BadUrl(String),
    #[error("实体类型未知: {0}")]
    BadKind(String),
    #[error("id 不是路径安全 UUIDv7/附件 sha256: {0}")]
    BadId(String),
    #[error("路径片段被拒绝: {0}")]
    BadSegment(String),
}

/// 路径错误进端口词汇：清单/调用方给了越界名字，属于协议异常而非网络异常。
impl From<PathError> for RemoteError {
    fn from(e: PathError) -> RemoteError {
        RemoteError::Protocol(e.to_string())
    }
}

/// HTTP 状态码 → [`RemoteError`]（§12 表的端口形态）。
pub fn map_status(status: u16) -> RemoteError {
    map_net(NetError::classify_status(status))
}

/// 传输层错误 → [`RemoteError`]。
///
/// 两处刻意不是一对一：
/// * `Connect`/`Dns` → `Offline`（§12："连接失败 / DNS → 离线，停止本轮并退避"）；
/// * `Timeout` → `Server`：链路是通的、服务器不回话，报"离线"会把用户支去查网线。
/// * `Tls`/`RedirectCrossOrigin` → `Protocol`：证书不受信任与跨源重定向都是"别再跟这台
///   服务器说话"的信号，必须停轮（`RemoteError::Protocol` 是端口词汇里唯一的停轮位），
///   绝不能被翻译成"远端没有这份数据"。
pub fn map_net(e: NetError) -> RemoteError {
    match e {
        NetError::Timeout | NetError::Server => RemoteError::Server,
        NetError::Connect | NetError::Dns => RemoteError::Offline,
        NetError::Auth => RemoteError::Auth,
        NetError::Forbidden => RemoteError::Forbidden,
        NetError::NotFound => RemoteError::NotFound,
        NetError::Precondition => RemoteError::Precondition,
        NetError::Quota => RemoteError::Quota,
        NetError::Cancelled => RemoteError::Cancelled,
        NetError::Unsupported => RemoteError::Protocol("服务器不支持所需语义（405/501）".into()),
        NetError::Tls => RemoteError::Protocol("证书不受信任，已停止访问该服务器".into()),
        NetError::RedirectCrossOrigin => RemoteError::Protocol("跨源重定向被拒绝（不携带凭据跟随）".into()),
        NetError::Protocol(msg) => RemoteError::Protocol(msg),
    }
}

impl WebDavError {
    /// 端口词汇形态（供 `Result<_, RemoteError>` 边界使用）。
    pub fn to_remote(&self) -> RemoteError {
        match self {
            WebDavError::Config(m) | WebDavError::Net(NetError::Protocol(m)) => RemoteError::Protocol(m.clone()),
            WebDavError::Path(p) => RemoteError::Protocol(p.to_string()),
            WebDavError::Net(e) => map_net(e.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_taxonomy_matches_protocol_section_12() {
        assert_eq!(map_status(401), RemoteError::Auth);
        assert_eq!(map_status(407), RemoteError::Auth);
        assert_eq!(map_status(403), RemoteError::Forbidden);
        assert_eq!(map_status(404), RemoteError::NotFound);
        assert_eq!(map_status(409), RemoteError::Precondition);
        assert_eq!(map_status(412), RemoteError::Precondition);
        assert_eq!(map_status(423), RemoteError::Precondition);
        assert_eq!(map_status(507), RemoteError::Quota);
        assert_eq!(map_status(500), RemoteError::Server);
        assert_eq!(map_status(503), RemoteError::Server);
        assert!(map_status(405).halts_round(), "405 语义不支持必须停轮并记诊断");
        assert!(map_net(NetError::Connect).retryable(), "离线可继续下一轮，不是终止态");
        assert!(!map_net(NetError::Connect).halts_round());
        assert!(map_net(NetError::Auth).halts_round(), "401 必须停轮");
        assert!(!map_net(NetError::Tls).retryable(), "证书错误不得降级重试");
    }
}
