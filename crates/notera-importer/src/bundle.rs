//! 导出 / 导入的自描述 ZIP（DATA-MODEL §15）。
//!
//! 布局刻意保持"人能直接看懂"：
//!
//! ```text
//! manifest.json      格式号、协议号、导出时间、各类计数
//! folders.json       文件夹信封数组
//! notes/<id>.json    一条笔记一个信封（与上传时的记录逐字同构）
//! tombstones.json    永久删除的公告记录（防"导出→清空→导入"复活）
//! attachments/<sha>  内容寻址的附件字节
//! ```
//!
//! 读回来时**不信文件名**：附件字节必须自己算一遍 sha256 与文件名对上才算数。

use crate::error::ImportError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::Path;

/// bundle 结构版本。与同步协议号是两回事：前者是"这个包怎么排版"，后者是"记录怎么编码"。
pub const BUNDLE_FORMAT: u32 = 1;
pub const MAX_PROTOCOL: u64 = 1;

const MANIFEST: &str = "manifest.json";
const FOLDERS: &str = "folders.json";
const TOMBSTONES: &str = "tombstones.json";
const NOTES_PREFIX: &str = "notes/";
const ATTACH_PREFIX: &str = "attachments/";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub format: u32,
    pub protocol: u64,
    pub exported_at: String,
    pub app_version: String,
    pub root_id: Option<String>,
    pub counts: BTreeMap<String, usize>,
    /// 这个包**不是**整库：按文件夹导出时只含子树 + 祖先链，永久删除的笔记公告无法归属到
    /// 文件夹，因此不在包里。缺 `default` 是为老包留的兼容位 —— 已经导出给用户的包必须
    /// 还能读回来，否则"升级导致旧备份不可恢复"就是我们自己制造的数据丢失。
    #[serde(default)]
    pub partial: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bundle {
    pub manifest: Option<Manifest>,
    pub folders: Vec<serde_json::Value>,
    pub notes: Vec<serde_json::Value>,
    pub tombstones: Vec<serde_json::Value>,
    /// (文件名里写的 sha, 真实字节)。读回时 sha 已重新核算过。
    pub attachments: Vec<(String, Vec<u8>)>,
}

impl Bundle {
    /// 落库顺序：文件夹先于笔记（笔记有外键指向文件夹），墓碑最后。
    pub fn records_in_apply_order(&self) -> Vec<serde_json::Value> {
        let mut out = Vec::new();
        out.extend(self.folders.iter().cloned());
        out.extend(self.notes.iter().cloned());
        out.extend(self.tombstones.iter().cloned());
        out
    }
}

fn write_options() -> zip::write::FileOptions<'static, ()> {
    zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated)
}

pub fn write_bundle(path: &Path, bundle: &Bundle) -> Result<(), ImportError> {
    let file = std::fs::File::create(path).map_err(|e| ImportError::Io { path: path.display().to_string(), source: e })?;
    let mut zip = zip::ZipWriter::new(file);
    let opts = write_options();

    let manifest = bundle.manifest.clone().unwrap_or(Manifest {
        format: BUNDLE_FORMAT,
        protocol: 1,
        exported_at: String::new(),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        root_id: None,
        counts: BTreeMap::new(),
        // 调用方没给 manifest 时合成的这份 = 整库，不是子树
        partial: false,
    });
    write_json(&mut zip, &opts, MANIFEST, &manifest)?;
    write_json(&mut zip, &opts, FOLDERS, &bundle.folders)?;
    write_json(&mut zip, &opts, TOMBSTONES, &bundle.tombstones)?;
    for note in &bundle.notes {
        let id = note_id(note)?;
        write_json(&mut zip, &opts, &format!("{NOTES_PREFIX}{id}.json"), note)?;
    }
    for (sha, bytes) in &bundle.attachments {
        zip.start_file(format!("{ATTACH_PREFIX}{sha}"), opts)
            .map_err(|e| ImportError::Bundle(e.to_string()))?;
        zip.write_all(bytes).map_err(|e| ImportError::Bundle(e.to_string()))?;
    }
    zip.finish().map_err(|e| ImportError::Bundle(e.to_string()))?;
    Ok(())
}

fn write_json<T: Serialize>(
    zip: &mut zip::ZipWriter<std::fs::File>,
    opts: &zip::write::FileOptions<'_, ()>,
    name: &str,
    value: &T,
) -> Result<(), ImportError> {
    let body = serde_json::to_vec_pretty(value).map_err(|e| ImportError::Bundle(e.to_string()))?;
    zip.start_file(name, *opts).map_err(|e| ImportError::Bundle(e.to_string()))?;
    zip.write_all(&body).map_err(|e| ImportError::Bundle(e.to_string()))?;
    Ok(())
}

pub fn read_bundle(path: &Path) -> Result<Bundle, ImportError> {
    let file = std::fs::File::open(path).map_err(|e| ImportError::Io { path: path.display().to_string(), source: e })?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| ImportError::Bundle(format!("不是可读的 ZIP: {e}")))?;
    let mut bundle = Bundle::default();
    let mut raw_attachments: BTreeMap<String, Vec<u8>> = BTreeMap::new();

    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).map_err(|e| ImportError::Bundle(e.to_string()))?;
        let name = entry.name().to_string();
        let mut buf = Vec::new();
        entry.read_to_end(&mut buf).map_err(|e| ImportError::Bundle(e.to_string()))?;
        match name.as_str() {
            MANIFEST => {
                bundle.manifest = Some(serde_json::from_slice(&buf).map_err(|e| ImportError::Bundle(format!("manifest.json 读不懂: {e}")))?);
            }
            FOLDERS => bundle.folders = read_array(&name, &buf)?,
            TOMBSTONES => bundle.tombstones = read_array(&name, &buf)?,
            _ if name.starts_with(NOTES_PREFIX) => bundle.notes.push(read_value(&name, &buf)?),
            _ if name.starts_with(ATTACH_PREFIX) => {
                let sha = name.trim_start_matches(ATTACH_PREFIX);
                raw_attachments.insert(sha.to_string(), buf);
            }
            _ => return Err(ImportError::Bundle(format!("包里有意料之外的条目: {name}"))),
        }
    }

    let manifest = bundle
        .manifest
        .clone()
        .ok_or_else(|| ImportError::Bundle("缺少 manifest.json：无法判断这是什么包".to_string()))?;
    if manifest.format != BUNDLE_FORMAT {
        return Err(ImportError::Bundle(format!("bundle 格式号 {} 不认识（本程序写的是 {BUNDLE_FORMAT}）", manifest.format)));
    }
    if manifest.protocol > MAX_PROTOCOL {
        return Err(ImportError::Bundle(format!("协议号 {} 高于本程序支持 {MAX_PROTOCOL}", manifest.protocol)));
    }

    // 附件按内容寻址：文件名说的 sha 必须等于字节自己算出来的，否则整包作废。
    for (sha, bytes) in &raw_attachments {
        let got = notera_crypto::sha256_hex(bytes);
        if &got != sha {
            return Err(ImportError::Bundle(format!("附件 {sha} 内容与文件名不符（实际 {got}）")));
        }
    }
    bundle.attachments = raw_attachments.into_iter().collect();
    Ok(bundle)
}

fn read_array(name: &str, buf: &[u8]) -> Result<Vec<serde_json::Value>, ImportError> {
    if buf.is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_slice(buf).map_err(|e| ImportError::Bundle(format!("{name} 读不懂: {e}")))
}

fn read_value(name: &str, buf: &[u8]) -> Result<serde_json::Value, ImportError> {
    serde_json::from_slice(buf).map_err(|e| ImportError::Bundle(format!("{name} 读不懂: {e}")))
}

fn note_id(note: &serde_json::Value) -> Result<String, ImportError> {
    note.get("id")
        .and_then(|i| i.as_str())
        .map(|i| i.to_string())
        .ok_or_else(|| ImportError::Bundle("笔记信封缺少 id，无法决定文件名".to_string()))
}
