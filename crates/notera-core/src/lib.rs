//! notera-core —— 全系统共享的类型、ID、时钟、错误词表与不变式断言。
//!
//! 铁律（见 docs/ARCHITECTURE-MAP.md §2）：零 I/O、零 async、零跨 crate 依赖。
//! 这里放的是"词汇表"，不是策略。任何 WebDAV/同步概念都不许进入本 crate。

pub mod error;

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};

// --------------------------------------------------------------------- IDs ---

/// 实体 ID：UUIDv7 字符串（36 字符小写带连字符）。
///
/// 选 v7 的理由（实测 `uuidv7-monotonic-pathsafe`）：跨设备无需协调即可生成、
/// 时间有序（利于清单分段与"最近"查询）、且是文件名安全的。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EntityId(String);

impl EntityId {
    /// 生成新 ID（I1：一旦生成就永不复用、永不改写）。
    pub fn new() -> Self {
        Self(uuid::Uuid::now_v7().hyphenated().to_string())
    }

    pub fn parse(s: &str) -> Result<Self, IdentityError> {
        let t = s.trim().to_ascii_lowercase();
        let u = uuid::Uuid::parse_str(&t).map_err(|_| IdentityError::Malformed(s.to_string()))?;
        Ok(Self(u.hyphenated().to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 作为远端路径片段。**必须**校验字符集：目录穿越即安全事故。
    pub fn as_path_segment(&self) -> &str {
        debug_assert!(self.is_path_safe());
        &self.0
    }

    pub fn is_path_safe(&self) -> bool {
        !self.0.is_empty()
            && self.0.len() == 36
            && !self.0.contains("..")
            && !self.0.contains('/')
            && !self.0.contains('\\')
            && self.0.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    }

    /// 清单分段的二分定位用（v7 前缀即时间序）。
    pub fn sort_key(&self) -> &str {
        &self.0
    }
}

impl Default for EntityId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for EntityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for EntityId {
    type Err = IdentityError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

/// 设备身份：本机安装时生成一次，存 `meta.device_id`，跨同步不变。
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DeviceId(pub EntityId);

impl DeviceId {
    pub fn new() -> Self {
        Self(EntityId::new())
    }
    pub fn parse(s: &str) -> Result<Self, IdentityError> {
        Ok(Self(EntityId::parse(s)?))
    }
}

impl Default for DeviceId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum IdentityError {
    #[error("ID 不是合法 UUID: {0}")]
    Malformed(String),
}

// ------------------------------------------------------------------- Rev ---

/// 修订号：每实体单调递增的 Lamport 式整数（I2）。
///
/// **禁止**手写 `rev + 1`：一律经 [`next_rev`]，否则跨设备并发下会撞号。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Rev(pub u64);

impl Rev {
    pub const ZERO: Rev = Rev(0);
    pub fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for Rev {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// I2 的唯一推进入口：本地头部与已观测到的远端头部取大者 +1。
///
/// 这样即使两台设备离线并发编辑，编号也不会相等，"谁改过"可判；
/// 而"改了什么"由 [`ContentHash`] 判定，不靠时间（I4/R3）。
pub fn next_rev(local: Rev, observed_remote: Rev) -> Rev {
    Rev(local.get().max(observed_remote.get()) + 1)
}

// ------------------------------------------------------------------ Hash ---

/// 内容哈希，规范形态 `sha256:<64hex>`（权威）。
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ContentHash(String);

impl ContentHash {
    pub fn from_hex(hex64: &str) -> Result<Self, HashError> {
        if hex64.len() != 64 || !hex64.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(HashError::Malformed(hex64.to_string()));
        }
        Ok(Self(format!("sha256:{}", hex64.to_ascii_lowercase())))
    }

    pub fn of(bytes: &[u8]) -> Self {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(bytes);
        Self(format!("sha256:{}", hex_lower(&h.finalize())))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 清单内的截断提示（12 hex）。仅作快速路径，**不能**单独作为相等判定的依据。
    pub fn short(&self) -> String {
        self.0["sha256:".len()..][..12].to_string()
    }

    pub fn matches_hex(&self, other: &str) -> bool {
        self.0.eq_ignore_ascii_case(other) || self.0 == other
    }
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum HashError {
    #[error("哈希不是 64 位十六进制: {0}")]
    Malformed(String),
}

// ------------------------------------------------------------------ Kind ---

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EntityKind {
    Note,
    Folder,
    Attachment,
}

impl EntityKind {
    /// 远端目录名。
    pub fn dir(self) -> &'static str {
        match self {
            EntityKind::Note => "note",
            EntityKind::Folder => "folder",
            EntityKind::Attachment => "attachments",
        }
    }
    /// 清单条目里的单字符类型标记（省流量，实测每字节都影响 gzip）。
    pub fn tag(self) -> char {
        match self {
            EntityKind::Note => 'n',
            EntityKind::Folder => 'f',
            EntityKind::Attachment => 'a',
        }
    }
    pub fn from_tag(t: &str) -> Option<Self> {
        match t {
            "n" => Some(EntityKind::Note),
            "f" => Some(EntityKind::Folder),
            "a" => Some(EntityKind::Attachment),
            _ => None,
        }
    }
}

impl fmt::Display for EntityKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.dir())
    }
}

// ----------------------------------------------------------------- Clock ---

/// 墙上时间只用于**展示与诊断**，永不参与新旧判定（I4/R3）。
/// 因此把它抽象成 trait，测试可注入假时钟。
pub trait Clock: Send + Sync {
    fn now(&self) -> Timestamp;
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Timestamp(String);

impl Timestamp {
    /// RFC 3339 毫秒 UTC；字典序即时间序。
    pub fn new(dt: chrono::DateTime<chrono::Utc>) -> Self {
        Self(dt.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn parse(s: &str) -> Option<Self> {
        chrono::DateTime::parse_from_rfc3339(s).ok().map(|d| Self::new(d.with_timezone(&chrono::Utc)))
    }
    /// 毫秒刻度。只用于**缓存有效期**这类墙上时间差（探测节奏、退避），
    /// 不参与记录新旧判定（I4/R3 约束的是数据，不是时钟）。
    pub fn as_millis(&self) -> Option<i64> {
        chrono::DateTime::parse_from_rfc3339(&self.0).ok().map(|d| d.timestamp_millis())
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        Timestamp::new(chrono::Utc::now())
    }
}

/// 测试用假时钟：只在测试里推进，用于崩溃/退避/超时的确定性复现。
#[derive(Debug, Default)]
pub struct FakeClock(AtomicU64);

impl FakeClock {
    pub fn new_at(t: Timestamp) -> Self {
        let secs = chrono::DateTime::parse_from_rfc3339(&t.0)
            .map(|d| d.timestamp())
            .unwrap_or(0) as u64;
        Self(AtomicU64::new(secs))
    }
    pub fn advance(&self, secs: u64) {
        self.0.fetch_add(secs, Ordering::SeqCst);
    }
}

impl Clock for FakeClock {
    fn now(&self) -> Timestamp {
        Timestamp::new(
            chrono::DateTime::from_timestamp(self.0.load(Ordering::SeqCst) as i64, 0)
                .unwrap_or_default(),
        )
    }
}

// ---------------------------------------------------------- canonical JSON ---

/// 稳定序列化：对象键按 Unicode 码点升序、无数组重排、无多余空白、整数不写成浮点。
///
/// 这是内容哈希的前提。实测（`canonical-json-for-hashing`）：同一逻辑文档在
/// 键插入顺序不同时输出一致。
pub fn canonical_json(v: &serde_json::Value) -> String {
    use serde_json::Value;
    let mut out = String::new();
    write_value(v, &mut out);
    return out;

    fn write_value(v: &Value, out: &mut String) {
        match v {
            Value::Object(m) => {
                let mut keys: Vec<&String> = m.keys().collect();
                keys.sort();
                out.push('{');
                for (i, k) in keys.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    out.push_str(&Value::String((*k).clone()).to_string());
                    out.push(':');
                    write_value(&m[*k], out);
                }
                out.push('}');
            }
            Value::Array(a) => {
                out.push('[');
                for (i, x) in a.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_value(x, out);
                }
                out.push(']');
            }
            // 整数统一输出（serde_json 已区分 i64/u64/f64）
            other => out.push_str(&other.to_string()),
        }
    }
}

/// 对 JSON 值直接求权威哈希。
pub fn hash_json(v: &serde_json::Value) -> ContentHash {
    ContentHash::of(canonical_json(v).as_bytes())
}

// ------------------------------------------------------------------- hex ---

pub fn hex_lower(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// -------------------------------------------------------------- 不变式断言 ---

/// I2：rev 必须严格递增。返回 Err 供上层拒绝写入（而不是静默修正）。
pub fn assert_rev_advances(prev: Rev, next: Rev) -> Result<(), InvariantViolation> {
    if next.get() <= prev.get() {
        return Err(InvariantViolation {
            id: "I2",
            detail: format!("rev 未递增: {prev} -> {next}"),
        });
    }
    Ok(())
}

/// I1：ID 必须路径安全且非空。
pub fn assert_id_valid(id: &EntityId) -> Result<(), InvariantViolation> {
    if !id.is_path_safe() {
        return Err(InvariantViolation {
            id: "I1",
            detail: format!("ID 不是路径安全的 UUIDv7: {id}"),
        });
    }
    Ok(())
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
#[error("不变式 {id} 被违反: {detail}")]
pub struct InvariantViolation {
    pub id: &'static str,
    pub detail: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rev_never_collides_across_devices() {
        // A 本地到 7，同时看到远端 9 → 下一个必须是 10，不能撞 A 自己的 8。
        assert_eq!(next_rev(Rev(7), Rev(9)), Rev(10));
        assert_eq!(next_rev(Rev(9), Rev(3)), Rev(10));
        assert_eq!(next_rev(Rev(0), Rev(0)), Rev(1));
    }

    #[test]
    fn id_is_pathsafe_and_orderable() {
        let a = EntityId::new();
        assert_id_valid(&a).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let b = EntityId::new();
        assert!(a.as_str() < b.as_str(), "v7 应时间有序: {a} !< {b}");
        assert!(EntityId::parse("../../etc/passwd").is_err());
    }

    #[test]
    fn canonical_json_is_key_order_independent() {
        let mut m = serde_json::Map::new();
        m.insert("z".into(), serde_json::json!(1));
        m.insert("a".into(), serde_json::json!({"y":1,"b":2}));
        let c1 = canonical_json(&serde_json::Value::Object(m.clone()));
        let mut m2 = serde_json::Map::new();
        m2.insert("a".into(), serde_json::json!({"b":2,"y":1}));
        m2.insert("z".into(), serde_json::json!(1));
        assert_eq!(c1, canonical_json(&serde_json::Value::Object(m2)), "{c1}");
    }

    #[test]
    fn sha256_known_answer() {
        // FIPS 180-4 标准向量：哈希错则全系统判定失效。
        assert_eq!(
            ContentHash::of(b"abc").as_str(),
            "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn short_hash_is_prefix_of_authoritative() {
        let h = ContentHash::of(b"notera");
        let full = h.as_str().to_string();
        // short() 是 hex 的**前缀**（截断提示），不是后缀。
        assert!(full.starts_with(&format!("sha256:{}", h.short())), "{full} / {}", h.short());
        assert_eq!(h.short().len(), 12);
        assert_eq!(full.len(), "sha256:".len() + 64);
    }
}
