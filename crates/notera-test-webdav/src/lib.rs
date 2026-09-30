//! # notera-test-webdav
//!
//! 真实 TCP 的 HTTP/1.1 + WebDAV 子集服务器，带**确定性**故障注入。
//!
//! 铁律（docs/TEST-PLAN.md §0.7）：HTTP/WebDAV 层禁止 mock。本 crate 就是那条铁律的
//! 载体 —— 它自己解析请求行/头部/body，真起 `TcpListener`，真写 socket。
//! 协议 bug 只能被真实实现抓到。
//!
//! 依赖隔离（docs/ARCHITECTURE-MAP.md §2）：本 crate **不**依赖 `notera-net`
//! 也不依赖 `reqwest`；它自己实现 DAV 子集。产品 crate 不得依赖本 crate。
//!
//! ## 控制面
//!
//! * `POST /_control/reset` —— `RESET`
//! * `POST /_control/inject` —— 安装 [`Injection`]（`FAIL(...)` / `OFF` 的注入记法）
//! * `POST /_control/stop` / `/_control/start` —— `OFF(close-listener)` / `ON`
//! * `GET  /_control/inspect` —— `STATS`：请求序列（method/path/status/bytes）
//! * `GET  /_fs/dump?prefix=...` —— `DUMP`：服务端权威快照（测试唯一允许的
//!   服务端状态断言手段）
//!
//! 控制面路径**永不**受注入影响 —— 否则服务器挂掉后就再也关不掉了。
//!
//! ## 确定性
//!
//! 同一 [`Injection`] 配置 + 同一请求序列 ⇒ 同一 `request_log()`（除 `ts_ms`）与
//! 同一状态。所有注入决策都基于**已服务请求计数**（`seq`）与路径匹配，
//! 不使用随机数、不使用墙上时钟。

mod control;
mod handler;
mod http;
mod http_proxy;
mod inject;
mod server;
mod socks5;
mod state;
mod xml;

pub use http_proxy::HttpForwardProxy;
pub use inject::{Injection, LoggedRequest};
pub use server::{Backend, Started, TestServer};
pub use socks5::Socks5Forwarder;

/// 测试里唯一允许的服务端断言手段的返回类型别名（便于调用方写文档）。
pub use state::DumpEntry;

/// 本 crate 的语义版本，仅用于日志/调试。
pub const TEST_WEBDAV_BUILD: &str = env!("CARGO_PKG_VERSION");
