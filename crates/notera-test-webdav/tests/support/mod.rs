//! 集成测试用的**裸 socket** HTTP/1.1 客户端。
//!
//! 刻意不用 `reqwest`：test-webdav 的自测必须独立于产品侧客户端实现，
//! 否则两边同时错就测不出来（docs/TEST-PLAN.md §"其自测必须与 notera-webdav
//! 客户端测试分离编写"）。

#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

#[derive(Debug, Clone)]
pub struct RawResp {
    pub version: String,
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl RawResp {
    pub fn header(&self, name: &str) -> Option<&str> {
        let n = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(k, _)| *k == n)
            .map(|(_, v)| v.as_str())
    }
    pub fn etag(&self) -> Option<&str> {
        self.header("etag")
    }
}

/// 组一个 `Content-Length` 形式的请求。
pub fn build_request(method: &str, path: &str, headers: &[H], body: &[u8]) -> Vec<u8> {
    let mut out = format!("{method} {path} HTTP/1.1\r\n").into_bytes();
    let mut has_host = false;
    for (k, v) in headers {
        if k.eq_ignore_ascii_case("host") {
            has_host = true;
        }
        out.extend_from_slice(format!("{k}: {v}\r\n").as_bytes());
    }
    if !has_host {
        out.extend_from_slice(b"host: 127.0.0.1\r\n");
    }
    out.extend_from_slice(format!("content-length: {}\r\n", body.len()).as_bytes());
    out.extend_from_slice(b"\r\n");
    out.extend_from_slice(body);
    out
}

/// 组一个 `Transfer-Encoding: chunked` 形式的请求（分片自定）。
pub fn build_chunked_request(method: &str, path: &str, headers: &[H], chunks: &[&[u8]]) -> Vec<u8> {
    let mut out = format!("{method} {path} HTTP/1.1\r\n").into_bytes();
    for (k, v) in headers {
        out.extend_from_slice(format!("{k}: {v}\r\n").as_bytes());
    }
    out.extend_from_slice(b"host: 127.0.0.1\r\ntransfer-encoding: chunked\r\n\r\n");
    for c in chunks {
        out.extend_from_slice(format!("{:x}\r\n", c.len()).as_bytes());
        out.extend_from_slice(c);
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(b"0\r\n\r\n");
    out
}

pub async fn send(
    addr: std::net::SocketAddr,
    method: &str,
    path: &str,
    headers: &[H],
    body: &[u8],
) -> RawResp {
    let mut stream = TcpStream::connect(addr).await.expect("connect");
    stream
        .write_all(&build_request(method, path, headers, body))
        .await
        .expect("write");
    // HEAD 的响应只有头部没有正文，不能按 Content-Length 去等正文。
    read_response_head(&mut stream, method == "HEAD").await
}

/// 一次请求，但显式带 `Connection: close`。
pub async fn send_once(
    addr: std::net::SocketAddr,
    method: &str,
    path: &str,
    headers: &[H],
    body: &[u8],
) -> RawResp {
    let mut h: Vec<H> = headers.to_vec();
    h.push(("connection", "close".into()));
    send(addr, method, path, &h, body).await
}

pub async fn send_raw_bytes(addr: std::net::SocketAddr, bytes: &[u8]) -> RawResp {
    let mut stream = TcpStream::connect(addr).await.expect("connect");
    stream.write_all(bytes).await.expect("write");
    read_response(&mut stream).await
}

/// keep-alive：同一连接上依次发多个请求，逐个收响应。
pub async fn send_pipeline(addr: std::net::SocketAddr, requests: &[Vec<u8>]) -> Vec<RawResp> {
    let mut stream = TcpStream::connect(addr).await.expect("connect");
    let mut out = Vec::new();
    for req in requests {
        stream.write_all(req).await.expect("write req");
        out.push(read_response(&mut stream).await);
    }
    out
}

pub async fn read_response(stream: &mut TcpStream) -> RawResp {
    read_response_head(stream, false).await
}

/// `no_body`：HEAD 用 —— 头部里虽然有 `Content-Length`，但服务端不写正文。
pub async fn read_response_head(stream: &mut TcpStream, no_body: bool) -> RawResp {
    let mut buf: Vec<u8> = Vec::with_capacity(4096);
    let mut byte = [0u8; 1];
    // 先读到头部结束。
    loop {
        match stream.read(&mut byte).await {
            Ok(0) => break,
            Ok(_) => {
                buf.push(byte[0]);
                if buf.len() >= 4 && &buf[buf.len() - 4..] == b"\r\n\r\n" {
                    break;
                }
            }
            Err(e) => panic!("读响应失败: {e}"),
        }
    }
    let head_end = buf.len();
    let text = String::from_utf8_lossy(&buf).into_owned();
    let (head, rest) = text.split_once("\r\n\r\n").unwrap_or((text.as_str(), ""));
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or("");
    let mut sp = status_line.splitn(3, ' ');
    let version = sp.next().unwrap_or("HTTP/1.1").to_string();
    let status: u16 = sp.next().unwrap_or("0").parse().unwrap_or(0);
    let mut headers = Vec::new();
    for l in lines {
        if let Some((k, v)) = l.split_once(':') {
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
    }
    let mut body: Vec<u8> = rest.as_bytes().to_vec();
    if !no_body && status != 304 && status != 204 {
        if let Some(cl) = headers
            .iter()
            .find(|(k, _)| k == "content-length")
            .and_then(|(_, v)| v.parse::<usize>().ok())
        {
            while body.len() < cl {
                match stream.read(&mut byte).await {
                    Ok(0) => break,
                    Ok(_) => body.push(byte[0]),
                    Err(e) => panic!("读 body 失败: {e}"),
                }
            }
            body.truncate(cl);
        } else if headers
            .iter()
            .any(|(k, v)| k == "transfer-encoding" && v.contains("chunked"))
        {
            body = read_chunked_body(stream, body, head_end).await;
        } else {
            // 无长度：读到关闭。
            loop {
                match stream.read(&mut byte).await {
                    Ok(0) => break,
                    Ok(_) => body.push(byte[0]),
                    Err(_) => break,
                }
            }
        }
    }
    RawResp {
        version,
        status,
        headers,
        body,
    }
}

async fn read_chunked_body(
    stream: &mut TcpStream,
    mut have: Vec<u8>,
    consumed_head: usize,
) -> Vec<u8> {
    let _ = consumed_head;
    let mut out = Vec::new();
    loop {
        let line = read_until_blank_line(stream, &mut have).await;
        let size =
            usize::from_str_radix(line.split(';').next().unwrap_or("").trim(), 16).unwrap_or(0);
        if size == 0 {
            break;
        }
        while have.len() < size + 2 {
            let mut b = [0u8; 1];
            match stream.read(&mut b).await {
                Ok(0) => break,
                Ok(_) => have.push(b[0]),
                Err(_) => break,
            }
        }
        out.extend_from_slice(&have[..size]);
        have.drain(..size + 2);
    }
    out
}

async fn read_until_blank_line(stream: &mut TcpStream, have: &mut Vec<u8>) -> String {
    let mut byte = [0u8; 1];
    loop {
        if let Some(pos) = find_crlf(have) {
            let line = String::from_utf8_lossy(&have[..pos]).into_owned();
            have.drain(..pos + 2);
            return line;
        }
        match stream.read(&mut byte).await {
            Ok(0) => return String::new(),
            Ok(_) => have.push(byte[0]),
            Err(_) => return String::new(),
        }
    }
}

fn find_crlf(buf: &[u8]) -> Option<usize> {
    buf.windows(2).position(|w| w == b"\r\n")
}

/// 唯一临时目录（不引入 `tempfile` 依赖）。
pub fn tmp_dir(tag: &str) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let id = N.fetch_add(1, Ordering::SeqCst);
    let mut p = std::env::temp_dir();
    p.push(format!(
        "notera-test-webdav-{tag}-{}-{id}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).expect("创建临时目录");
    p
}

pub fn cleanup(dir: &PathBuf) {
    let _ = std::fs::remove_dir_all(dir);
}

/// 头部元组别名（`'static str` 名字省构造负担）。
pub type H = (&'static str, String);

pub fn h(k: &'static str, v: impl Into<String>) -> H {
    (k, v.into())
}
