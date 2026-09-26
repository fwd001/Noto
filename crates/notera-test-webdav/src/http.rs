//! 手写 HTTP/1.1 报文解析 —— 本 crate 存在的意义之一就是"不借 mock 层作弊"，
//! 因此请求行 / 头部 / body（`Content-Length` 与 `Transfer-Encoding: chunked`）
//! 全部在这里自己解析；响应用 [`write_response`] 手写字节。

use std::io;

use percent_encoding::percent_decode_str;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
pub use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};

/// 单行上限：超过即判定为畸形报文。
const MAX_LINE: usize = 64 * 1024;
/// header 条数上限。
const MAX_HEADERS: usize = 256;
/// body 硬上限（64 MiB）。
const MAX_BODY: usize = 64 * 1024 * 1024;

/// 读侧：连接拆开后带缓冲的一半。
pub type Reader = BufReader<OwnedReadHalf>;
/// 写侧。
pub type Writer = OwnedWriteHalf;

/// 拆连接为读写两半（tokio 没有 `BufStream`，拆半是标准做法）。
pub fn split(stream: tokio::net::TcpStream) -> (Reader, Writer) {
    let (r, w) = stream.into_split();
    (BufReader::new(r), w)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadError {
    /// 报文不合法。
    Malformed(String),
    /// body 超过硬上限。
    TooLarge,
    /// 注入要求"读够 N 字节就断开"—— 半上传。
    TruncatedAfter(PartialUpload),
    Io(String),
}

/// 半上传的现场：请求头是完整的（所以 method/path 已知），body 只读到一半。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartialUpload {
    pub method: String,
    pub path: String,
    pub bytes: usize,
}

impl From<io::Error> for ReadError {
    fn from(e: io::Error) -> Self {
        ReadError::Io(e.to_string())
    }
}

#[derive(Debug, Clone)]
pub struct Request {
    /// 原始方法名（大写）。自定义动词（PROPFIND/MOVE/…）走这条路，不依赖任何 HTTP 库。
    pub method: String,
    /// 请求目标原文（origin-form 或 absolute-form）；留给控制面/诊断用。
    #[allow(dead_code)]
    pub target: String,
    pub version: String,
    /// 头部名已转小写；同名多次出现则保留多条。
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// 请求目标是否为绝对形式（= 经 HTTP 代理转发的痕迹）。
    pub absolute_form: bool,
    /// 是否用 chunked 传的 body；留给诊断用。
    #[allow(dead_code)]
    pub chunked: bool,
    /// 解码后的路径（不含 query）。
    pub path: String,
    /// query（不含 `?`）。
    pub query: String,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        let n = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(k, _)| *k == n)
            .map(|(_, v)| v.as_str())
    }

    /// 同名多值（如 `Via`）全部返回。
    #[allow(dead_code)]
    pub fn headers_of(&self, name: &str) -> Vec<&str> {
        let n = name.to_ascii_lowercase();
        self.headers
            .iter()
            .filter(|(k, _)| *k == n)
            .map(|(_, v)| v.as_str())
            .collect()
    }

    pub fn wants_close(&self) -> bool {
        self.header("connection")
            .map(|v| v.trim().eq_ignore_ascii_case("close"))
            .unwrap_or(false)
    }
}

/// 读取一个请求；`Ok(None)` 表示对端在请求行边界干净地关闭了连接。
///
/// `truncate_upload_at`：故障注入用 —— 只读够 N 字节就当"连接被切断"，
/// 且不返回半份 body，因此服务端**不可能**留下半个有效文件。
pub async fn read_request(
    r: &mut Reader,
    truncate_upload_at: Option<usize>,
) -> Result<Option<Request>, ReadError> {
    let line = match read_line(r).await? {
        None => return Ok(None),
        Some(l) => l,
    };
    if line.trim().is_empty() {
        return Ok(None);
    }
    let mut parts = line.splitn(3, ' ');
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("").to_string();
    let version = parts.next().unwrap_or("HTTP/1.0").to_string();
    if method.is_empty() || target.is_empty() || !version.starts_with("HTTP/") {
        return Err(ReadError::Malformed(format!("请求行不可解析: {line:?}")));
    }

    let mut headers: Vec<(String, String)> = Vec::new();
    loop {
        match read_line(r).await? {
            None => {
                if headers.is_empty() {
                    return Ok(None);
                }
                return Err(ReadError::Malformed("头部区块被截断".into()));
            }
            Some(l) if l.trim().is_empty() => break,
            Some(l) => {
                if headers.len() >= MAX_HEADERS {
                    return Err(ReadError::Malformed("头部条数超限".into()));
                }
                let (k, v) = match l.split_once(':') {
                    Some(x) => x,
                    None => return Err(ReadError::Malformed(format!("头部行缺冒号: {l:?}"))),
                };
                headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
            }
        }
    }

    let absolute_form = target.starts_with("http://") || target.starts_with("https://");
    let (path, query) = split_target(&target, absolute_form);

    let chunked = header_value(&headers, "transfer-encoding")
        .map(|v| v.to_ascii_lowercase().contains("chunked"))
        .unwrap_or(false);

    let body = if chunked {
        read_chunked(r).await?
    } else {
        let want = header_value(&headers, "content-length")
            .map(|v| v.trim().parse::<usize>().unwrap_or(0))
            .unwrap_or(0);
        if want > MAX_BODY {
            return Err(ReadError::TooLarge);
        }
        match truncate_upload_at {
            Some(n) if want > n => {
                read_exact_limited(r, n).await?;
                return Err(ReadError::TruncatedAfter(PartialUpload {
                    method: method.clone(),
                    path: path.clone(),
                    bytes: n,
                }));
            }
            _ => read_exactly(r, want).await?,
        }
    };

    Ok(Some(Request {
        method,
        target,
        version,
        headers,
        body,
        absolute_form,
        chunked,
        path,
        query,
    }))
}

fn header_value(headers: &[(String, String)], name: &str) -> Option<String> {
    headers
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.clone())
}

fn split_target(target: &str, absolute_form: bool) -> (String, String) {
    let rest = if absolute_form {
        // `http://host:port/path?q` → `/path?q`
        match target.find("//").map(|i| &target[i + 2..]) {
            Some(after) => match after.find('/') {
                Some(i) => &after[i..],
                None => "/",
            },
            None => "/",
        }
    } else {
        target
    };
    let (raw_path, raw_q) = match rest.split_once('?') {
        Some((p, q)) => (p, q),
        None => (rest, ""),
    };
    (decode(raw_path), decode(raw_q))
}

fn decode(s: &str) -> String {
    percent_decode_str(s).decode_utf8_lossy().into_owned()
}

async fn read_line(r: &mut Reader) -> Result<Option<String>, ReadError> {
    let mut buf: Vec<u8> = Vec::with_capacity(128);
    let mut byte = [0u8; 1];
    loop {
        match r.read(&mut byte).await {
            Ok(0) => {
                if buf.is_empty() {
                    return Ok(None);
                }
                return Err(ReadError::Malformed("行尾被截断".into()));
            }
            Ok(_) => {
                if byte[0] == b'\n' {
                    if buf.ends_with(b"\r") {
                        buf.pop();
                    }
                    return Ok(Some(String::from_utf8_lossy(&buf).into_owned()));
                }
                buf.push(byte[0]);
                if buf.len() > MAX_LINE {
                    return Err(ReadError::Malformed("行过长".into()));
                }
            }
            Err(e) => return Err(ReadError::Io(e.to_string())),
        }
    }
}

async fn read_exactly(r: &mut Reader, n: usize) -> Result<Vec<u8>, ReadError> {
    let mut buf = vec![0u8; n];
    if n > 0 {
        r.read_exact(&mut buf)
            .await
            .map_err(|e| ReadError::Io(e.to_string()))?;
    }
    Ok(buf)
}

async fn read_exact_limited(r: &mut Reader, n: usize) -> Result<(), ReadError> {
    let mut left = n;
    let mut scratch = [0u8; 8192];
    while left > 0 {
        let take = left.min(scratch.len());
        r.read_exact(&mut scratch[..take])
            .await
            .map_err(|e| ReadError::Io(e.to_string()))?;
        left -= take;
    }
    Ok(())
}

async fn read_chunked(r: &mut Reader) -> Result<Vec<u8>, ReadError> {
    let mut out: Vec<u8> = Vec::new();
    loop {
        let line = match read_line(r).await? {
            Some(l) => l,
            None => return Err(ReadError::Malformed("chunk 长度行被截断".into())),
        };
        let size_hex = line.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_hex, 16)
            .map_err(|_| ReadError::Malformed(format!("chunk 长度非法: {line:?}")))?;
        if size == 0 {
            // 吞掉 trailers 直到空行。
            loop {
                match read_line(r).await? {
                    None => return Ok(out),
                    Some(l) if l.trim().is_empty() => return Ok(out),
                    Some(_) => continue,
                }
            }
        }
        if out.len() + size > MAX_BODY {
            return Err(ReadError::TooLarge);
        }
        out.extend_from_slice(&read_exactly(r, size).await?);
        // chunk 之后的 CRLF
        let mut crlf = [0u8; 2];
        r.read_exact(&mut crlf)
            .await
            .map_err(|e| ReadError::Io(e.to_string()))?;
        if &crlf != b"\r\n" {
            return Err(ReadError::Malformed("chunk 结尾不是 CRLF".into()));
        }
    }
}

/// 写出响应。`head_only` 用于 HEAD：头部照给（含 `Content-Length`），正文不写。
pub async fn write_response(
    w: &mut Writer,
    version: &str,
    status: u16,
    headers: &[(String, String)],
    body: &[u8],
    head_only: bool,
) -> io::Result<()> {
    let mut out: Vec<u8> = Vec::with_capacity(body.len() + 256);
    out.extend_from_slice(format!("{version} {status} {}\r\n", reason_phrase(status)).as_bytes());
    for (k, v) in headers {
        out.extend_from_slice(format!("{k}: {v}\r\n").as_bytes());
    }
    out.extend_from_slice(b"\r\n");
    w.write_all(&out).await?;
    if !head_only {
        w.write_all(body).await?;
    }
    w.flush().await
}

fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        206 => "Partial Content",
        207 => "Multi-Status",
        301 => "Moved Permanently",
        302 => "Found",
        304 => "Not Modified",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        412 => "Precondition Failed",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        507 => "Insufficient Storage",
        _ => "Status",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_form_yields_origin_path() {
        let (p, q) = split_target("http://127.0.0.1:80/a/b.json?x=1", true);
        assert_eq!(p, "/a/b.json");
        assert_eq!(q, "x=1");
    }

    #[test]
    fn percent_decoding_applies_to_path() {
        let (p, _) = split_target("/a/b%20c.json", false);
        assert_eq!(p, "/a/b c.json");
    }
}
