//! notera-webdav —— `notera_sync::RemotePort` 的生产适配器（WebDAV）。
//!
//! 规范：docs/SYNC-PROTOCOL.md（§1 布局 / §3 信封 / §4 清单 / §5 写入策略 / §12 错误）。
//! 出口：docs/PROXY.md §1 —— 本 crate **不**依赖任何 HTTP 客户端，全部请求经
//! [`notera_net::HttpClient`] 出门；代理、TLS、分层超时与审计因此天然覆盖每一条请求。
//!
//! 分工：
//! * [`path`] —— §1 的远端布局与目录穿越闸门（唯一允许拼路径的地方）；
//! * [`caps`] —— §5 的 `cap_mask` 与写入策略选择（探测本身归 host/配置层）；
//! * [`client`] —— [`WebDavRemote`]，六个端口方法的全部行为；
//! * [`error`] —— §12 的状态码 → 端口词汇换算（薄包 `NetError::classify_status`）；
//! * [`auth`] —— Basic 凭据与它的脱敏边界。
//!
//! 明确**不**在本 crate 的事：清单压实决策、`missing_remote` 分类、退避节奏、
//! `protocol.json` 协商写入、附件队列、租约 —— 那些是引擎与 host 的判断，
//! 本适配器只负责"把一次远端读写做成真的，或者如实失败"。

mod auth;
mod caps;
mod client;
mod error;
mod lease;
mod path;
mod probe;
mod record;

pub use auth::Credentials;
pub use caps::{Caps, WriteStrategy};
pub use client::{WebDavConfig, WebDavRemote, MAX_RECORD_BYTES};
pub use error::{map_net, map_status, PathError, WebDavError};
pub use lease::LockDoc;
pub use path::{kind_dir, kind_matches, RecordPath, RemotePath, DEFAULT_ROOT_PREFIX};
pub use probe::ProbeReport;
pub use record::WireMeta;

/// 本 crate 实现的端口（re-export 只为让集成侧一行看全）。
/// 只 re-export **端口**：`SyncEngine` 是 sync 的东西，从传输适配器这里冒出去
/// 等于给集成侧第二个"引擎入口"，而 ARCHITECTURE-MAP §1 规定这条边只有契约。
pub use notera_sync::RemotePort;
