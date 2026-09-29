//! 207 multistatus 的**生成**与 PROPFIND / PROPPATCH 请求体的**解析**。
//!
//! 手写生成是有意的：207 的字节形态就是要被客户端 XML 解析器吃的东西，
//! 生成侧越直白越好查；解析侧用 `quick-xml` 真流式读，不靠字符串包含糊弄。

use quick_xml::events::Event;
use quick_xml::reader::Reader;

/// PROPFIND 请求体里要的是什么。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PropFind {
    AllProp,
    PropName,
    Prop(Vec<String>),
    /// 没有请求体（很多客户端这么干）—— 按 allprop 处理。
    Empty,
}

/// PROPPATCH 的条目：`(性质, 名字, 值)`。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PropPatch {
    Set(Vec<(String, String)>),
    Remove(Vec<String>),
}

pub fn local_name(raw: &str) -> String {
    let s = raw;
    match s.rsplit_once(':') {
        Some((_, l)) => l.to_string(),
        None => s.to_string(),
    }
}

/// 解析 PROPFIND body。
pub fn parse_propfind(body: &[u8]) -> PropFind {
    if body.iter().all(|b| b.is_ascii_whitespace()) || body.is_empty() {
        return PropFind::Empty;
    }
    let mut reader = Reader::from_reader(body);
    // 0.42 默认对没有分号的裸 `&` 整篇报错；旧版本是把原文交给上层。
    // 这台测试服务器不许因为解析器变严而改变它对请求的反应（那会把"客户端发了什么"
    // 和"注入器怎么解读"两件事混在一起，故障注入的结论就不干净了）。
    reader.config_mut().allow_dangling_amp = true;
    let mut props = Vec::new();
    let mut kind = PropFind::Empty;
    // depth 记录在头部，不在 body；这里只取 propfind 的子元素。
    let mut in_propfind = false;
    let mut depth_attr = None;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let name = local_name(e.name().as_ref());
                match name.as_str() {
                    "propfind" => {
                        in_propfind = true;
                        for a in e.attributes().flatten() {
                            if local_name(a.key.as_ref()) == "depth" {
                                depth_attr = Some(
                                    a.normalized_value(quick_xml::XmlVersion::default())
                                        .map(|c| c.into_owned())
                                        .unwrap_or_default(),
                                );
                            }
                        }
                    }
                    "allprop" if in_propfind => kind = PropFind::AllProp,
                    "propname" if in_propfind => kind = PropFind::PropName,
                    "prop" if in_propfind => {
                        // prop 容器下的每个元素都是被请求的属性名
                        let mut inner = Vec::new();
                        loop {
                            match reader.read_event() {
                                Ok(Event::Empty(e2)) => inner.push(local_name(e2.name().as_ref())),
                                Ok(Event::Start(e2)) => inner.push(local_name(e2.name().as_ref())),
                                Ok(Event::End(e2)) => {
                                    if local_name(e2.name().as_ref()) == "prop" {
                                        break;
                                    }
                                }
                                Ok(Event::Eof) => break,
                                Err(_) => break,
                                _ => {}
                            }
                        }
                        if !inner.is_empty() {
                            props.extend(inner);
                            kind = PropFind::Prop(props.clone());
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    let _ = depth_attr;
    match kind {
        PropFind::Empty => PropFind::AllProp,
        other => other,
    }
}

/// PROPPATCH body 解析为 set/remove 序列。
pub fn parse_proppatch(body: &[u8]) -> Vec<PropPatch> {
    let mut out = Vec::new();
    if body.is_empty() {
        return out;
    }
    let mut reader = Reader::from_reader(body);
    reader.config_mut().allow_dangling_amp = true;
    let mut mode: Option<&'static str> = None;
    let mut set: Vec<(String, String)> = Vec::new();
    let mut remove: Vec<String> = Vec::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => match local_name(e.name().as_ref()).as_str() {
                "set" => mode = Some("set"),
                "remove" => mode = Some("remove"),
                "prop" if mode.is_some() => {
                    // prop 容器里每个子元素是一个属性
                    loop {
                        match reader.read_event() {
                            Ok(Event::Empty(e2)) => {
                                let n = local_name(e2.name().as_ref());
                                match mode {
                                    Some("set") => set.push((n, String::new())),
                                    Some("remove") => remove.push(n),
                                    _ => {}
                                }
                            }
                            Ok(Event::Start(e2)) => {
                                let n = local_name(e2.name().as_ref());
                                let text = reader
                                    .read_text(e2.name())
                                    .map(|t| t.into_inner().into_owned())
                                    .unwrap_or_default();
                                let text = text.trim().to_string();
                                match mode {
                                    Some("set") => set.push((n, text)),
                                    Some("remove") => remove.push(n),
                                    _ => {}
                                }
                            }
                            Ok(Event::End(e2)) => {
                                if local_name(e2.name().as_ref()) == "prop" {
                                    break;
                                }
                            }
                            Ok(Event::Eof) => break,
                            Err(_) => break,
                            _ => {}
                        }
                    }
                }
                _ => {}
            },
            Ok(Event::End(e)) => {
                let n = local_name(e.name().as_ref());
                if n == "set" || n == "remove" {
                    mode = None;
                }
                if n == "propertyupdate" {
                    break;
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    if !set.is_empty() {
        out.push(PropPatch::Set(set));
    }
    if !remove.is_empty() {
        out.push(PropPatch::Remove(remove));
    }
    out
}

pub fn remove_props(ops: &[PropPatch]) -> Vec<String> {
    let mut v = Vec::new();
    for op in ops {
        if let PropPatch::Remove(names) = op {
            v.extend(names.iter().cloned());
        }
    }
    v
}

pub fn set_props(ops: &[PropPatch]) -> Vec<(String, String)> {
    let mut v = Vec::new();
    for op in ops {
        if let PropPatch::Set(props) = op {
            v.extend(props.iter().cloned());
        }
    }
    v
}

/// XML 文本转义。
pub fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ if (c as u32) < 0x20 && c != '\n' && c != '\t' => {
                out.push_str(&format!("&#{};", c as u32))
            }
            _ => out.push(c),
        }
    }
    out
}

/// href 里保留 `/`，其余非 unreserved 字节百分号编码（UTF-8 逐字节）。
pub fn href_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'.'
            | b'_'
            | b'~'
            | b'/'
            | b':'
            | b'@' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// 一个 multistatus 条目。
pub struct MsEntry {
    pub href: String,
    pub is_dir: bool,
    pub etag: Option<String>,
    pub len: u64,
    pub last_modified: String,
    pub props: Vec<(String, String)>,
}

/// 生成 207 multistatus。
///
/// `requested` 决定回哪些属性：`allprop` 回标准四件套 + 自定义属性；
/// `Prop(list)` 只回被点名的（真实客户端就是这么发的）。
pub fn multistatus(entries: &[MsEntry], requested: &PropFind) -> String {
    let mut xml = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
    xml.push_str("<D:multistatus xmlns:D=\"DAV:\">\n");
    for e in entries {
        xml.push_str("<D:response>\n");
        xml.push_str(&format!("<D:href>{}</D:href>\n", esc(&e.href)));
        xml.push_str("<D:propstat>\n<D:prop>\n");
        if wants(requested, "resourcetype") {
            xml.push_str(if e.is_dir {
                "<D:resourcetype><D:collection/></D:resourcetype>\n"
            } else {
                "<D:resourcetype/>\n"
            });
        }
        if wants(requested, "getetag") {
            if let Some(et) = &e.etag {
                xml.push_str(&format!("<D:getetag>{}</D:getetag>\n", esc(et)));
            }
        }
        if wants(requested, "getcontentlength") && !e.is_dir {
            xml.push_str(&format!(
                "<D:getcontentlength>{}</D:getcontentlength>\n",
                e.len
            ));
        }
        if wants(requested, "getlastmodified") {
            xml.push_str(&format!(
                "<D:getlastmodified>{}</D:getlastmodified>\n",
                esc(&e.last_modified)
            ));
        }
        if wants(requested, "displayname") {
            let name = e
                .href
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or("");
            xml.push_str(&format!("<D:displayname>{}</D:displayname>\n", esc(name)));
        }
        for (k, v) in &e.props {
            // 自定义属性：allprop 下回；点名时按点名回。
            if wants(requested, k) {
                xml.push_str(&format!("<{k}>{}</{k}>\n", esc(v)));
            }
        }
        xml.push_str("</D:prop>\n<D:status>HTTP/1.1 200 OK</D:status>\n</D:propstat>\n");
        xml.push_str("</D:response>\n");
    }
    xml.push_str("</D:multistatus>\n");
    xml
}

/// 标准属性名与自定义属性名共用一张表。
fn wants(requested: &PropFind, name: &str) -> bool {
    match requested {
        PropFind::AllProp | PropFind::Empty | PropFind::PropName => true,
        PropFind::Prop(list) => list.iter().any(|p| p == name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_allprop_and_explicit_props() {
        let body = br#"<?xml version="1.0"?><propfind xmlns="DAV:"><allprop/></propfind>"#;
        assert_eq!(parse_propfind(body), PropFind::AllProp);
        let body = br#"<propfind xmlns="DAV:"><prop><getetag xmlns="DAV:"/><x:custom xmlns:x="urn:x"/></prop></propfind>"#;
        match parse_propfind(body) {
            PropFind::Prop(v) => assert!(
                v.contains(&"getetag".to_string()) && v.contains(&"custom".to_string()),
                "{v:?}"
            ),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parses_proppatch_set_and_remove() {
        let body = br#"<?xml version="1.0"?><propertyupdate xmlns="DAV:"><D:set xmlns:D="DAV:"><D:prop><x:author>me</x:author></D:prop></D:set><D:remove xmlns:D="DAV:"><D:prop><x:old/></D:prop></D:remove></propertyupdate>"#;
        let ops = parse_proppatch(body);
        assert_eq!(set_props(&ops), vec![("author".into(), "me".into())]);
        assert_eq!(remove_props(&ops), vec!["old".to_string()]);
    }

    #[test]
    fn multistatus_has_required_shape() {
        let xml = multistatus(
            &[MsEntry {
                href: "/.notes/".into(),
                is_dir: true,
                etag: None,
                len: 0,
                last_modified: "x".into(),
                props: vec![("author".into(), "me".into())],
            }],
            &PropFind::AllProp,
        );
        assert!(xml.starts_with("<?xml"), "{xml}");
        assert!(xml.contains("<D:multistatus xmlns:D=\"DAV:\">"));
        assert!(xml.contains("<D:response>"));
        assert!(xml.contains("<D:href>/.notes/</D:href>"));
        assert!(xml.contains("<D:propstat>"));
        assert!(xml.contains("<D:prop>"));
        assert!(xml.contains("<D:collection/>"));
        assert!(xml.contains("HTTP/1.1 200 OK"));
        assert!(xml.contains("<author>me</author>"));
    }

    #[test]
    fn named_props_only_return_named() {
        let xml = multistatus(
            &[MsEntry {
                href: "/a.json".into(),
                is_dir: false,
                etag: Some("\"deadbeef\"".into()),
                len: 7,
                last_modified: "x".into(),
                props: vec![("author".into(), "me".into())],
            }],
            &PropFind::Prop(vec!["getetag".to_string()]),
        );
        assert!(xml.contains("<D:getetag>"));
        assert!(!xml.contains("getcontentlength"));
        assert!(!xml.contains("<author>"));
    }

    #[test]
    fn escapes_bad_bytes() {
        assert_eq!(esc("<a&\">"), "&lt;a&amp;&quot;&gt;");
    }

    /// 注入器读 PROPPATCH 属性值是**原文**，而且一个没分号的裸 `&` 不许让整条请求失败。
    ///
    /// 这条是给 quick-xml 换版本用的对照组：0.42 默认对裸 `&` 整篇报错，而旧版本是把原文
    /// 交给上层；这里两边都要钉住 —— 反向的变异（顺手把值反转义了）同样要红，
    /// 否则"客户端到底发了什么字节"这件事就没证据了。
    #[test]
    fn property_values_are_read_verbatim_and_a_lone_amp_does_not_kill_the_request() {
        let body = br#"<?xml version="1.0"?><D:propertyupdate xmlns:D="DAV:"><D:set><D:prop><author>Tom & Jerry &amp; co</author><note-tag>x &amp; y</note-tag></D:prop></D:set></D:propertyupdate>"#;
        assert_eq!(
            parse_proppatch(body),
            vec![PropPatch::Set(vec![
                ("author".to_string(), "Tom & Jerry &amp; co".to_string()),
                ("note-tag".to_string(), "x &amp; y".to_string()),
            ])],
            "值必须是原文（裸 & 留着、&amp; 也不解），且这条请求不许解析失败"
        );
    }
}
