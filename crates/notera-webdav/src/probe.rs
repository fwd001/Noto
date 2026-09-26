//! §5 服务器能力探测：首次连接与每日一次，结果写进 `sync_accounts.cap_mask`。
//!
//! 两条硬规矩：
//! 1. **探测失败 ≠ 能力缺失**。传输层报错必须冒出去，不能顺手当成"这台服务器不支持"——
//!    猜低了会掉到 S3 盲写复验，那才有覆盖丢数据的风险（见 `caps.rs` 的同段说明）。
//! 2. 探测对象放在库根下的 `probe/` 里，用完尽力删。绝不碰真实记录、清单或附件。
//!
//! `CHUNKED` 这里**不探**：`RequestSpec` 的 body 是 `Vec<u8>`，发不出真正的
//! `Transfer-Encoding: chunked`，硬凑一个头部只会得到一个"看起来支持"的假阳性。
//! 半上传检测本来就走读回复验，不依赖这一位。

use crate::caps::Caps;
use crate::client::WebDavRemote;
use notera_net::HttpMethod;
use notera_sync::RemoteError;

/// 一次探测的逐项结论。`Err` 只在传输层出问题时返回；`false` 是"实测不支持"。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProbeReport {
    pub strong_etag: bool,
    pub conditional_put: bool,
    pub overwrite_f_move: bool,
    pub depth_infinity: bool,
    pub range: bool,
}

impl ProbeReport {
    pub fn to_caps(&self) -> Caps {
        let mut mask = 0u32;
        if self.strong_etag {
            mask |= Caps::STRONG_ETAG;
        }
        if self.conditional_put {
            mask |= Caps::CONDITIONAL_PUT;
        }
        if self.overwrite_f_move {
            mask |= Caps::OVERWRITE_F_MOVE;
        }
        if self.depth_infinity {
            mask |= Caps::DEPTH_INFINITY;
        }
        if self.range {
            mask |= Caps::RANGE;
        }
        Caps::from_mask(mask)
    }

    /// 人读的一行摘要（`notera-cli dav-probe` 的输出）。
    pub fn describe(&self) -> String {
        let bit = |on: bool| if on { "yes" } else { "no " };
        format!(
            "strong_etag={} conditional_put={} overwrite_f_move={} depth_infinity={} range={}",
            bit(self.strong_etag),
            bit(self.conditional_put),
            bit(self.overwrite_f_move),
            bit(self.depth_infinity),
            bit(self.range),
        )
    }
}

const BODY: &[u8] = b"{\"probe\":true}";
const BODY2: &[u8] = b"0123456789";

impl WebDavRemote {
    /// 跑完 §5 的五项探测。整体一次往返失败（服务器不可达）会返回 `Err` 而不是"全 false"。
    pub async fn probe_caps(&self) -> Result<ProbeReport, RemoteError> {
        let mut r = ProbeReport::default();
        r.strong_etag = self.probe_strong_etag().await?;
        r.conditional_put = self.probe_conditional_put().await?;
        r.overwrite_f_move = self.probe_overwrite_f_move().await?;
        r.depth_infinity = self.probe_depth_infinity().await?;
        r.range = self.probe_range().await?;
        self.cleanup_probe_files().await;
        Ok(r)
    }

    fn probe_url(&self, name: &str) -> String {
        self.paths().url(&["probe", name])
    }

    /// `PUT` 之后带 `If-None-Match` 的 `GET` 能拿到 304 —— 空轮快路径的前提。
    async fn probe_strong_etag(&self) -> Result<bool, RemoteError> {
        let url = self.probe_url("etag.json");
        let put = self.put_plain(&url, BODY).await?;
        if !is_write_ok(put.status) {
            return Ok(false);
        }
        let etag = match put.etag.clone().or(self.etag_of(&url).await?) {
            Some(e) => e,
            None => return Ok(false),
        };
        let got = self.get_raw(&url, Some(&etag)).await?;
        Ok(got.status == 304)
    }

    /// 拿一个必然不匹配的 `If-Match` 去 PUT：服务器真在判定才会回 412。
    /// 回 2xx 说明它把条件头当装饰 —— 那 S1 就不能用。
    async fn probe_conditional_put(&self) -> Result<bool, RemoteError> {
        let url = self.probe_url("cput.json");
        self.put_plain(&url, BODY).await?;
        let s = self
            .spec(HttpMethod::Put, &url)?
            .with_header("content-type", "application/json")
            .with_body(BODY.to_vec())
            .with_if_match("\"notera-bogus-etag\"");
        let resp = self.send_once(s).await?;
        Ok(resp.status == 412)
    }

    /// `MOVE` 到一个已存在的目标且 `Overwrite: F`：支持的话必须 412，而不是默默覆盖。
    async fn probe_overwrite_f_move(&self) -> Result<bool, RemoteError> {
        let a = self.probe_url("mv-a.json");
        let b = self.probe_url("mv-b.json");
        self.put_plain(&a, BODY).await?;
        self.put_plain(&b, BODY).await?;
        let moved = self.move_raw(&a, &b, false, None).await?;
        if moved.status != 412 {
            return Ok(false);
        }
        // 只有"目标已存在所以拒绝"才是我们要的语义：源文件必须还在。
        let still_there = self.head_raw(&a).await?;
        Ok(still_there.status == 200)
    }

    async fn probe_depth_infinity(&self) -> Result<bool, RemoteError> {
        let root = self.paths().url(&[]);
        let body = br#"<?xml version="1.0"?><d:propfind xmlns:d="DAV:"><d:prop><d:resourcetype/></d:prop></d:propfind>"#;
        let s = self
            .spec(HttpMethod::Propfind, &root)?
            .with_header("depth", "infinity")
            .with_header("content-type", "application/xml")
            .with_body(body.to_vec());
        let resp = self.send_once(s).await?;
        if resp.status != 207 {
            return Ok(false);
        }
        // 207 但只回一个响应体 = 它把 infinity 当成 0/1 处理了，仍然不可依赖。
        let hits = resp.body[..]
            .windows(b"<d:response".len())
            .filter(|w| *w == b"<d:response")
            .count();
        Ok(hits > 1)
    }

    async fn probe_range(&self) -> Result<bool, RemoteError> {
        let url = self.probe_url("range.bin");
        self.put_plain(&url, BODY2).await?;
        let s = self.spec(HttpMethod::Get, &url)?.with_header("range", "bytes=0-0");
        let resp = self.send_once(s).await?;
        if resp.status != 206 {
            return Ok(false);
        }
        Ok(resp.body.len() == 1 && resp.header("content-range").is_some())
    }

    async fn cleanup_probe_files(&self) {
        for name in ["etag.json", "cput.json", "mv-a.json", "mv-b.json", "range.bin"] {
            self.best_effort_delete(&self.probe_url(name)).await;
        }
    }
}

fn is_write_ok(status: u16) -> bool {
    matches!(status, 200 | 201 | 204)
}
