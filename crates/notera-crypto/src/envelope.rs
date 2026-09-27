//! 记录信封：`records/<kind>/<id>.json` 的内容（DATA-MODEL §11 / SYNC-PROTOCOL §3）。
//!
//! wire 上的字段集是**契约冻结点**（ARCHITECTURE-MAP §6）：加可选字段 = 次版本，
//! 改语义或删字段 = 破坏性变更 → `protocol` +1。因此 [`Envelope`] 的公共字段集合
//! 一字不能动（store/webdav/sync 用结构体字面量构造它）。
//!
//! ## 校验的分工（为什么 `to_wire` 比 `from_wire` 多一道富文本检查）
//! * [`Envelope::to_wire`]：**写出侧**。本机是权威，绝不允许把坏数据发布出去（I6），
//!   所以除了信封约束还要用 `notera-richtext` 校验 note 的 payload 本体。
//! * [`Envelope::from_wire`]：**读入侧**。信封级约束一律严格（不合格必 `Err`），但
//!   payload **内部**的语义（例如 `doc.v` 超前）不在这里拒收：那是同步层的只读降级路径
//!   （FWD-03/FWD-06：记录要能被完整显示、要能计入诊断，而不是让整轮同步失败）。

use notera_core::{canonical_json, EntityId, EntityKind, Rev};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    b64, hmac_sha256, sha256_hex, split_hash, CryptoError, HMAC_SHA256_PREFIX, KEY_LEN, PROTOCOL,
    SHA256_PREFIX,
};

/// 加密算法。v1 只有 `None`，但结构从 v1 就带着走（ADR-0002）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum EncAlg {
    /// 明文：`payload` 非空、`ct` 为 null。
    #[serde(rename = "none")]
    #[default]
    None,
    /// E2EE：`ct` 非空、`payload` 为 null，且 `hash_alg` 必须是 `hmac-sha256`。
    #[serde(rename = "aes-256-gcm-siv")]
    Aes256GcmSiv,
}

/// `hash` 字段用的摘要算法。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HashAlg {
    /// 明文等价指纹（`alg=none` 时的唯一合法值）。
    #[default]
    Sha256,
    /// keyed 摘要：开启 E2EE 后**必须**用它，否则泄露明文等价指纹（SYNC-PROTOCOL §3）。
    HmacSha256,
}

/// 信封的加密元数据。
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct EncMeta {
    pub alg: EncAlg,
    /// 密钥代次（换密钥不丢旧记录）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kid: Option<u32>,
    /// base64 的 12 字节 nonce（实测固定长度）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nonce: Option<String>,
    #[serde(rename = "hash_alg", default)]
    pub hash_alg: HashAlg,
}

/// 一条可同步实体的完整远端记录。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    pub protocol: u16,
    #[serde(with = "kind_wire")]
    pub kind: EntityKind,
    pub id: EntityId,
    pub rev: Rev,
    /// 本地/远端最后一次确认一致的 rev（三方合并的 base；DATA-MODEL §4.3）。
    pub sync_rev: Rev,
    /// `sha256:<64hex>`（明文）或 `hmac-sha256:<64hex>`（E2EE）。
    pub hash: String,
    pub updated_at: String,
    pub device: String,
    #[serde(default)]
    pub deleted_at: Option<String>,
    #[serde(default)]
    pub purged: bool,
    #[serde(default)]
    pub enc: EncMeta,
    #[serde(default)]
    pub payload: Option<Value>,
    #[serde(default)]
    pub ct: Option<String>,
}

impl Envelope {
    /// 明文笔记信封（`alg=none`，`hash = sha256(canonical(payload))`）。
    pub fn for_note(
        id: &EntityId,
        rev: Rev,
        sync_rev: Rev,
        doc: &Value,
        deleted_at: Option<&str>,
        device: &str,
        updated_at: &str,
    ) -> Self {
        Self::for_entity(
            EntityKind::Note,
            id,
            rev,
            sync_rev,
            doc,
            deleted_at,
            device,
            updated_at,
        )
    }

    /// 明文文件夹信封。`folder` 是文件夹元数据（`name` / `parent_id` / `order` …）。
    pub fn for_folder(
        id: &EntityId,
        rev: Rev,
        sync_rev: Rev,
        folder: &Value,
        deleted_at: Option<&str>,
        device: &str,
        updated_at: &str,
    ) -> Self {
        Self::for_entity(
            EntityKind::Folder,
            id,
            rev,
            sync_rev,
            folder,
            deleted_at,
            device,
            updated_at,
        )
    }

    // 形参就是 `note_revisions` 那一行的列。收成一个参数结构体不会少一个字段，
    // 只会多一处"把字段从结构体搬到形参"的样板 —— 那里才是会漏的地方。
    #[allow(clippy::too_many_arguments)]
    fn for_entity(
        kind: EntityKind,
        id: &EntityId,
        rev: Rev,
        sync_rev: Rev,
        doc: &Value,
        deleted_at: Option<&str>,
        device: &str,
        updated_at: &str,
    ) -> Self {
        Self {
            protocol: PROTOCOL,
            kind,
            id: id.clone(),
            rev,
            sync_rev,
            hash: format!(
                "{SHA256_PREFIX}{}",
                sha256_hex(canonical_json(doc).as_bytes())
            ),
            updated_at: updated_at.to_string(),
            device: device.to_string(),
            deleted_at: deleted_at.map(str::to_string),
            purged: false,
            enc: EncMeta::default(),
            payload: Some(doc.clone()),
            ct: None,
        }
    }

    /// **永久删除**的墓碑公告（SYNC-PROTOCOL §3 末段）：`payload` 为 null，只保留
    /// `id/rev/hash/deleted_at/purged`。
    ///
    /// `hash` 的定义：`sha256(canonical(null))`。规范没规定墓碑的哈希取什么，这里定一个
    /// 确定值，使"同一条永久删除"在任意设备上重复上传都是同一内容（幂等，§11.1）。
    pub fn for_purged(
        kind: EntityKind,
        id: &EntityId,
        rev: Rev,
        sync_rev: Rev,
        deleted_at: &str,
        device: &str,
        updated_at: &str,
    ) -> Self {
        let mut e = Self::for_entity(
            kind,
            id,
            rev,
            sync_rev,
            &Value::Null,
            Some(deleted_at),
            device,
            updated_at,
        );
        e.payload = None;
        e.purged = true;
        e
    }

    /// 明文 payload（`alg=none` 才有；E2EE 或未带 payload 的返回 `None`）。
    pub fn payload_doc(&self) -> Option<&Value> {
        self.payload.as_ref()
    }

    /// 校验 `kind` 与 `id` 与调用方期望的一致。
    ///
    /// 这两个约束（SYNC-PROTOCOL §3 的后两行）**只有调用方知道**：`kind` 来自远端目录名，
    /// `id` 来自文件名。webdav/sync 层必须在写库前调它。
    pub fn check_matches(&self, kind: EntityKind, id: &EntityId) -> Result<(), CryptoError> {
        if self.kind != kind {
            return Err(CryptoError::KindMismatch);
        }
        if &self.id != id {
            return Err(CryptoError::IdMismatch);
        }
        Ok(())
    }

    /// 全部**信封级**约束。不合法一律 `Err`，绝不返回"看起来能用"的对象。
    pub fn validate(&self) -> Result<(), CryptoError> {
        if self.protocol != PROTOCOL {
            return Err(CryptoError::Malformed(format!(
                "protocol {} 不被本客户端支持（支持值 {PROTOCOL}）",
                self.protocol
            )));
        }
        // id 会被拼进远端路径：目录穿越是安全事故，必须在边界掐死。
        if !self.id.is_path_safe() {
            return Err(CryptoError::Malformed(format!(
                "id 不是路径安全的 UUID: {:?}",
                self.id.as_str()
            )));
        }
        if self.device.trim().is_empty() {
            return Err(CryptoError::Malformed("device 不能为空".into()));
        }
        check_timestamp(&self.updated_at, "updated_at")?;
        if let Some(d) = &self.deleted_at {
            check_timestamp(d, "deleted_at")?;
        }

        // ① payload 与 ct 恰好一个非空（纯 tombstone 例外，见 match 分支注释）。
        match (&self.payload, &self.ct) {
            (Some(_), Some(_)) => return Err(CryptoError::PayloadAndCtMutuallyExclusive),
            (None, None) => {
                // SYNC-PROTOCOL §3 末段：`purged=true` 的墓碑公告 payload 为 null。
                // 那一条记录仍然自带 id/rev/hash/deleted_at/purged，足以独立判定状态（R1）。
                // 其余情况下"两个都空"就是坏数据。
                if !self.purged {
                    return Err(CryptoError::PayloadAndCtMutuallyExclusive);
                }
            }
            (None, Some(_)) => {
                if self.purged {
                    return Err(CryptoError::Malformed(
                        "purged 墓碑不携带 ct（它只公告删除事实）".into(),
                    ));
                }
            }
            (Some(_), None) => {
                if self.purged {
                    return Err(CryptoError::Malformed(
                        "purged 墓碑的 payload 必须为 null".into(),
                    ));
                }
            }
        }

        // ② enc 元数据与 payload/ct 形态一致。
        match self.enc.alg {
            EncAlg::None => {
                if self.enc.hash_alg != HashAlg::Sha256 {
                    return Err(CryptoError::Malformed(
                        "alg=none 时 hash_alg 必须是 sha256".into(),
                    ));
                }
                if self.enc.nonce.is_some() || self.enc.kid.is_some() {
                    return Err(CryptoError::Malformed(
                        "alg=none 不该有 nonce/kid（加密元数据残留）".into(),
                    ));
                }
                if self.ct.is_some() {
                    return Err(CryptoError::Malformed("alg=none 却有 ct".into()));
                }
            }
            EncAlg::Aes256GcmSiv => {
                if self.enc.hash_alg != HashAlg::HmacSha256 {
                    return Err(CryptoError::Malformed(
                        "开启 E2EE 后 hash_alg 必须转 hmac-sha256，否则泄露明文等价指纹".into(),
                    ));
                }
                if self.ct.is_none() {
                    return Err(CryptoError::Malformed("alg≠none 必须有 ct".into()));
                }
                if self.payload.is_some() {
                    return Err(CryptoError::Malformed("alg≠none 不得带 payload".into()));
                }
                let nonce = self
                    .enc
                    .nonce
                    .as_deref()
                    .ok_or_else(|| CryptoError::Malformed("缺少 nonce".into()))?;
                let _ = b64::decode_nonce(nonce)?;
                if self.enc.kid.is_none() {
                    return Err(CryptoError::Malformed(
                        "缺少 kid（不知道用哪把钥匙）".into(),
                    ));
                }
            }
        }

        // ③ hash 形态 + 明文时的内容一致性。
        let (prefix, hex) = split_hash(&self.hash).ok_or_else(|| {
            CryptoError::Malformed(format!(
                "hash 必须是 sha256:/hmac-sha256: 加 64 hex: {:?}",
                self.hash
            ))
        })?;
        let want_prefix = match self.enc.hash_alg {
            HashAlg::Sha256 => SHA256_PREFIX,
            HashAlg::HmacSha256 => HMAC_SHA256_PREFIX,
        };
        if prefix != want_prefix {
            return Err(CryptoError::Malformed(format!(
                "hash 前缀 {prefix} 与 hash_alg {:?} 不符",
                self.enc.hash_alg
            )));
        }
        if !hex
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        {
            return Err(CryptoError::Malformed(
                "hash 必须是小写十六进制（大写会让同一条记录有两个哈希值）".into(),
            ));
        }
        if matches!(self.enc.alg, EncAlg::None) {
            if let Some(p) = &self.payload {
                let actual = sha256_hex(canonical_json(p).as_bytes());
                if actual != hex {
                    return Err(CryptoError::HashMismatch {
                        expected: self.hash.clone(),
                        actual: format!("{SHA256_PREFIX}{actual}"),
                    });
                }
            }
        }

        // ④ I3：删除是状态不是消失 —— 永久删除必须带删除事实。
        if self.purged && self.deleted_at.is_none() {
            return Err(CryptoError::Malformed(
                "purged=true 必须带 deleted_at（删除事实必须比数据活得久）".into(),
            ));
        }
        Ok(())
    }

    /// 校验后序列化。**写出侧**额外检查 note 的 payload 本体（见模块头分工说明）。
    pub fn to_wire(&self) -> Result<String, CryptoError> {
        self.validate()?;
        if self.kind == EntityKind::Note {
            if let Some(p) = &self.payload {
                notera_richtext::parse_from_value(p).map_err(|e| {
                    CryptoError::Malformed(format!("笔记 payload 不是合法富文本: {e}"))
                })?;
            }
        }
        serde_json::to_string(self).map_err(|e| CryptoError::Malformed(e.to_string()))
    }

    /// 严格解析远端记录。任何一条约束不满足即 `Err`（SYNC-PROTOCOL §3 的约束表）。
    ///
    /// 顺序刻意是：先看原始 JSON 的 `kind`（未知 kind 必须给出准确错误，而不是 serde
    /// 的通用报错），再整体反序列化，最后跑全部约束。
    pub fn from_wire(wire: &str) -> Result<Envelope, CryptoError> {
        let raw: Value =
            serde_json::from_str(wire).map_err(|e| CryptoError::Malformed(e.to_string()))?;
        check_kind_value(&raw)?;
        let env: Envelope =
            serde_json::from_value(raw).map_err(|e| CryptoError::Malformed(e.to_string()))?;
        env.validate()?;
        Ok(env)
    }

    /// 明文 → E2EE：`payload` 封进 `ct`，`hash` 转为 keyed 摘要，`alg/kid/nonce` 落元数据。
    ///
    /// 只用于"本机确定要发布密文"的路径（Phase 2）。输入信封必须先过 [`Envelope::validate`]，
    /// 免得把一份 hash 已经对不上的明文密封成"看起来合法"的密文。
    pub fn seal_with_key(&self, kid: u32, key: &[u8; KEY_LEN]) -> Result<Envelope, CryptoError> {
        self.validate()?;
        if !matches!(self.enc.alg, EncAlg::None) {
            return Err(CryptoError::Malformed("信封已经是密文形态".into()));
        }
        let pt = self
            .payload
            .as_ref()
            .ok_or_else(|| CryptoError::Malformed("没有 payload 可密封".into()))?;
        let canonical = canonical_json(pt).into_bytes();
        let (ct, nonce) = crate::seal_plaintext(&canonical, key)?;
        let mac = hmac_sha256(key, &canonical);
        let mut out = self.clone();
        out.payload = None;
        out.ct = Some(b64::encode(&ct));
        out.enc = EncMeta {
            alg: EncAlg::Aes256GcmSiv,
            kid: Some(kid),
            nonce: Some(b64::encode(&nonce)),
            hash_alg: HashAlg::HmacSha256,
        };
        out.hash = format!("{HMAC_SHA256_PREFIX}{}", notera_core::hex_lower(&mac));
        out.validate()?;
        Ok(out)
    }

    /// 密文 → 明文 payload（校验 keyed 摘要 + AEAD 标签）。
    pub fn open_with_key(&self, key: &[u8; KEY_LEN]) -> Result<Value, CryptoError> {
        if !matches!(self.enc.alg, EncAlg::Aes256GcmSiv) {
            return Err(CryptoError::Malformed("信封不是密文形态".into()));
        }
        let ct = self
            .ct
            .as_deref()
            .ok_or_else(|| CryptoError::Malformed("缺少 ct".into()))?;
        let nonce = self
            .enc
            .nonce
            .as_deref()
            .ok_or_else(|| CryptoError::Malformed("缺少 nonce".into()))?;
        let bytes = b64::decode(ct)?;
        let pt = crate::open(&bytes, &b64::decode_nonce(nonce)?, key)?;
        let want = format!(
            "{HMAC_SHA256_PREFIX}{}",
            notera_core::hex_lower(&hmac_sha256(key, &pt))
        );
        if want != self.hash {
            return Err(CryptoError::HashMismatch {
                expected: self.hash.clone(),
                actual: want,
            });
        }
        serde_json::from_slice(&pt).map_err(|e| CryptoError::Malformed(e.to_string()))
    }
}

/// 原始 JSON 里的 `kind` 是否是本客户端认识的实体类型。
///
/// 独立成一个函数，是因为 `Envelope::from_wire` 要在 serde 之前给出**准确**错误：
/// 未知 `kind` 的记录不得进入权威表（INV-08/FWD-06）。
pub fn check_kind_value(raw: &Value) -> Result<(), CryptoError> {
    let kind = raw
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| CryptoError::Malformed("信封缺少 kind".into()))?;
    parse_kind(kind)
        .map(|_| ())
        .ok_or_else(|| CryptoError::Malformed(format!("未知实体类型: {kind:?}")))
}

fn parse_kind(s: &str) -> Option<EntityKind> {
    match s {
        "note" => Some(EntityKind::Note),
        "folder" => Some(EntityKind::Folder),
        // 目录名是 "attachments"（EntityKind::dir()），单数同样接受，避免两侧写法打架。
        "attachment" | "attachments" => Some(EntityKind::Attachment),
        _ => None,
    }
}

/// `kind` 的 wire 形态：与 `EntityKind::dir()`（远端目录名）一致。
mod kind_wire {
    use serde::de::Error as _;
    use serde::{Deserialize, Deserializer, Serializer};

    use notera_core::EntityKind;

    pub(super) fn serialize<S: Serializer>(kind: &EntityKind, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(kind.dir())
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<EntityKind, D::Error> {
        let raw = String::deserialize(d)?;
        crate::envelope::parse_kind(&raw)
            .ok_or_else(|| D::Error::custom(format!("未知实体类型: {raw:?}")))
    }
}

/// RFC 3339 的**严格子集**校验：`YYYY-MM-DDTHH:MM:SS[.fff…](Z | ±HH:MM)`。
///
/// 手写而不引 `chrono`：本 crate 的依赖表由 workspace 锁死（只有 sha2/aes-gcm-siv/argon2/
/// rand/base64），加依赖不是本层能决定的事。`updated_at` 只用于展示与诊断（I4/R3），
/// 所以这里只要求"形态对"，绝不做任何时间比较。
fn check_timestamp(s: &str, field: &str) -> Result<(), CryptoError> {
    let bad = |why: &str| CryptoError::Malformed(format!("{field} 不是 RFC 3339 毫秒 UTC: {why}"));
    let b = s.as_bytes();
    if b.len() < 20 {
        return Err(bad("长度不足"));
    }
    let digit = |i: usize| -> bool { b[i].is_ascii_digit() };
    for i in [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18] {
        if !digit(i) {
            return Err(bad("日期/时间数字位错位"));
        }
    }
    if b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' {
        return Err(bad("分隔符不对"));
    }
    let num = |a: usize, c: usize| -> u32 { s[a..c].parse::<u32>().unwrap_or(u32::MAX) };
    if !(1..=12).contains(&num(5, 7)) {
        return Err(bad("月份越界"));
    }
    if !(1..=31).contains(&num(8, 10)) {
        return Err(bad("日越界"));
    }
    if num(11, 13) > 23 || num(14, 16) > 59 || num(17, 19) > 60 {
        return Err(bad("时分秒越界（秒允许 60 以容纳闰秒）"));
    }
    // 小数部分（可选）+ 时区。
    let mut i = 19usize;
    if i < b.len() && (b[i] == b'.' || b[i] == b',') {
        i += 1;
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == start {
            return Err(bad("小数点后面没有数字"));
        }
    }
    if i == b.len() {
        return Err(bad("缺少时区标记"));
    }
    match b[i] {
        b'Z' | b'z' => {
            if i + 1 != b.len() {
                return Err(bad("Z 之后还有内容"));
            }
        }
        b'+' | b'-' => {
            if b.len() != i + 6 || b[i + 3] != b':' || !digit(i + 1) || !digit(i + 2) {
                return Err(bad("时区偏移形态不对"));
            }
        }
        _ => return Err(bad("时区标记不对")),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const T: &str = "2026-09-25T09:12:03.441Z";
    const T2: &str = "2026-09-26T10:00:00Z";
    const DEV: &str = "0192e6c1-0000-7000-8000-00000000000d";

    fn note_id() -> EntityId {
        EntityId::parse("0192e6c1-0000-7000-8000-00000000abcd").unwrap()
    }
    fn other_id() -> EntityId {
        EntityId::parse("0192e6c1-0000-7000-8000-0000000000ff").unwrap()
    }
    fn doc() -> Value {
        json!({
            "v": 1,
            "content": [
                { "id": "p1aaaaaa", "type": "paragraph", "content": [{ "text": "同步测试内容" }] },
                { "id": "mmmmmmmm", "type": "mermaid", "attrs": { "code": "A-->B" } }
            ]
        })
    }
    fn note_env() -> Envelope {
        Envelope::for_note(&note_id(), Rev(7), Rev(6), &doc(), None, DEV, T)
    }

    // ------------------------------------------------------------ 往返 ---

    #[test]
    fn note_envelope_roundtrips_through_wire() {
        let e = note_env();
        assert_eq!(e.kind, EntityKind::Note);
        assert!(matches!(e.enc.alg, EncAlg::None));
        assert_eq!(e.rev, Rev(7));
        assert_eq!(e.sync_rev, Rev(6));
        let wire = e.to_wire().unwrap();
        let back = Envelope::from_wire(&wire).expect("自己写出的信封必须能读回来");
        assert_eq!(back, e);
        assert_eq!(back.payload_doc(), Some(&doc()));
        assert_eq!(back.check_matches(EntityKind::Note, &note_id()), Ok(()));
    }

    #[test]
    fn wire_field_set_matches_the_frozen_contract() {
        // ARCHITECTURE-MAP §6：信封字段集是契约冻结点，逐字段钉死。
        let wire = note_env().to_wire().unwrap();
        let v: Value = serde_json::from_str(&wire).unwrap();
        let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(|s| s.as_str()).collect();
        keys.sort();
        assert_eq!(
            keys,
            vec![
                "ct",
                "deleted_at",
                "device",
                "enc",
                "hash",
                "id",
                "kind",
                "payload",
                "protocol",
                "purged",
                "rev",
                "sync_rev",
                "updated_at"
            ]
        );
        assert_eq!(
            v["kind"],
            json!("note"),
            "kind 用远端目录名，不是 Rust 变体名"
        );
        assert_eq!(v["protocol"], json!(PROTOCOL));
        assert_eq!(v["deleted_at"], Value::Null);
        assert_eq!(v["purged"], json!(false));
        assert_eq!(v["ct"], Value::Null);
        assert_eq!(v["enc"]["alg"], json!("none"));
        assert_eq!(v["enc"]["hash_alg"], json!("sha256"));
        assert!(
            v["hash"].as_str().unwrap().starts_with("sha256:"),
            "hash 是带前缀的权威形态"
        );
        assert_eq!(v["hash"], json!(note_env().hash));
    }

    #[test]
    fn hash_is_sha256_of_canonical_payload_so_key_order_cannot_change_it() {
        // 同一逻辑文档、不同键序 → 同一个 hash。这是"哈希即身份"的前提。
        let reordered = json!({
            "content": [
                { "content": [{ "text": "同步测试内容" }], "type": "paragraph", "id": "p1aaaaaa" },
                { "type": "mermaid", "attrs": { "code": "A-->B" }, "id": "mmmmmmmm" }
            ],
            "v": 1
        });
        let a = Envelope::for_note(&note_id(), Rev(7), Rev(6), &doc(), None, DEV, T);
        let b = Envelope::for_note(&note_id(), Rev(7), Rev(6), &reordered, None, DEV, T);
        assert_eq!(
            a.hash, b.hash,
            "键序影响了内容哈希：{} vs {}",
            a.hash, b.hash
        );
        assert_eq!(
            a.hash,
            format!(
                "sha256:{}",
                crate::sha256_hex(canonical_json(&doc()).as_bytes())
            )
        );
        // 一个字的差别必须是完全不同的哈希。
        let mut changed = doc();
        changed["content"][0]["content"][0]["text"] = json!("同步测试内容！");
        let c = Envelope::for_note(&note_id(), Rev(8), Rev(6), &changed, None, DEV, T2);
        assert_ne!(a.hash, c.hash);
    }

    #[test]
    fn folder_envelope_roundtrips_and_keeps_folder_metadata() {
        let meta = json!({ "name": "工作笔记", "parent_id": null, "order": 3 });
        let e = Envelope::for_folder(&note_id(), Rev(2), Rev(1), &meta, None, DEV, T);
        let back = Envelope::from_wire(&e.to_wire().unwrap()).unwrap();
        assert_eq!(back, e);
        assert_eq!(back.kind, EntityKind::Folder);
        assert_eq!(back.payload_doc(), Some(&meta));
        assert_eq!(
            back.check_matches(EntityKind::Note, &note_id()),
            Err(CryptoError::KindMismatch)
        );
    }

    #[test]
    fn to_wire_is_byte_stable() {
        let e = note_env();
        assert_eq!(e.to_wire().unwrap(), e.to_wire().unwrap());
    }

    // ---------------------------------------------------- 严格校验（SYNC §3） ---

    #[test]
    fn payload_and_ct_are_mutually_exclusive() {
        let mut e = note_env();
        e.ct = Some(b64::encode("看起来像密文".as_bytes()));
        assert_eq!(e.to_wire(), Err(CryptoError::PayloadAndCtMutuallyExclusive));
        // 读入侧同样必须拒收（不是"忽略 ct 继续用 payload"）。
        let wire = {
            let mut v: Value = serde_json::from_str(&note_env().to_wire().unwrap()).unwrap();
            v["ct"] = json!("0000000000000000000000000000");
            v.to_string()
        };
        assert_eq!(
            Envelope::from_wire(&wire),
            Err(CryptoError::PayloadAndCtMutuallyExclusive)
        );
    }

    #[test]
    fn neither_payload_nor_ct_is_rejected_unless_purged() {
        let wire = {
            let mut v: Value = serde_json::from_str(&note_env().to_wire().unwrap()).unwrap();
            v["payload"] = Value::Null;
            v.to_string()
        };
        assert_eq!(
            Envelope::from_wire(&wire),
            Err(CryptoError::PayloadAndCtMutuallyExclusive)
        );
    }

    #[test]
    fn tampered_payload_is_rejected_by_hash() {
        let mut v: Value = serde_json::from_str(&note_env().to_wire().unwrap()).unwrap();
        // 只改一个汉字，hash 不动。
        v["payload"]["content"][0]["content"][0]["text"] = json!("同步测试内容被改");
        let wire = v.to_string();
        let err = Envelope::from_wire(&wire).unwrap_err();
        match err {
            CryptoError::HashMismatch { expected, actual } => {
                assert_eq!(expected, note_env().hash);
                assert_ne!(expected, actual, "错误里必须带上两个哈希供诊断");
            }
            other => panic!("期望 HashMismatch，实际 {other:?}"),
        }
    }

    #[test]
    fn tampered_hash_field_itself_is_rejected() {
        let mut v: Value = serde_json::from_str(&note_env().to_wire().unwrap()).unwrap();
        let flipped = v["hash"].as_str().unwrap().replace("sha256:", "sha256:0");
        v["hash"] = json!(flipped);
        assert!(matches!(
            Envelope::from_wire(&v.to_string()),
            Err(CryptoError::Malformed(_))
        ));
    }

    #[test]
    fn unknown_kind_is_rejected_with_an_accurate_error() {
        // FWD-06：未知 kind 的记录不得进入权威表。
        let mut v: Value = serde_json::from_str(&note_env().to_wire().unwrap()).unwrap();
        v["kind"] = json!("widget");
        let err = Envelope::from_wire(&v.to_string()).unwrap_err();
        assert!(
            matches!(&err, CryptoError::Malformed(m) if m.contains("widget")),
            "{err:?}"
        );
        // 缺失 kind 同样拒。
        v.as_object_mut().unwrap().remove("kind");
        assert!(matches!(
            Envelope::from_wire(&v.to_string()),
            Err(CryptoError::Malformed(_))
        ));
    }

    #[test]
    fn unsafe_id_is_rejected_because_it_becomes_a_path() {
        // notera-webdav 会把这个 id 拼进 records/note/<id>.json —— 目录穿越是安全事故。
        for bad in [
            "../../etc/passwd",
            "",
            "a-b",
            "0192e6c1-0000-7000-8000-00000000abcd/../../x",
        ] {
            let wire = {
                let mut v: Value = serde_json::from_str(&note_env().to_wire().unwrap()).unwrap();
                v["id"] = json!(bad);
                v.to_string()
            };
            let err = Envelope::from_wire(&wire).unwrap_err();
            assert!(
                matches!(&err, CryptoError::Malformed(m) if m.contains("id")),
                "{bad} → {err:?}"
            );
        }
    }

    #[test]
    fn protocol_and_shape_mismatches_are_rejected() {
        for tweak in [
            (json!(2), json!("note"), json!(T), json!("")), // 协议超前
            (json!(PROTOCOL), json!("note"), json!("昨天下午"), json!("")), // 时间戳形态错
            (json!(PROTOCOL), json!("note"), json!(T), json!("")), // device 为空
        ] {
            let wire = {
                let mut v: Value = serde_json::from_str(&note_env().to_wire().unwrap()).unwrap();
                v["protocol"] = tweak.0;
                v["kind"] = tweak.1;
                v["updated_at"] = tweak.2;
                v["device"] = tweak.3;
                v.to_string()
            };
            assert!(
                matches!(Envelope::from_wire(&wire), Err(CryptoError::Malformed(_))),
                "必须拒收：{wire}"
            );
        }
        // 截断 / 非 JSON / 顶层不是对象：都不许"尽力解析"出一个能用的信封。
        let good = note_env().to_wire().unwrap();
        for bad in [
            String::new(),
            "{".to_string(),
            "[]".to_string(),
            good[..20].to_string(),
        ] {
            assert!(
                matches!(Envelope::from_wire(&bad), Err(CryptoError::Malformed(_))),
                "{bad}"
            );
        }
    }

    #[test]
    fn check_matches_catches_wrong_file_or_wrong_directory() {
        let e = note_env();
        assert_eq!(e.check_matches(EntityKind::Note, &note_id()), Ok(()));
        assert_eq!(
            e.check_matches(EntityKind::Folder, &note_id()),
            Err(CryptoError::KindMismatch)
        );
        assert_eq!(
            e.check_matches(EntityKind::Note, &other_id()),
            Err(CryptoError::IdMismatch)
        );
        // 两个都不对时先报 kind（顺序稳定，便于诊断）。
        assert_eq!(
            e.check_matches(EntityKind::Folder, &other_id()),
            Err(CryptoError::KindMismatch)
        );
    }

    // ------------------------------------------------------- 删除与墓碑 ---

    #[test]
    fn deleted_note_keeps_payload_and_deletion_fact() {
        let e = Envelope::for_note(&note_id(), Rev(8), Rev(7), &doc(), Some(T2), DEV, T2);
        let back = Envelope::from_wire(&e.to_wire().unwrap()).unwrap();
        assert_eq!(back.deleted_at.as_deref(), Some(T2));
        assert!(
            back.payload_doc().is_some(),
            "删除记录仍带正文（回收站要能预览）"
        );
        assert!(!back.purged);
    }

    #[test]
    fn purged_tombstone_carries_no_payload_but_still_roundtrips() {
        // SYNC-PROTOCOL §3 末段：purged 的记录 payload 为 null。
        let e = Envelope::for_purged(EntityKind::Note, &note_id(), Rev(9), Rev(8), T2, DEV, T2);
        assert!(e.payload_doc().is_none());
        assert!(e.purged);
        let wire = e.to_wire().unwrap();
        let back = Envelope::from_wire(&wire).expect("墓碑必须可读回来");
        assert_eq!(back, e);
        // 墓碑没有 deleted_at 就不成立（I3：删除事实必须比数据活得久）。
        let mut no_fact = e.clone();
        no_fact.deleted_at = None;
        assert!(matches!(no_fact.to_wire(), Err(CryptoError::Malformed(_))));
        // 墓碑也不许偷偷带 ct。
        let mut with_ct = e.clone();
        with_ct.ct = Some(b64::encode(b"x"));
        assert!(matches!(with_ct.to_wire(), Err(CryptoError::Malformed(_))));
    }

    // ------------------------------------------------------------ E2EE ---

    #[test]
    fn sealed_envelope_roundtrips_and_authenticates() {
        let key = crate::derive_key("口令".as_bytes(), &[7u8; 16]);
        let plain = note_env();
        let sealed = plain.seal_with_key(3, &key).unwrap();
        assert!(
            sealed.payload_doc().is_none(),
            "密文信封不得再带明文 payload"
        );
        assert!(sealed.ct.is_some());
        assert!(matches!(sealed.enc.alg, EncAlg::Aes256GcmSiv));
        assert_eq!(sealed.enc.kid, Some(3));
        assert_eq!(sealed.enc.hash_alg, HashAlg::HmacSha256);
        assert!(
            sealed.hash.starts_with(HMAC_SHA256_PREFIX),
            "开启 E2EE 后 hash 必须换 keyed 摘要"
        );
        // 密文里绝不能残留明文字符串。
        let wire = sealed.to_wire().unwrap();
        assert!(!wire.contains("同步测试内容"), "密文信封泄露了明文：{wire}");
        let back = Envelope::from_wire(&wire).unwrap();
        assert_eq!(back, sealed);
        assert_eq!(
            back.open_with_key(&key).unwrap(),
            json!(serde_json::from_str::<Value>(&canonical_json(&doc())).unwrap()),
            "解出来的必须是同一份 canonical payload"
        );
        // ct 长度 = canonical(payload) 长度 + 16（INV-12 在信封层的表现）。
        assert_eq!(
            b64::decode(back.ct.as_deref().unwrap()).unwrap().len(),
            canonical_json(&doc()).len() + 16
        );
    }

    #[test]
    fn sealed_envelope_rejects_wrong_key_and_tampering() {
        let key = crate::derive_key("口令".as_bytes(), &[7u8; 16]);
        let sealed = note_env().seal_with_key(1, &key).unwrap();
        assert!(matches!(
            sealed.open_with_key(&crate::derive_key("错口令".as_bytes(), &[7u8; 16])),
            Err(CryptoError::Auth(_)) | Err(CryptoError::HashMismatch { .. })
        ));
        let mut v: Value = serde_json::from_str(&sealed.to_wire().unwrap()).unwrap();
        let mut bytes = b64::decode(v["ct"].as_str().unwrap()).unwrap();
        bytes[0] ^= 1;
        v["ct"] = json!(b64::encode(&bytes));
        let bad = Envelope::from_wire(&v.to_string()).unwrap();
        assert!(matches!(
            bad.open_with_key(&key),
            Err(CryptoError::Auth(_)) | Err(CryptoError::HashMismatch { .. })
        ));
    }

    #[test]
    fn enc_meta_must_be_self_consistent() {
        // alg≠none 但 hash_alg 还是明文摘要 → 泄露等价指纹，必须拒。
        let key = [1u8; 32];
        let sealed = note_env().seal_with_key(1, &key).unwrap();
        let cases: Vec<(EncMeta, Option<Value>, Option<String>)> = vec![
            (
                EncMeta {
                    alg: EncAlg::Aes256GcmSiv,
                    kid: Some(1),
                    nonce: sealed.enc.nonce.clone(),
                    hash_alg: HashAlg::Sha256,
                },
                None,
                sealed.ct.clone(),
            ),
            (EncMeta::default(), None, None), // 两个都没有
            (
                EncMeta {
                    alg: EncAlg::None,
                    kid: Some(1),
                    nonce: None,
                    hash_alg: HashAlg::Sha256,
                },
                sealed.payload_doc().cloned(),
                None,
            ), // alg=none 却留着 kid
            (
                EncMeta {
                    alg: EncAlg::Aes256GcmSiv,
                    kid: None,
                    nonce: None,
                    hash_alg: HashAlg::HmacSha256,
                },
                None,
                sealed.ct.clone(),
            ), // 缺 kid/nonce
        ];
        for (enc, payload, ct) in cases {
            let mut e = sealed.clone();
            e.enc = enc;
            e.payload = payload;
            e.ct = ct;
            assert!(e.validate().is_err(), "自相矛盾的 enc 必须拒：{:?}", e.enc);
        }
        // 密封前先校验：hash 已经对不上的明文不许被"洗白"成密文。
        let mut liar = note_env();
        liar.hash = format!("sha256:{}", crate::sha256_hex("随便算的".as_bytes()));
        assert!(matches!(
            liar.seal_with_key(1, &key),
            Err(CryptoError::HashMismatch { .. })
        ));
    }

    #[test]
    fn note_payload_must_be_a_valid_document_before_it_leaves_the_machine() {
        // 写出侧多一道富文本校验：坏数据不许被发布出去（I6）。
        let bad = Envelope::for_note(
            &note_id(),
            Rev(1),
            Rev(0),
            &json!({ "name": "这不是文档" }),
            None,
            DEV,
            T,
        );
        assert!(matches!(bad.to_wire(), Err(CryptoError::Malformed(_))));
        // 但读入侧不因 payload 内部语义而整条拒收（只读降级由同步层处理）。
        let mut v: Value = serde_json::from_str(&note_env().to_wire().unwrap()).unwrap();
        v["payload"]["v"] = json!(99);
        let h = crate::sha256_hex(canonical_json(&v["payload"]).as_bytes());
        v["hash"] = json!(format!("sha256:{h}"));
        let read = Envelope::from_wire(&v.to_string()).expect("超前版本要能读，不能丢记录");
        assert_eq!(read.payload_doc().unwrap()["v"], json!(99));
        // 文件夹 payload 不是富文本，不该被误校验。
        let f = Envelope::for_folder(
            &note_id(),
            Rev(1),
            Rev(0),
            &json!({ "name": "夹子" }),
            None,
            DEV,
            T,
        );
        assert!(f.to_wire().is_ok());
    }

    // ---------------------------------------------------- 时间戳形态校验 ---

    #[test]
    fn timestamp_forms_are_checked_exactly_enough() {
        for good in [
            T,
            T2,
            "2026-09-25T09:12:03Z",
            "2026-12-31T23:59:59.999999Z",
            "2026-09-25T09:12:03+08:00",
        ] {
            assert_eq!(check_timestamp(good, "t"), Ok(()), "{good}");
        }
        for bad in [
            "",
            "2026-09-25",
            "2026-09-25T09:12:03",
            "2026-09-25T09:12:03.Z",
            "2026-13-25T09:12:03Z",
            "2026-09-25T25:12:03Z",
            "2026-09-25 09:12:03Z",
            "yesterday",
            "2026-09-25T09:12:03+0800",
        ] {
            assert!(check_timestamp(bad, "t").is_err(), "{bad} 必须被拒");
        }
    }
}
