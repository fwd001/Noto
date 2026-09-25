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
    let mut parts = line.trim_end().split_whitespace();
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
        let (status, payload) = match crate::commands::dispatch(&app, name, args) {
            Ok(v) => (200u16, serde_json::json!({ "ok": true, "value": v })),
            Err(e) => (400, serde_json::json!({ "ok": false, "error": e })),
        };
        return respond(&mut stream, status, "application/json", payload.to_string().as_bytes());
    }

    if target.starts_with("/events") {
        // 极简 SSE：把事件总线的内容按行推给前端
        let rx = app.subscribe();
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: keep-alive\r\n\r\n")?;
        stream.flush()?;
        for _ in 0..3 {
            if let Ok(e) = rx.try_recv() {
                let s = serde_json::to_string(&e).unwrap_or_default();
                let _ = write!(stream, "data: {s}\n\n");
                let _ = stream.flush();
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        return Ok(());
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
        .map(|(h, p)| h.trim_matches(|c| c == '[' || c == ']'))
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
}
