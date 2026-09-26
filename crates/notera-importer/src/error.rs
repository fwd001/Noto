//! 导入侧错误词表。
//!
//! 闸门顺序（`source` 模块实现）是契约的一部分，每一道都有**独立变体**，
//! 因为 UI 要按类别给不同文案：超限要提示"拆分文件"、二进制要提示"这不是文本"、
//! 编码不合法要提示"请转成 UTF-8"。三者绝不能都塌成一个 `Invalid`。

use notera_core::EntityId;

/// 导入器的一切失败。
#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    /// 第一道闸门：体积。超过上限直接拒绝，**不截断**（截断 = 静默丢内容）。
    #[error("文件 {path} 有 {size} 字节，超过导入上限 {limit} 字节：请拆分后再导入")]
    TooLarge { path: String, size: u64, limit: u64 },

    /// 第二道闸门：二进制嗅探（前 `window` 字节里出现 NUL）。
    #[error("文件 {path} 看起来不是文本（前 {window} 字节的第 {offset} 字节处出现 NUL）")]
    Binary { path: String, offset: usize, window: usize },

    /// 第三道闸门：编码。UTF-8（可带 BOM）与带 BOM 的 UTF-16 之外一律拒绝。
    ///
    /// 刻意**不**引入 GB18030/Big5 之类的解码依赖：猜错代码页会把用户正文变成乱码，
    /// 那比"拒绝并让用户另存为 UTF-8"破坏性更大。
    #[error("文件 {path} 不是 UTF-8，也不是带 BOM 的 UTF-16（{detail}）：请转成 UTF-8 后重试")]
    InvalidEncoding { path: String, detail: String },

    /// 读文件失败（不存在、权限、目录……）。
    #[error("读取 {path} 失败: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },

    /// 目标文件夹不存在 / 在回收站里。导入器不建文件夹"顺手救活"它（I3：不覆盖既有事实）。
    #[error("目标文件夹不存在或已在回收站: {0}")]
    FolderNotFound(EntityId),

    /// 自己产出的文档过不了 richtext 校验 —— 属于本 crate 的 bug，必须冒出来而不是硬写。
    #[error("生成的文档未通过 richtext 校验（未写入任何数据）: {0}")]
    InvalidDoc(String),

    /// 存储层拒绝（I6 闸门、约束、SQL）。
    #[error(transparent)]
    Store(#[from] notera_store::StoreError),

    /// 导出 bundle 自己不对：格式号不认识、manifest 缺失、附件内容与文件名不符……
    /// 单列一个变体是因为这些必须**整包拒绝**，而不是像单文件那样跳过继续。
    #[error("导出包不可用（未导入任何数据）: {0}")]
    Bundle(String),
}

impl ImportError {
    /// 是否属于"这个文件进不来，但别的文件照进"一类的单文件失败。
    pub fn is_source_rejection(&self) -> bool {
        matches!(
            self,
            ImportError::TooLarge { .. }
                | ImportError::Binary { .. }
                | ImportError::InvalidEncoding { .. }
                | ImportError::Io { .. }
        )
    }
}
