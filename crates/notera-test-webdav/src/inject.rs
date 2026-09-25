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
///   rule := ["post:"] [METHOD " "] pathglob
///   ```
///   * `METHOD` 可选（`PUT`/`GET`/`MOVE`/`PROPFIND`…，大小写不敏感）；
///   * `pathglob` 支持精确、`前缀*`、`*.json` 后缀、纯 `*`；
///   * `post:` 前缀 = **副作用先执行再返回该状态**，即 `FAIL(partial-write)`
///     （服务端已落盘但回 500，SY-FAULT-09 需要这个形态）。
/// * `corrupt_manifest` —— GET/HEAD 的 `*manifest*` 响应在发出前篡改 body 中间 1 字节
///   （`FAIL(corrupt-body)`）。落盘内容不变，因此**只有客户端视角**看到脏数据，
///   sha256 复算必然不一致。
/// * `truncate_upload_at` —— 读够 N 字节就断开（半上传）。服务端**不存储**半份 body。
/// * `require_proxy` —— 只接受"经代理到达"的连接（CONNECT 隧道或绝对形式请求目标），
///   其余一律 403。这是 PROXY.md §9 证据链②。
/// * `reset_after` —— 第 N 个数据请求处理完后清空全部状态（模拟服务器侧被清空）。
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
    pub reset_after: Option<usize>,
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

    /// 该请求是否受注入影响（控制面永不受影响）。
    pub fn applies_to(&self, path: &str) -> bool {
        !path.starts_with("/_control") && !path.starts_with("/_fs")
    }

    /// 规则命中判定。返回 `(status, 是否先执行副作用)`。
    pub fn match_status(&self, method: &str, path: &str) -> Option<(u16, bool)> {
        for (rule, code) in &self.status_for {
            let (rule, post) = match rule.strip_prefix("post:") {
                Some(r) => (r, true),
                None => (rule.as_str(), false),
            };
            let (m, glob) = match rule.split_once(' ') {
                Some((m, g)) => (Some(m.to_ascii_uppercase()), g),
                None => (None, rule),
            };
            if let Some(m) = m {
                if m != method {
                    continue;
                }
            }
            if path_glob(glob, path) {
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
        assert_eq!(
            i.match_status("PUT", "/.notes/records/note/a.json"),
            Some((500, true))
        );
        assert_eq!(i.match_status("GET", "/x.json"), Some((401, false)));
        assert_eq!(i.match_status("DELETE", "/x.json"), None);
    }

    #[test]
    fn control_plane_is_exempt() {
        let i = Injection::hang();
        assert!(!i.applies_to("/_control/reset"));
        assert!(!i.applies_to("/_fs/dump"));
        assert!(i.applies_to("/.notes/manifest/index.json"));
    }
}
