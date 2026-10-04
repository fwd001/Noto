//! 上行密封与下行开封：E2EE 挂在同步引擎与 WebDAV 之间那一格。
//!
//! 规范来源：ADR-0002（信封 v1 即预留 E2EE 结构）、SYNC-PROTOCOL §3 wire 约束表、
//! ARCHITECTURE-MAP §3 不变式 I6（脏数据不入库）。
//!
//! ## 插在哪一格，为什么是那一格
//!
//! 同步引擎（`notera-sync`）用 `envelope_wire()` 拿到**明文**信封字节，直接交给
//! `put_record()` 上行。本模块就在两者之间：上行前 `seal`，下行后 `open`。
//!
//! 为什么不放进 `notera-store`��`envelope_wire` 同时被"写后复验"和"上行"复用，
//! 在那里密封会让**落库的读回**也变成密文 —— 那是另一个问题（ADR-0016 本地静态加密，
//! 独立议题），两件事混在一起就分不清是哪条链在起作用。
//!
//! ## 三条不许放松的约束
//!
//! 1. **I6 脏数据不入库**：解不开的信封一律返回 `Err`，由上层计入拒收并留下原因；
//!    绝不返回"半个信封"，也不静默丢���
//! 2. **条件写不许重试**：`seal` 之后 ETag 条件失效，所以密封只做一次、且必须在
//!    `probe_record_etag` **之前**完成（否则拿到的 ETag 属于明文那一版）。
//! 3. **墓碑是互斥规则的例外**：`purged:true` 时 `payload` 与 `ct` 皆为 null
//!    （ADR-0002 末尾的待确认项，本模块按此实现）。

use notera_crypto::{CryptoError, EncAlg, Envelope, KeyVault, HMAC_SHA256_PREFIX};

/// 一次上行/下行的结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Sealed {
    /// 明文形态（未启用加密，或本来就是墓碑）。
    Plaintext(Vec<u8>),
    /// 密文形态，可上行。
    Ciphertext { wire: Vec<u8>, kid: u32 },
}

impl Sealed {
    /// 交给 `put_record` 的字节。
    pub fn wire(&self) -> &[u8] {
        match self {
            Sealed::Plaintext(w) => w,
            Sealed::Ciphertext { wire, .. } => wire,
        }
    }

    /// 本次是否真的产生了密文。统计与界面提示用它。
    pub fn is_encrypted(&self) -> bool {
        matches!(self, Sealed::Ciphertext { .. })
    }
}

/// 密钥标识。第一把钥匙 = 1（0 留作"未加密"，让 `kid: None` 与 `kid: 0` 不混淆）。
pub const PRIMARY_KID: u32 = 1;

/// 上行前密封。
///
/// `plain_wire` 是 `envelope_wire()` 给出的明文信封字节。
/// `vault` 为 `None` 时**原样返回明文** —— 未启用加密的设备必须还能读能写，
/// 否则一启用就把自己锁在外面了。
pub fn seal_outgoing(plain_wire: &[u8], vault: Option<&KeyVault>) -> Result<Sealed, CryptoError> {
    let Some(vault) = vault else {
        return Ok(Sealed::Plaintext(plain_wire.to_vec()));
    };
    let env = Envelope::from_wire(
        std::str::from_utf8(plain_wire)
            .map_err(|e| CryptoError::Malformed(format!("上行信封不是合法 UTF-8: {e}")))?,
    )?;
    // 墓碑（purged）没有 payload，密封它只会造出"内容为空但声称已加密"的怪东西。
    if env.payload.is_none() {
        return Ok(Sealed::Plaintext(plain_wire.to_vec()));
    }
    let sealed = env.seal_with_key(PRIMARY_KID, vault.master())?;
    let wire = sealed
        .to_wire()
        .map_err(|e| CryptoError::Malformed(format!("密文信封无法序列化: {e}")))?;
    Ok(Sealed::Ciphertext {
        wire: wire.into_bytes(),
        kid: PRIMARY_KID,
    })
}

/// 下行后开封。返回明文信封字节。
///
/// 墓碑与明文形态**原样透传** —— 它们没有可解的东西，硬解只会造出假失败。
pub fn open_incoming(wire: &[u8], vault: Option<&KeyVault>) -> Result<Vec<u8>, CryptoError> {
    // **先看有没有加密，再谈校验**。这个顺序是有代价的：不开加密时本函数必须是
    // **完全透明的旁路** —— 存量代码（含引擎那批毫秒级单测用的极简 wire）原来
    // 走的是 `WireMeta::parse_outgoing` 那条校验链，我若在这里先跑一遍
    // `Envelope::from_wire`，那些形状就会被提前判成"信封结构不合法"，
    // 于是"没启用加密"也换了行为 —— 那是回归，不是收紧。
    if !looks_encrypted(wire) {
        return Ok(wire.to_vec());
    }
    let Some(vault) = vault else {
        return Err(CryptoError::Malformed(
            "远端是密文但本机没有启用加密（或尚未解锁）—— 这一条拒收".into(),
        ));
    };
    let text = std::str::from_utf8(wire)
        .map_err(|e| CryptoError::Malformed(format!("下行信封不是合法 UTF-8: {e}")))?;
    let env = Envelope::from_wire(text)?;
    if matches!(env.enc.alg, EncAlg::None) {
        return Ok(wire.to_vec());
    }
    // 走到这里说明远端确实是密文。墓碑不参与密封（见 seal_outgoing），
    // 所以"密文 + 无 payload"只能是畸形，交给 open_with_key 去撞 AEAD 标签或摘要校验。
    let plain = env.open_with_key(vault.master())?;
    let mut back = env.clone();
    back.payload = Some(plain);
    back.ct = None;
    back.enc.alg = EncAlg::None;
    back.enc.kid = None;
    back.enc.nonce = None;
    // hash 与 hash_alg 必须一起回到明文形态：`hmac-sha256:` 前缀留着会让
    // 下一次 `check_matches` 与 manifest 的校验按 keyed 摘要去比明文内容，必然不符。
    back.enc.hash_alg = notera_crypto::HashAlg::Sha256;
    // hash 必须用**与 store 落库时同一套**算法重算：`hash_json(doc)` = `sha256(canonical_json)`
    // （notera_core 的 `ContentHash` 是权威）。用别的算法算出来的摘要，
    // 下一次 `check_matches` / manifest 校验就会把正确的记录判成"被改过"。
    let plain_doc = back.payload_doc().expect("刚放回 payload");
    back.hash = notera_core::hash_json(plain_doc).as_str().to_string();
    back.validate()?;
    let wire = back
        .to_wire()
        .map_err(|e| CryptoError::Malformed(format!("解密后的信封无法序列化: {e}")))?;
    Ok(wire.into_bytes())
}

/// 这条下行记录是不是一条"本机打不开"的密文。
///
/// 界面用它把失败说清楚：**不是"服务器坏了"，而是"这台设备还没有那把钥匙"**。
pub fn is_undecryptable(wire: &[u8], vault: Option<&KeyVault>) -> bool {
    vault.is_none() && looks_encrypted(wire)
}

/// 只看`enc.alg` 这一格，**不做全信封校验**。
///
/// 为什么单独一个函数：`open_incoming` 要在"未启用加密"时当完全透明的旁路，
/// 而判定"这是不是密文"只需要读一个字段。若在这里就 `Envelope::from_wire`，
/// 未启用加密的存量路径也会被新校验拦下（实测会打挂引擎那 4 条单测）。
/// 读不出来（不是 JSON / 没有 enc / alg 是 none）一律当"不是密文"。
fn looks_encrypted(wire: &[u8]) -> bool {
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(wire) else {
        return false;
    };
    v.get("enc")
        .and_then(|e| e.get("alg"))
        .and_then(|a| a.as_str())
        .map(|a| a != "none")
        .unwrap_or(false)
}

/// 摘要前缀是否已经是 keyed 形态。历史迁移用它判断"这条要不要重算 hash"。
pub fn is_keyed_hash(hash: &str) -> bool {
    hash.starts_with(HMAC_SHA256_PREFIX)
}
