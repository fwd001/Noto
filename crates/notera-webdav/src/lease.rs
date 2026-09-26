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
    reader.config_mut().trim_text(true);
    let mut out = Vec::new();
    let mut inside_href = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) if e.local_name().as_ref() == b"href" => inside_href = true,
            Ok(Event::End(e)) if e.local_name().as_ref() == b"href" => inside_href = false,
            Ok(Event::Text(t)) if inside_href => {
                if let Ok(v) = t.unescape() {
                    out.push(v.trim().to_string());
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    out
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
    pub async fn lease_publish(&self, device: &str, token: &str, expires_at: &str, seq: u64) -> Result<(), RemoteError> {
        let doc = LockDoc { device: device.to_string(), token: token.to_string(), expires_at: expires_at.to_string(), seq };
        let body = serde_json::to_vec(&doc).map_err(|e| RemoteError::Protocol(e.to_string()))?;
        let url = self.lock_url(device);
        let s = self
            .spec(notera_net::HttpMethod::Put, &url)?
            .with_header("content-type", "application/json")
            .with_body(body);
        let resp = self.send_once(s).await?;
        if !matches!(resp.status, 200 | 201 | 204) {
            return Err(RemoteError::Protocol(format!("租约写入被拒: {}", resp.status)));
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
    pub async fn lease_peers(&self, device: &str, known_devices: &[String]) -> Result<Vec<LockDoc>, RemoteError> {
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
