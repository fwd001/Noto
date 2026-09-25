//! 唯一 HTTP 出口（docs/PROXY.md §1 铁律）。
//!
//! ```text
//! 所有 HTTP 请求  ──►  notera-net::HttpClient  ──►  reqwest  ──►  socket
//!                      （代理解析 / TLS / 超时 / 重试 / 审计）
//! ```
//!
//! 本 crate 是全系统**唯一**允许直接驱动 socket 的 HTTP 客户端。强制手段：
//!
//! 1. 只暴露 [`HttpClient::send`] / [`HttpClient::send_with_retry`]，不 `pub` 底层客户端类型；
//! 2. `notera-webdav` 的依赖声明里只有 `notera-net`，没有 `reqwest`；
//! 3. CI 架构测试全仓 grep `reqwest::Client`（见 docs/CI-CD.md）。
//!
//! [`ProxyProfile`] 与 [`TlsPolicy`] 定义在本 crate 内部（契约如此）：
//! `notera-config` 只负责持久化，不参与传输决策，避免"配置层反过来塑造出口"。

mod client;
mod error;
mod proxy;
mod retry;
mod tls;
mod types;

pub use client::HttpClient;
pub use error::NetError;
pub use proxy::{host_matches, ProxyMode, ProxyProfile};
pub use retry::RetryPolicy;
pub use tls::TlsPolicy;
pub use types::{HttpMethod, RequestSpec, Response, RouteProof, Timeouts, REDACTED, REDACTED_BASIC};

/// 规范默认超时（PROXY.md §7）。
pub const DEFAULT_CONNECT: std::time::Duration = std::time::Duration::from_secs(8);
pub const DEFAULT_READ: std::time::Duration = std::time::Duration::from_secs(20);
pub const DEFAULT_WRITE: std::time::Duration = std::time::Duration::from_secs(30);
pub const DEFAULT_PER_REQUEST: std::time::Duration = std::time::Duration::from_secs(45);
