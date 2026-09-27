//! 记录信封的**轻量**读取（docs/SYNC-PROTOCOL.md §3）。
//!
//! 分工要讲清楚，否则这里容易长出一份和 `notera-crypto::Envelope::from_wire` 打架的
//! 校验：语义校验（payload/ct 互斥、`hash == sha256(canonical(payload))`、时间戳格式……）
//! 属于落库前的 **store** 侧（I6：脏数据不入库）。本适配器只关心"这一坨字节能不能被
//! 唯一地复验"，也就是写路径需要的四个字段：`rev`、`hash`、以及可选的 `id`/`kind`。
//!
//! 因此这里的解析**故意宽松**：缺 `rev` 或缺 `hash` 的记录我们拒绝上传（写上去就无法
//! 复验，等于静默丢数据），但别人存的形态古怪的文件不会让我们误判成"内容相同"。

use notera_sync::RemoteError;
use serde_json::Value;

/// 一条记录的可复验字段。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WireMeta {
    pub rev: u64,
    pub hash: String,
    pub id: Option<String>,
    pub kind: Option<String>,
    pub purged: bool,
}

impl WireMeta {
    /// 我方**要写出去**的字节必须自带 rev + hash：否则写后复验无从谈起。
    pub fn parse_outgoing(wire: &[u8]) -> Result<WireMeta, RemoteError> {
        let meta = Self::parse_lenient(wire).ok_or_else(|| {
            RemoteError::Protocol("记录缺少 rev/hash 字段，写入后无法复验，已拒绝上传".into())
        })?;
        if meta.hash.trim().is_empty() {
            return Err(RemoteError::Protocol("记录 hash 为空，已拒绝上传".into()));
        }
        Ok(meta)
    }

    /// 宽松读取：不是信封（或不是 JSON 对象）就返回 `None`，绝不 panic。
    pub fn parse_lenient(wire: &[u8]) -> Option<WireMeta> {
        let v: Value = serde_json::from_slice(wire).ok()?;
        let obj = v.as_object()?;
        let rev = obj.get("rev").and_then(as_u64)?;
        let hash = obj
            .get("hash")
            .and_then(Value::as_str)
            .map(|s| s.trim().to_ascii_lowercase())?;
        Some(WireMeta {
            rev,
            hash,
            id: obj.get("id").and_then(Value::as_str).map(str::to_string),
            kind: obj.get("kind").and_then(Value::as_str).map(str::to_string),
            purged: obj.get("purged").and_then(Value::as_bool).unwrap_or(false),
        })
    }

    /// §11.1 的幂等判据：远端已是我们正要写的那一条。
    pub fn is_same_commit(&self, other: &WireMeta) -> bool {
        self.rev == other.rev && self.hash == other.hash
    }
}

fn as_u64(v: &Value) -> Option<u64> {
    match v.as_u64() {
        Some(n) => Some(n),
        None => v.as_i64().and_then(|i| u64::try_from(i).ok()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outgoing_requires_rev_and_hash() {
        assert!(
            WireMeta::parse_outgoing(b"{\"rev\":3}").is_err(),
            "缺 hash 的记录无法复验，必须拒写"
        );
        assert!(WireMeta::parse_outgoing(b"not json").is_err());
        assert!(WireMeta::parse_outgoing(b"[]").is_err());
        let ok =
            WireMeta::parse_outgoing(br#"{"rev":3,"hash":"sha256:AA","kind":"note","id":"x"}"#)
                .expect("合法");
        assert_eq!(ok.rev, 3);
        assert_eq!(
            ok.hash, "sha256:aa",
            "hash 归一小写，免得大小写两种写法判成两条内容"
        );
        assert_eq!(ok.kind.as_deref(), Some("note"));
    }

    #[test]
    fn foreign_shapes_are_none_not_error() {
        assert!(WireMeta::parse_lenient(br#"{"attachment":true}"#).is_none());
        assert!(WireMeta::parse_lenient(br#"{"rev":"x","hash":1}"#).is_none());
    }
}
