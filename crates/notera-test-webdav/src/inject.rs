//! 故障注入配置与请求日志。
//!
//! **确定性契约**：同一 [`Injection`] + 同一请求序列 ⇒ 同一日志（除 `ts_ms`）与同一
//! 服务端状态。为此本模块禁止使用随机数与墙上时钟做决策 —— 所有判决只依赖
//! "第几个受注入影响的请求"（`seq`）与路径/动词匹配。

use std::fmt;

use serde::{Deserialize, Serialize};

/// 注入配置（对应 docs/TEST-PLAN.md §记法 的 `FAIL(...)` / `OFF` / `LATENCY` / `RESET`）。
///
/// 字段语义：
///
/// * `latency_ms` —— 每个**数据**请求前 `sleep`（`FAIL(latency)`）。控制面不受影响。
/// * `drop_after_n` —— 从第 `n+1` 个数据请求起直接切断 TCP、不回应（`FAIL(abort)`）。
/// * `timeout_all` —— 所有数据请求永不回应并挂住连接（`FAIL(hang)`）。
/// * `status_for` —— 规则表，按序第一条命中生效（`FAIL(status=…,target=…)`）。
///   规则串文法：
///   ```text
///   rule := ["post:"] [METHOD " "] pathglob ["#" N]
///   ```
///   * `METHOD` 可选（`PUT`/`GET`/`MOVE`/`PROPFIND`…，大小写不敏感）；
///   * `pathglob` 支持精确、`前缀*`、`*.json` 后缀、纯 `*`；
///     注意 `*X` 只认**后缀**，没有"包含"这种形态（想要包含写 `*X*` 的前缀+后缀两段规则，
///     或直接写精确路径）；
///   * `#N` = 记法表里的 `times=N`：这条规则只在前 N 次**命中**时生效，之后让位给后面的
///     规则。`hang_for` / `abort_for` 同一份文法、同一个额度机制（额度按列表位置记账）；
///   * `post:` 前缀 = **副作用先执行再返回该状态**，即 `FAIL(partial-write)`
///     （服务端已落盘但回 500，SY-FAULT-09 需要这个形态）。
///   * `#` 是保留分隔符：路径里要字面 `#` 请写 `%23`。
///   * 文法写错（`#0` / `#abc` / 光一个 `#`）在 [`TestServer`] 设置注入时直接 panic ——
///     静默降级会让整条测试绿着而什么都没注入。
/// * `corrupt_manifest` —— GET/HEAD 的 `*manifest*` 响应在发出前篡改 body 中间 1 字节
///   （`FAIL(corrupt-body)`）。落盘内容不变，因此**只有客户端视角**看到脏数据，
///   sha256 复算必然不一致。
/// * `truncate_upload_at` —— 读够 N 字节就断开（半上传）。服务端**不存储**半份 body。
/// * `require_proxy` —— 只接受"经代理到达"的连接（CONNECT 隧道或绝对形式请求目标），
///   其余一律 403。这是 PROXY.md §9 证据链②。
/// * `ignore_range` —— 把请求里的 `Range` 头摘掉再交给 handler，于是答 `200` + 全文
///   （`FAIL(ignore-range)`）。§14 兼容矩阵要的就是这个形态：**探测时老实答 206、
///   正式请求却忽略 Range**（后面挂了个不认 Range 的节点），客户端必须按"这是整份"处理。
/// * `reset_after` —— 第 N 个数据请求处理完后清空全部状态（模拟服务器侧被清空）。
/// * `hang_for` —— `FAIL(hang,target=…)`：只挂命中规则的请求，其余照常服务。
///   与 `timeout_all`（整个服务器不应答）是两种脾气，§27「只有附件端点超时，
///   文本轮必须照常完成」要的是前者。
/// * `abort_for` —— `FAIL(abort,target=…)`：只掐命中规则的请求的连接，其余照常服务。
///   与 `drop_after_n`（从第 n+1 个数据请求起全部断）是两种脾气：§27「上传中断」要的是
///   "暂存写得好好的，只有发布那一步（MOVE）被掐"这种**按动作**的形态。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Injection {
    pub latency_ms: u64,
    pub drop_after_n: Option<usize>,
    pub timeout_all: bool,
    pub status_for: Vec<(String, u16)>,
    pub corrupt_manifest: bool,
    pub truncate_upload_at: Option<usize>,
    pub require_proxy: bool,
    pub ignore_range: bool,
    pub reset_after: Option<usize>,
    /// `FAIL(hang, target=…)` —— 只挂**命中规则**的那些请求，其余照常服务。
    ///
    /// 为什么单独有它而不复用 `timeout_all`：那个是全服务器挂死，用来测"网络不通"；
    /// 而 §27 要的形态是"**只有附件端点**不回应，文本轮必须照常完成"。两种脾气在真实
    /// 服务器上都很常见（附件走的是另一个反代 / 另一个存储桶），用一个全局挂起去测它，
    /// 测到的是别的东西。
    pub hang_for: Vec<String>,
    /// `FAIL(abort, target=…)` —— 只掐**命中规则**的那些请求的连接，其余照常服务。
    ///
    /// 为什么单独有它而不只用 `drop_after_n`：那个按"第几个数据请求"计数，一开就是从那
    /// 个数往后**全部**断连，测到的是"网络整体断了"。§27 的「上传中断」要的是精确形态
    /// —— 比如 PUT 到暂存的好好的，只有**发布那一步（MOVE）**被中间设备掐掉。那种"半路
    /// 断在一半"的形态只有按路径/动词掐才造得出来。
    ///
    /// 规则文法与 `status_for`、`hang_for` **同一份**（`rule_hit`）：三套各写一遍迟早漂成
    /// 三种语法，而漂掉的 matcher 在消费者那里表现为"产品没问题"。
    pub abort_for: Vec<String>,
}

impl Injection {
    pub fn none() -> Injection {
        Injection::default()
    }

    /// `FAIL(status=code, target=pattern)`
    pub fn status(pattern: impl Into<String>, code: u16) -> Injection {
        Injection {
            status_for: vec![(pattern.into(), code)],
            ..Default::default()
        }
    }

    /// `FAIL(partial-write, target=pattern)`：落盘后仍返回 code。
    pub fn partial_write(pattern: impl Into<String>, code: u16) -> Injection {
        Injection {
            status_for: vec![(format!("post:{}", pattern.into()), code)],
            ..Default::default()
        }
    }

    /// `FAIL(hang)`
    pub fn hang() -> Injection {
        Injection {
            timeout_all: true,
            ..Default::default()
        }
    }

    /// `FAIL(hang,target=pattern)` —— 只挂命中这一条规则（文法同 `status_for`）的请求。
    pub fn hang_on(pattern: impl Into<String>) -> Injection {
        Injection {
            hang_for: vec![pattern.into()],
            ..Default::default()
        }
    }

    /// 从规则尾部剥出 `#N`（= 记法表里的 `times=N`，命中 N 次之后这条规则不再命中）。
    ///
    /// 文法写错**不许静默降级**：作者写 `#abc` / `#0` 时以为自己在限次数，而"当成无限次"
    /// 会让那条测试红在别处 —— 这正是本仓库反复踩的那一类。解析错误由 [`Injection::check_rules`]
    /// 在 `inject()` 那一刻（测试自己的线程里）报出来；这里的 `ok()?` 只是第二道兜底。
    /// 因此 `#` 成为保留分隔符：要匹配字面 `#` 请写百分号编码 `%23`（现有规则里没有这种路径）。
    fn split_times(rule: &str) -> Result<(&str, Option<u64>), String> {
        let Some((glob, n)) = rule.rsplit_once('#') else {
            return Ok((rule, None));
        };
        let trimmed = n.trim();
        if trimmed.is_empty() || trimmed.starts_with('-') {
            return Err(format!("规则 `{rule}` 的 #N 不是正整数"));
        }
        match trimmed.parse::<u64>() {
            Ok(0) | Err(_) => Err(format!(
                "规则 `{rule}` 的 #N 不是正整数（#0 没有意义：别写这条规则就是了）"
            )),
            Ok(limit) => Ok((glob, Some(limit))),
        }
    }

    /// 这套规则文法的自检：`inject()` 在设置注入时调用，写错立刻 panic（而不是让测试挂在别处）。
    pub fn check_rules(&self) -> Result<(), String> {
        for rule in self
            .status_for
            .iter()
            .map(|(r, _)| r)
            .chain(self.hang_for.iter())
            .chain(self.abort_for.iter())
        {
            Self::split_times(rule)?;
        }
        Ok(())
    }

    /// 这条规则**本次**是否该生效（命中且没超出 `#N` 的额度）。
    /// `None` = 不生效；`Some(post)` = 生效，并带回"是否先落盘再回错"那个 `post:` 标记。
    ///
    /// 台账的键是**「哪条列表 + 第几个位置」**（`key` 由调用方给出），不是规则原文：
    /// 按原文记账会让 `status_for` 与 `abort_for` 里恰好同形的两条**共用一份额度**，
    /// 于是第二条静默失效 —— 而"静默失效"正是这套工装最不该有的失败模式。
    /// 按位置记账的代价是"规则列表中途变更会让额度错位"，而 [`Injection`] 在一次注入里
    /// 是不可变的（换配置走 `TestServer::inject`，那里连台账一起清）。
    fn rule_hit(
        rule: &str,
        method: &str,
        path: &str,
        key: &str,
        hits: &mut std::collections::HashMap<String, u64>,
    ) -> Option<bool> {
        let (rule, limit) = Self::split_times(rule).ok()?;
        let (rule, post) = match rule.strip_prefix("post:") {
            Some(r) => (r, true),
            None => (rule, false),
        };
        let (m, glob) = match rule.split_once(' ') {
            Some((m, g)) => (Some(m.to_ascii_uppercase()), g),
            None => (None, rule),
        };
        if let Some(m) = m {
            if m != method {
                return None;
            }
        }
        if !path_glob(glob, path) {
            return None;
        }
        let Some(limit) = limit else {
            return Some(post);
        };
        let seen = hits.entry(key.to_owned()).or_insert(0);
        if *seen >= limit {
            return None;
        }
        *seen += 1;
        Some(post)
    }

    /// 该请求是否要被**挂住**（永不回应）。`hits` 是 `#N` 的台账（见 [`Self::rule_hit`]）。
    pub fn hangs(
        &self,
        method: &str,
        path: &str,
        hits: &mut std::collections::HashMap<String, u64>,
    ) -> bool {
        self.hang_for.iter().enumerate().any(|(i, rule)| {
            Self::rule_hit(rule, method, path, &format!("hang#{i}"), hits).is_some()
        })
    }

    /// `FAIL(abort,target=pattern)` —— 只掐命中规则的请求，其余照常服务。
    pub fn abort_on(pattern: impl Into<String>) -> Injection {
        Injection {
            abort_for: vec![pattern.into()],
            ..Default::default()
        }
    }

    /// 该请求是否要被**掐断**（连接直接断，不回应）。与 [`Injection::hangs`] 同文法、
    /// 两种脾气：hang 是"对面不答应"，abort 是"对面挂了"—— 客户端的超时路径与
    /// 传输错误路径不是同一条，所以两个旋钮必须分开存在。
    pub fn aborts(
        &self,
        method: &str,
        path: &str,
        hits: &mut std::collections::HashMap<String, u64>,
    ) -> bool {
        self.abort_for.iter().enumerate().any(|(i, rule)| {
            Self::rule_hit(rule, method, path, &format!("abort#{i}"), hits).is_some()
        })
    }

    /// `FAIL(latency,ms=…)`
    pub fn latency(ms: u64) -> Injection {
        Injection {
            latency_ms: ms,
            ..Default::default()
        }
    }

    /// `FAIL(abort)` —— 从第 `after_n+1` 个请求起断连。
    pub fn abort_after(after_n: usize) -> Injection {
        Injection {
            drop_after_n: Some(after_n),
            ..Default::default()
        }
    }

    /// `FAIL(ignore-range)` —— 忽略客户端的 `Range` 头，一律答 `200` + 全文。
    pub fn ignore_range() -> Injection {
        Injection {
            ignore_range: true,
            ..Default::default()
        }
    }

    /// 该请求是否受注入影响（控制面永不受影响）。
    pub fn applies_to(&self, path: &str) -> bool {
        !path.starts_with("/_control") && !path.starts_with("/_fs")
    }

    /// 规则命中判定。返回 `(status, 是否先执行副作用)`。
    ///
    /// `hits` 是 `#N`（times）的台账：由服务器那份 `Store` 持有，换一次注入就清空。
    /// 没有它，"两次 503 然后好"这种形态就表达不出来 —— 而那正是 SY-FAULT-03/04 与
    /// §28「重试」那一格唯一能写成判据的形状。
    pub fn match_status(
        &self,
        method: &str,
        path: &str,
        hits: &mut std::collections::HashMap<String, u64>,
    ) -> Option<(u16, bool)> {
        for (i, (rule, code)) in self.status_for.iter().enumerate() {
            if let Some(post) = Self::rule_hit(rule, method, path, &format!("status#{i}"), hits) {
                return Some((*code, post));
            }
        }
        None
    }
}

/// `pathglob`：精确 / `前缀*` / `*后缀` / 纯 `*`（以及 `**` 视作 `*`）。
pub fn path_glob(glob: &str, path: &str) -> bool {
    let g = glob.trim();
    if g == "*" || g == "**" {
        return true;
    }
    if let Some(prefix) = g.strip_suffix('*') {
        if prefix.is_empty() {
            return true;
        }
        return path.starts_with(prefix);
    }
    if let Some(suffix) = g.strip_prefix('*') {
        return path.ends_with(suffix);
    }
    g == path
}

/// 一条请求审计。`status == 0` 表示"没有响应"（被丢弃 / 挂起 / 半上传切断）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoggedRequest {
    pub seq: u64,
    pub method: String,
    pub path: String,
    pub status: u16,
    /// 请求 body 字节数（半上传时为已读到的字节数）。
    pub bytes: u64,
    /// 墙上时钟（毫秒），仅用于人读诊断；确定性断言一律排除它。
    pub ts_ms: u64,
}

impl LoggedRequest {
    /// 去掉时间戳后的可比对指纹 —— "两次相同注入的 request_log 一致"就比这个。
    pub fn signature(&self) -> (u64, &str, &str, u16, u64) {
        (self.seq, &self.method, &self.path, self.status, self.bytes)
    }
}

impl fmt::Display for LoggedRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "#{} {} {} -> {} ({} B)",
            self.seq, self.method, self.path, self.status, self.bytes
        )
    }
}

/// 状态里累计的"被丢弃连接"计数（不进 `request_log`，只进 `/_control/inspect`）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InjectionCounters {
    pub connections_dropped: u64,
    pub hung: u64,
    pub truncated: u64,
    pub corrupted: u64,
    pub rejections_403_not_proxied: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_forms() {
        assert!(path_glob("*", "/a/b"));
        assert!(path_glob("/.notes/*", "/.notes/records/note/x.json"));
        assert!(!path_glob("/.notes/*", "/other"));
        assert!(path_glob("*.json", "/a/b.json"));
        assert!(path_glob("/a/b.json", "/a/b.json"));
        assert!(!path_glob("/a/b.json", "/a/b.jsonx"));
    }

    #[test]
    fn rule_grammar() {
        let i = Injection {
            status_for: vec![
                ("post:PUT /.notes/records/*".into(), 500),
                ("GET /x.json".into(), 401),
            ],
            ..Default::default()
        };
        let mut hits = std::collections::HashMap::new();
        assert_eq!(
            i.match_status("PUT", "/.notes/records/note/a.json", &mut hits),
            Some((500, true))
        );
        assert_eq!(
            i.match_status("GET", "/x.json", &mut hits),
            Some((401, false))
        );
        assert_eq!(i.match_status("DELETE", "/x.json", &mut hits), None);
    }

    /// `#N`（times）：这条规则只在前 N 次**命中**时生效 —— SY-FAULT-03/04 与 §28「重试」
    /// 那一格唯一能写成判据的形状（"两次 503 然后好"没有它就只能靠猜）。
    ///
    /// 这里的 glob 一律用文法真支持的形态（精确 / `前缀*` / `*后缀` / 纯 `*`）。
    /// 上一版我写了 `*flaky*.json` 当作"包含"，`path_glob` 里 `*X` 只认**后缀**，于是那条
    /// 规则永远不命中 —— 红得很安静，看起来像"#N 没实现"。文法不认识的写法不会报错，
    /// 只会静默不命中，所以判据要挑形状。
    #[test]
    fn times_limit_fires_exactly_n_times_then_stops() {
        let i = Injection {
            status_for: vec![("GET *flaky.json#2".into(), 503)],
            ..Default::default()
        };
        let path = "/.notes/records/flaky.json";
        let mut hits = std::collections::HashMap::new();
        assert_eq!(i.match_status("GET", path, &mut hits), Some((503, false)));
        assert_eq!(i.match_status("GET", path, &mut hits), Some((503, false)));
        assert_eq!(
            i.match_status("GET", path, &mut hits),
            None,
            "额度用完之后这条规则还在生效"
        );

        // 额度只按**命中**记账：动词或路径不打在这条规则上的请求不许吃掉它（否则
        // "重试两次"会变成"一轮里随机两次"，判据就没了确定性）。
        let mut fresh = std::collections::HashMap::new();
        assert_eq!(i.match_status("PUT", path, &mut fresh), None);
        assert_eq!(
            i.match_status("GET", "/.notes/records/other.json", &mut fresh),
            None
        );
        assert_eq!(i.match_status("GET", path, &mut fresh), Some((503, false)));
        assert_eq!(i.match_status("GET", path, &mut fresh), Some((503, false)));
        assert_eq!(i.match_status("GET", path, &mut fresh), None);

        // 额度用完之后由**下一条**规则接管，不是直接放行 —— 这正是"先 503 两次再 500"
        // 这类组合的写法，必须能被表达。
        let j = Injection {
            status_for: vec![("GET *flaky.json#2".into(), 503), ("GET *".into(), 500)],
            ..Default::default()
        };
        let mut k = std::collections::HashMap::new();
        assert_eq!(j.match_status("GET", path, &mut k), Some((503, false)));
        assert_eq!(j.match_status("GET", path, &mut k), Some((503, false)));
        assert_eq!(
            j.match_status("GET", path, &mut k),
            Some((500, false)),
            "第一条用完额度后没有落到下一条"
        );
    }

    /// 三条列表**各自**都要吃到 `#N`，且台账按位置记账、互不串用（同一份 hits 传三处）。
    /// 键若是规则原文，`status_for` 与 `abort_for` 里同形的那两条会共用一份额度，
    /// 第二条静默失效 —— 这条测试就是钉住那个。
    #[test]
    fn times_limit_applies_to_hang_and_abort_rules_too() {
        let mut hits = std::collections::HashMap::new();
        let hang = Injection {
            hang_for: vec!["GET /a/once*#1".into()],
            ..Default::default()
        };
        assert!(hang.hangs("GET", "/a/once.json", &mut hits));
        assert!(
            !hang.hangs("GET", "/a/once.json", &mut hits),
            "hang 的 #1 额度没用上"
        );
        let abort = Injection {
            abort_for: vec!["GET /a/once*#1".into()],
            ..Default::default()
        };
        // 同一条规则原文、挂在另一张列表上：hang 已经把它的额度吃光，abort 必须还有一次自己的。
        assert!(
            abort.aborts("GET", "/a/once.json", &mut hits),
            "台账按规则原文记账，把 abort 的额度也扣掉了"
        );
        assert!(!abort.aborts("GET", "/a/once.json", &mut hits));

        // 同一张列表里写两遍同一条规则 = 两份额度（按序第一条先命中，用完才轮到第二条）。
        let dup = Injection {
            status_for: vec![("GET /a/x*#1".into(), 503), ("GET /a/x*#1".into(), 500)],
            ..Default::default()
        };
        let mut d = std::collections::HashMap::new();
        assert_eq!(
            dup.match_status("GET", "/a/x.json", &mut d),
            Some((503, false))
        );
        assert_eq!(
            dup.match_status("GET", "/a/x.json", &mut d),
            Some((500, false))
        );
        assert_eq!(dup.match_status("GET", "/a/x.json", &mut d), None);
    }

    /// 文法写错要**当场报错**，不许被当成"这条规则永不命中"或"无限次"。
    /// 静默的那两种后果都是"测试绿着而什么都没注入"。
    #[test]
    fn malformed_times_suffix_is_rejected() {
        for bad in [
            "GET /a.json#abc",
            "GET /a.json#0",
            "GET /a.json#",
            "GET /a.json#-2",
        ] {
            let i = Injection {
                status_for: vec![(bad.into(), 503)],
                ..Default::default()
            };
            assert!(i.check_rules().is_err(), "这条规则应当被拒：{bad}");
        }
        let ok = Injection {
            status_for: vec![("GET /a.json#3".into(), 503)],
            hang_for: vec!["PUT /b#1".into()],
            abort_for: vec!["post:MOVE /c#9".into()],
            ..Default::default()
        };
        assert_eq!(ok.check_rules(), Ok(()));
    }

    #[test]
    fn control_plane_is_exempt() {
        let i = Injection::hang();
        assert!(!i.applies_to("/_control/reset"));
        assert!(!i.applies_to("/_fs/dump"));
        assert!(i.applies_to("/.notes/manifest/index.json"));
    }
}
