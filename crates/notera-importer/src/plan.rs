//! 计划 → 落地，两段式。
//!
//! [`ImportPlan`] 是**纯**值：只吃已经读进内存的 [`ImportSource`]，产出标题、文档、
//! 内容哈希，不碰 Store、不碰磁盘。于是"先给用户看要导什么"和"真正写库"是两件事，
//! 用户可以只出计划不落库（预览），也可以把同一份计划重放多次而不产生副本。
//!
//! [`apply`] 才写库，且只写两类东西：`create_note`（新笔记）与可选的一次
//! `create_folder`（目标文件夹不存在时）。**绝不**编辑、删除、移动任何既有笔记
//! （I3/INV-01：导入不覆盖），也绝不做文件夹级联。

use crate::error::ImportError;
use crate::frontmatter::{self, FrontMatter};
use crate::markdown;
use crate::source::{ImportSource, SourceKind};
use notera_core::{ContentHash, EntityId};
use notera_richtext::{parse_from_value, Document, BlockType};
use notera_store::{NoteQuery, Store};
use serde_json::Value;
use std::collections::HashSet;
use std::path::Path;

/// `list_notes` 的页大小（Store 的 `limit = 0` 也就是 500，这里显式写出来）。
const NOTE_PAGE: u32 = 500;

/// 标题是从哪一档来的（优先级：front-matter `title` > 第一个标题块 > 文件名去扩展名）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TitleSource {
    /// front-matter 的 `title:`。这一档会**补一个一级标题块**到文档开头（除非正文本来
    /// 就以标题开头），否则 `notes.title` 那列拿不到它 —— 那一列是从 doc 派生的（I5）。
    FrontMatter,
    /// 文档里第一个 `heading` 块的文本。
    FirstHeading,
    /// 文件名去扩展名。**不**改文档：Store 的 `notes.title` 会退化成第一行非空文本
    /// （schema 里没有"外部标题"这一列，见报告里的限制说明）。
    Filename,
}

/// 计划里的一条笔记。
#[derive(Clone, Debug)]
pub struct ImportItem {
    /// 报告用名字。
    pub label: String,
    pub source_path: Option<std::path::PathBuf>,
    pub kind: SourceKind,
    pub title: String,
    pub title_source: TitleSource,
    pub front_matter: FrontMatter,
    /// 已过 `parse()`（normalize + validate）的文档，可直接入库。
    pub doc: Document,
    /// = Store 侧会算出的 `content_hash`：`sha256(canonical(doc))`。幂等靠它。
    pub content_hash: ContentHash,
    /// 源字节的哈希（区分"同一文件两次"与"两个内容相同的文件"）。
    pub source_hash: ContentHash,
}

impl ImportItem {
    /// 交给 `Store::create_note` 的文档值。
    pub fn doc_value(&self) -> Result<Value, ImportError> {
        serde_json::to_value(&self.doc).map_err(|e| ImportError::InvalidDoc(e.to_string()))
    }
}

/// 没被拒绝、但也不产生笔记的源（按 TEST-PLAN FT-IO-06：空文件"按报告跳过"）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkippedSource {
    pub label: String,
    pub reason: SkipReason,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkipReason {
    /// 0 字节，或整份只有空白字符。
    Blank,
}

impl SkipReason {
    pub fn as_str(self) -> &'static str {
        match self {
            SkipReason::Blank => "blank",
        }
    }
}

/// 单个源在闸门/解析阶段失败（一条失败不影响其他条）。
#[derive(Debug)]
pub struct FailedSource {
    pub label: String,
    pub error: ImportError,
}

/// 一份完整的导入计划。
///
/// `Clone` 刻意**不**派生：`FailedSource` 里装的是 `ImportError`（含 `StoreError`，
/// 自带 `rusqlite::Error`，不可克隆）。要复制计划就只复制 `items`。
#[derive(Debug, Default)]
pub struct ImportPlan {
    pub items: Vec<ImportItem>,
    pub skipped: Vec<SkippedSource>,
    pub failures: Vec<FailedSource>,
}

impl ImportPlan {
    /// 纯构造：只吃内存里的源。
    pub fn build<'a>(sources: impl IntoIterator<Item = &'a ImportSource>) -> Self {
        let mut plan = ImportPlan::default();
        for src in sources {
            push_source(&mut plan, src);
        }
        plan
    }

    /// 读盘 + 构造计划。**只读磁盘**，对 Store 没有任何副作用。
    pub fn from_paths(paths: &[&Path]) -> Self {
        let mut plan = ImportPlan::default();
        for p in paths {
            let label = p
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| p.display().to_string());
            match ImportSource::read(p) {
                Ok(src) => push_source(&mut plan, &src),
                Err(error) => plan.failures.push(FailedSource { label, error }),
            }
        }
        plan
    }

    /// 计划是否无事可做。
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// 条目总数（新增 + 跳过 + 失败 = 交给它的源数）。
    pub fn total(&self) -> usize {
        self.items.len() + self.skipped.len() + self.failures.len()
    }
}

/// 便捷入口：一堆源 → 计划。
pub fn plan(sources: &[ImportSource]) -> ImportPlan {
    ImportPlan::build(sources)
}

/// 一个源进计划的唯一路径：空白 → 跳过（有报告），其余 → 条目或失败。
/// `build` 与 `from_paths` 都必须走这里，否则"空文件"在两条入口下行为不一致。
fn push_source(plan: &mut ImportPlan, src: &ImportSource) {
    if src.text.trim().is_empty() {
        plan.skipped.push(SkippedSource { label: src.label.clone(), reason: SkipReason::Blank });
        return;
    }
    match plan_one(src) {
        Ok(item) => plan.items.push(item),
        Err(error) => plan.failures.push(FailedSource { label: src.label.clone(), error }),
    }
}

fn plan_one(src: &ImportSource) -> Result<ImportItem, ImportError> {
    let (fm, body) = frontmatter::split(&src.text);
    let fm_title = fm.title.clone().map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
    let prefix = markdown::id_prefix(&src.source_hash.short());
    let draft = markdown::to_document(&body, src.kind, &prefix, None);
    // front-matter 的 title 只有变成 doc 里"第一个标题"才落得进 `notes.title`（派生列，I5）。
    // 已有同名标题块时不补第二个，否则用户看到的是标题重复两遍。
    let inject =
        fm_title.is_some() && first_heading_text(&draft).as_deref() != fm_title.as_deref();
    let draft = if inject {
        markdown::to_document(&body, src.kind, &prefix, fm_title.as_deref())
    } else {
        draft
    };
    let draft_value = serde_json::to_value(&draft).map_err(|e| ImportError::InvalidDoc(e.to_string()))?;
    // 验收闸门就是 richtext 自己的闸门：过不了 parse() 的文档绝不进计划。
    let doc = parse_from_value(&draft_value)
        .map_err(|e| ImportError::InvalidDoc(format!("{}: {e}", src.label)))?;
    let canonical = notera_richtext::canonical(&doc);
    let content_hash = ContentHash::of(canonical.as_bytes());
    let (title, title_source) = match (fm_title, first_heading_text(&doc)) {
        (Some(t), _) => (t, TitleSource::FrontMatter),
        (None, Some(h)) => (h, TitleSource::FirstHeading),
        (None, None) => (src.file_stem(), TitleSource::Filename),
    };
    Ok(ImportItem {
        label: src.label.clone(),
        source_path: src.path.clone(),
        kind: src.kind,
        title,
        title_source,
        front_matter: fm,
        doc,
        content_hash,
        source_hash: src.source_hash.clone(),
    })
}

fn first_heading_text(doc: &Document) -> Option<String> {
    doc.content
        .iter()
        .find(|b| b.type_ == BlockType::Heading && !b.plain_text().trim().is_empty())
        .map(|b| b.plain_text().trim().to_string())
}

/// 写库的目标文件夹。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FolderTarget {
    /// 按 id 指已有的文件夹。不存在或已在回收站 → [`ImportError::FolderNotFound`]，
    /// 导入器不会顺手把它"救活"。
    Existing(EntityId),
    /// 按名字定位（同父同名且未删）；找不到就创建它，并在报告里说明。
    Named { name: String, parent: Option<EntityId> },
}

impl FolderTarget {
    /// 按名字给一个目标文件夹。
    pub fn named(name: impl Into<String>, parent: Option<EntityId>) -> Self {
        FolderTarget::Named { name: name.into(), parent }
    }
}

impl From<EntityId> for FolderTarget {
    fn from(id: EntityId) -> Self {
        FolderTarget::Existing(id)
    }
}

/// 一条真正写进库的笔记。
#[derive(Clone, Debug)]
pub struct CreatedNote {
    pub label: String,
    pub note_id: EntityId,
    /// Store 侧从 doc 派生出的标题（`Filename` 一档与计划标题不同是已知偏离，见报告）。
    pub stored_title: String,
    pub content_hash: String,
}

/// 一条因为"内容哈希已存在于目标文件夹"而跳过的计划条目。
#[derive(Clone, Debug)]
pub struct DuplicateNote {
    pub label: String,
    pub content_hash: String,
    /// 计划内重复（同一个文件给了两次）还是库里已有。
    pub in_plan: bool,
}

/// 一条写库失败的计划条目（失败只影响这一条）。
#[derive(Debug)]
pub struct FailedWrite {
    pub label: String,
    pub error: ImportError,
}

/// `apply` 的结果：新增 / 重复 / 失败 + 文件夹事实，全部可核对
/// （FT-IO-02 要求的"报告可观测：X+Y+Z = 条目数"）。
#[derive(Debug, Default)]
pub struct ApplyReport {
    pub target_folder: EntityId,
    pub folder_name: String,
    /// 目标文件夹是本次创建的还是本来就有的。
    pub folder_created: bool,
    pub created: Vec<CreatedNote>,
    pub duplicates: Vec<DuplicateNote>,
    pub failed: Vec<FailedWrite>,
    /// 计划阶段就跳过的源数（空文件）。
    pub plan_skipped: usize,
    /// 计划阶段就失败的源数（超限 / 二进制 / 编码）。
    pub plan_failures: usize,
}

impl ApplyReport {
    pub fn created_count(&self) -> usize {
        self.created.len()
    }

    pub fn duplicate_count(&self) -> usize {
        self.duplicates.len()
    }

    pub fn failed_count(&self) -> usize {
        self.failed.len()
    }

    /// 账是否对得上：新增 + 重复 + 失败 = 计划条目数。
    pub fn accounts_for(&self, plan: &ImportPlan) -> bool {
        self.created_count() + self.duplicate_count() + self.failed_count() == plan.items.len()
            && self.plan_skipped == plan.skipped.len()
            && self.plan_failures == plan.failures.len()
    }
}

/// 把计划落进 Store：只创建笔记（以及必要时一个目标文件夹），从不改动既有笔记。
pub fn apply(
    store: &Store,
    folder: &FolderTarget,
    plan: &ImportPlan,
) -> Result<ApplyReport, ImportError> {
    let (target, folder_name, folder_created) = resolve_folder(store, folder)?;
    let mut report = ApplyReport {
        target_folder: target.clone(),
        folder_name,
        folder_created,
        plan_skipped: plan.skipped.len(),
        plan_failures: plan.failures.len(),
        ..Default::default()
    };
    let seen = existing_content_hashes(store, &target)?;
    let mut made_this_run: HashSet<String> = HashSet::new();
    for item in &plan.items {
        let hash = item.content_hash.as_str().to_string();
        // 两种"重复"要分开报：库里已有同内容（重放导入）vs 计划内部自我重复（同一文件给了两次）。
        let in_plan = made_this_run.contains(&hash);
        if seen.contains(&hash) || in_plan {
            report.duplicates.push(DuplicateNote {
                label: item.label.clone(),
                content_hash: hash,
                in_plan,
            });
            continue;
        }
        // 单条目失败只记这一条：一批里一条坏的不该脏掉其余的（FT-IO-02 的"逐条可观测"）。
        let value = match item.doc_value() {
            Ok(v) => v,
            Err(error) => {
                report.failed.push(FailedWrite { label: item.label.clone(), error });
                continue;
            }
        };
        match store.create_note(&target, value) {
            Ok(note) => {
                made_this_run.insert(hash);
                report.created.push(CreatedNote {
                    label: item.label.clone(),
                    note_id: note.id,
                    stored_title: note.title,
                    content_hash: note.content_hash,
                });
            }
            Err(error) => report
                .failed
                .push(FailedWrite { label: item.label.clone(), error: ImportError::Store(error) }),
        }
    }
    Ok(report)
}

/// 计划里所有条目的内容哈希是否都已躺在目标文件夹里（幂等复验用）。
pub fn hashes_in_folder(store: &Store, folder: &EntityId) -> Result<HashSet<String>, ImportError> {
    existing_content_hashes(store, folder)
}

fn resolve_folder(store: &Store, target: &FolderTarget) -> Result<(EntityId, String, bool), ImportError> {
    match target {
        FolderTarget::Existing(id) => {
            let f = store
                .get_folder(id)?
                .filter(|f| f.deleted_at.is_none())
                .ok_or_else(|| ImportError::FolderNotFound(id.clone()))?;
            Ok((f.id, f.name, false))
        }
        FolderTarget::Named { name, parent } => {
            let hit = store
                .list_folders()?
                .into_iter()
                .find(|f| &f.name == name && &f.parent_id == parent && f.deleted_at.is_none());
            if let Some(f) = hit {
                return Ok((f.id, f.name, false));
            }
            let f = store.create_folder(parent.as_ref(), name)?;
            Ok((f.id, f.name, true))
        }
    }
}

/// 目标文件夹里已有笔记的内容哈希（幂等判定的全部依据）。
///
/// `notes` 表没有"来源文件"这一列（DATA-MODEL §5.1），所以能用的只有内容哈希：
/// 于是**去重的粒度是"这篇内容"而不是"这个路径"** —— 同一文件改名再导会再生成一条，
/// 同一文件导进两个文件夹也各有一条。这是 schema 限制，不是本 crate 的选择。
fn existing_content_hashes(store: &Store, folder: &EntityId) -> Result<HashSet<String>, ImportError> {
    let mut out = HashSet::new();
    let mut offset = 0u32;
    loop {
        let q = NoteQuery { folder: Some(folder.clone()), trash: false, limit: NOTE_PAGE, offset };
        let rows = store.list_notes(&q)?;
        let n = rows.len();
        for r in rows {
            out.insert(r.content_hash);
        }
        if (n as u32) < NOTE_PAGE {
            break;
        }
        offset += NOTE_PAGE;
    }
    Ok(out)
}

/// 单文件一步到位（预览请用 [`plan`] + [`apply`]）。
pub fn import_paths(store: &Store, folder: &FolderTarget, paths: &[&Path]) -> Result<(ImportPlan, ApplyReport), ImportError> {
    let plan = ImportPlan::from_paths(paths);
    let report = apply(store, folder, &plan)?;
    Ok((plan, report))
}

/// 由源构造文档（导出侧/测试用的最小入口）。
pub fn document_for(src: &ImportSource) -> Result<Document, ImportError> {
    Ok(plan_one(src)?.doc)
}

// ============================================================ 单元测试 ===

#[cfg(test)]
mod tests {
    use super::*;
    use notera_core::ContentHash;
    use std::path::Path;

    fn source(name: &str, text: &str) -> ImportSource {
        ImportSource::from_bytes(Some(Path::new(name)), text.as_bytes()).expect("fixture 必须能解码")
    }

    #[test]
    fn title_precedence_front_matter_beats_first_heading_beats_filename() {
        // 1) front-matter `title:` 最高，并且会补一个一级标题块，让派生列拿得到它
        let a = plan_one(&source("post.md", "---\ntitle: 头部标题\n---\n# 正文标题\n\n内容\n")).expect("a");
        assert_eq!(a.title, "头部标题");
        assert_eq!(a.title_source, TitleSource::FrontMatter);
        assert_eq!(a.doc.content[0].type_, BlockType::Heading);
        assert_eq!(a.doc.content[0].plain_text(), "头部标题");
        assert_eq!(
            notera_richtext::extract(&a.doc).title,
            "头部标题",
            "补的标题块必须让 Store 的派生 title 与计划一致"
        );

        // 2) 没有 front-matter title 时看第一个标题块
        let b = plan_one(&source("note.md", "# 一号标题\n\n## 二号\n")).expect("b");
        assert_eq!(b.title, "一号标题");
        assert_eq!(b.title_source, TitleSource::FirstHeading);

        // 3) 都没有 → 文件名去扩展名（不改文档：Store 的 title 会退成首行文本）
        let c = plan_one(&source("我的清单.txt", "买牛奶\n买咖啡\n")).expect("c");
        assert_eq!(c.title, "我的清单");
        assert_eq!(c.title_source, TitleSource::Filename);
        assert_eq!(c.doc.content[0].type_, BlockType::Paragraph);
    }

    #[test]
    fn front_matter_title_matching_the_h1_does_not_duplicate_it() {
        let p = plan_one(&source("post.md", "---\ntitle: 同一个标题\n---\n# 同一个标题\n\n内容\n")).expect("p");
        assert_eq!(p.title_source, TitleSource::FrontMatter);
        assert_eq!(
            p.doc.content.iter().filter(|b| b.type_ == BlockType::Heading).count(),
            1,
            "正文已有同名标题时不能再补一个"
        );
    }

    #[test]
    fn blank_sources_are_skipped_not_failed() {
        let p = ImportPlan::build([&source("e1.md", ""), &source("e2.md", "  \n\t\n")]);
        assert_eq!(p.skipped.len(), 2);
        assert_eq!(p.skipped[0].reason, SkipReason::Blank);
        assert!(p.items.is_empty());
        assert!(p.failures.is_empty());
        assert_eq!(p.total(), 2);
    }

    #[test]
    fn planned_hash_is_exactly_what_store_will_compute() {
        // 计划里的 hash 必须等于 Store 侧的算法（richtext parse → canonical → sha256），
        // 否则幂等判定永远命中不了。
        let item = plan_one(&source("x.md", "# T\n\n段落\n")).expect("item");
        let reparsed = parse_from_value(&item.doc_value().expect("value")).expect("parse");
        let canonical = notera_richtext::canonical(&reparsed);
        assert_eq!(item.content_hash, ContentHash::of(canonical.as_bytes()));
        assert_eq!(reparsed, item.doc, "产物必须已经规范化到位（幂等的另一半）");
    }

    #[test]
    fn planning_is_deterministic_across_runs() {
        let mk = || plan_one(&source("d.md", "# 标题\n\n- 一\n- 二\n\n```rs\nfn f() {}\n```\n")).expect("mk");
        let (a, b) = (mk(), mk());
        assert_eq!(a.doc, b.doc);
        assert_eq!(a.content_hash, b.content_hash);
        assert_eq!(notera_richtext::canonical(&a.doc), notera_richtext::canonical(&b.doc));
    }
}
