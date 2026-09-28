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
            && self
                .0
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
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
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
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

/// 两个内容哈希是否可视为"同一版内容"：允许 `sha256:` 前缀有无与大小写，并允许清单里
/// 那种**短形式**（前 12 位十六进制）与全哈希相比。
///
/// 为什么需要它：远端索引 `sync_remote_index.hash12` 存的是 12 位短哈希，本地行上是全哈希。
/// 判定表 P7（两侧都改但内容相同 → 收敛，不算冲突）原先拿两者直接 `==`，于是它**永远不
/// 成立** —— 内容完全一样的两侧也会被记成一次冲突，用户收到一张没有意义的卡片。
/// 短哈希只用于"要不要重新公告 / 这是不是同一版"这类收敛判定，绝不当防伪手段：真正写库的
/// 闸门始终是 I6 那条整值复核（`content_hash == sha256(canonical(doc))`）。
pub fn same_content_hash(a: &str, b: &str) -> bool {
    let norm = |s: &str| {
        let t = s.trim();
        t.strip_prefix("sha256:").unwrap_or(t).to_ascii_lowercase()
    };
    let (x, y) = (norm(a), norm(b));
    if x.is_empty() || y.is_empty() {
        return false;
    }
    let (short, long) = if x.len() <= y.len() {
        (x.as_str(), y.as_str())
    } else {
        (y.as_str(), x.as_str())
    };
    if short.len() == long.len() {
        return short == long;
    }
    short.len() >= 12 && long.starts_with(short)
}

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
        chrono::DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|d| Self::new(d.with_timezone(&chrono::Utc)))
    }
    /// 毫秒刻度。只用于**缓存有效期**这类墙上时间差（探测节奏、退避），
    /// 不参与记录新旧判定（I4/R3 约束的是数据，不是时钟）。
    pub fn as_millis(&self) -> Option<i64> {
        chrono::DateTime::parse_from_rfc3339(&self.0)
            .ok()
            .map(|d| d.timestamp_millis())
    }
    /// `as_millis` 的反向：给"多久之后过期"算一个时间戳。
    pub fn from_millis(ms: i64) -> Self {
        Self::new(
            chrono::DateTime::from_timestamp(ms / 1000, ((ms % 1000).max(0) as u32) * 1_000_000)
                .unwrap_or_default(),
        )
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

// --------------------------------------------------- §20 崩溃注入（L5）---

/// 全部合法注入点。**文档、注入代码、测试三处引用同一份名字**，
/// 免得测试里写 `"after_aply"` 这种拼错的名字然后"崩溃恢复已验证"是假的。
pub const CRASH_POINTS: &[&str] = &[
    "after_local_write",
    "before_records_push",
    "after_records_push",
    "before_attachment_upload",
    "after_attachment_upload",
    "before_apply",
    "after_apply",
    "before_manifest_commit",
    "after_manifest_commit",
    "after_segment_write",
    "after_quarantine_move",
];

/// 需要**零引用夹具**才走得到的注入点：GC 只在"没有任何笔记引用 + 服务器已有副本"时才挪字节，
/// 而 9 点崩溃矩阵的子进程夹具刚 `attach_blob` 完、那条笔记还活着 ⇒ 引用为 1，
/// `after_quarantine_move` 永远不会被经过。把它放进大库那份名单也能"混过去"（矩阵会跳过它），
/// 但那是对的理由不对：**这一格要的是"崩在挪与写账之间"，不是"崩在写完账之后"**。
///
/// 名单的理由与上面那条相同：矩阵按 `CRASH_POINTS` 减去这两份名单遍历，
/// `attachment_gc` 按 `CRASH_POINTS_NEED_GC` 遍历，"新点没人覆盖"就会红而不是静静少测一格。
pub const CRASH_POINTS_NEED_GC: &[&str] = &["after_quarantine_move"];

/// 需要**大库夹具**才走得到的注入点：窗口超过 `WINDOW_MAX`(200) 才会触发清单压实，
/// 而 9 点崩溃矩阵的子进程夹具只写 1 条笔记，永远碰不到这一步。
///
/// 这条名单存在的理由是"别把没跑到的点算成跑过了"：崩溃矩阵按 `CRASH_POINTS` 减去这份
/// 名单遍历，`compaction_crash` 按这份名单遍历，两边各自断言进程真的死在点上。将来新增
/// 注入点又没归进任何一边，下面的 `crash_point_lists_partition_registry` 就会红。
pub const CRASH_POINTS_NEED_LARGE_LIBRARY: &[&str] = &["after_segment_write"];

/// 被注入杀死时进程用的退出码：刻意避开 `notera-cli` 的 0/1/2，
/// 这样"崩溃注入"和"断言失败"在日志里不会混成一回事。
pub const CRASH_EXIT_CODE: i32 = 77;

/// 在名为 `name` 的提交点**让进程当场消失**（仅当 `NOTERA_CRASH_AT==name`）。
///
/// 为什么是 `process::exit` 而不是 `panic!`：panic 会展开栈、跑析构、还可能被
/// 上层 `catch_unwind` 接住 —— 那测的就不是"写一半时断电"。`exit` 什么都不收尾，
/// 恢复只能依赖 WAL 事务与协议写序（记录 → 附件 → 清单），而这正是要验的东西。
///
/// 只在 debug 构建里有效：正式产物不允许留"一个环境变量就能让应用自杀"的开关。
#[cfg(debug_assertions)]
#[inline]
pub fn crash_point(name: &str) {
    use std::sync::OnceLock;
    static TARGET: OnceLock<Option<String>> = OnceLock::new();
    debug_assert!(CRASH_POINTS.contains(&name), "未登记的崩溃注入点：{name}");
    let target = TARGET.get_or_init(|| std::env::var("NOTERA_CRASH_AT").ok());
    if target.as_deref() == Some(name) {
        eprintln!("NOTERA_CRASH_AT={name}：在该提交点强制退出进程");
        std::process::exit(CRASH_EXIT_CODE);
    }
}

/// release 构建里这是个空函数：连读环境变量都不做。
#[cfg(not(debug_assertions))]
#[inline]
pub fn crash_point(_name: &str) {}

#[cfg(test)]
mod crash_tests {
    use super::*;

    /// 崩溃注入名单不能各自漂移：大库专属的那几个必须是全表的子集，且全表非空。
    /// 这条断言是给"加了新点却忘了归进任何一个夹具"准备的。
    #[test]
    fn crash_point_lists_partition_registry() {
        for extra in CRASH_POINTS_NEED_LARGE_LIBRARY {
            assert!(
                CRASH_POINTS.contains(extra),
                "{extra} 未登记在 CRASH_POINTS"
            );
        }
        assert_eq!(
            CRASH_POINTS_NEED_LARGE_LIBRARY.len(),
            1,
            "大库专属点应当逐个有据可查"
        );
        for extra in CRASH_POINTS_NEED_GC {
            assert!(
                CRASH_POINTS.contains(extra),
                "{extra} 未登记在 CRASH_POINTS"
            );
        }
        assert_eq!(CRASH_POINTS_NEED_GC.len(), 1, "GC 专属点应当逐个有据可查");
        assert!(
            CRASH_POINTS.len() > CRASH_POINTS_NEED_LARGE_LIBRARY.len() + CRASH_POINTS_NEED_GC.len()
        );
    }

    #[test]
    fn crash_point_names_are_unique_and_non_empty() {
        let mut sorted = CRASH_POINTS.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), CRASH_POINTS.len(), "注入点名字重复了");
        assert!(CRASH_POINTS.iter().all(|p| !p.is_empty()));
    }

    #[test]
    fn an_unnamed_point_is_not_accidentally_instrumented() {
        // 测试进程本身没设 NOTERA_CRASH_AT：任何 crash_point 调用都必须活着回来
        for p in CRASH_POINTS {
            crash_point(p);
        }
    }
}

#[cfg(test)]
mod hash_shape_tests {
    use super::same_content_hash;

    const FULL: &str = "abc123def4560000000000000000000000000000000000000000000000000000";

    #[test]
    fn full_hashes_match_regardless_of_prefix_and_case() {
        assert!(same_content_hash(FULL, &format!("sha256:{FULL}")));
        assert!(same_content_hash(
            &format!("sha256:{}", FULL.to_ascii_uppercase()),
            FULL
        ));
    }

    #[test]
    fn a_12_char_manifest_hash_matches_the_full_hash_it_came_from() {
        // 生产形状：远端索引 `hash12` 存 12 位，本地行上存整条。
        assert!(same_content_hash(&FULL[..12], &format!("sha256:{FULL}")));
        assert!(
            same_content_hash(&format!("sha256:{FULL}"), &FULL[..12]),
            "两侧谁长谁短都要成立"
        );
    }

    #[test]
    fn different_content_never_matches_and_short_or_empty_never_matches() {
        assert!(!same_content_hash(FULL, &"f".repeat(64)));
        assert!(!same_content_hash(&FULL[..12], &FULL[6..18]));
        // 短到不像清单哈希（<12 位）就不许当"同一版"，否则两位前缀就能互相冒充
        assert!(!same_content_hash(&FULL[..6], FULL));
        assert!(!same_content_hash("", FULL));
        assert!(!same_content_hash("sha256:", FULL));
    }
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
        assert!(
            full.starts_with(&format!("sha256:{}", h.short())),
            "{full} / {}",
            h.short()
        );
        assert_eq!(h.short().len(), 12);
        assert_eq!(full.len(), "sha256:".len() + 64);
    }
}
