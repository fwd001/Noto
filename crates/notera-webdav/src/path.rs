//! 远端路径构造与路径安全闸门（docs/SYNC-PROTOCOL.md §1）。
//!
//! 规范把这条职责钉死在本模块：*"路径构造集中在 `notera-webdav::RemotePath`，
//! **必须**校验 `id` 字符集并拒绝任何 `..`（目录穿越即安全事故）"*。
//!
//! 实现取向是**白名单**而不是黑名单：每个路径片段只允许 `[A-Za-z0-9_.-]`，并且显式
//! 拒绝任何 `..` 序列与 `.` / `..` 整段。因此 `/`、`\`、`?`、`#`、`:`、空格、控制字符、
//! 非 ASCII，以及**未解码的百分号形式**（`%2e%2e`、`..%2F`）都进不来 —— `%` 本身不在允许集内，
//! 而我们也从不生成百分号编码：`notera-net` 收到的 URL 与这里拼出的字符串逐字节相同。

use notera_core::{EntityId, EntityKind};

use crate::error::PathError;

/// 根前缀默认值（§1）。
pub const DEFAULT_ROOT_PREFIX: &str = "/.notes";

/// 路径片段长度上限：真实 id 是 36 字符 UUID，超限即异常输入。
const MAX_SEGMENT_LEN: usize = 96;
/// 完整 URL 长度上限（防御性：坏清单不该让我们发出荒谬请求）。
const MAX_URL_LEN: usize = 4096;

/// `<base_url><root_prefix>/...` 的全部拼法。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemotePath {
    /// 不含尾部 `/` 的 origin（或 origin + 路径前缀），如 `http://127.0.0.1:53124`。
    base: String,
    /// 以 `/` 开头、不含尾部 `/` 的根前缀，如 `/.notes`。
    root: String,
}

impl RemotePath {
    /// 校验 base URL 与根前缀并建好拼路器。
    pub fn new(base_url: &str, root_prefix: &str) -> Result<RemotePath, PathError> {
        let base = normalize_base(base_url)?;
        let root = normalize_root(root_prefix)?;
        Ok(RemotePath { base, root })
    }

    /// origin（用于 `/_fs/dump` 这类根外探测）。
    pub fn base_url(&self) -> &str {
        &self.base
    }

    /// 已规范化的根前缀（可能为空串 = 直接挂在 origin 下）。
    pub fn root_prefix(&self) -> &str {
        &self.root
    }

    /// 一个远端对象在其根内的相对路径，如 `/manifest/index.json`。
    pub fn rel(segs: &[&str]) -> String {
        let mut out = String::new();
        for s in segs {
            out.push('/');
            out.push_str(s);
        }
        out
    }

    /// 完整 URL。`segs` 必须是本模块生成的（即已校验过的）片段。
    pub fn url(&self, segs: &[&str]) -> String {
        let mut out = String::with_capacity(self.base.len() + self.root.len() + 16);
        out.push_str(&self.base);
        out.push_str(&self.root);
        out.push_str(&Self::rel(segs));
        out
    }

    // ------------------------------------------------------- 对象名 ---

    /// `protocol.json`：根自描述与版本协商（§2）。
    pub fn protocol_json(&self) -> String {
        self.url(&["protocol.json"])
    }

    /// `manifest/index.json`：权威清单。
    pub fn manifest_index(&self) -> String {
        self.url(&["manifest", "index.json"])
    }

    /// `manifest/index.json.prev`：上一版清单（D1 回退用）。
    pub fn manifest_prev(&self) -> String {
        self.url(&["manifest", "index.json.prev"])
    }

    /// `manifest/index.json.tmp-<device>-<nonce>`（§4.4 步骤 3）。
    pub fn manifest_tmp(&self, device: &str, nonce: u64) -> Result<String, PathError> {
        let name = format!("index.json.tmp-{}-{}", checked_token(device)?, nonce);
        Ok(self.url(&["manifest", &checked_name(&name, "清单暂存名")?]))
    }

    /// `manifest/<name>.json`：基线分段（§1 `seg-<NNNN>.json`）。
    /// `attachments/<2hex>/<sha256>`（§1）。内容寻址的名字**就是**校验和：
    /// 名字不合法意味着连"该校验什么"都不成立，因此在拼路径阶段就拒，不发请求。
    pub fn attachment(&self, sha256: &str) -> Result<String, PathError> {
        check_sha_hex(sha256)?;
        Ok(self.url(&["attachments", &sha256[..2], sha256]))
    }

    pub fn segment(&self, name: &str) -> Result<String, PathError> {
        check_segment_name(name)?;
        Ok(self.url(&["manifest", &format!("{name}.json")]))
    }

    /// 一条实体记录（含附件）的对象路径。
    ///
    /// 附件按 §1 内容寻址到 `attachments/<2hex>/<sha256>`，**不带** `.json` 后缀，
    /// 且 id 是 64 位十六进制而不是 UUIDv7 —— 因此它走独立的 id 校验分支。
    pub fn record(&self, kind: &str, id: &str) -> Result<RecordPath, PathError> {
        let dir = kind_dir(kind)?;
        match dir {
            "attachments" => {
                check_sha_hex(id)?;
                let two = &id[..2];
                Ok(RecordPath {
                    url: self.url(&["attachments", two, id]),
                    dir: "attachments",
                    on_server: format!("/attachments/{two}/{id}"),
                })
            }
            d => {
                check_uuid_id(id)?;
                Ok(RecordPath {
                    url: self.url(&["records", d, &format!("{id}.json")]),
                    dir: d,
                    on_server: format!("/records/{d}/{id}.json"),
                })
            }
        }
    }

    /// `tmp/<device>-<nonce>.json`：原子写暂存（§1，>24h 由维护轮清理）。
    pub fn tmp(&self, device: &str, nonce: u64) -> Result<String, PathError> {
        let name = format!("{}-{}", checked_token(device)?, nonce);
        Ok(self.url(&["tmp", &format!("{}.json", checked_name(&name, "暂存名")?)]))
    }

    /// 记录原子写的暂存路径：`tmp/<device>-<nonce>-<kind>-<id>.json`。
    ///
    /// nonce 每次自增，因此同一实体的两个写永不撞名（§11.3 C2：残留交给 24h 维护轮清理）。
    pub fn record_tmp(
        &self,
        kind: &str,
        id: &str,
        device: &str,
        nonce: u64,
    ) -> Result<String, PathError> {
        let dir = kind_dir(kind)?;
        if dir == "attachments" {
            check_sha_hex(id)?;
        } else {
            check_uuid_id(id)?;
        }
        let name = format!("{}-{}-{}-{}", checked_token(device)?, nonce, dir, id);
        Ok(self.url(&[
            "tmp",
            &format!("{}.json", checked_name(&name, "记录暂存名")?),
        ]))
    }

    /// 一个 URL 是否落在本根之内（`MOVE` 的 `Destination` 必须满足，否则是把数据搬出根）。
    pub fn is_in_root(&self, url: &str) -> bool {
        let prefix = format!("{}{}", self.base, self.root);
        match url.strip_prefix(&prefix) {
            Some(rest) => rest.is_empty() || rest.starts_with('/'),
            None => false,
        }
    }
}

/// 一条记录的目标路径 + 服务器侧可见路径（测试断言与诊断用）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordPath {
    pub url: String,
    pub dir: &'static str,
    pub on_server: String,
}

/// `kind` 词汇归一：`RemotePort` 收到的是清单单字符标记（`n`/`f`/`a`，省流量），
/// 而信封与目录名用的是 `EntityKind::dir()`。两种都收，别的都拒。
pub fn kind_dir(kind: &str) -> Result<&'static str, PathError> {
    let dir = match kind {
        "n" | "note" => EntityKind::Note.dir(),
        "f" | "folder" => EntityKind::Folder.dir(),
        "a" | "attachment" | "attachments" => EntityKind::Attachment.dir(),
        other => return Err(PathError::BadKind(other.to_string())),
    };
    Ok(dir)
}

/// 记录 `kind` 是否指向同一个对象目录（§3 "kind 与路径目录一致"）。
pub fn kind_matches(kind: &str, other: &str) -> bool {
    match (kind_dir(kind), kind_dir(other)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn check_uuid_id(id: &str) -> Result<(), PathError> {
    let parsed = EntityId::parse(id).map_err(|_| PathError::BadId(id.to_string()))?;
    notera_core::assert_id_valid(&parsed)
        .map_err(|e| PathError::BadId(format!("{id} ({})", e.detail)))?;
    if parsed.as_str() != id {
        return Err(PathError::BadId(format!("{id} 不是规范化小写形态")));
    }
    Ok(())
}

fn check_sha_hex(sha: &str) -> Result<(), PathError> {
    if sha.len() != 64
        || !sha
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(PathError::BadId(format!("附件 sha256 非法: {sha}")));
    }
    Ok(())
}

/// 分段名必须是 `seg-<digits>`（§1）。分段名来自服务器清单，属于不可信输入。
fn check_segment_name(name: &str) -> Result<(), PathError> {
    let digits = name
        .strip_prefix("seg-")
        .ok_or_else(|| PathError::BadSegment(name.to_string()))?;
    if digits.is_empty() || digits.len() > 8 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(PathError::BadSegment(name.to_string()));
    }
    Ok(())
}

/// 设备 id 当路径片段用：必须是路径安全 UUID。
fn checked_token(device: &str) -> Result<String, PathError> {
    let parsed = EntityId::parse(device).map_err(|_| PathError::BadId(device.to_string()))?;
    notera_core::assert_id_valid(&parsed)
        .map_err(|e| PathError::BadId(format!("{device} ({})", e.detail)))?;
    Ok(parsed.as_str().to_string())
}

fn checked_name(raw: &str, what: &str) -> Result<String, PathError> {
    for part in raw.split('/') {
        validate_name(part, what)?;
    }
    Ok(raw.to_string())
}

/// 核心闸门：白名单字符集 + 拒绝 `.` / `..` 序列。
fn validate_name(raw: &str, what: &str) -> Result<String, PathError> {
    if raw.is_empty() || raw.len() > MAX_SEGMENT_LEN {
        return Err(PathError::BadSegment(format!("{what} 长度非法: {raw:?}")));
    }
    if raw.contains("..") || raw == "." || raw == ".." {
        return Err(PathError::BadSegment(format!(
            "{what} 含目录穿越片段: {raw:?}"
        )));
    }
    if !raw
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        // `/` `\` `%` `?` `#` `:` 空格 非 ASCII 全部落在这里。
        return Err(PathError::BadSegment(format!("{what} 含非法字符: {raw:?}")));
    }
    Ok(raw.to_string())
}

fn normalize_base(raw: &str) -> Result<String, PathError> {
    let t = raw.trim();
    if t.is_empty() {
        return Err(PathError::BadUrl("base_url 为空".into()));
    }
    let Some((scheme, rest)) = t.split_once("//") else {
        return Err(PathError::BadUrl(format!(
            "base_url 缺少 scheme/authority: {t}"
        )));
    };
    if scheme != "http:" && scheme != "https:" {
        return Err(PathError::BadUrl(format!(
            "只支持 http/https，得到 {scheme:?}"
        )));
    }
    if rest.contains('@') {
        // 凭据写进 URL 会进日志，且跨源重定向的凭据闸门无法判定（PROXY.md §3/§9）。
        return Err(PathError::BadUrl(
            "base_url 不得含 user:pass@，请改用 Credentials".into(),
        ));
    }
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(authority_end);
    if authority.is_empty() {
        return Err(PathError::BadUrl("base_url 缺主机".into()));
    }
    let prefix = normalize_root(tail)?;
    Ok(format!("{scheme}//{authority}{prefix}"))
}

fn normalize_root(raw: &str) -> Result<String, PathError> {
    let t = raw.trim();
    let without_query = t.split(['?', '#']).next().unwrap_or(t);
    let mut out = String::new();
    for seg in without_query.split('/') {
        if seg.is_empty() {
            continue;
        }
        out.push('/');
        out.push_str(&validate_name(seg, "根前缀")?);
    }
    Ok(out)
}

/// 发出请求前的最后一道全局检查：长度与控制字符。
pub(crate) fn assert_url_sane(url: &str) -> Result<(), PathError> {
    if url.len() > MAX_URL_LEN {
        return Err(PathError::BadUrl(format!("URL 过长（{} 字节）", url.len())));
    }
    if url.chars().any(|c| c.is_control() || c == ' ') {
        return Err(PathError::BadUrl("URL 含控制字符或空格".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use notera_core::EntityId;

    fn paths() -> RemotePath {
        RemotePath::new("http://dav.local:5005", DEFAULT_ROOT_PREFIX).expect("合法 base")
    }

    #[test]
    fn layout_matches_protocol_section_1() {
        let p = paths();
        let id = EntityId::new();
        assert_eq!(
            p.manifest_index(),
            "http://dav.local:5005/.notes/manifest/index.json"
        );
        assert_eq!(
            p.manifest_prev(),
            "http://dav.local:5005/.notes/manifest/index.json.prev"
        );
        assert_eq!(
            p.protocol_json(),
            "http://dav.local:5005/.notes/protocol.json"
        );
        assert_eq!(
            p.segment("seg-0000").unwrap(),
            "http://dav.local:5005/.notes/manifest/seg-0000.json"
        );
        assert_eq!(
            p.record("n", id.as_str()).unwrap().url,
            format!("http://dav.local:5005/.notes/records/note/{id}.json")
        );
        assert_eq!(
            p.record("folder", id.as_str()).unwrap().url,
            format!("http://dav.local:5005/.notes/records/folder/{id}.json")
        );
        let sha = "a".repeat(64);
        assert_eq!(
            p.record("a", &sha).unwrap().url,
            format!("http://dav.local:5005/.notes/attachments/aa/{sha}")
        );
        assert!(p
            .manifest_tmp(&id.to_string(), 7)
            .unwrap()
            .ends_with(&format!("manifest/index.json.tmp-{id}-7")));
        assert!(p
            .record_tmp("n", id.as_str(), &id.to_string(), 3)
            .unwrap()
            .ends_with(&format!("tmp/{id}-3-note-{id}.json")));
        assert!(p
            .tmp(&id.to_string(), 9)
            .unwrap()
            .ends_with(&format!("tmp/{id}-9.json")));
    }

    #[test]
    fn traversal_and_smuggling_never_reach_a_url() {
        let p = paths();
        let evil = [
            "../etc/passwd",
            "..\\..\\windows\\x",
            "/etc/passwd",
            "..%2F..%2Fx",
            "%2e%2e/%2e%2e/etc",
            "..",
            ".",
            "a/../b",
            "seg-0000.json?x=1",
            "seg-0000#f",
            "note/../manifest/index",
            "笔记",
            "seg 0000",
            "a\nb",
            "",
            "note\x00",
        ];
        for bad in evil {
            assert!(p.segment(bad).is_err(), "分段名必须被拒绝: {bad:?}");
            assert!(p.record("n", bad).is_err(), "记录 id 必须被拒绝: {bad:?}");
        }
        // 附件的 id 词表是 sha256，不是 UUID：非十六进制同样拒。
        assert!(p.record("a", "../../etc").is_err());
        assert!(p.record("a", &"z".repeat(64)).is_err());
        // 未知 kind 不是"退化成 note"，是拒绝。
        assert!(p.record("bogus", &EntityId::new().to_string()).is_err());
        assert!(p
            .record_tmp("n", "../../x", &EntityId::new().to_string(), 1)
            .is_err());
        assert!(
            RemotePath::new("http://u:p@dav.local", "/.notes").is_err(),
            "凭据不得进 URL"
        );
        assert!(RemotePath::new("ftp://dav.local", "/.notes").is_err());
        assert!(RemotePath::new("http://dav.local", "/.notes/../../etc").is_err());
        assert_eq!(
            RemotePath::new("http://dav.local/.notes/", "")
                .unwrap()
                .root_prefix(),
            ""
        );
        // base_url 自带前缀时并入，不静默丢弃。
        assert_eq!(
            RemotePath::new(
                "https://d.example/remote.php/dav/files/u",
                DEFAULT_ROOT_PREFIX
            )
            .unwrap()
            .manifest_index(),
            "https://d.example/remote.php/dav/files/u/.notes/manifest/index.json"
        );
    }

    #[test]
    fn move_destination_must_stay_in_root() {
        let p = paths();
        assert!(p.is_in_root(&p.manifest_prev()));
        assert!(!p.is_in_root("http://dav.local:5005/other/index.json"));
        assert!(!p.is_in_root("http://evil.local/.notes/x"));
        assert!(!p.is_in_root("http://dav.local:5005/.notesish/x"));
    }
}
