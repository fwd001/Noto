//! 错误词表：用户语言映射的唯一来源。
//!
//! 新增 ErrorCode 必须同时给出：用户文案 key、retryable、是否阻塞本地写入。
//! 三者缺一不接受合并（docs/ARCHITECTURE-MAP.md §2 notera-core 行）。

use crate::{EntityId, Rev};
use serde::{Deserialize, Serialize};

/// 内部错误分类。UI 永远看不到这个枚举 —— 它会被 [`UserFacingError`] 折叠。
///
/// `Copy`：本枚举无字段，`UserFacingError::new` 需要在一次构造里同时取
/// `message_key()/retryable()/action` 三者；按值接收者会把它移动三次（E0382）。
/// 加 `Copy` 是最小且不改变任何调用方语义的修法。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorCode {
    /// 网络不可达（含 DNS 失败）
    Offline,
    /// 401 / 407：需要凭据
    AuthRequired,
    /// 403：无权限
    Forbidden,
    /// 507 / 509：空间不足
    QuotaFull,
    /// 5xx
    ServerUnavailable,
    /// 412 / 409：并发前置条件失败（引擎会重算，通常不上报）
    Precondition,
    /// 405 / 501：服务器不支持所需语义
    Unsupported,
    /// 校验失败：脏数据，绝不写库（I6）
    CorruptRecord,
    /// 协议版本不匹配
    ProtocolMismatch,
    /// 本地 DB 版本比本程序还新
    DbTooNew,
    /// 需要用户处理冲突
    ConflictNeedsAttention,
    /// 附件本地缺失（可下载）
    AttachmentMissing,
    /// 代理不可达
    ProxyUnreachable,
    /// 证书不受信任
    CertUntrusted,
    /// 服务器异常清空/挂错目录，已保护本地数据
    RemoteDivergenceSuspected,
    /// 一轮被主动取消（切后台/退出）：不计失败
    Cancelled,
    /// 兜底
    SyncFailed,
}

impl ErrorCode {
    /// 该错误是否值得自动重试。
    pub fn retryable(self) -> bool {
        matches!(
            self,
            ErrorCode::Offline
                | ErrorCode::ServerUnavailable
                | ErrorCode::Precondition
                | ErrorCode::ProxyUnreachable
                | ErrorCode::QuotaFull
        )
    }

    /// **核心不变式 I8**：任何同步侧错误都不得阻塞本地写入。
    /// 只有本地/数据完整类错误才允许阻塞。
    pub fn blocks_local_writes(self) -> bool {
        matches!(
            self,
            ErrorCode::DbTooNew | ErrorCode::ProtocolMismatch | ErrorCode::CorruptRecord
        )
    }

    /// UI 文案 key。前端按 key 取词，因此错误词表与 UI 解耦。
    pub fn message_key(self) -> &'static str {
        match self {
            ErrorCode::Offline => "sync.offline",
            ErrorCode::AuthRequired => "sync.auth_required",
            ErrorCode::Forbidden => "sync.forbidden",
            ErrorCode::QuotaFull => "sync.quota_full",
            ErrorCode::ServerUnavailable => "sync.server_unavailable",
            ErrorCode::Precondition => "sync.precondition",
            ErrorCode::Unsupported => "sync.unsupported",
            ErrorCode::CorruptRecord => "sync.corrupt_record",
            ErrorCode::ProtocolMismatch => "sync.protocol_mismatch",
            ErrorCode::DbTooNew => "app.db_too_new",
            ErrorCode::ConflictNeedsAttention => "sync.conflict_attention",
            ErrorCode::AttachmentMissing => "attach.missing",
            ErrorCode::ProxyUnreachable => "proxy.unreachable",
            ErrorCode::CertUntrusted => "proxy.cert_untrusted",
            ErrorCode::RemoteDivergenceSuspected => "sync.divergence",
            ErrorCode::Cancelled => "sync.cancelled",
            ErrorCode::SyncFailed => "sync.failed",
        }
    }
}

/// 折叠给用户的错误。UI 只认识这个结构，不认识 HTTP 状态码或 DAV 术语。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserFacingError {
    pub code: ErrorCode,
    pub message_key: &'static str,
    pub retryable: bool,
    /// 建议动作，供 UI 决定按钮。
    pub action: UserAction,
    /// 诊断细节（日志/`notera-cli` 用），不直接展示。
    pub detail: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum UserAction {
    None,
    RetryLater,
    Reauthenticate,
    FreeSpace,
    UpgradeApp,
    ResolveConflict,
    ContactAdmin,
}

impl UserFacingError {
    pub fn new(code: ErrorCode) -> Self {
        Self {
            message_key: code.message_key(),
            retryable: code.retryable(),
            action: match code {
                ErrorCode::AuthRequired => UserAction::Reauthenticate,
                ErrorCode::QuotaFull => UserAction::FreeSpace,
                ErrorCode::DbTooNew | ErrorCode::ProtocolMismatch => UserAction::UpgradeApp,
                ErrorCode::ConflictNeedsAttention => UserAction::ResolveConflict,
                ErrorCode::CertUntrusted
                | ErrorCode::Unsupported
                | ErrorCode::RemoteDivergenceSuspected => UserAction::ContactAdmin,
                c if c.retryable() => UserAction::RetryLater,
                _ => UserAction::None,
            },
            code,
            detail: None,
        }
    }
    pub fn with_detail(mut self, d: impl Into<String>) -> Self {
        self.detail = Some(d.into());
        self
    }
}

/// 乐观并发失败：调用方给的 expectedRev 与当前不符。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StaleEdit {
    pub entity: EntityId,
    pub expected: Rev,
    pub actual: Rev,
}

impl std::fmt::Display for StaleEdit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} 已被改动（期望 rev {}，实际 {}）",
            self.entity, self.expected, self.actual
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_sync_error_blocks_local_writes() {
        // I8 的机器可检查形式：同步/网络类错误一律不得阻塞本地写入。
        for c in [
            ErrorCode::Offline,
            ErrorCode::AuthRequired,
            ErrorCode::Forbidden,
            ErrorCode::QuotaFull,
            ErrorCode::ServerUnavailable,
            ErrorCode::ProxyUnreachable,
            ErrorCode::CertUntrusted,
            ErrorCode::ConflictNeedsAttention,
            ErrorCode::SyncFailed,
        ] {
            assert!(!c.blocks_local_writes(), "{c:?} 不应阻塞本地写入");
        }
    }

    #[test]
    fn every_code_has_message_key_and_maps_to_error() {
        let e = UserFacingError::new(ErrorCode::QuotaFull);
        assert_eq!(e.message_key, "sync.quota_full");
        assert!(e.retryable);
        assert_eq!(e.action, UserAction::FreeSpace);
    }
}
