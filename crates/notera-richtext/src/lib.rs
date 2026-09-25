//! notera-richtext —— 权威富文本模型（不是 HTML，是与编辑器无关的块级树）。
//!
//! 规范来源：`docs/DATA-MODEL.md` §10（模型 / 规范化 / 前向兼容）、§7.1（派生列）、
//! `docs/CONFLICT-RESOLUTION.md` §3（块级三方合并）、`docs/ARCHITECTURE-MAP.md` §3
//! 不变式 I5/I6/I7。
//!
//! 依赖方向：只依赖 `notera-core`。**不得**知道同步这件事（ARCHITECTURE-MAP §2）：
//! 这里没有 rev / push / pull / 远端 的任何词汇。
//!
//! ## 三条硬约束
//! 1. **preserve-unknown（I7）**：未知 `type` → [`BlockType::Unknown`]（存原名），未知 mark
//!    → [`MarkKind::Unknown`]，未知 `attrs` 键原样保留；`canonical()`/`to_json()` 必须把它们
//!    写回去。旧客户端摧毁新内容是 P0 事故。
//! 2. **canonical 稳定**：同一逻辑文档（键插入顺序不同）必须逐字节相同，因为 `content_hash`
//!    建立在它之上（哈希不稳 = 同步判定失效）。
//! 3. **merge 是纯函数、永不 panic、永不静默丢内容**：证不出无损就返回
//!    [`MergeOutcome::Conflict`]，合并产物过不了 `validate()` 就丢弃它（§3.4）。

mod codec;
mod extract;
mod merge;
mod model;

#[cfg(test)]
mod merge_tests;
#[cfg(test)]
mod tests;

pub use codec::{
    block_ids, canonical, normalize, parse, parse_for_read, parse_from_value, to_json, validate,
};
pub use extract::{extract, Extracted};
pub use merge::{merge, MergeOutcome, ReadOnlyReason};
pub use model::{
    supports, Block, BlockType, Document, Inline, Mark, MarkKind, RichError, DOC_FORMAT,
};
