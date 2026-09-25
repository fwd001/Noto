//! `StoreError` —— 仓储层错误词表。
//!
//! 契约要求的变体：`StaleEdit` / `NotFound` / `Constraint` / `Sql` / `Migration` / `ReadOnly`。
//! 其余（`InvalidDoc` / `Rejected` / `Rich` / `DocTooNew` / `Io` / `Identity`）是**追加**变体，
//! 用途是把 I6 闸门（脏数据拒绝）与本地 IO 分开，便于上层按类别映射 `ErrorCode`。

use notera_core::{DeviceId, EntityId, EntityKind, Rev};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error(transparent)]
    Sql(#[from] rusqlite::Error),

    #[error("迁移失败 v{from} → v{to}: {detail}")]
    Migration { from: u32, to: u32, detail: String },

    /// 乐观并发失败（`expected_rev` 与当前 `rev` 不符）。
    ///
    /// 注意：`notera_core::error::StaleEdit` 只是 Display 结构体、不是 `std::error::Error`，
    /// 所以这里不用 `#[from]`（它会要求 source 关系），改由下方手写 `From`。
    #[error("{0}")]
    StaleEdit(notera_core::error::StaleEdit),

    #[error("实体不存在: {kind:?} {id}")]
    NotFound { kind: EntityKind, id: EntityId },

    #[error("约束不满足: {0}")]
    Constraint(String),

    /// DATA-MODEL §2 / ADR-0012：库版本高于本程序支持值时**只读**，绝不降级写回。
    #[error("数据库 schema 版本 {db} 高于本程序支持的 {supported}: 已进入只读模式，不执行迁移、不写回")]
    ReadOnly { db: u32, supported: u32 },

    /// 需要只读但原因不是版本（例如 `doc.v` 超前）。
    #[error("文档格式 v{doc} 高于本程序支持的 {supported}: 该笔记只读，禁止写回（I7）")]
    DocTooNew { doc: u16, supported: u16 },

    #[error("文档校验失败（I6，未写入任何数据）: {0}")]
    InvalidDoc(String),

    #[error("远端记录被拒绝（I6，未写入任何数据）: {0}")]
    Rejected(String),

    #[error("富文本层拒绝: {0}")]
    Rich(String),

    #[error("附件落盘失败: {0}")]
    Io(#[from] std::io::Error),

    #[error("ID 非法: {0}")]
    Identity(#[from] notera_core::IdentityError),
}

impl StoreError {
    /// 是否属于"I6 拒绝"类：调用方可据此只计数、不视为本地库故障。
    pub fn is_rejection(&self) -> bool {
        matches!(self, StoreError::InvalidDoc(_) | StoreError::Rejected(_) | StoreError::DocTooNew { .. })
    }

    pub(crate) fn not_found(kind: EntityKind, id: EntityId) -> Self {
        StoreError::NotFound { kind, id }
    }

    pub(crate) fn stale_edit(entity: EntityId, expected: Rev, actual: Rev, _device: &DeviceId) -> Self {
        StoreError::StaleEdit(notera_core::error::StaleEdit {
            entity,
            expected,
            actual,
        })
    }
}

impl From<notera_core::error::StaleEdit> for StoreError {
    fn from(e: notera_core::error::StaleEdit) -> Self {
        StoreError::StaleEdit(e)
    }
}
