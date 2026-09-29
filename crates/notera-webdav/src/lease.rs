//! §11.4 尽力而为租约：`locks/<device>.json`。
//!
//! 三条底线，写在代码里以免被"顺手优化"掉：
//! 1. **它不是正确性依赖**。读不到别人的租约 = 当作没人持有，继续提交；
//!    租约这一层坏了只能退回 §11.2 的前两层，绝不能变成"永远不同步"。
//!    （对比 §5：那里"探测未完成"不许当成结论，因为猜低会掉进盲写；这里最坏
//!    只是少让一次路，方向不同。）
//! 2. 自己的旧租约**总是覆盖**。换 token 不代表换设备，同机重启不该被
//!    自己上次崩溃留下的文件挡住。
//! 3. 不碰 `LOCK`/`UNLOCK`。实测多数后端不实现（本仓库测试服务器直接回 501），
//!    把它当依赖等于封死一批服务器。

use notera_core::Timestamp;
use notera_sync::RemoteError;

use crate::client::WebDavRemote;

/// 一台设备贴在服务器上的租约。
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LockDoc {
    pub device: String,
    /// 本次启动的随机串：只用于区分"同一台设备的两次运行"，不参与任何判定。
    pub token: String,
    pub expires_at: String,
    /// 贴租约时本机已应用的清单 seq（诊断用）。
    pub seq: u64,
}

impl LockDoc {
    /// 租约是否还"新鲜"。解析不出时间一律按过期处理 —— 过期就是"别挡路"。
    pub fn is_fresh(&self, now_ms: i64) -> bool {
        match Timestamp::parse(&self.expires_at).and_then(|t| t.as_millis()) {
            Some(ms) => ms > now_ms,
            None => false,
        }
    }
}

/// 从 PROPFIND 响应里取成员路径。按 DAV 命名空间里的 `href` 取，
/// 不认前缀写法（`<d:href>` / `<href>` / `<D:href>` 都见过）。
fn propfind_hrefs(body: &[u8]) -> Vec<String> {
    use quick_xml::events::Event;
    use quick_xml::reader::Reader;
    let mut reader = Reader::from_reader(body);
    // 不 trim 每个文本事件：一条 href 现在是**拼出来的**（引用会被拆成单独事件），
    // 逐段 trim 会吃掉引用前后的空格，交给下面收尾时整段 trim。
    reader.config_mut().trim_text(false);
    // 没分号的裸 `&` 放行（默认是整篇报错）。服务器真写出这种不合 XML 的 href 时，
    // 报错 = 这个循环 break = **后面所有别人的租约都看不见** = 两边同时写同一条笔记；
    // 放行 = 路径里多一个 `&`，最坏只是多让一次路。选后者。
    // 摘掉这一行会红在 `an_unrecognized_reference_keeps_the_lease`（第二条租约整个消失）。
    reader.config_mut().allow_dangling_amp = true;
    let mut out = Vec::new();
    let mut inside_href = false;
    let mut buf = String::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) if e.local_name().as_ref() == "href" => {
                inside_href = true;
                buf.clear();
            }
            Ok(Event::End(e)) if e.local_name().as_ref() == "href" => {
                inside_href = false;
                out.push(resolve_refs(&buf));
            }
            // 0.42 起解析器不再顺手反转义：`&amp;` 会单独成一个事件，
            // 只接 Text 就会把一条路径拆成两段（`/a/locks/x` 与 `.json`），
            // 于是那条租约既不在真路径上、也挡不住任何人。
            Ok(Event::Text(t)) if inside_href => buf.push_str(&t.xml10_content()),
            Ok(Event::CData(t)) if inside_href => buf.push_str(&t.xml10_content()),
            Ok(Event::GeneralRef(r)) if inside_href => {
                buf.push('&');
                buf.push_str(&r.xml10_content());
                buf.push(';');
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    out
}

/// 把一段 href 原文里的引用解成字符；**认不出的引用原样留着**。
///
/// 为什么不走旧写法：0.37 那一版这里做的是 `if let Ok(v) = t.unescape()` —— 意思是
/// "有一个解不开的引用就把这条租约整条丢掉"，而主指令禁的正是这种看不见的丢失，
/// 在这一层它的后果是两台设备同时写同一条笔记。现在宁可让路径带着 `&nbsp;`
/// 这种怪样子（顶多多让一次路），也不许少看一条租约。
fn resolve_refs(raw: &str) -> String {
    let trimmed = raw.trim();
    match quick_xml::escape::unescape(trimmed) {
        Ok(v) => v.into_owned(),
        Err(_) => trimmed.to_string(),
    }
}

impl WebDavRemote {
    fn lock_url(&self, device: &str) -> String {
        self.paths().url(&["locks", &format!("{device}.json")])
    }

    fn locks_dir(&self) -> String {
        self.paths().url(&["locks"])
    }

    /// 贴上/续上本设备的租约（无条件 PUT：这一份只有我们写）。
    /// 失败按 best-effort 处理：调用方记一条 debug 就继续，绝不因为贴不上而停止同步。
    pub async fn lease_publish(
        &self,
        device: &str,
        token: &str,
        expires_at: &str,
        seq: u64,
    ) -> Result<(), RemoteError> {
        let doc = LockDoc {
            device: device.to_string(),
            token: token.to_string(),
            expires_at: expires_at.to_string(),
            seq,
        };
        let body = serde_json::to_vec(&doc).map_err(|e| RemoteError::Protocol(e.to_string()))?;
        let url = self.lock_url(device);
        let s = self
            .spec(notera_net::HttpMethod::Put, &url)?
            .with_header("content-type", "application/json")
            .with_body(body);
        let resp = self.send_once(s).await?;
        if !matches!(resp.status, 200 | 201 | 204) {
            return Err(RemoteError::Protocol(format!(
                "租约写入被拒: {}",
                resp.status
            )));
        }
        Ok(())
    }

    /// 尽力删掉自己的租约（正常收尾用；删不掉也会自己过期）。
    pub async fn lease_release(&self, device: &str) {
        let url = self.lock_url(device);
        self.best_effort_delete(&url).await;
    }

    /// 读出**别的设备**的租约。两条来源，都失败就什么也拿不到（= 不挡路）：
    /// * `PROPFIND Depth:1` 列目录（服务器普遍支持；不支持就跳过）；
    /// * 外加调用方给的一组"已知设备 id"逐个 `GET`——这是从清单的
    ///   `generated_by` 学到的对手，恰好就是最可能撞车的那台，且不需要列目录能力。
    pub async fn lease_peers(
        &self,
        device: &str,
        known_devices: &[String],
    ) -> Result<Vec<LockDoc>, RemoteError> {
        let mut found: Vec<LockDoc> = Vec::new();
        let mut seen: Vec<String> = Vec::new();
        for name in self.list_lock_names().await.unwrap_or_default() {
            if let Some(doc) = self.read_lock(&name).await {
                seen.push(doc.device.clone());
                found.push(doc);
            }
        }
        for other in known_devices {
            if other == device || seen.iter().any(|d| d == other) {
                continue;
            }
            if let Some(doc) = self.read_lock(&format!("{other}.json")).await {
                found.push(doc);
            }
        }
        found.retain(|d| d.device != device);
        Ok(found)
    }

    /// 列出 `locks/` 下的文件名。任何不支持/读不出都返回空表。
    async fn list_lock_names(&self) -> Result<Vec<String>, RemoteError> {
        let dir = self.locks_dir();
        let body = br#"<?xml version="1.0"?><d:propfind xmlns:d="DAV:"><d:prop><d:resourcetype/></d:prop></d:propfind>"#;
        let s = self
            .spec(notera_net::HttpMethod::Propfind, &dir)?
            .with_header("depth", "1")
            .with_header("content-type", "application/xml")
            .with_body(body.to_vec());
        let resp = match self.send_once(s).await {
            Ok(r) => r,
            Err(_) => return Ok(Vec::new()),
        };
        if resp.status != 207 {
            return Ok(Vec::new());
        }
        // 只取文件名：目录本身也在响应里，靠 `.json` 后缀滤掉。
        let names = propfind_hrefs(&resp.body)
            .into_iter()
            .map(|h| h.rsplit('/').next().unwrap_or_default().to_string())
            .filter(|tail| tail.ends_with(".json"))
            .collect();
        Ok(names)
    }

    async fn read_lock(&self, name: &str) -> Option<LockDoc> {
        let url = self.paths().url(&["locks", name]);
        let resp = self.get_raw(&url, None).await.ok()?;
        if resp.status != 200 {
            return None;
        }
        serde_json::from_slice(&resp.body).ok()
    }
}

/// PROPFIND 响应解析的**行为锚**：给换 XML 解析库版本时当对照组用。
///
/// 为什么单独钉这一条：`propfind_hrefs` 是我们唯一一处"读服务器给的一堆字节然后决定租约
/// 谁在"的地方，而它依赖三件在不同解析器版本间最容易漂的事 —— 命名空间前缀怎么算 local name、
/// 文本里的实体引用要不要解、`trim_text` 的边界。漂了的后果不是报错，是**看不见的租约**：
/// 列不到别人，于是两边同时写同一份（§11.4 那条互斥就没了）。
#[cfg(test)]
mod parse_pins {
    use super::propfind_hrefs;

    #[test]
    fn hrefs_are_matched_by_local_name_not_by_prefix() {
        let body =
            br#"<?xml version="1.0"?><d:multistatus xmlns:d="DAV:" xmlns:S="http://sabredav.org/">
          <response><S:href> /a/locks/dev1.json </S:href></response>
          <response><href>/b/locks/dev2.json</href></response>
          <response><D:href>/c/locks/dev3.json</D:href></response>
          <response><D:propstat><D:prop><d:resourcetype/></D:prop></D:propstat></response>
        </d:multistatus>"#
                .to_vec();
        assert_eq!(
            propfind_hrefs(&body),
            vec![
                "/a/locks/dev1.json".to_string(),
                "/b/locks/dev2.json".to_string(),
                "/c/locks/dev3.json".to_string(),
            ],
            "换前缀、换大小写、或者首尾空白，都不该让一条租约看不见"
        );
    }

    #[test]
    fn entity_references_inside_href_are_decoded() {
        let body = br#"<?xml version="1.0"?><d:multistatus xmlns:d="DAV:">
          <response><d:href>/x/locks/a&amp;b.json</d:href></response>
          <response><d:href>/x/locks/c&#65;d.json</d:href></response>
          <response><d:href>/x/locks/%E4%B8%AD.json</d:href></response>
        </d:multistatus>"#
            .to_vec();
        assert_eq!(
            propfind_hrefs(&body),
            vec![
                "/x/locks/a&b.json".to_string(),
                "/x/locks/cAd.json".to_string(),
                "/x/locks/%E4%B8%AD.json".to_string(),
            ],
            "实体引用必须解成字符；百分号转义是 URL 层的事，解析器不该替我们解"
        );
    }

    /// 认不出的引用**不许把这条租约弄丢**。
    ///
    /// 这条钉的是这次换版本时改的一个决定：0.37 那版遇到 `&nbsp;` 这种没定义的引用会
    /// 让整条 href 消失（`unescape()` 报错 → `if let Ok` 静默跳过），而"少看一条租约"的
    /// 后果是两条设备同时写同一条笔记。现在宁可把 `&nbsp;` 原样留在路径里。
    #[test]
    fn an_unrecognized_reference_keeps_the_lease() {
        let body = br#"<?xml version="1.0"?><d:multistatus xmlns:d="DAV:">
          <response><d:href>/x/locks/a&nbsp;b.json</d:href></response>
          <response><d:href>/x/locks/c&d.json</d:href></response>
        </d:multistatus>"#
            .to_vec();
        assert_eq!(
            propfind_hrefs(&body),
            vec![
                "/x/locks/a&nbsp;b.json".to_string(),
                "/x/locks/c&d.json".to_string(),
            ],
            "不合 ENML/HTML 的引用与没分号的裸 & 都要留在列表里，一条不许少"
        );
    }
}
