//! 端到端：真文件 + 真 Store（临时目录），以及"无损"这条硬要求的语料测试。
//!
//! 为什么用真 SQLite：幂等、"不覆盖既有笔记"、派生列（`notes.title`）这三件事
//! 只有在 Store 的真实写入路径上才可证（`derive::prepare` 会重算 hash）。
//! 不引入 `tempfile`：它是 `notera-store` 的 dev-dependency，本 crate 要用就得改
//! `Cargo.lock`（依赖漂移），所以这里用 `std::env::temp_dir()` + 唯一目录 + Drop 清理。

use notera_core::{DeviceId, EntityId};
use notera_importer::{
    apply, document_for, hashes_in_folder, plan, ApplyReport, FolderTarget, ImportError,
    ImportPlan, ImportSource, SourceKind, TitleSource, MAX_SOURCE_BYTES,
};
use notera_richtext::{canonical, extract, parse_from_value, BlockType, Document};
use notera_store::{NoteQuery, Store};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

// ---------------------------------------------------------------- 夹具 ---

static SEQ: AtomicUsize = AtomicUsize::new(0);

/// 临时目录（Drop 即删）。
struct Tmp {
    path: PathBuf,
}

impl Tmp {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "notera-importer-{tag}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("临时目录可建");
        Tmp { path: dir }
    }

    fn dir(&self) -> &Path {
        &self.path
    }

    /// 在临时目录里写一个文件，返回完整路径。
    fn write(&self, name: &str, bytes: impl AsRef<[u8]>) -> PathBuf {
        let p = self.path.join(name);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("父目录可建");
        }
        std::fs::write(&p, bytes.as_ref()).expect("写文件");
        p
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

struct Fix {
    /// 只为了活着：库目录被删掉的话 Store 就废了。
    #[allow(dead_code)]
    tmp: Tmp,
    store: Store,
    default_folder: EntityId,
}

impl Fix {
    fn open() -> Self {
        let tmp = Tmp::new("store");
        let store = Store::open(&tmp.dir().join("db"), DeviceId::new()).expect("Store::open");
        let default_folder = store
            .list_folders()
            .expect("list_folders")
            .into_iter()
            .find(|f| f.system_kind.as_deref() == Some("default"))
            .expect("默认本必须存在")
            .id;
        Fix {
            tmp,
            store,
            default_folder,
        }
    }

    fn folder(&self) -> FolderTarget {
        FolderTarget::from(self.default_folder.clone())
    }

    /// 列表 SQL 不读 doc（DATA-MODEL §13），所以这里拿的是全库非回收站笔记数。
    fn note_count(&self) -> usize {
        self.store
            .list_notes(&NoteQuery {
                folder: None,
                trash: false,
                limit: u32::MAX,
                offset: 0,
            })
            .expect("list_notes")
            .len()
    }

    fn revisions(&self) -> u32 {
        self.store.stats().expect("stats").revisions
    }
}

fn source_from_str(name: &str, text: &str) -> ImportSource {
    ImportSource::from_bytes(Some(Path::new(name)), text.as_bytes()).expect("fixture 解码")
}

fn plan_of(name: &str, text: &str) -> ImportPlan {
    plan(&[source_from_str(name, text)])
}

fn doc_of(name: &str, text: &str) -> Document {
    document_for(&source_from_str(name, text)).expect("fixture 文档")
}

fn single_note_plan(text: &str) -> ImportPlan {
    plan_of("note.md", text)
}

// ------------------------------------------------------------ 无损语料 ---

/// 一个语料条目：`moved` 列出**设计上**不进正文的 token（URL 片段、围栏语言名、
/// front-matter 键与值），其余 token 必须逐个出现在 `extract(doc).plain_text` 里。
struct Case {
    name: &'static str,
    source: &'static str,
    moved: &'static [&'static str],
}

const CORPUS: &[Case] = &[
    Case {
        name: "nested_lists",
        source: "- 顶层\n  - 二级\n    - 三级\n      - 四级\n- 回到顶层\n",
        moved: &[],
    },
    Case {
        name: "unclosed_fence",
        source: "前文\n\n```rust\nfn 未闭合() {\n    println!(\"里面\")\n",
        moved: &["rust"],
    },
    Case {
        name: "stars_inside_words",
        source: "un*be*lievable 与 snake_case_name 与 开头 **没有闭合\n",
        moved: &[],
    },
    Case {
        name: "cjk_and_fullwidth",
        source: "# 中文标题\n\n这是一段中文，里面有（括号）、顿号、以及 **强调**。\n",
        moved: &[],
    },
    Case {
        name: "emoji_zwj",
        source: "家庭 👨‍👩‍👧 与单个 😀 与 **加粗 emoji 😀**\n",
        moved: &[],
    },
    Case {
        name: "tabs_everywhere",
        source: "- 制表符项\n\t- tab 缩进的子项\n\t中间还有 tab\n",
        moved: &[],
    },
    Case {
        name: "crlf",
        source: "# 窗口换行\r\n\r\n段落一\r\n段落二\r\n",
        moved: &[],
    },
    Case {
        name: "trailing_whitespace",
        source: "行尾有空格   \n下一行\t\t\n",
        moved: &[],
    },
    Case {
        name: "reference_links",
        source: "看 [文字][标] 与 [另一个][标2]。\n\n[标]: https://ref.example/one \"一个\"\n[标2]: https://ref.example/two\n",
        moved: &[],
    },
    Case {
        name: "html_blocks",
        source: "<div class=\"box\"><p>原样 HTML</p></div>\n\n<details>\n<summary>展开</summary>\n正文\n</details>\n",
        moved: &[],
    },
    Case {
        name: "setext_and_odd_markers",
        source: "Setext 标题\n============\n\n小线\n----\n\n#无井号空格\n",
        moved: &[],
    },
    Case {
        name: "footnotes_and_tables",
        source: "这里有脚注[^1]。\n\n| 列甲 | 列乙 |\n|---|---|\n| 一 | 二 |\n\n[^1]: 脚注正文\n",
        moved: &[],
    },
    Case {
        name: "inline_and_standalone_images",
        source: "![配图](images/a.png)\n\n句子里的 ![行内](b.png) 图\n",
        moved: &["images", "a", "png", "b"],
    },
    Case {
        name: "links_and_autolinks",
        source: "点 [文字](https://link.example/z) 或 <https://angle.example/y> 或 https://bare.example/x\n",
        moved: &["https", "link", "example", "z"],
    },
    Case {
        name: "blockquote_lazy",
        source: "> 引用第一行\n> > 嵌套引用\n懒 continuation 行\n",
        moved: &[],
    },
    Case {
        name: "task_list_mixed",
        source: "- [ ] 待办项\n- [x] 已完成项\n1. 有序一\n2. 有序二\n",
        // `[x]` 里的 x 是语法标记：它的语义搬进了 attrs.checked（见下面的专项断言）
        moved: &["x"],
    },
    Case {
        name: "front_matter_head",
        source: "---\ntitle: 头部标题\ndate: 2026-03-01\ntags: [甲, 乙]\nauthor: 佚名\n---\n\n正文段\n",
        moved: &["title", "头部标题", "date", "tags", "author", "佚名", "甲", "乙"],
    },
    Case {
        name: "codespans_and_escapes",
        source: "带 `行内代码` 与 转义 \\* 星号 \\_ 下划线 与 ~~删除线~~\n",
        moved: &[],
    },
    Case {
        name: "hard_wrapped_paragraph",
        source: "这一句被\n手工折成了\n三行\n",
        moved: &[],
    },
    Case {
        name: "tilde_fence_with_specials",
        source: "~~~sh\nls -la *.md | wc -l\n~~~\n",
        moved: &["sh"],
    },
];

/// 语法字符集 = 解析器"允许吃掉"的字符（取超集，所以断言更严而不是更松）。
const SYNTAX_CHARS: &[char] = &[
    '#', '>', '*', '~', '`', '_', '[', ']', '(', ')', '!', '-', '+', '\\', '"', '\'', '/', '.',
    ':', '<', '|', '{', '}', '$', '%', '&', '?', ',', ';', '=', '^', '@',
];

/// 源文本里的"词"：剥掉空白与语法字符后剩下的连续片段；纯数字（列表序号）不算词。
fn words(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in s.chars() {
        if c.is_whitespace() || SYNTAX_CHARS.contains(&c) {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out.sort();
    out.dedup();
    out.retain(|w| !w.chars().all(|c| c.is_ascii_digit()));
    out
}

fn case_source(name: &str) -> &'static str {
    CORPUS
        .iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("语料 {name} 不存在"))
        .source
}

#[test]
fn lossless_corpus_every_word_lands_in_the_document() {
    let mut checked = 0usize;
    for case in CORPUS {
        let doc = doc_of("corpus.md", case.source);
        let plain = extract(&doc).plain_text;
        for tok in words(case.source) {
            if case.moved.contains(&tok.as_str()) {
                continue;
            }
            checked += 1;
            assert!(
                plain.contains(&tok),
                "[{}] token {tok:?} 从文档里消失了\n--- 源 ---\n{}\n--- 正文 ---\n{plain}",
                case.name,
                case.source
            );
        }
    }
    assert!(
        checked > 60,
        "语料断言数只有 {checked}，说明 words() 没在真正切词"
    );
    // `--nocapture` 时给出覆盖面，便于报告里说"断言了多少个 token"。
    eprintln!(
        "lossless 语料：{} 例 / {checked} 个 token 全部命中正文",
        CORPUS.len()
    );
}

#[test]
fn moved_tokens_land_in_attrs_instead_of_vanishing() {
    // "搬进 attrs"与"丢掉"必须可区分：逐条证明被搬走的东西还在文档里。
    let doc = doc_of("corpus.md", case_source("links_and_autolinks"));
    let hrefs: Vec<String> = doc
        .content
        .iter()
        .flat_map(|b| b.content.iter())
        .flat_map(|i| i.marks.iter())
        .filter(|m| m.kind.wire_name() == "link")
        .map(|m| m.attrs["href"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        hrefs.contains(&"https://link.example/z".to_string()),
        "实得 {hrefs:?}"
    );
    assert!(
        hrefs.contains(&"https://angle.example/y".to_string()),
        "实得 {hrefs:?}"
    );
    assert!(
        hrefs.contains(&"https://bare.example/x".to_string()),
        "实得 {hrefs:?}"
    );

    let img = doc_of("corpus.md", case_source("inline_and_standalone_images"));
    let srcs: Vec<String> = img
        .content
        .iter()
        .filter(|b| b.type_ == BlockType::Image)
        .map(|b| b.attrs["src"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        srcs,
        vec!["images/a.png".to_string()],
        "只有独占一行的图片被提升成块"
    );

    let fm = plan_of("corpus.md", case_source("front_matter_head"));
    let item = &fm.items[0];
    assert_eq!(item.front_matter.date.as_deref(), Some("2026-03-01"));
    assert_eq!(
        item.front_matter.ignored.get("author").map(String::as_str),
        Some("佚名")
    );
}

#[test]
fn every_corpus_document_passes_parse_and_canonical_is_stable() {
    for case in CORPUS {
        let doc = doc_of("corpus.md", case.source);
        let value = serde_json::to_value(&doc).expect("可序列化");
        let reparsed =
            parse_from_value(&value).unwrap_or_else(|e| panic!("[{}] parse 失败: {e}", case.name));
        assert_eq!(reparsed, doc, "[{}] 产物必须已经是规范化形态", case.name);
        let once = canonical(&doc);
        let twice = canonical(&notera_richtext::parse(&once).expect("canonical 文本必须可再解析"));
        assert_eq!(
            once, twice,
            "[{}] canonical 不稳 → content_hash 不稳",
            case.name
        );
        let mut ids = std::collections::HashSet::new();
        for b in &doc.content {
            assert!(
                (8..=32).contains(&b.id.len()),
                "[{}] id 长度违约: {}",
                case.name,
                b.id
            );
            assert!(
                ids.insert(b.id.clone()),
                "[{}] id 重复: {}",
                case.name,
                b.id
            );
        }
    }
}

#[test]
fn unknown_markup_stays_literal_text() {
    // 无损的反面证据：我们不懂的东西必须原样躺在正文里，而不是被"修好"。
    let doc = doc_of("x.md", "[a][b]\n<div>x</div>\n[^1]: y\nsetext\n=====\n");
    let plain = extract(&doc).plain_text;
    for token in ["[a][b]", "<div>x</div>", "[^1]: y", "setext", "====="] {
        assert!(plain.contains(token), "{token} 必须原样保留；实得 {plain}");
    }
}

// ------------------------------------------------------- 磁盘侧闸门 ---

#[test]
fn files_on_disk_hit_the_three_gates_in_order() {
    let tmp = Tmp::new("gates");
    let big = tmp.write("big.md", vec![b'a'; MAX_SOURCE_BYTES as usize + 1]);
    let mut bin_bytes = "# 标题\n".as_bytes().to_vec();
    bin_bytes.extend([0u8, 1, 2, 3]);
    let bin = tmp.write("bin.md", &bin_bytes);
    let gbk = tmp.write("gbk.md", [0xD6u8, 0xD0, 0xCE, 0xC4]);
    let ok = tmp.write("ok.md", "# 能进\n");

    let paths: Vec<&Path> = vec![&big, &bin, &gbk, &ok];
    let p = ImportPlan::from_paths(&paths);
    assert_eq!(p.items.len(), 1, "只有一个文件能出计划条目");
    assert_eq!(p.failures.len(), 3);
    let kinds: Vec<&'static str> = p
        .failures
        .iter()
        .map(|f| match &f.error {
            ImportError::TooLarge { .. } => "TooLarge",
            ImportError::Binary { .. } => "Binary",
            ImportError::InvalidEncoding { .. } => "InvalidEncoding",
            ImportError::Io { .. } => "Io",
            ImportError::FolderNotFound(_) => "FolderNotFound",
            ImportError::InvalidDoc(_) => "InvalidDoc",
            ImportError::Store(_) => "Store",
            ImportError::Bundle(_) => "Bundle",
        })
        .collect();
    assert_eq!(
        kinds,
        vec!["TooLarge", "Binary", "InvalidEncoding"],
        "实得 {kinds:?}"
    );
    assert!(p.failures.iter().all(|f| f.error.is_source_rejection()));
    assert_eq!(p.items[0].title, "能进");
}

#[test]
fn bom_crlf_tabs_and_utf16_files_import_from_disk() {
    let tmp = Tmp::new("encoding");
    let utf8bom = tmp.write("bom.md", "\u{feff}# 有 BOM\n\n正文\n".as_bytes());
    let crlf = tmp.write("crlf.md", "# 窗口\n\r\n- 项\r\n".as_bytes());
    let tabs = tmp.write("tabs.txt", "\t制表开头\n第二行\n".as_bytes());
    let mut u16: Vec<u8> = vec![0xFF, 0xFE];
    u16.extend(
        "# UTF16 标题\n"
            .encode_utf16()
            .flat_map(|c| c.to_le_bytes()),
    );
    let u16path = tmp.write("u16.md", &u16);

    let paths: Vec<&Path> = vec![&utf8bom, &crlf, &tabs, &u16path];
    let p = ImportPlan::from_paths(&paths);
    let bad: Vec<String> = p
        .failures
        .iter()
        .map(|f| format!("{}: {}", f.label, f.error))
        .collect();
    assert!(bad.is_empty(), "不该有失败: {bad:?}");
    assert_eq!(p.items.len(), 4);
    assert_eq!(p.items[0].title, "有 BOM", "UTF-8 BOM 不得污染首个块判定");
    assert_eq!(p.items[1].title, "窗口");
    assert_eq!(
        p.items[2].kind,
        SourceKind::PlainText,
        "没有任何 markdown 标记 → 纯文本"
    );
    assert_eq!(
        p.items[2].title, "tabs",
        "纯文本没有标题块 → 用文件名（第三档）"
    );
    assert_eq!(p.items[2].title_source, TitleSource::Filename);
    assert!(extract(&p.items[2].doc).plain_text.contains("制表开头"));
    assert_eq!(
        p.items[3].title, "UTF16 标题",
        "带 BOM 的 UTF-16 与 UTF-8 同权"
    );
}

#[test]
fn reading_a_missing_or_oversize_file_is_a_typed_error() {
    let tmp = Tmp::new("typed");
    let missing = tmp.dir().join("nope.md");
    assert!(matches!(
        ImportSource::read(&missing),
        Err(ImportError::Io { .. })
    ));
    let big = tmp.write("big.txt", vec![b'x'; MAX_SOURCE_BYTES as usize + 1]);
    match ImportSource::read(&big) {
        Err(ImportError::TooLarge { path, size, limit }) => {
            assert_eq!(path, "big.txt");
            assert_eq!(size, MAX_SOURCE_BYTES + 1);
            assert_eq!(limit, MAX_SOURCE_BYTES);
        }
        other => panic!("实得 {other:?}"),
    }
}

// ---------------------------------------------------------- 写库侧 ---

#[test]
fn apply_is_idempotent_by_content_hash() {
    let fx = Fix::open();
    let p = ImportPlan::build(&[
        source_from_str("a.md", "# 甲\n\n内容 A\n"),
        source_from_str("b.md", "# 乙\n\n内容 B\n"),
        source_from_str("c.md", "# 丙\n\n内容 C\n"),
    ]);
    let first = apply(&fx.store, &fx.folder(), &p).expect("apply 1");
    assert_eq!(first.created_count(), 3);
    assert_eq!(fx.note_count(), 3);
    assert_eq!(fx.revisions(), 3);

    // 同一份计划重放：一条都不许多，而且一条 revision 都不许多（真的什么都没写）
    let second = apply(&fx.store, &fx.folder(), &p).expect("apply 2");
    assert_eq!(second.created_count(), 0, "重放不得新建");
    assert_eq!(second.duplicate_count(), 3);
    assert_eq!(fx.note_count(), 3);
    assert_eq!(fx.revisions(), 3, "幂等重放不该产生任何写入");
    assert!(
        second.duplicates.iter().all(|d| !d.in_plan),
        "撞的是库里的内容，不是计划内部"
    );
    assert!(second.accounts_for(&p));

    // 换文件名重放同一批内容：内容哈希相同 → 仍然幂等（去重键是内容，不是路径）
    let renamed = ImportPlan::build([&source_from_str("完全不同的名字.md", "# 甲\n\n内容 A\n")]);
    let third = apply(&fx.store, &fx.folder(), &renamed).expect("apply 3");
    assert_eq!(
        third.created_count(),
        0,
        "路径改名不产生副本：去重看内容哈希"
    );
    assert_eq!(fx.note_count(), 3);

    // 真正新增一条：只有 1 条增长
    let grown = single_note_plan("# 丁\n\n内容 D\n");
    let fourth = apply(&fx.store, &fx.folder(), &grown).expect("apply 4");
    assert_eq!(fourth.created_count(), 1);
    assert_eq!(fx.note_count(), 4);
    assert_eq!(fx.revisions(), 4);
}

#[test]
fn same_file_given_twice_in_one_plan_creates_one_note() {
    let fx = Fix::open();
    let s = source_from_str("dup.md", "# 重复\n\n同一条目两次\n");
    let p = ImportPlan::build([&s, &s]);
    assert_eq!(p.items.len(), 2, "计划本身不擅自去重（用户要看得见）");
    let r = apply(&fx.store, &fx.folder(), &p).expect("apply");
    assert_eq!(r.created_count(), 1);
    assert_eq!(r.duplicate_count(), 1);
    assert!(r.duplicates[0].in_plan);
    assert_eq!(fx.note_count(), 1);
    assert!(r.accounts_for(&p));
}

#[test]
fn import_never_modifies_an_existing_note() {
    let fx = Fix::open();
    // 用户先手写了两条笔记（一条与要导入的内容哈希相同，一条不同）
    let mine = fx
        .store
        .create_note(
            &fx.default_folder,
            serde_json::to_value(doc_of("x.md", "# 已存在的\n\n别动我\n")).expect("value"),
        )
        .expect("用户笔记");
    let before = fx.store.get_note(&mine.id).expect("读").expect("在");
    let p = single_note_plan("# 已存在的\n\n别动我\n");
    let r = apply(&fx.store, &fx.folder(), &p).expect("apply");
    let after = fx.store.get_note(&mine.id).expect("读").expect("在");
    assert_eq!(r.created_count(), 0);
    assert_eq!(r.duplicate_count(), 1);
    assert_eq!(after.rev, before.rev, "导入绝不推进既有笔记的 rev");
    assert_eq!(
        after.content_hash, before.content_hash,
        "导入绝不改既有笔记的内容"
    );
    assert_eq!(
        after.updated_at, before.updated_at,
        "导入绝不碰既有笔记的时间戳"
    );
    assert_eq!(fx.revisions(), 1, "没有任何新 revision = 一次写入都没发生");
}

#[test]
fn missing_folder_is_refused_and_named_folder_is_created_once() {
    let fx = Fix::open();
    let ghost = EntityId::parse("00000000-0000-0000-0000-000000000001").expect("固定 UUID");
    let p = single_note_plan("# 无处可去\n");
    let e = apply(&fx.store, &FolderTarget::from(ghost.clone()), &p)
        .expect_err("不存在的文件夹必须报错");
    assert!(matches!(e, ImportError::FolderNotFound(_)), "实得 {e:?}");
    assert_eq!(fx.note_count(), 0, "报错时一行都不该写");

    let target = FolderTarget::named("导入进来的", None);
    let r1 = apply(&fx.store, &target, &p).expect("apply");
    assert!(r1.folder_created, "第一次要如实报告：文件夹是本次新建的");
    let r2 = apply(&fx.store, &target, &p).expect("apply 2");
    assert!(!r2.folder_created, "第二次必须复用同一个文件夹，不建第二个");
    assert_eq!(r1.target_folder, r2.target_folder);
    assert_eq!(r2.created_count(), 0);
    assert_eq!(fx.note_count(), 1);
    let same_name = fx
        .store
        .list_folders()
        .expect("folders")
        .into_iter()
        .filter(|f| f.name == "导入进来的")
        .count();
    assert_eq!(same_name, 1, "同名文件夹只能有一个");
    assert_eq!(
        hashes_in_folder(&fx.store, &r1.target_folder)
            .expect("hashes")
            .len(),
        1
    );
}

#[test]
fn deleted_folder_is_not_resurrected() {
    let fx = Fix::open();
    let f = fx.store.create_folder(None, "会被删掉的").expect("建夹");
    fx.store.delete_folder(&f.id).expect("删夹");
    let e = apply(
        &fx.store,
        &FolderTarget::from(f.id.clone()),
        &single_note_plan("# x\n"),
    )
    .expect_err("回收站里的夹不能写");
    assert!(matches!(e, ImportError::FolderNotFound(_)), "实得 {e:?}");
    // 名字复用：同名的旧夹在回收站 → 新建一个，绝不改旧的那行
    let r = apply(
        &fx.store,
        &FolderTarget::named("会被删掉的", None),
        &single_note_plan("# y\n"),
    )
    .expect("apply");
    assert!(r.folder_created);
    assert_ne!(r.target_folder, f.id);
    assert!(fx
        .store
        .list_notes(&NoteQuery::in_folder(&f.id))
        .expect("旧夹列表")
        .is_empty());
}

#[test]
fn derived_title_and_body_reach_the_store_columns() {
    let fx = Fix::open();
    let p = plan_of("post.md", "---\ntitle: 头部来的标题\n---\n\n正文一段\n");
    let item = &p.items[0];
    assert_eq!(item.title_source, TitleSource::FrontMatter);
    let r = apply(&fx.store, &fx.folder(), &p).expect("apply");
    assert_eq!(
        r.created[0].stored_title, "头部来的标题",
        "front-matter 标题要真的落进 notes.title"
    );
    let note = fx
        .store
        .get_note(&r.created[0].note_id)
        .expect("读")
        .expect("在");
    assert!(note.plain_text.contains("正文一段"));
    assert!(note.summary.contains("正文一段"));
    assert_eq!(
        note.content_hash,
        item.content_hash.as_str(),
        "计划里的 hash == Store 算出的 hash"
    );
    assert_eq!(note.doc, serde_json::to_value(&item.doc).expect("value"));
    assert_eq!(note.rev.get(), 1, "新建笔记 rev = 1");
    assert!(fx.store.startup_violations().is_empty(), "库不许自带违约");
}

#[test]
fn one_bad_item_does_not_block_the_others_and_report_sums_up() {
    let fx = Fix::open();
    let mut p = ImportPlan::build(&[
        source_from_str("good.md", "# 好的一条\n"),
        source_from_str("bad.md", "# 太新的\n"),
    ]);
    // 手工把第二条的 doc.v 推到本客户端之上 → Store 的 I7 只读闸门必拒（DocTooNew）
    p.items[1].doc.v = notera_richtext::DOC_FORMAT + 1;

    let r: ApplyReport = apply(&fx.store, &fx.folder(), &p).expect("apply");
    assert_eq!(r.created_count(), 1);
    assert_eq!(r.failed_count(), 1);
    assert!(
        matches!(r.failed[0].error, ImportError::Store(_)),
        "实得 {:?}",
        r.failed[0].error
    );
    assert!(
        r.accounts_for(&p),
        "报告必须能对上账：新增+重复+失败 = 条目数"
    );
    assert_eq!(fx.note_count(), 1, "一条失败不脏整批");
}

#[test]
fn empty_files_are_skipped_with_a_report_not_a_note() {
    let tmp = Tmp::new("empty");
    let e = tmp.write("空.md", b"");
    let ok = tmp.write("有内容.md", "# 有内容\n");
    let p = ImportPlan::from_paths(&[&e, &ok]);
    assert_eq!(p.skipped.len(), 1);
    assert_eq!(p.total(), 2);
    let fx = Fix::open();
    let r = apply(&fx.store, &fx.folder(), &p).expect("apply");
    assert_eq!(r.created_count(), 1);
    assert_eq!(r.plan_skipped, 1);
    assert!(r.accounts_for(&p));
}

#[test]
fn plan_alone_never_writes_to_the_store() {
    let fx = Fix::open();
    let (before, revs) = (fx.note_count(), fx.revisions());
    let p = ImportPlan::build(&[
        source_from_str("x.md", "# 只出计划\n"),
        source_from_str("y.md", "纯文本笔记\n"),
    ]);
    assert_eq!(p.items.len(), 2);
    assert_eq!(
        (fx.note_count(), fx.revisions()),
        (before, revs),
        "构造计划不得碰库（纯函数）"
    );
    let r = apply(&fx.store, &fx.folder(), &p).expect("apply");
    assert_eq!(r.created_count(), 2);
    assert_eq!(fx.note_count(), before + 2);
}

#[test]
fn importer_output_is_readable_by_the_richtext_gate() {
    // 验收标准不是"我觉得合法"，而是 richtext 自己说合法：parse() 零错误 + validate 通过。
    for src_text in [
        "# H1\n## H2\n\n段落\n\n- 列表\n\n> 引用\n\n```\ncode\n```\n\n---\n",
        "![x](y.png)\n[a](b)\n***z***\n\n1. 一\n   - [ ] 嵌套任务\n",
        "\ttab 缩进\n中文 emoji 😀\n",
    ] {
        let doc = doc_of("gate.md", src_text);
        let value = serde_json::to_value(&doc).expect("可序列化");
        let parsed = parse_from_value(&value).expect("必须过 parse");
        notera_richtext::validate(&parsed).expect("必须过 validate");
        assert!(!parsed.content.is_empty());
    }
}

/// 大批量（500 篇）跑一遍：幂等 + 无重复 id + 无 panic。
#[test]
fn bulk_import_of_five_hundred_files_is_idempotent() {
    let fx = Fix::open();
    let sources: Vec<ImportSource> = (0..500)
        .map(|i| {
            source_from_str(
                &format!("n{i}.md"),
                &format!("# 笔记 {i}\n\n第 {i} 篇的正文 *强调*\n"),
            )
        })
        .collect();
    let p = ImportPlan::build(&sources);
    assert_eq!(p.items.len(), 500);
    let r1 = apply(&fx.store, &fx.folder(), &p).expect("apply 1");
    assert_eq!(r1.created_count(), 500);
    let r2 = apply(&fx.store, &fx.folder(), &p).expect("apply 2");
    assert_eq!(r2.created_count(), 0);
    assert_eq!(r2.duplicate_count(), 500);
    assert_eq!(fx.note_count(), 500);
    assert_eq!(
        hashes_in_folder(&fx.store, &fx.default_folder)
            .expect("hashes")
            .len(),
        500
    );
}

#[test]
fn block_id_derivation_is_stable_across_two_builds() {
    let a = doc_of("stable.md", "# 稳定\n\n段落\n");
    let b = doc_of("改名也不影响.md", "# 稳定\n\n段落\n");
    assert_eq!(
        canonical(&a),
        canonical(&b),
        "内容相同 → 文档逐字节相同（id = 源哈希短值 + 序号）"
    );
    for x in &a.content {
        assert!(x.id.starts_with("in-"), "id 形态: {}", x.id);
    }
}

#[test]
fn markdown_and_plain_text_produce_the_expected_block_types() {
    let d = doc_of(
        "t.md",
        "# 标题\n\n段落文字\n\n```sh\nls\n```\n\n- 无序\n1. 有序\n- [x] 任务\n\n> 引用\n\n---\n",
    );
    let types: Vec<BlockType> = d.content.iter().map(|b| b.type_.clone()).collect();
    assert_eq!(
        types,
        vec![
            BlockType::Heading,
            BlockType::Paragraph,
            BlockType::CodeBlock,
            BlockType::BulletList,
            BlockType::OrderedList,
            BlockType::ChecklistItem,
            BlockType::BlockQuote,
            BlockType::HorizontalRule,
        ]
    );
    let t = doc_of("t.txt", "只是文字\n也是文字\n");
    assert_eq!(
        t.content
            .iter()
            .map(|b| b.type_.clone())
            .collect::<Vec<_>>(),
        vec![BlockType::Paragraph, BlockType::Paragraph]
    );
    assert_eq!(t.content[0].plain_text(), "只是文字");
}

#[test]
fn markdown_in_a_txt_file_is_imported_as_markdown_not_rejected() {
    // 扩展名从不构成拒绝理由：.txt 里装 markdown 就按 markdown 走。
    let d = doc_of("t.txt", "# 被嗅探出来了\n\n- 项目\n");
    assert_eq!(
        d.content
            .iter()
            .map(|b| b.type_.clone())
            .collect::<Vec<_>>(),
        vec![BlockType::Heading, BlockType::BulletList]
    );
    // 同一段内容换个 .md 名字，产物逐字节相同（差别只在探测，不在结果）
    assert_eq!(
        canonical(&d),
        canonical(&doc_of("t.md", "# 被嗅探出来了\n\n- 项目\n"))
    );
}

#[test]
fn plain_text_kind_keeps_markup_literal() {
    let s = source_from_str(
        "calc.txt",
        "2 * 3 = 6 与 **两星** 与 `反引号` 与 #井号\n第二行\n",
    );
    assert_eq!(
        s.kind,
        SourceKind::PlainText,
        "没有任何块级标记 + 只有一个行内标记 → 纯文本"
    );
    let doc = document_for(&s).expect("doc");
    assert_eq!(
        extract(&doc).plain_text,
        "2 * 3 = 6 与 **两星** 与 `反引号` 与 #井号\n第二行",
        "纯文本模式下标记一个都不解释"
    );
    assert!(doc.content.iter().all(|b| b.type_ == BlockType::Paragraph));
    assert!(doc
        .content
        .iter()
        .all(|b| b.content.iter().all(|i| i.marks.is_empty())));
}

/// 跨重启：导入后重开库，再重放同一份计划，仍然一条都不许多。
#[test]
fn idempotence_survives_a_reopen() {
    let tmp = Tmp::new("reopen");
    let p = single_note_plan("# 重启还在\n\n内容\n");
    {
        let store = Store::open(&tmp.dir().join("db"), DeviceId::new()).expect("open");
        let folder = store
            .list_folders()
            .expect("f")
            .into_iter()
            .find(|f| f.system_kind.as_deref() == Some("default"))
            .expect("默认本")
            .id;
        assert_eq!(
            apply(&store, &FolderTarget::from(folder.clone()), &p)
                .expect("1")
                .created_count(),
            1
        );
    }
    let store = Store::open(&tmp.dir().join("db"), DeviceId::new()).expect("reopen");
    let folder = store
        .list_folders()
        .expect("f")
        .into_iter()
        .find(|f| f.system_kind.as_deref() == Some("default"))
        .expect("默认本")
        .id;
    let r = apply(&store, &FolderTarget::from(folder.clone()), &p).expect("2");
    assert_eq!(
        r.created_count(),
        0,
        "重开库后仍要幂等（去重键在库里，不在内存里）"
    );
    assert_eq!(
        store
            .list_notes(&NoteQuery::in_folder(&folder))
            .expect("列表")
            .len(),
        1
    );
}

// ------------------------------------------------ .enex（Evernote 导入）---

/// 真 .enex 的形状：正文是**一整段 CDATA 包起来的 ENML**，附件 base64 内嵌在
/// `<resource>` 里，编码写在 `<data encoding="base64">` 的**属性**上。
fn enex_fixture() -> String {
    use base64::Engine as _;
    let png = base64::engine::general_purpose::STANDARD.encode([0x89u8, b'P', b'N', b'G', 7, 7]);
    let enml = "<en-note><div>甲</div><h2>小节</h2><en-media hash=\"AA11\" type=\"image/png\" />\
<table><tbody><tr><td>左</td><td>右</td></tr></tbody></table></en-note>";
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<en-export version=\"6.5.1\">\n\
<note><title>报销单</title><content><![CDATA[{enml}]]></content>\
<created>20230102T030405Z</created><tag>财务</tag>\
<resource><data encoding=\"base64\">{png}</data><mime>image/png</mime>\
<resource-attributes><file-name>shot.png</file-name></resource-attributes>\
<md5>AA11</md5></resource></note>\n\
<note><title>第二条</title><content><![CDATA[<en-note><div>只有正文</div></en-note>]]></content></note>\n\
</en-export>"
    )
}

#[test]
fn an_enex_imports_as_several_notes_with_attachments_into_a_real_store() {
    let fix = Fix::open();
    let enex = enex_fixture();
    let src = source_from_str("我的笔记.enex", &enex);
    assert_eq!(src.kind, SourceKind::Enex, "扩展名要把它路由到 enex 那条路");

    let p = plan(&[src]);
    assert!(
        p.failures.is_empty(),
        "不该有失败条目：{:?}",
        p.failures
            .iter()
            .map(|f| f.error.to_string())
            .collect::<Vec<_>>()
    );
    assert_eq!(p.items.len(), 2, "一份文件两条笔记");
    assert!(!p.items[0].blobs.is_empty(), "附件字节要跟着条目走");

    let r = apply(&fix.store, &fix.folder(), &p).expect("apply");
    assert_eq!(
        r.created.len(),
        2,
        "{:?}",
        r.created.iter().map(|c| &c.label).collect::<Vec<_>>()
    );
    assert_eq!(
        r.created[0].stored_title, "报销单",
        "notes.title 是派生列：Evernote 的标题必须真的落进去，不然列表里看到的是正文第一行"
    );

    // 附件那条边：字节写盘 + 行登记成可用 + 被这条笔记引用（计数 0 的 blob 会被 GC 删）
    let sha = &p.items[0].blobs[0].0;
    let (state, _media) = fix.store.attachment_for_state(sha);
    assert_eq!(
        state, "available",
        "导入的附件该是可用的，不是占位：{state}"
    );
    assert!(
        fix.store.attachment_refs(sha).expect("refs") >= 1,
        "doc 里的 image 块必须被存储层认出来并建引用"
    );

    // 本库表达不了的字段：点名给用户，不静默吞
    assert!(
        r.notices.iter().any(|s| s.contains("tag")),
        "Evernote 的 <tag> 没落地必须报出来：{:?}",
        r.notices
    );
    assert!(
        r.notices.iter().any(|s| s.contains("table")),
        "表格未映射要点名：{:?}",
        r.notices
    );

    // 幂等：同一份文件再导一次，一条都不新建
    let again = source_from_str("我的笔记.enex", &enex);
    let r2 = apply(&fix.store, &fix.folder(), &plan(&[again])).expect("replay apply");
    assert!(r2.created.is_empty(), "重放不该新建：{:?}", r2.created);
    assert_eq!(r2.duplicates.len(), 2, "两条都该被判成重复");
    assert_eq!(fix.note_count(), 2, "库里总数不变");
}

#[test]
fn enex_goes_through_the_same_size_gate_as_other_sources() {
    // 这条钉的是"闸门不因新格式而绕行"：.enex 一样受 MAX_SOURCE_BYTES 约束。
    // 真实 Evernote 导出很容易超过它 —— 那必须是一条看得见的失败，不是截断后静静导入。
    let big = "x".repeat(MAX_SOURCE_BYTES as usize + 1024);
    let src = ImportSource::from_bytes(Some(Path::new("大导出.enex")), big.as_bytes());
    // 两种都可接受：读盘阶段就拒，或计划阶段留下一条失败。唯独不接受"截断后继续导"。
    if let Ok(s) = src {
        let p = plan(&[s]);
        assert_eq!(
            p.failures.len(),
            1,
            "超限必须留下一条失败，而不是悄悄导一半：items={}",
            p.items.len()
        );
        assert!(p.items.is_empty(), "超限的文件不该产出任何笔记");
    }
}
