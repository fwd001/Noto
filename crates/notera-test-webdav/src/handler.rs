//! WebDAV 动词处理（dav 子集）。
//!
//! 判定只用服务端状态与请求头：`Last-Modified` 是固定值，纯粹为了报文合法，
//! 协议本身禁止拿它判新旧（SYNC-PROTOCOL §0 R3）。

use crate::http::Request;
use crate::inject::Injection;
use crate::state::{is_descendant, normalize, MkdirError, Node, Store};
use crate::xml::{
    href_escape, multistatus, parse_propfind, parse_proppatch, remove_props, set_props, MsEntry,
    PropFind,
};

/// 支持的动词（`OPTIONS` 的 `Allow` 也用它）。
pub const ALLOW: &str = "OPTIONS, GET, HEAD, PUT, DELETE, MOVE, COPY, PROPFIND, PROPPATCH, MKCOL";
/// `DAV` 能力头：class 1 + class 2；不实现 LOCK，故不声明 3。
pub const DAV_CAPS: &str = "1, 2";
/// `Last-Modified` 固定值：确定性优先。
const LAST_MODIFIED: &str = "Wed, 21 Oct 2015 07:28:00 GMT";

#[derive(Clone, Debug)]
pub struct Reply {
    pub status: u16,
    /// 头部名大小写按写出习惯；`content-length` 由 server 层补。
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Reply {
    pub fn new(status: u16) -> Reply {
        Reply {
            status,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }
    pub fn with(mut self, k: &str, v: impl Into<String>) -> Reply {
        self.headers.push((k.to_string(), v.into()));
        self
    }
    pub fn text(mut self, msg: &str) -> Reply {
        self.body = msg.as_bytes().to_vec();
        self.headers
            .push(("content-type".into(), "text/plain; charset=utf-8".into()));
        self
    }
    pub fn json(status: u16, body: &str) -> Reply {
        Reply::new(status)
            .with("content-type", "application/json; charset=utf-8")
            .body_bytes(body.as_bytes().to_vec())
    }
    pub fn xml(status: u16, body: &str) -> Reply {
        Reply::new(status)
            .with("content-type", "application/xml; charset=utf-8")
            .body_bytes(body.as_bytes().to_vec())
    }
    pub fn body_bytes(mut self, b: Vec<u8>) -> Reply {
        self.body = b;
        self
    }
    pub fn status(mut self, code: u16) -> Reply {
        self.status = code;
        self
    }
}

/// 请求处理入口。`proxied` = 该连接是否经代理到达（CONNECT 隧道或绝对形式目标）。
pub fn handle(store: &mut Store, req: &Request, inj: &Injection, proxied: bool) -> Reply {
    // 代理独占（PROXY.md §9 证据链②）：非经代理进来的请求一律 403。
    if inj.require_proxy && !proxied {
        return Reply::new(403).text("此服务器只接受经代理到达的连接");
    }
    let Some(path) = normalize(&req.path) else {
        return Reply::new(400).text("非法路径（疑似目录穿越）");
    };

    // `status_for` 规则：非 `post:` 在副作用之前短路；`post:` 先做副作用再改状态。
    let forced = inj.match_status(&req.method, &path, &mut store.rule_hits);
    if let Some((code, false)) = forced {
        return injected_reply(code, &path);
    }
    let real = dispatch(store, req, &path);
    match forced {
        Some((code, true)) => real.status(code),
        _ => real,
    }
}

fn injected_reply(code: u16, path: &str) -> Reply {
    let base = Reply::new(code);
    match code {
        401 => base
            .with("www-authenticate", "Basic realm=\"notera-test\"")
            .text("注入：未授权"),
        403 => base.text(&format!("注入：禁止访问 {path}")),
        404 => base.text(&format!("注入：未找到 {path}")),
        405 => base.with("allow", ALLOW).text("注入：方法不允许"),
        409 => base.text("注入：冲突"),
        412 => base.text("注入：前置条件失败"),
        500 => base.text("注入：服务器错误"),
        503 => base.text("注入：暂不可用"),
        507 => base.text("注入：存储空间不足"),
        other => base.text(&format!("注入状态 {other}")),
    }
}

fn dispatch(store: &mut Store, req: &Request, path: &str) -> Reply {
    match req.method.as_str() {
        "OPTIONS" => options(store, path),
        "GET" => get(store, req, path),
        "HEAD" => get(store, req, path),
        "PUT" => put(store, req, path),
        "DELETE" => delete(store, path),
        "MKCOL" => mkcol(store, path),
        "PROPFIND" => propfind(store, req, path),
        "PROPPATCH" => proppatch(store, req, path),
        "MOVE" => move_it(store, req, path),
        "COPY" => copy_it(store, req, path),
        "LOCK" | "UNLOCK" | "UNPROPFIND" | "SEARCH" => Reply::new(501)
            .text("本测试服务器不实现 LOCK（租约是尽力而为，见 SYNC-PROTOCOL §11.2 / ADR）"),
        other => Reply::new(405)
            .with("allow", ALLOW)
            .text(&format!("未实现的方法 {other}")),
    }
}

fn options(store: &Store, path: &str) -> Reply {
    let r = Reply::new(if store.contains(path) { 200 } else { 404 })
        .with("allow", ALLOW)
        .with("dav", DAV_CAPS)
        .with("ms-author-via", "DAV");
    if store.contains(path) {
        return r.with("accept-ranges", "bytes");
    }
    r
}

fn get(store: &Store, req: &Request, path: &str) -> Reply {
    let Some(node) = store.get(path) else {
        return Reply::new(404).text(&format!("未找到 {path}"));
    };
    if node.is_dir {
        return Reply::new(405)
            .with("allow", ALLOW)
            .text("GET 不能作用于集合");
    }
    let etag = node.etag();

    // If-None-Match → 304（空轮快路径的前提）。
    if let Some(inm) = req.header("if-none-match") {
        if etag_matches(inm, &etag) {
            return Reply::new(304)
                .with("etag", etag)
                .with("last-modified", LAST_MODIFIED);
        }
    }
    let base = Reply::new(200)
        .with("etag", etag.clone())
        .with("last-modified", LAST_MODIFIED)
        .with("accept-ranges", "bytes")
        .with("content-type", content_type(path));

    if let Some(range) = req.header("range") {
        if let Some((from, to)) = parse_range(range, node.bytes.len()) {
            return base
                .status(206)
                .with(
                    "content-range",
                    format!("bytes {from}-{to}/{}", node.bytes.len()),
                )
                .body_bytes(node.bytes[from..=to].to_vec());
        }
        if !range.trim().is_empty() {
            return base
                .status(416)
                .with("content-range", format!("bytes */{}", node.bytes.len()));
        }
    }
    base.body_bytes(node.bytes.clone())
}

fn put(store: &mut Store, req: &Request, path: &str) -> Reply {
    if store.get(path).map(|n| n.is_dir).unwrap_or(false) {
        return Reply::new(405).with("allow", ALLOW).text("目标是集合");
    }
    let existed = store.contains(path);
    let current_etag = store.get(path).map(etag_of);

    // If-Match: 必须与**当前**实例匹配；目标不存在时（含 `*`）一律 412。
    if let Some(im) = req.header("if-match") {
        let ok = match &current_etag {
            Some(cur) => etag_matches(im, cur),
            None => false,
        };
        if !ok {
            return Reply::new(412).text("If-Match 不匹配");
        }
    }
    // If-None-Match: `*` = "只允许创建"；目标不存在则通过。
    if let Some(inm) = req.header("if-none-match") {
        let hit = match &current_etag {
            Some(cur) => etag_matches(inm, cur),
            None => false,
        };
        if hit {
            return Reply::new(412).text("If-None-Match 命中（目标已存在）");
        }
    }
    match store.write_file(path, req.body.clone()) {
        Ok(()) => {
            let etag = store.get(path).map(etag_of).unwrap_or_default();
            // 新建 201 / 覆盖 204（RFC 9110 §9.3.4）。
            Reply::new(if existed { 204 } else { 201 }).with("etag", etag)
        }
        Err(e) => Reply::new(500).text(&format!("写入失败: {e}")),
    }
}

fn delete(store: &mut Store, path: &str) -> Reply {
    if !store.contains(path) {
        return Reply::new(404).text(&format!("未找到 {path}"));
    }
    store.delete(path);
    Reply::new(204)
}

fn mkcol(store: &mut Store, path: &str) -> Reply {
    match store.mkdir(path) {
        Ok(Ok(())) => Reply::new(201).text("集合已创建"),
        Ok(Err(MkdirError::Exists)) => Reply::new(405).text("集合已存在"),
        Ok(Err(MkdirError::NoParent)) => Reply::new(409).text("父集合不存在"),
        Err(e) => Reply::new(500).text(&format!("MKCOL 失败: {e}")),
    }
}

fn propfind(store: &Store, req: &Request, path: &str) -> Reply {
    let Some(root) = store.get(path) else {
        return Reply::new(404).text(&format!("未找到 {path}"));
    };
    let is_dir = root.is_dir;
    let depth = req
        .header("depth")
        .map(|v| v.trim().to_ascii_lowercase())
        .unwrap_or_else(|| "infinity".into());

    let mut entries: Vec<MsEntry> = Vec::new();
    if !is_dir || depth == "0" {
        entries.push(entry_of(store, path));
    } else if depth == "1" {
        entries.push(entry_of(store, path));
        entries.extend(
            store
                .children(path)
                .iter()
                .filter_map(|p| entry_of_opt(store, p)),
        );
    } else if depth == "infinity" {
        entries.push(entry_of(store, path));
        entries.extend(
            store
                .descendants(path)
                .iter()
                .filter_map(|p| entry_of_opt(store, p)),
        );
    } else {
        // Depth: N —— 只展开 N 层（真实服务器多数不支持，这里支持以便测试）。
        let max = depth.parse::<usize>().unwrap_or(1);
        entries.push(entry_of(store, path));
        let base = path.trim_end_matches('/');
        for p in store.descendants(path) {
            let rest = p.strip_prefix(base).unwrap_or(&p);
            if rest.matches('/').count() <= max {
                if let Some(e) = entry_of_opt(store, &p) {
                    entries.push(e);
                }
            }
        }
    }
    let requested = parse_propfind(&req.body);
    let xml = multistatus(&entries, &requested);
    Reply::xml(207, &xml).with("vary", "Depth, Destination, Overwrite, Content-Type")
}

fn proppatch(store: &mut Store, req: &Request, path: &str) -> Reply {
    if !store.contains(path) {
        return Reply::new(404).text(&format!("未找到 {path}"));
    }
    let ops = parse_proppatch(&req.body);
    let removed = remove_props(&ops);
    let set = set_props(&ops);
    if let Err(e) = store.remove_props(path, &removed) {
        return Reply::new(500).text(&format!("PROPPATCH 失败: {e}"));
    }
    if let Err(e) = store.set_props(path, set) {
        return Reply::new(500).text(&format!("PROPPATCH 失败: {e}"));
    }
    let entry = entry_of(store, path);
    let xml = multistatus(std::slice::from_ref(&entry), &PropFind::AllProp);
    Reply::xml(207, &xml)
}

fn move_it(store: &mut Store, req: &Request, path: &str) -> Reply {
    let Some(dest) = destination(req) else {
        return Reply::new(400).text("缺少 Destination 头");
    };
    if !store.contains(path) {
        return Reply::new(404).text(&format!("源 {path} 不存在"));
    }
    if dest == path {
        return Reply::new(400).text("Destination 不能是自身");
    }
    if is_descendant(&dest, path) {
        return Reply::new(403).text("不能把集合移动到自己的子树里");
    }
    if let Some(im) = req.header("if-match") {
        let cur = store.get(path).map(etag_of);
        let ok = match &cur {
            Some(c) => etag_matches(im, c),
            None => false,
        };
        if !ok {
            return Reply::new(412).text("If-Match 不匹配");
        }
    }
    let dest_exists = store.contains(&dest);
    // RFC 4918 §9.8.5：目标存在且 Overwrite: F → 412。
    if dest_exists && !overwrite_allowed(req) {
        return Reply::new(412).text("目标已存在且 Overwrite: F");
    }
    if dest_exists {
        store.delete(&dest);
    }
    if let Err(e) = store.rename(path, &dest) {
        return Reply::new(500).text(&format!("MOVE 失败: {e}"));
    }
    let etag = store.get(&dest).map(etag_of);
    let mut r = Reply::new(if dest_exists { 204 } else { 201 });
    if let Some(e) = etag {
        r = r.with("etag", e);
    }
    r
}

fn copy_it(store: &mut Store, req: &Request, path: &str) -> Reply {
    let Some(dest) = destination(req) else {
        return Reply::new(400).text("缺少 Destination 头");
    };
    if !store.contains(path) {
        return Reply::new(404).text(&format!("源 {path} 不存在"));
    }
    if dest == path || is_descendant(&dest, path) {
        return Reply::new(400).text("Destination 与源冲突");
    }
    let dest_exists = store.contains(&dest);
    if dest_exists && !overwrite_allowed(req) {
        return Reply::new(412).text("目标已存在且 Overwrite: F");
    }
    if dest_exists {
        store.delete(&dest);
    }
    if let Err(e) = store.copy(path, &dest) {
        return Reply::new(500).text(&format!("COPY 失败: {e}"));
    }
    Reply::new(if dest_exists { 204 } else { 201 })
}

fn overwrite_allowed(req: &Request) -> bool {
    req.header("overwrite")
        .map(|v| !v.trim().eq_ignore_ascii_case("f"))
        .unwrap_or(true)
}

fn entry_of(store: &Store, path: &str) -> MsEntry {
    let node = store.get(path);
    let is_dir = node.map(|n| n.is_dir).unwrap_or(false);
    MsEntry {
        href: href_for(path, is_dir),
        is_dir,
        etag: node.filter(|n| !n.is_dir).map(etag_of),
        len: node.map(|n| n.bytes.len() as u64).unwrap_or(0),
        last_modified: LAST_MODIFIED.to_string(),
        props: node.map(|n| n.props.clone()).unwrap_or_default(),
    }
}

fn entry_of_opt(store: &Store, path: &str) -> Option<MsEntry> {
    store.get(path).map(|_| entry_of(store, path))
}

fn href_for(path: &str, is_dir: bool) -> String {
    if is_dir {
        href_escape(&format!("{}/", path.trim_end_matches('/')))
    } else {
        href_escape(path)
    }
}

fn etag_of(node: &Node) -> String {
    node.etag()
}

/// `Destination` 可为绝对 URL 或绝对路径。
fn destination(req: &Request) -> Option<String> {
    let raw = req.header("destination")?.trim();
    if raw.is_empty() {
        return None;
    }
    let path_part = if raw.starts_with("http://") || raw.starts_with("https://") {
        let after = &raw[raw.find("//")? + 2..];
        match after.find('/') {
            Some(i) => after[i..].to_string(),
            None => "/".to_string(),
        }
    } else {
        raw.to_string()
    };
    let decoded = percent_encoding::percent_decode_str(&path_part)
        .decode_utf8_lossy()
        .into_owned();
    normalize(decoded.split_once('?').map(|(p, _)| p).unwrap_or(&decoded))
}

/// ETag 列表匹配：`*`、逗号分隔、弱 `W/` 前缀容忍。
pub fn etag_matches(header_value: &str, etag: &str) -> bool {
    let v = header_value.trim();
    if v == "*" {
        return true;
    }
    v.split(',').any(|c| {
        let c = c.trim().trim_start_matches("W/");
        c.trim() == etag.trim() || c.trim_matches('"') == etag.trim().trim_matches('"')
    })
}

fn parse_range(header_value: &str, len: usize) -> Option<(usize, usize)> {
    let spec = header_value.trim().strip_prefix("bytes=")?;
    let one = spec.split(',').next()?.trim();
    let (a, b) = one.split_once('-')?;
    if a.is_empty() {
        let n: usize = b.trim().parse().ok()?;
        if n == 0 || len == 0 {
            return None;
        }
        return Some((len.saturating_sub(n), len - 1));
    }
    let from: usize = a.trim().parse().ok()?;
    if from >= len {
        return None;
    }
    let to = if b.trim().is_empty() {
        len - 1
    } else {
        b.trim().parse::<usize>().ok()?.min(len - 1)
    };
    if from > to {
        return None;
    }
    Some((from, to))
}

fn content_type(path: &str) -> &'static str {
    if path.ends_with(".json") {
        "application/json"
    } else if path.ends_with(".xml") {
        "application/xml"
    } else {
        "application/octet-stream"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn etag_matching_covers_star_and_weak() {
        assert!(etag_matches("*", "\"abc\""));
        assert!(etag_matches("\"abc\"", "\"abc\""));
        assert!(etag_matches("W/\"abc\", \"def\"", "\"def\""));
        assert!(!etag_matches("\"xyz\"", "\"abc\""));
    }

    #[test]
    fn range_parsing() {
        assert_eq!(parse_range("bytes=0-0", 10), Some((0, 0)));
        assert_eq!(parse_range("bytes=2-4", 10), Some((2, 4)));
        assert_eq!(parse_range("bytes=5-", 10), Some((5, 9)));
        assert_eq!(parse_range("bytes=-3", 10), Some((7, 9)));
        assert_eq!(parse_range("bytes=10-20", 10), None);
        assert_eq!(parse_range("items=0-1", 10), None);
    }
}
