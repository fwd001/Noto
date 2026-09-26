//! 本地 dev 桥：把同一份 `commands::dispatch` 以 HTTP 暴露给纯浏览器前端。
//!
//! 为什么存在：桌面 UI 的黑盒验证（TEST-PLAN L5）需要能被自动化驱动。
//! WebView2 可用时应当直接测真窗口；但 CI/开发机上用 Playwright 驱动
//! "浏览器里的同一份前端 + 同一份 Rust 核心" 能拿到同等强度的结论。
//!
//! **这不是 mock**：落到的是真实 SQLite、真实 Store、真实同步引擎。
//! 绑定 127.0.0.1，且只在 dev 构建里启用（release 由 host 的 feature 关掉）。

use crate::{App, BusEvent};
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub const DEFAULT_PORT: u16 = 17323;

#[derive(Clone)]
pub struct DevServer {
    stop: Arc<AtomicBool>,
}

pub fn start(app: App, port: u16) -> std::io::Result<DevServer> {
    // 发布版里这条命令面一律不开：它能不带任何凭据地驱动真实 Store（删笔记、清库、
    // 改账户配置）。绑 127.0.0.1 与 Origin 白名单都不足以让它进 release ——
    // 同机上的任意进程都能连。留在编译期之外还有一层好处：调用方拿到的就是
    // 一个 PermissionDenied，而不是"函数不存在"这种编译期惊喜。
    if !cfg!(debug_assertions) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "dev 桥只在 debug 构建启用",
        ));
    }
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().expect("loopback addr");
    let listener = TcpListener::bind(addr)?;
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            if stop2.load(Ordering::SeqCst) {
                break;
            }
            let Ok(stream) = stream else { continue };
            let app = app.clone();
            std::thread::spawn(move || {
                let _ = handle(app, stream);
            });
        }
    });
    Ok(DevServer { stop })
}

impl DevServer {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn handle(app: App, mut stream: TcpStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Ok(());
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("GET").to_string();
    let target = parts.next().unwrap_or("/").to_string();

    let mut headers = Vec::new();
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h)? == 0 {
            break;
        }
        let t = h.trim_end();
        if t.is_empty() {
            break;
        }
        if let Some((k, v)) = t.split_once(':') {
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
    }
    let len = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; len];
    if len > 0 {
        std::io::Read::read_exact(&mut reader, &mut body)?;
    }

    // 开发桥只接受本机来源，避免把命令面暴露到局域网
    if !same_origin_ok(&headers) {
        return respond(&mut stream, 403, "application/json", br#"{"error":"origin"}"#);
    }

    if method == "OPTIONS" {
        return respond(&mut stream, 204, "text/plain", b"");
    }

    if target.starts_with("/cmd/") {
        let name = target.trim_start_matches("/cmd/").split('?').next().unwrap_or("");
        let args: serde_json::Value = if body.is_empty() { Ok(serde_json::Value::Null) } else { serde_json::from_slice(&body) }
            .unwrap_or(serde_json::Value::Null);
        // 与 Tauri 通道**同一形状**：成功是裸 DTO（`invoke` 直接 resolve 出值），
        // 失败是 CmdError。曾经这里包了一层 {ok,value}，前端 unwrap 拿不到 payload，
        // 浏览器模式下每条命令都变成 null —— 两条通道必须是一条契约。
        let (status, payload) = match crate::commands::dispatch(&app, name, args) {
            Ok(v) => (200u16, v),
            Err(e) => (400, serde_json::to_value(&e).unwrap_or(serde_json::json!({ "code": "storage" }))),
        };
        return respond(&mut stream, status, "application/json", payload.to_string().as_bytes());
    }

    if target.starts_with("/events") {
        // SSE：头里必须带 CORS（少了就是浏览器 ERR_FAILED），并且一直推到客户端断开。
        let rx = app.subscribe();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: keep-alive\r\nAccess-Control-Allow-Origin: *\r\n\r\n"
        )?;
        stream.flush()?;
        loop {
            match rx.recv_timeout(std::time::Duration::from_millis(500)) {
                Ok(event) => {
                    let s = serde_json::to_string(&event).unwrap_or_default();
                    // 写失败 = 浏览器已经走了；此时退出，App 侧的死订阅会被 emit 清理。
                    if writeln!(stream, "data: {s}\n").is_err() || stream.flush().is_err() {
                        return Ok(());
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    if writeln!(stream, ": ping\n").is_err() || stream.flush().is_err() {
                        return Ok(());
                    }
                }
            }
        }
    }

    if target.starts_with("/health") {
        return respond(&mut stream, 200, "application/json", br#"{"ok":true}"#);
    }

    respond(&mut stream, 404, "application/json", br#"{"error":"not found"}"#)
}

fn same_origin_ok(headers: &[(String, String)]) -> bool {
    let Some(origin) = headers.iter().find(|(k, _)| k == "origin").map(|(_, v)| v.clone()) else {
        return true; // 非浏览器客户端（curl / 测试）
    };
    let o = origin.trim_end_matches('/').to_ascii_lowercase();
    if o == "tauri://localhost" || o == "tauri:" || o == "http://tauri.localhost" || o == "https://tauri.localhost" {
        return true;
    }
    // 必须解析出**主机部分**再比较：`starts_with("http://127.0.0.1:")` 之外的
    // 前缀写法会放过 http://127.0.0.1.evil.example 这类仿冒域。
    let Some(rest) = o.strip_prefix("http://").or_else(|| o.strip_prefix("https://")) else {
        return false;
    };
    let host_port = rest.split('/').next().unwrap_or("");
    let host = host_port
        .rsplit_once(':')
        .map(|(h, _p)| h.trim_matches(|c| c == '[' || c == ']'))
        .unwrap_or(host_port);
    matches!(host, "127.0.0.1" | "localhost" | "::1")
}

fn respond(stream: &mut TcpStream, status: u16, ctype: &str, body: &[u8]) -> std::io::Result<()> {
    let text = match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        403 => "Forbidden",
        _ => "Not Found",
    };
    write!(
        stream,
        "HTTP/1.1 {status} {text}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Headers: content-type\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)?;
    stream.flush()
}

/// 事件 → 前端可见形状（与 UI 契约一致）。
pub fn to_json(e: &BusEvent) -> serde_json::Value {
    serde_json::to_value(e).unwrap_or(serde_json::Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmpdir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("notera-dev-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn unknown_command_reports_machine_readable_error() {
        let app = App::boot(&tmpdir("cmd")).unwrap();
        let err = crate::commands::dispatch(&app, "nope", serde_json::json!({})).unwrap_err();
        assert_eq!(err.code, "unknown_command");
        assert_eq!(err.message_key, "cmd.unknown_command");
        assert!(!err.retryable);
    }

    #[test]
    fn origin_filter_blocks_external_pages() {
        // 桥只服务本机前端。任意网页都能打命令面 = 把"删库"能力开放给任何打开过的页面。
        assert!(same_origin_ok(&[]), "无 Origin 头（curl/测试）应放行");
        assert!(same_origin_ok(&[("origin".into(), "http://127.0.0.1:5173".into())]));
        assert!(same_origin_ok(&[("origin".into(), "http://localhost:5173".into())]));
        assert!(same_origin_ok(&[("origin".into(), "tauri://localhost".into())]));
        assert!(!same_origin_ok(&[("origin".into(), "http://evil.example".into())]), "外部来源必须拒绝");
        assert!(!same_origin_ok(&[("origin".into(), "http://127.0.0.1.evil.example".into())]), "前缀匹配不得放过仿冒域");
    }

    /// 两条通道必须是**一条契约**：HTTP 桥的响应体要与 Tauri `invoke` resolve 出来的值逐字节同形。
    /// 漂了不会编译错，只会让浏览器模式下每条命令静默变成 null。
    #[test]
    fn http_channel_returns_the_same_shape_as_tauri_invoke() {
        use std::io::{Read, Write};
        let app = App::boot(&tmpdir("shape")).unwrap();
        let (port, _server) = (18900u16..18940)
            .find_map(|p| match start(app.clone(), p) {
                Ok(s) => Some((p, s)),
                Err(_) => None,
            })
            .expect("找不到可用的测试端口");

        let expected = crate::commands::dispatch(&app, "stats", serde_json::json!({})).unwrap();
        let body = {
            let mut c = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
            write!(
                c,
                "POST /cmd/stats HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{{}}"
            )
            .unwrap();
            let mut buf = String::new();
            c.read_to_string(&mut buf).unwrap();
            buf.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default()
        };
        let got: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(got, expected, "HTTP 通道不得再包 {{ok,value}} 一层");
        assert!(got.get("notes").is_some(), "应为裸 DTO 而不是封套：{got}");
    }
}
