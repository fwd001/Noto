//! 服务器能力位（docs/SYNC-PROTOCOL.md §5）。
//!
//! 位值即 `sync_accounts.cap_mask` 的持久化形态，因此这里**只做表达**，不做探测：
//! 探测在首次连接与每日一次由 `notera-host` 发起（§5 的探测表），结果写库后
//! 再交回本适配器。判定策略前必须假定这些位是被实测过的，而不是猜的。
//!
//! 默认值 [`Caps::conventional`] 故意取"现代服务器通常具备"的那一档：
//! 猜高了的失败形态是 412/405（→ [`notera_sync::RemoteError::Precondition`] 或就地降级），
//! 猜低了的失败形态是盲写覆盖 —— 后者会丢数据，前者不会。

/// `cap_mask` 的一位或多位。
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Caps(u32);

impl Caps {
    /// 无任何能力：一切写走 S3 盲写复验。
    pub const NONE: u32 = 0;
    /// `PUT` 后 `GET` 带 `If-None-Match` 能拿到 304 —— 空轮快路径的前提（§5）。
    pub const STRONG_ETAG: u32 = 1 << 0;
    /// 条件 `PUT`（`If-Match`/`If-None-Match` 真的被服务器判定）—— 决定写入策略 S1。
    pub const CONDITIONAL_PUT: u32 = 1 << 1;
    /// `MOVE` 到已存在目标且 `Overwrite: F` 会回 412 —— 决定写入策略 S2。
    pub const OVERWRITE_F_MOVE: u32 = 1 << 2;
    /// `PROPFIND Depth: infinity` 可用（D2 重建阶梯用）。
    pub const DEPTH_INFINITY: u32 = 1 << 3;
    /// `GET` + `Range` 回 206（附件断点续传用）。
    pub const RANGE: u32 = 1 << 4;
    /// 接受分块请求体（影响半上传检测方式）。
    pub const CHUNKED: u32 = 1 << 5;

    /// 全部已知位。
    pub const ALL: u32 = Self::STRONG_ETAG
        | Self::CONDITIONAL_PUT
        | Self::OVERWRITE_F_MOVE
        | Self::DEPTH_INFINITY
        | Self::RANGE
        | Self::CHUNKED;

    pub const fn from_mask(mask: u32) -> Caps {
        Caps(mask)
    }
    pub const fn mask(self) -> u32 {
        self.0
    }
    pub const fn has(self, bit: u32) -> bool {
        bit != 0 && (self.0 & bit) != 0
    }
    pub const fn with(self, bit: u32) -> Caps {
        Caps(self.0 | bit)
    }
    pub const fn without(self, bit: u32) -> Caps {
        Caps(self.0 & !bit)
    }
    pub const fn none() -> Caps {
        Caps(Self::NONE)
    }
    pub const fn full() -> Caps {
        Caps(Self::ALL)
    }

    /// 未探测时的保守默认：条件 PUT + Overwrite:F + 强 ETag + Range，不含 `Depth:infinity`。
    pub const fn conventional() -> Caps {
        Caps(Self::STRONG_ETAG | Self::CONDITIONAL_PUT | Self::OVERWRITE_F_MOVE | Self::RANGE)
    }

    /// 按 §5 的表挑选写入策略。
    pub fn write_strategy(self) -> WriteStrategy {
        if self.has(Self::CONDITIONAL_PUT) {
            WriteStrategy::S1
        } else if self.has(Self::OVERWRITE_F_MOVE) {
            WriteStrategy::S2
        } else {
            WriteStrategy::S3
        }
    }
}

impl Default for Caps {
    fn default() -> Self {
        Caps::conventional()
    }
}

impl std::fmt::Debug for Caps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut names: Vec<&str> = Vec::new();
        for (bit, name) in [
            (Self::STRONG_ETAG, "strong_etag"),
            (Self::CONDITIONAL_PUT, "conditional_put"),
            (Self::OVERWRITE_F_MOVE, "overwrite_f_move"),
            (Self::DEPTH_INFINITY, "depth_infinity"),
            (Self::RANGE, "range"),
            (Self::CHUNKED, "chunked"),
        ] {
            if self.has(bit) {
                names.push(name);
            }
        }
        f.debug_struct("Caps")
            .field("mask", &self.0)
            .field("caps", &names)
            .finish()
    }
}

/// 写入策略（SYNC-PROTOCOL §5）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteStrategy {
    /// 条件 `PUT` + `If-Match`（或创建时的 `If-None-Match: *`）。并发保护强。
    S1,
    /// `PUT tmp` → 回读校验 → `MOVE tmp→dest`（`Overwrite: F`）。并发保护强。
    S2,
    /// 盲 `PUT` + 立即复验。降级路径：存在覆盖窗口，靠复验检出。
    S3,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strategy_follows_the_capability_table() {
        assert_eq!(Caps::full().write_strategy(), WriteStrategy::S1);
        assert_eq!(
            Caps::from_mask(Caps::OVERWRITE_F_MOVE | Caps::STRONG_ETAG).write_strategy(),
            WriteStrategy::S2
        );
        assert_eq!(
            Caps::from_mask(Caps::STRONG_ETAG).write_strategy(),
            WriteStrategy::S3
        );
        assert_eq!(Caps::none().write_strategy(), WriteStrategy::S3);
    }

    #[test]
    fn mask_roundtrips_and_zero_is_never_a_hit() {
        let c = Caps::conventional();
        assert_eq!(Caps::from_mask(c.mask()), c);
        assert!(!c.has(Caps::NONE), "空位不得被判成具备某能力");
        assert!(!c.has(Caps::DEPTH_INFINITY));
        assert!(c.has(Caps::CONDITIONAL_PUT));
    }
}
