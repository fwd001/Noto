//! notera-crypto —— 记录信封（wire 形态）与密码学原语。
//!
//! 规范来源：`docs/DATA-MODEL.md` §11（信封）、`docs/SYNC-PROTOCOL.md` §3（wire 约束表）、
//! ADR-0002（v1 即预留 E2EE 结构）、`docs/ARCHITECTURE-MAP.md` §3 不变式 I6（脏数据不入库）。
//!
//! 依赖方向：只依赖 `notera-core`（+ `notera-richtext` 仅用于 payload 的规范化语义说明）。
//! **不得**依赖 store / webdav / sync：本 crate 不知道"远端"这件事。
//!
//! ## 这个 crate 存在的唯一理由
//! 信封是"本机权威数据"与"外部世界"的边界。边界上任何一条约束放松，脏数据就会进入权威表
//! （违反 I6），而且是在几台设备分别落盘之后才被发现。所以 [`Envelope::from_wire`] 的每个
//! 分支都宁可 `Err` 也不返回一个"看起来能用"的对象。
//!
//! ## ⚠️ 性能约束（实测，不是偏好）
//! [`derive_key`] = argon2id `m=19 MiB, t=2, p=1`，实测 ≈400 ms/次。
//! **禁止**在 UI 可感知路径（输入框、列表渲染、首帧、同步轮次内的每次校验）调用它：
//! ARCHITECTURE-MAP §5 明令"在 UI 线程调用 argon2id"属于禁止模式。
//! 调用方必须：只在用户显式解锁/设置口令时调用，且放到后台线程，并自带缓存（密钥派生结果
//! 按 (passphrase, salt) 复用，绝不为每条记录派生一次）。本 crate 只提供函数，不提供调度。

mod envelope;

pub use envelope::{check_kind_value, EncAlg, EncMeta, Envelope, HashAlg};

use sha2::{Digest, Sha256};

/// 协议版本（契约冻结点：ARCHITECTURE-MAP §6「信封字段集」）。
pub const PROTOCOL: u16 = 1;

/// `ContentHash` 的权威前缀。
pub const SHA256_PREFIX: &str = "sha256:";
/// E2EE 启用后的哈希前缀（SYNC-PROTOCOL §3：否则泄露明文等价指纹）。
pub const HMAC_SHA256_PREFIX: &str = "hmac-sha256:";

/// 密码学/信封层错误。
#[derive(thiserror::Error, Clone, Debug, PartialEq, Eq)]
pub enum CryptoError {
    /// `payload` 与 `ct` 必须恰好一个非空（SYNC-PROTOCOL §3 第一行约束）。
    #[error("payload 与 ct 互斥：必须恰好一个非空")]
    PayloadAndCtMutuallyExclusive,
    /// 内容哈希对不上：数据在传输中被改，或写入方算错了。
    #[error("哈希不符：期望 {expected}，实际 {actual}")]
    HashMismatch { expected: String, actual: String },
    /// 信封的 `kind` 与调用方请求的目录/实体类型不一致。
    #[error("信封 kind 与请求的实体类型不一致")]
    KindMismatch,
    /// 信封的 `id` 与文件名/请求 ID 不一致。
    #[error("信封 id 与请求的实体 ID 不一致")]
    IdMismatch,
    /// 结构不合法（缺字段、版本不支持、时间戳格式错、哈希格式错……）。
    #[error("信封结构不合法: {0}")]
    Malformed(String),
    /// AEAD 认证失败：密钥错或密文/标签被篡改。
    #[error("认证失败（密钥错误或密文被篡改）: {0}")]
    Auth(String),
}

// ------------------------------------------------------------------- 摘要 ---

/// 裸 64 位小写十六进制 sha256（不带 `sha256:` 前缀）。
///
/// 权威形态是 `notera_core::ContentHash`（带前缀）；这里给需要裸 hex 的调用方
/// （附件寻址、HMAC 输出、fixture 断言）。
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    notera_core::hex_lower(&h.finalize())
}

/// 流式算文件 sha256。备份/恢复要校验的是整库快照，动辄上百 MB，
/// 一次性读进内存会把"备份"变成内存压力源。
pub fn sha256_hex_file(path: &std::path::Path) -> Result<String, std::io::Error> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buf)?;
        if read == 0 {
            break;
        }
        h.update(&buf[..read]);
    }
    Ok(notera_core::hex_lower(&h.finalize()))
}

/// 校验 `bytes` 的 sha256 是否等于 `expected`。
///
/// `expected` 接受 `sha256:<64hex>` 或裸 `<64hex>`，大小写不敏感。
/// 比较按字节做**等长、全字节**遍历（不因前缀相同而提前返回），避免把"逐字节短路"
/// 变成对远端数据的可测信道。
pub fn verify_bytes(bytes: &[u8], expected: &str) -> bool {
    let hex = expected
        .trim()
        .strip_prefix(SHA256_PREFIX)
        .unwrap_or(expected.trim());
    let Some(want) = decode_hex(hex) else {
        return false;
    };
    let mut h = Sha256::new();
    h.update(bytes);
    let got = h.finalize();
    if got.len() != want.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in got.iter().zip(want.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = Vec::with_capacity(32);
    let bs = s.as_bytes();
    for pair in bs.chunks(2) {
        let hi = (pair[0] as char).to_digit(16)?;
        let lo = (pair[1] as char).to_digit(16)?;
        out.push(((hi << 4) | lo) as u8);
    }
    Some(out)
}

/// 把 `hash` 字段拆成 `(前缀, 裸hex)`。
pub(crate) fn split_hash(s: &str) -> Option<(&str, &str)> {
    for p in [SHA256_PREFIX, HMAC_SHA256_PREFIX] {
        if let Some(rest) = s.strip_prefix(p) {
            if rest.len() == 64 && rest.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Some((p, rest));
            }
            return None;
        }
    }
    None
}

// ------------------------------------------------------------------- KDF ---

/// argon2id 参数（实测 ≈400 ms/次；见模块头 ⚠️ 约束）。
pub const ARGON2_M_KIB: u32 = 19 * 1024;
/// 迭代次数。
pub const ARGON2_T: u32 = 2;
/// 并行度。
pub const ARGON2_P: u32 = 1;
/// 派生密钥长度（AES-256-GCM-SIV 密钥）。
pub const KEY_LEN: usize = 32;
/// salt 长度（记录在 lib-state，跨设备不变）。
pub const SALT_LEN: usize = 16;

/// 口令 → 256 位密钥（argon2id，`m=19 MiB / t=2 / p=1`，RFC 9106 的推荐形态）。
///
/// 纯函数：同 `(passphrase, salt)` 必得同输出（测试断言这一点）。
///
/// ⚠️ **实测 ≈400 ms**。绝不允许出现在 UI 可感知路径或每条记录的校验循环里；
/// 调用方负责：后台线程 + 显式用户动作触发 + 结果缓存。盐必须由调用方安全随机生成
/// 并持久化（`lib_state.kdf_salt`），换盐即等于换一把钥匙 —— 旧记录将永远解不开。
pub fn derive_key(passphrase: &[u8], salt: &[u8; SALT_LEN]) -> [u8; KEY_LEN] {
    let params = argon2::Params::new(ARGON2_M_KIB, ARGON2_T, ARGON2_P, Some(KEY_LEN))
        .expect("Params::new 在 m=19MiB/t=2/p=1/out=32 下不会失败");
    let ctx = argon2::Argon2::new(
        argon2::Algorithm::Argon2id,
        // argon2 0.6 只提供 V0x10 / V0x13；V0x13 是当前版本（RFC 9106 的 0x13）。
        argon2::Version::V0x13,
        params,
    );
    let mut out = [0u8; KEY_LEN];
    let ok = ctx.hash_password_into(passphrase, salt, &mut out);
    debug_assert!(ok.is_ok(), "argon2id 参数越界: {ok:?}");
    out
}

// -------------------------------------------------------------- AEAD 原语 ---

/// 加密（AES-256-GCM-SIV，RFC 8452）。返回 `(ct‖tag, nonce)`。
///
/// 实测（`envelope-aes-256-gcm-siv`）：**标签恒 16 字节**，所以 `ct.len() == pt.len() + 16`
/// 恒成立（INV-12）；nonce 12 字节，由 CSPRNG 生成。
/// AES-GCM-**SIV**（不是 GCM）是刻意的：它抗 nonce 误用（重复 nonce 不泄露明文），
/// 而"崩溃后重放同一 nonce"在同步引擎里是可达状态，不能靠"我们保证不重复"这种论证。
pub fn seal_plaintext(pt: &[u8], key: &[u8; KEY_LEN]) -> Result<(Vec<u8>, [u8; 12]), CryptoError> {
    use aes_gcm_siv::{aead::Aead, Aes256GcmSiv, KeyInit, Nonce};
    let cipher = Aes256GcmSiv::new(&aes_gcm_siv::Key::<Aes256GcmSiv>::from(*key));
    let mut nonce = [0u8; 12];
    use rand::RngCore;
    rand::rng().fill_bytes(&mut nonce);
    let ct = cipher
        .encrypt(&Nonce::from(nonce), pt)
        .map_err(|e| CryptoError::Auth(format!("加密失败: {e}")))?;
    debug_assert_eq!(ct.len(), pt.len() + 16, "标签必须恒 16 字节");
    Ok((ct, nonce))
}

/// 解密并校验。任何篡改（哪怕只翻 1 位）都返回 [`CryptoError::Auth`]。
pub fn open(ct: &[u8], nonce: &[u8; 12], key: &[u8; KEY_LEN]) -> Result<Vec<u8>, CryptoError> {
    use aes_gcm_siv::{aead::Aead, Aes256GcmSiv, KeyInit, Nonce};
    if ct.len() < 16 {
        return Err(CryptoError::Auth("密文短于 16 字节标签".into()));
    }
    let cipher = Aes256GcmSiv::new(&aes_gcm_siv::Key::<Aes256GcmSiv>::from(*key));
    cipher
        .decrypt(&Nonce::from(*nonce), ct)
        .map_err(|e| CryptoError::Auth(format!("认证失败: {e}")))
}

// ------------------------------------------------------------------- HMAC ---

/// RFC 2104 HMAC-SHA256（不引额外 crate：workspace 只锁了 `sha2`）。
///
/// E2EE 打开后 `hash` 必须是 keyed 摘要（SYNC-PROTOCOL §3），否则等价指纹泄露明文。
pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    const B: usize = 64; // SHA-256 块长
    let mut k = [0u8; B];
    if key.len() > B {
        let digest = Sha256::digest(key);
        k[..32].copy_from_slice(&digest);
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0u8; B];
    let mut opad = [0u8; B];
    for i in 0..B {
        ipad[i] = k[i] ^ 0x36;
        opad[i] = k[i] ^ 0x5c;
    }
    let inner = {
        let mut h = Sha256::new();
        h.update(ipad);
        h.update(msg);
        h.finalize()
    };
    let mut o = Sha256::new();
    o.update(opad);
    o.update(inner);
    let out = o.finalize();
    let mut r = [0u8; 32];
    r.copy_from_slice(&out);
    r
}

/// base64（标准字母表，带 padding）：nonce / ct 在 wire 上的形态。
pub mod b64 {
    use base64::{engine::general_purpose::STANDARD, Engine as _};

    use crate::CryptoError;

    pub fn encode(bytes: &[u8]) -> String {
        STANDARD.encode(bytes)
    }

    pub fn decode(s: &str) -> Result<Vec<u8>, CryptoError> {
        STANDARD
            .decode(s)
            .map_err(|e| CryptoError::Malformed(format!("base64 解不开: {e}")))
    }

    /// nonce 必须是 12 字节（实测：AES-256-GCM-SIV 固定 12 B）。
    pub fn decode_nonce(s: &str) -> Result<[u8; 12], CryptoError> {
        let raw = decode(s)?;
        let bytes: [u8; 12] = raw
            .as_slice()
            .try_into()
            .map_err(|_| CryptoError::Malformed(format!("nonce 长度应为 12，实际 {}", raw.len())))?;
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_known_answer_vector() {
        // FIPS 180-4：`abc`。摘要错则全系统的"是否改过"判定失效。
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    }

    #[test]
    fn verify_bytes_accepts_both_forms() {
        let h = sha256_hex(b"notera");
        assert!(verify_bytes(b"notera", &h));
        assert!(verify_bytes(b"notera", &format!("sha256:{h}")));
        assert!(verify_bytes(b"notera", &h.to_uppercase()));
        assert!(!verify_bytes(b"notera2", &h));
        assert!(!verify_bytes(b"notera", "not-hex"));
        assert!(!verify_bytes(b"notera", ""));
    }

    #[test]
    fn hmac_matches_rfc4231_vector() {
        // RFC 4231 TC2：key="Jefe", data="what do ya want for nothing?"
        let mac = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(
            notera_core::hex_lower(&mac),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn hash_field_splits_only_authoritative_forms() {
        let h = sha256_hex(b"x");
        assert_eq!(split_hash(&format!("sha256:{h}")).map(|(_, r)| r), Some(h.as_str()));
        assert!(split_hash(&h).is_none(), "裸 hex 不是 wire 上的权威形态");
        assert!(split_hash("sha256:zz").is_none());
    }

    #[test]
    fn seal_open_roundtrip_and_tag_is_16_bytes() {
        let key = [7u8; 32];
        for pt in [
            b"".to_vec(),
            b"x".to_vec(),
            "同步测试内容".as_bytes().to_vec(),
            (0..4096u32).map(|i| i as u8).collect::<Vec<u8>>(),
        ] {
            let (ct, nonce) = seal_plaintext(&pt, &key).unwrap();
            assert_eq!(ct.len(), pt.len() + 16, "INV-12: 标签必须恰好 16 字节");
            assert_eq!(nonce.len(), 12);
            assert_eq!(open(&ct, &nonce, &key).unwrap(), pt);
        }
    }

    #[test]
    fn single_bit_flip_is_always_rejected() {
        let key = [3u8; 32];
        let pt = "用户输入过的内容".as_bytes().to_vec();
        let (ct, nonce) = seal_plaintext(&pt, &key).unwrap();
        for i in 0..ct.len() {
            for bit in 0..8 {
                let mut bad = ct.clone();
                bad[i] ^= 1 << bit;
                assert!(
                    open(&bad, &nonce, &key).is_err(),
                    "翻转字节 {i} 的 bit {bit} 必须被拒（INV-12）"
                );
            }
        }
        // nonce 篡改同样必须被拒。
        for bit in 0..8 {
            let mut bad_nonce = nonce;
            bad_nonce[0] ^= 1 << bit;
            assert!(open(&ct, &bad_nonce, &key).is_err());
        }
        // 换钥匙必须打不开。
        assert!(open(&ct, &nonce, &[4u8; 32]).is_err());
    }

    #[test]
    fn seal_nonce_is_random_per_call() {
        let key = [9u8; 32];
        let (_, n1) = seal_plaintext(b"same".as_slice(), &key).unwrap();
        let (_, n2) = seal_plaintext(b"same".as_slice(), &key).unwrap();
        assert_ne!(n1, n2, "nonce 复用会把 GCM-SIV 退化成确定性输出");
    }

    #[test]
    fn derive_key_is_deterministic_and_salt_sensitive() {
        // 注意：本测试会真的跑 argon2id（≈400 ms/次）。它是 L0 单元测试，不在任何
        // 用户可感知路径上 —— 见模块头的 ⚠️ 约束。
        let salt = [1u8; 16];
        let a = derive_key(b"hunter2", &salt);
        let b = derive_key(b"hunter2", &salt);
        assert_eq!(a, b, "同输入必同输出，否则记录永远解不开");
        assert_ne!(a, derive_key(b"hunter3", &salt));
        assert_ne!(a, derive_key(b"hunter2", &[2u8; 16]));
        assert_ne!(a, [0u8; 32]);
    }

    #[test]
    fn b64_nonce_length_is_enforced() {
        let enc = b64::encode(&[0u8; 12]);
        assert_eq!(b64::decode_nonce(&enc).unwrap(), [0u8; 12]);
        assert!(b64::decode_nonce(&b64::encode(&[0u8; 11])).is_err());
        assert!(b64::decode_nonce("!!!").is_err());
    }
}
