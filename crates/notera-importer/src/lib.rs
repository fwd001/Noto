//! notera-importer —— 把用户手里的文件变成笔记，**一个字都不许丢**。
//!
//! 规范来源：`docs/DATA-MODEL.md` §10（富文本 schema）、§15（导出/导入/备份）、
//! `docs/ARCHITECTURE-MAP.md` §3（不变式 I3/I5/I6）、`docs/TEST-PLAN.md` FT-IO-06 / CP-09。
//! 依赖方向（ARCHITECTURE-MAP §5 的 `notera-importer` 行）：core, richtext, crypto, store, sync；
//! **不**依赖 net / webdav —— 导入器不发请求，附件下载与备份包搬运是上层的事。
//!
//! ## 这一层做什么
//! 1. [`ImportSource`]：读文件 + 三道闸门（体积 → 二进制 → 编码）+ 类型探测。
//!    探测只决定"按 Markdown 还是按纯文本解析"，**永不**因为扩展名而拒绝一个文件。
//! 2. [`markdown`]：Markdown / 纯文本 → [`Document`]。
//!    产物在计划阶段就已经过 `parse()`（normalize + validate），所以入库的文档必然合法。
//! 3. [`frontmatter`]：`---` 头块。只理解 `title` / `date` / `tags`，其余键进
//!    `ignored` 与 `entries`，调用方拿得到，不存在静默丢弃。
//! 4. [`ImportPlan`] → [`apply`]：计划是纯值，落地才写库。幂等：同一份计划重放两次，
//!    第二次一条都不新建（去重键 = 目标文件夹内已有笔记的 `content_hash`）。
//!
//! ## "无损"在这里具体是什么
//! * 任何**非空白字符**要么出现在某个块的 `content[].text` 里，要么出现在块/mark 的
//!   `attrs` 里（`level` / `lang` / `checked` / `number` / `src` / `href` / `title`），
//!   要么被明确记为语法标记（`#`、`>`、列表标记、围栏、成对包裹符、`[ ]`/`[x]`、行尾空白、BOM）。
//! * 看不懂的东西一律按字面进正文：引用式链接、HTML 块、setext 标题、footnote、
//!   未闭合的 `**`、非标点的反斜杠……都不被删、不被改写。
//! * 三处**设计内**的偏离（都有测试钉着）：
//!   1. URL / 图片 `src` 从正文搬进 attrs —— 内容没丢，但 `extract().plain_text` 看不见它；
//!   2. front-matter 头块的那几行不进正文 —— 值搬到 `title` / `date` / `tags` / `ignored`；
//!   3. 字面序号与标题井号（`1.`、`#`）搬到 `attrs.number` / `attrs.level`。
//!
//! ## 这一块不该长什么
//! 备份 ZIP 导出、`.enex`（Evernote ENML）结构化解析：见交付报告的 BLOCKED 清单。
//! 今天 `.enex` 会被当作"看不懂的文本"整篇进正文（可读、无损，但不是结构化笔记）。

mod bundle;
mod error;
mod frontmatter;
mod markdown;
mod plan;
mod source;

pub use bundle::{Bundle, Manifest, MAX_PROTOCOL, read_bundle, write_bundle, BUNDLE_FORMAT};
pub use error::ImportError;
pub use frontmatter::FrontMatter;
pub use markdown::{id_prefix, MAX_INLINE_PARSE_CHARS};
pub use plan::{
    apply, document_for, hashes_in_folder, import_paths, plan, ApplyReport, CreatedNote,
    DuplicateNote, FailedSource, FailedWrite, FolderTarget, ImportItem, ImportPlan, SkipReason,
    SkippedSource, TitleSource,
};
pub use source::{
    ImportSource, KindSource, SourceKind, BINARY_SNIFF_BYTES, MAX_SOURCE_BYTES,
};

/// 让 `notera_richtext::Document` 在本 crate 的公共签名里保持可达。
pub use notera_richtext::Document;
