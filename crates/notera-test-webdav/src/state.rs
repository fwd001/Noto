//! 权威状态：`{ path → 资源 }` 一张表 + 两种后端（内存 / 磁盘）。
//!
//! ETag = 内容 sha256（强 ETag），所以 `RESTART` 后 ETag 天然稳定；
//! Fs 模式下 `restart()` 会**丢弃内存表并从磁盘重建**，于是"重启后数据仍在"
//! 是真的磁盘事实而不是"其实没重启"（docs/TEST-PLAN.md SY-INT-07）。

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// 后端选择。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BackendKind {
    Mem,
    Fs(PathBuf),
}

/// 一个资源（文件或集合）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    pub is_dir: bool,
    pub bytes: Vec<u8>,
    /// PROPPATCH 存进来的自定义属性。
    pub props: Vec<(String, String)>,
}

impl Node {
    pub fn file(bytes: Vec<u8>) -> Node {
        Node {
            is_dir: false,
            bytes,
            props: Vec::new(),
        }
    }
    pub fn dir() -> Node {
        Node {
            is_dir: true,
            bytes: Vec::new(),
            props: Vec::new(),
        }
    }
    /// 强 ETag：内容 sha256，带引号，无 `W/` 前缀。
    pub fn etag(&self) -> String {
        format!("\"{}\"", sha256_hex(&self.bytes))
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex_lower(&h.finalize())
}

/// 权威形态 `sha256:<64hex>`（与 `notera_core::ContentHash` 一致）。
pub fn sha256_of(bytes: &[u8]) -> String {
    format!("sha256:{}", sha256_hex(bytes))
}

fn hex_lower(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// `/_fs/dump` 的一条。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DumpEntry {
    pub path: String,
    pub is_dir: bool,
    pub bytes: u64,
    /// `sha256:<64hex>`；集合为空串。
    pub sha256: String,
    /// 强 ETag；集合为空串。
    pub etag: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MkdirError {
    Exists,
    NoParent,
}

pub struct Store {
    backend: BackendKind,
    nodes: BTreeMap<String, Node>,
}

/// 路径规范化：拒绝穿越 / 反斜杠 / 空字节；折叠 `//` 与 `.`。
pub fn normalize(path: &str) -> Option<String> {
    if path.is_empty() || path.contains('\0') || path.contains('\\') {
        return None;
    }
    let mut segs: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => continue,
            ".." => return None,
            _ => segs.push(seg),
        }
    }
    Some(format!("/{}", segs.join("/")))
}

/// 父路径；`/a` → `None`（根没有父）。
pub fn parent(path: &str) -> Option<String> {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(i) if i > 0 => normalize(&trimmed[..i]),
        _ => None,
    }
}

pub fn is_descendant(path: &str, dir: &str) -> bool {
    let base = dir.trim_end_matches('/');
    if base.is_empty() {
        return path != "/";
    }
    path.starts_with(&format!("{base}/"))
}

/// `p`（在 `from` 子树内）映射到 `to` 子树内的等价路径。
pub fn rebase(p: &str, from: &str, to: &str) -> String {
    if p == from {
        return to.to_string();
    }
    match p.strip_prefix(from.trim_end_matches('/')) {
        Some(rest) => format!(
            "{}/{}",
            to.trim_end_matches('/'),
            rest.trim_start_matches('/')
        ),
        None => p.to_string(),
    }
}

impl Store {
    pub fn open(backend: BackendKind) -> io::Result<Store> {
        let mut store = Store {
            backend,
            nodes: BTreeMap::new(),
        };
        store.nodes.insert("/".into(), Node::dir());
        if let BackendKind::Fs(dir) = &store.backend {
            std::fs::create_dir_all(dir)?;
            store.reload()?;
        }
        Ok(store)
    }

    #[allow(dead_code)]
    pub fn backend(&self) -> &BackendKind {
        &self.backend
    }

    pub fn fs_root(&self) -> Option<&Path> {
        match &self.backend {
            BackendKind::Mem => None,
            BackendKind::Fs(p) => Some(p.as_path()),
        }
    }

    /// 以磁盘为准重建内存表（Fs 模式）；Mem 模式无操作。
    pub fn reload(&mut self) -> io::Result<()> {
        let Some(dir) = self.fs_root().map(|p| p.to_path_buf()) else {
            return Ok(());
        };
        std::fs::create_dir_all(&dir)?;
        self.nodes.clear();
        self.nodes.insert("/".into(), Node::dir());
        walk(&dir, &dir, &mut self.nodes)?;
        self.load_props();
        Ok(())
    }

    pub fn get(&self, path: &str) -> Option<&Node> {
        self.nodes.get(path)
    }
    #[allow(dead_code)]
    pub fn get_mut(&mut self, path: &str) -> Option<&mut Node> {
        self.nodes.get_mut(path)
    }
    pub fn contains(&self, path: &str) -> bool {
        self.nodes.contains_key(path)
    }
    /// 资源总数（含根集合）。
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }
    /// 除根之外是否无任何资源。
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.nodes.len() <= 1
    }

    /// 直接子资源。
    pub fn children(&self, dir: &str) -> Vec<String> {
        let base = dir.trim_end_matches('/');
        let mut out = Vec::new();
        for path in self.nodes.keys() {
            if path == dir || !is_descendant(path, dir) {
                continue;
            }
            let rest = path.strip_prefix(base).unwrap_or(path);
            if !rest[1..].contains('/') {
                out.push(path.clone());
            }
        }
        out.sort();
        out
    }

    pub fn descendants(&self, dir: &str) -> Vec<String> {
        let mut out: Vec<String> = self
            .nodes
            .keys()
            .filter(|p| is_descendant(p, dir))
            .cloned()
            .collect();
        out.sort();
        out
    }

    #[allow(dead_code)]
    pub fn all_paths(&self) -> Vec<String> {
        self.nodes.keys().cloned().collect()
    }

    /// PUT：自动补齐父集合（多数真实服务器允许隐式建目录）。
    pub fn write_file(&mut self, path: &str, bytes: Vec<u8>) -> io::Result<()> {
        if self.nodes.get(path).map(|n| n.is_dir).unwrap_or(false) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "目标是集合"));
        }
        if let Some(p) = parent(path) {
            self.makedirs(&p)?;
        }
        let props = self.nodes.get(path).map(|n| n.props.clone()).unwrap_or_default();
        self.nodes
            .insert(path.to_string(), Node { is_dir: false, bytes, props });
        self.persist(path)
    }

    /// MKCOL：严格 —— 已存在 → `Exists`，父不存在 → `NoParent`（RFC 4918 §9.3）。
    pub fn mkdir(&mut self, path: &str) -> Result<Result<(), MkdirError>, io::Error> {
        if self.contains(path) {
            return Ok(Err(MkdirError::Exists));
        }
        let par = parent(path).unwrap_or_else(|| "/".into());
        if !self.contains(&par) {
            return Ok(Err(MkdirError::NoParent));
        }
        self.nodes.insert(path.to_string(), Node::dir());
        if let Err(e) = self.persist_dir(path) {
            self.nodes.remove(path);
            return Err(e);
        }
        Ok(Ok(()))
    }

    /// 隐式建集合（PUT/MOVE/COPY 用）。
    pub fn makedirs(&mut self, path: &str) -> io::Result<()> {
        if path == "/" || self.contains(path) {
            return Ok(());
        }
        if let Some(p) = parent(path) {
            if !self.contains(&p) {
                self.makedirs(&p)?;
            }
        }
        self.nodes.insert(path.to_string(), Node::dir());
        self.persist_dir(path)
    }

    /// DELETE：集合级联删除子孙。
    pub fn delete(&mut self, path: &str) -> bool {
        if !self.contains(path) {
            return false;
        }
        let doomed: Vec<String> = if self.nodes.get(path).map(|n| n.is_dir).unwrap_or(false) {
            let mut v = self.descendants(path);
            v.push(path.to_string());
            v
        } else {
            vec![path.to_string()]
        };
        for p in &doomed {
            self.nodes.remove(p);
            self.unpersist(p);
        }
        true
    }

    /// MOVE：文件改名或整棵子树改名；返回是否真删除了源。
    pub fn rename(&mut self, from: &str, to: &str) -> io::Result<()> {
        let Some(src) = self.nodes.get(from).cloned() else {
            return Ok(());
        };
        let mut moved: Vec<(String, Node)> = Vec::new();
        if src.is_dir {
            let set: Vec<String> = self
                .nodes
                .keys()
                .filter(|p| **p == from || is_descendant(p, from))
                .cloned()
                .collect();
            for p in set {
                if let Some(node) = self.nodes.remove(&p) {
                    self.unpersist(&p);
                    moved.push((rebase(&p, from, to), node));
                }
            }
        } else {
            self.nodes.remove(from);
            self.unpersist(from);
            moved.push((to.to_string(), src));
        }
        self.materialize(&moved)
    }

    /// COPY：与 MOVE 同样处理子树，但保留源。
    pub fn copy(&mut self, from: &str, to: &str) -> io::Result<()> {
        let Some(src) = self.nodes.get(from).cloned() else {
            return Ok(());
        };
        let mut copies: Vec<(String, Node)> = Vec::new();
        if src.is_dir {
            let set: Vec<String> = self
                .nodes
                .keys()
                .filter(|p| **p == from || is_descendant(p, from))
                .cloned()
                .collect();
            for p in set {
                if let Some(node) = self.nodes.get(&p) {
                    copies.push((rebase(&p, from, to), node.clone()));
                }
            }
        } else {
            copies.push((to.to_string(), src));
        }
        self.materialize(&copies)
    }

    fn materialize(&mut self, moved: &[(String, Node)]) -> io::Result<()> {
        // 最深路径的祖先集合必须存在（否则 Fs 下 write 会失败）。
        if let Some(deepest) = moved.iter().map(|(p, _)| p.clone()).max_by_key(|p| p.len()) {
            if let Some(p) = parent(&deepest) {
                self.makedirs(&p)?;
            }
        }
        for (p, node) in moved {
            self.nodes.insert(p.clone(), node.clone());
            self.persist(p)?;
        }
        Ok(())
    }

    pub fn set_props(&mut self, path: &str, props: Vec<(String, String)>) -> io::Result<()> {
        if props.is_empty() {
            return Ok(());
        }
        {
            let Some(node) = self.nodes.get_mut(path) else {
                return Ok(());
            };
            for (k, v) in props {
                match node.props.iter_mut().find(|(ek, _)| *ek == k) {
                    Some(slot) => slot.1 = v,
                    None => node.props.push((k, v)),
                }
            }
            node.props.sort();
        }
        if let Some(dir) = self.fs_root().map(|p| p.to_path_buf()) {
            self.persist_props(&dir)?;
        }
        Ok(())
    }

    pub fn remove_props(&mut self, path: &str, names: &[String]) -> io::Result<()> {
        if names.is_empty() {
            return Ok(());
        }
        {
            let Some(node) = self.nodes.get_mut(path) else {
                return Ok(());
            };
            node.props.retain(|(k, _)| !names.contains(k));
        }
        if let Some(dir) = self.fs_root().map(|p| p.to_path_buf()) {
            self.persist_props(&dir)?;
        }
        Ok(())
    }

    fn persist(&mut self, path: &str) -> io::Result<()> {
        let Some(node) = self.nodes.get(path).cloned() else {
            return Ok(());
        };
        if node.is_dir {
            return self.persist_dir(path);
        }
        let Some(dir) = self.fs_root().map(|p| p.to_path_buf()) else {
            return Ok(());
        };
        let target = disk_path(&dir, path);
        if let Some(par) = target.parent() {
            std::fs::create_dir_all(par)?;
        }
        std::fs::write(&target, &node.bytes)?;
        Ok(())
    }

    fn persist_dir(&self, path: &str) -> io::Result<()> {
        let Some(dir) = self.fs_root() else {
            return Ok(());
        };
        std::fs::create_dir_all(disk_path(dir, path))
    }

    fn unpersist(&mut self, path: &str) {
        let Some(dir) = self.fs_root().map(|p| p.to_path_buf()) else {
            return;
        };
        let target = disk_path(&dir, path);
        if target.is_dir() {
            let _ = std::fs::remove_dir_all(&target);
        } else {
            let _ = std::fs::remove_file(&target);
        }
    }

    /// PROPPATCH 属性存 sidecar（不进 dump，避免污染协议状态）。
    fn persist_props(&self, dir: &Path) -> io::Result<()> {
        let mut map = serde_json::Map::new();
        for (p, n) in &self.nodes {
            if !n.props.is_empty() {
                let mut obj = serde_json::Map::new();
                for (k, v) in &n.props {
                    obj.insert(k.clone(), serde_json::Value::String(v.clone()));
                }
                map.insert(p.clone(), serde_json::Value::Object(obj));
            }
        }
        let body = serde_json::to_vec_pretty(&serde_json::Value::Object(map))
            .unwrap_or_else(|_| b"{}".to_vec());
        std::fs::write(dir.join(".test-webdav-props.json"), body)
    }

    fn load_props(&mut self) {
        let Some(dir) = self.fs_root().map(|p| p.to_path_buf()) else {
            return;
        };
        let Ok(text) = std::fs::read_to_string(dir.join(".test-webdav-props.json")) else {
            return;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
            return;
        };
        let Some(obj) = v.as_object() else { return };
        for (path, props) in obj {
            if let Some(node) = self.nodes.get_mut(path) {
                if let Some(m) = props.as_object() {
                    node.props = m
                        .iter()
                        .map(|(k, val)| (k.clone(), val.as_str().unwrap_or_default().to_string()))
                        .collect();
                    node.props.sort();
                }
            }
        }
    }

    /// `RESET` / `reset_after`：清成空 root。
    pub fn clear(&mut self) -> io::Result<()> {
        if let Some(dir) = self.fs_root().map(|p| p.to_path_buf()) {
            if dir.exists() {
                std::fs::remove_dir_all(&dir)?;
            }
            std::fs::create_dir_all(&dir)?;
        }
        self.nodes.clear();
        self.nodes.insert("/".into(), Node::dir());
        Ok(())
    }

    pub fn dump(&self, prefix: Option<&str>) -> Vec<DumpEntry> {
        let pfx = prefix.and_then(normalize);
        let mut out = Vec::new();
        for (path, node) in &self.nodes {
            if path == "/" {
                continue;
            }
            if let Some(p) = &pfx {
                if path != p && !path.starts_with(&format!("{p}/")) {
                    continue;
                }
            }
            out.push(DumpEntry {
                path: path.clone(),
                is_dir: node.is_dir,
                bytes: if node.is_dir { 0 } else { node.bytes.len() as u64 },
                sha256: if node.is_dir {
                    String::new()
                } else {
                    sha256_of(&node.bytes)
                },
                etag: if node.is_dir {
                    String::new()
                } else {
                    node.etag()
                },
            });
        }
        out
    }
}

fn disk_path(root: &Path, path: &str) -> PathBuf {
    let mut p = root.to_path_buf();
    for seg in path.split('/').filter(|s| !s.is_empty()) {
        p.push(seg);
    }
    p
}

fn walk(root: &Path, dir: &Path, nodes: &mut BTreeMap<String, Node>) -> io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let absolute = entry.path();
        let file_type = entry.file_type()?;
        let rel = absolute
            .strip_prefix(root)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "路径不在 root 下"))?;
        let Some(vpath) = normalize(&format!("/{}", rel.to_string_lossy().replace('\\', "/")))
        else {
            continue;
        };
        if vpath.ends_with(".test-webdav-props.json") {
            continue;
        }
        if file_type.is_dir() {
            nodes.insert(vpath, Node::dir());
            walk(root, &absolute, nodes)?;
        } else {
            nodes.insert(vpath, Node::file(std::fs::read(&absolute)?));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_rejects_traversal() {
        assert_eq!(normalize("/a/../../b"), None);
        assert_eq!(normalize("/a\\b"), None);
        assert_eq!(normalize("/a/./b/"), Some("/a/b".into()));
        assert_eq!(normalize("/"), Some("/".into()));
        assert_eq!(parent("/a/b.json"), Some("/a".into()));
        assert_eq!(parent("/a"), None);
        assert!(is_descendant("/a/b", "/a"));
        assert!(!is_descendant("/ab", "/a"));
        assert_eq!(rebase("/m/n/o.json", "/m", "/z"), "/z/n/o.json");
    }

    #[test]
    fn etag_is_content_derived() {
        // FIPS 180-4 已知向量 sha256("abc")。
        let n = Node::file(b"abc".to_vec());
        assert_eq!(
            n.etag(),
            "\"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad\""
        );
    }

    #[test]
    fn put_auto_creates_parents_but_mkcol_is_strict() {
        let mut s = Store::open(BackendKind::Mem).unwrap();
        s.write_file("/.notes/records/note/a.json", b"x".to_vec())
            .unwrap();
        assert!(s.contains("/.notes"));
        assert!(s.contains("/.notes/records"));
        // 已存在 → Exists；父不存在 → NoParent。
        assert!(matches!(s.mkdir("/.notes/records"), Ok(Err(MkdirError::Exists))));
        assert!(matches!(s.mkdir("/deep/x/y"), Ok(Err(MkdirError::NoParent))));
        assert!(matches!(s.mkdir("/.notes/locks"), Ok(Ok(()))));
    }

    #[test]
    fn children_and_descendants() {
        let mut s = Store::open(BackendKind::Mem).unwrap();
        s.write_file("/.notes/records/note/a.json", b"x".to_vec())
            .unwrap();
        s.write_file("/.notes/manifest/index.json", b"y".to_vec())
            .unwrap();
        assert_eq!(
            s.children("/.notes"),
            vec!["/.notes/manifest", "/.notes/records"]
        );
        assert_eq!(s.descendants("/.notes").len(), 5);
        assert_eq!(s.children("/.notes/records"), vec!["/.notes/records/note"]);
    }

    #[test]
    fn delete_dir_cascades_but_parents_survive() {
        let mut s = Store::open(BackendKind::Mem).unwrap();
        s.write_file("/a/b/c.json", b"x".to_vec()).unwrap();
        assert!(s.delete("/a"));
        assert!(!s.contains("/a/b/c.json"));
        assert!(s.contains("/"));
    }

    #[test]
    fn move_file_and_subtree() {
        let mut s = Store::open(BackendKind::Mem).unwrap();
        s.write_file("/t/a.json", b"x".to_vec()).unwrap();
        s.rename("/t/a.json", "/r/a.json").unwrap();
        assert!(!s.contains("/t/a.json"));
        assert!(s.contains("/r/a.json"));
        s.write_file("/m/n/o.json", b"y".to_vec()).unwrap();
        s.rename("/m", "/z").unwrap();
        assert!(s.contains("/z/n/o.json"));
        assert!(!s.contains("/m/n"));
    }

    #[test]
    fn copy_keeps_source_and_etag_matches() {
        let mut s = Store::open(BackendKind::Mem).unwrap();
        s.write_file("/a/x.bin", b"hello".to_vec()).unwrap();
        s.copy("/a/x.bin", "/b/x.bin").unwrap();
        assert!(s.contains("/a/x.bin"));
        assert_eq!(
            s.get("/b/x.bin").unwrap().etag(),
            s.get("/a/x.bin").unwrap().etag()
        );
    }

    #[test]
    fn dump_reports_hash_and_size() {
        let mut s = Store::open(BackendKind::Mem).unwrap();
        s.write_file("/.notes/protocol.json", b"{}".to_vec()).unwrap();
        let d = s.dump(Some("/.notes"));
        assert_eq!(d.len(), 2, "{d:?}"); // 集合 + 文件
        let f = d.iter().find(|e| !e.is_dir).unwrap();
        assert_eq!(f.bytes, 2);
        assert!(f.sha256.starts_with("sha256:"));
        assert!(d.iter().any(|e| e.is_dir && e.sha256.is_empty()));
    }
}
