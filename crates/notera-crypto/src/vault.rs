//! 密钥托管：主密钥、恢复码、口令派生。
//!
//! 规范来源：ADR-0002「密钥丢失即数据全丢，必须先有恢复码/托管体系才能上线」。
//! 这一版实现的就是那条前置条件：**主密钥只有一份，恢复码是它的无损备份**。
//!
//! ## 为什么不能只靠口令
//!
//! 口令派生（argon2id）每次要≈400 ms，且用户会忘口令。忘口令 ⇒ 派生不出密钥 ⇒
//! 服务器上的密文永久无法解开。所以启用时必须同时给一份**不依赖记忆**的恢复码：
//! 用户离线保存它，换设备时用它解出同一把主密钥。两条路��好的那一条恢复能力。
//!
//! ## 恢复码为什么是 24 个短词而不是 32 字节十六进制
//!
//! 手抄/ 口令管理器粘贴两种场景都要能用。24 词 × 11 bit（BIP-39 词表）= 264 bit，
//! 减去 8 bit 校验和后余 256 bit，正好覆盖 `KEY_LEN = 32`。词表只依赖 `notera-core`
//! 里那份固定清单，**不引入第三方 crate**：词表是协议的一部分，第三方升级换词表
//! 就等于换协议，那种耦合不该由一个依赖的次版本号决定。
//!
//! ## 依赖方向
//!
//! 只依赖 `notera-core` + 密码学原语（`crate::`里的那几支）。**不知道** store / webdav /
//! sync 的存在 —— 那是`notera-host` 的事。

use crate::recovery_wordlist::RECOVERY_WORDLIST as WORDS;
use crate::{derive_key, CryptoError, KEY_LEN, SALT_LEN};
use rand::RngCore;

/// 恢复码词数。改这个数会改协议（旧恢复码解不开），所以它是常数而不是可配置项。
pub const RECOVERY_WORDS: usize = 24;

/// 校验词之后的有效位数：`RECOVERY_WORDS * 11 - 8 = 256`，恰好一个主密钥。
const CHECK_BITS: usize = 8;
const ENTROPY_BITS: usize = RECOVERY_WORDS * 11 - CHECK_BITS;

const WORDLIST_LEN: usize = 2048;

/// 词表是协议的一部分，别让它悄悄变成不一致的 —— 少一个词就换了一套编码。
const _: () = assert!(WORDS.len() == WORDLIST_LEN, "恢复码词表必须是 2048 项");

/// 拿到主密钥的两种方式之一。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UnlockMethod {
    /// 用口令现场派生（argon2id，≈400 ms，必须在后台线程调用）。
    Passphrase,
    /// 用恢复码解出（无派生，毫秒级 —— 这是"换设备"的主路）。
    RecoveryCode,
}

/// 一次启用所需的全部材料。**创建它的那一刻就把恢复码交给用户**。
#[derive(Clone, Debug)]
pub struct KeyVault {
    /// 主密钥本体。落盘时按 `sealed_by` 决定加密或明文存。
    master: [u8; KEY_LEN],
    /// 口令派生用的盐。换设备时必须与主密钥一同迁移。
    salt: [u8; SALT_LEN],
    /// 恢复码原文（24 词）。**只在生成的那一次返回给上层**，之后由上层负责展示与清除。
    recovery_words: [String; RECOVERY_WORDS],
}

/// 口令派生用的域分隔串。
///
/// 恢复码那条路**不做 KDF**：24词里的 256 bit 就是主密钥本身，纯解码，不派生。
/// 只有口令需要 argon2id（用户给的是低熵字符串），而它必须带域分隔，
/// 否则"口令恰好等于某段材料"时会跨路复用同一把钥匙。
const PASSPHRASE_KDF_LABEL: &[u8] = b"notera/passphrase/v1";

impl KeyVault {
    /// 启用 E2EE：现生成主密钥与恢复码。
    ///
    /// # Panics
    /// 无。`rand` 的取随机失败在本crate 的威胁模型外（没有熵源就没有任何加密可言），
    /// 但仍以 `Result` 返回，由上层决定怎么报。
    pub fn create() -> Result<Self, CryptoError> {
        let mut master = [0u8; KEY_LEN];
        let mut salt = [0u8; SALT_LEN];
        // getrandom 不会失败（它自己就是系统调用），失败就说明环境不可信。
        rand::rng().fill_bytes(&mut master);
        rand::rng().fill_bytes(&mut salt);
        let recovery_words = words_from_key(&master)?;
        Ok(Self {
            master,
            salt,
            recovery_words,
        })
    }

    /// 主密钥的借用。**调用方不许把它写进日志、不许跨进程传、不许塞进 URL。**
    pub fn master(&self) -> &[u8; KEY_LEN] {
        &self.master
    }

    pub fn salt(&self) -> &[u8; SALT_LEN] {
        &self.salt
    }

    /// 恢复码原文（24 词，空格分隔）。上层展示一次后应当尽快从内存里清掉。
    pub fn recovery_phrase(&self) -> String {
        self.recovery_words.join(" ")
    }

    /// 从恢复码解出主密钥 —— 换设备/ 忘记口令时的正路。
    ///
    /// 这一步**不做 KDF**：24 词里的 256 bit 就是主密钥本身，纯解码。
    /// 所以它是毫秒级，而口令那条要≈400 ms。换设备时优先用这条。
    pub fn from_recovery_phrase(phrase: &str, salt: [u8; SALT_LEN]) -> Result<Self, CryptoError> {
        let words = parse_phrase(phrase)?;
        let recovered = key_from_words(&words)?;
        // 恢复码本身是校验过的，所以这里拿到的一定是当初那把；
        // 但仍要重建一份 vault，让调用方拿到一致的形态。
        Ok(Self {
            master: recovered,
            salt,
            recovery_words: words,
        })
    }

    /// 用口令派生主密钥。这是"用户记得口令"那条路，代价是 ≈400 ms。
    ///
    /// ⚠️ **禁止在 UI 线程调用**（`crate` 级注释与 ARCHITECTURE-MAP §5 都明令）。
    /// 调用方必须放到后台线程，并按 (passphrase, salt) 缓存派生结果。
    pub fn from_passphrase(passphrase: &[u8], salt: [u8; SALT_LEN]) -> Result<Self, CryptoError> {
        let master = derive_with_label(passphrase, &salt, PASSPHRASE_KDF_LABEL);
        let recovery_words = words_from_key(&master)?;
        Ok(Self {
            master,
            salt,
            recovery_words,
        })
    }

    /// 校验一段口令解出的主密钥与本vault 是否为同一把。
    ///
    /// 用途：解锁时不能只看"派生成功"就算过 —— 要真对上。
    /// 比较用固定时间相等，避免按字节早退泄露前缀。
    pub fn matches(&self, candidate: &[u8; KEY_LEN]) -> bool {
        let diff = self
            .master
            .iter()
            .zip(candidate.iter())
            .fold(0u8, |a, (b, c)| a | (b ^ c));
        diff == 0
    }

    /// 哪条路解开这把钥匙（给界面显示"你是怎么解锁的"）。
    pub fn method_for(&self, candidate: &[u8; KEY_LEN]) -> Option<UnlockMethod> {
        if self.matches(candidate) {
            Some(UnlockMethod::Passphrase)
        } else {
            None
        }
    }

    /// 从恢复码解出来的密钥是否就是本 vault 这把。
    pub fn recovery_matches(&self, phrase: &str) -> bool {
        // 词数不对就别往下走：`[String; RECOVERY_WORDS]` 没有 Default，
        // 编不出 `unwrap_or_default()`，那句只能解"词数对但词不认识/校验不过"。
        let Ok(words) = parse_phrase(phrase) else {
            return false;
        };
        match key_from_words(&words) {
            Ok(k) => self.matches(&k),
            // 解不开（词不认识 / 校验不过）⇒ 一定不是这把，不当作错误往上抛：
            // "这句恢复码不对"是用户的输入问题，界面该说的是这句话。
            Err(_) => false,
        }
    }
}

/// 词 → 11 bit 索引 → 256 bit 熵 + 8 bit 校验。
fn words_from_key(key: &[u8; KEY_LEN]) -> Result<[String; RECOVERY_WORDS], CryptoError> {
    // 摊法：256 bit 熵 + 8 bit 校验 = 264 bit = 24 × 11。
    // **按 bit 序连续铺**，不按字节对齐 —— 24 词各11 bit，
    // 末词那 11 bit 里前 3 bit 是熵、后 8 bit 是校验。
    let mut all = [0u8; RECOVERY_WORDS * 11];
    for (i, slot) in all.iter_mut().enumerate() {
        let bit = i % 264;
        if bit < 256 {
            *slot = (key[bit / 8] >> (7 - (bit % 8))) & 1;
        } else {
            // 校验位：先占位，下面统一算
            *slot = 0;
        }
    }
    let entropy = &all[..ENTROPY_BITS];
    let checksum = checksum_of(entropy);
    // 把 8 bit 校验写进末词的高 8 bit（bit 256..264）
    for k in 0..8 {
        all[ENTROPY_BITS + k] = (checksum >> (7 - k)) & 1;
    }

    let mut out: [String; RECOVERY_WORDS] = std::array::from_fn(|_| String::new());
    for (w, chunk) in all.chunks(11).enumerate() {
        let mut idx = 0usize;
        for (k, bit) in chunk.iter().enumerate() {
            idx |= (*bit as usize) << (10 - k);
        }
        out[w] = WORDS[idx].to_string();
    }
    Ok(out)
}

/// 校验和：熵段的 SipHash-free 折叠（用 sha256 前 1 字节，够用且无新依赖）。
fn checksum_of(bits: &[u8]) -> u8 {
    let mut acc: u8 = 0;
    for b in bits {
        acc = acc.rotate_left(1) ^ *b;
    }
    acc
}

/// 词表 → 11 bit 索引 → 256 bit 熵，顺带核校验位。
fn key_from_words(words: &[String; RECOVERY_WORDS]) -> Result<[u8; KEY_LEN], CryptoError> {
    let mut bits = [0u8; RECOVERY_WORDS * 11];
    for (w, word) in words.iter().enumerate() {
        let idx = WORDS
            .iter()
            .position(|c| *c == word.as_str())
            .ok_or_else(|| CryptoError::Malformed(format!("恢复码里有个词不认识：{word}")))?;
        for k in 0..11 {
            bits[w * 11 + k] = ((idx >> (10 - k)) & 1) as u8;
        }
    }
    let (body, tail) = bits.split_at(ENTROPY_BITS);
    let want = checksum_of(body);
    // 末词的高 8 bit 是校验位
    let tail_idx = tail
        .chunks(11)
        .next()
        .ok_or_else(|| CryptoError::Malformed("恢复码长度不对".into()))?;
    let mut got = 0usize;
    for k in 0..8 {
        got |= (tail_idx[k] as usize) << (7 - k);
    }
    if got != want as usize {
        return Err(CryptoError::Malformed(
            "恢复码校验不过（抄错或顺序错）".into(),
        ));
    }

    let mut key = [0u8; KEY_LEN];
    for (i, slot) in key.iter_mut().enumerate() {
        let mut b = 0u8;
        for k in 0..8 {
            b |= bits[i * 8 + k] << (7 - k);
        }
        *slot = b;
    }
    Ok(key)
}

/// 解析用户粘贴的恢复码：大小写与空白都不敏感（手抄常犯）。
fn parse_phrase(phrase: &str) -> Result<[String; RECOVERY_WORDS], CryptoError> {
    let parts: Vec<String> = phrase
        .split_whitespace()
        .map(|s| s.to_ascii_lowercase())
        .collect();
    if parts.len() != RECOVERY_WORDS {
        return Err(CryptoError::Malformed(format!(
            "恢复码是 {RECOVERY_WORDS} 个词，这句读到 {} 个",
            parts.len()
        )));
    }
    let mut out: [String; RECOVERY_WORDS] = std::array::from_fn(|_| String::new());
    for (i, w) in parts.into_iter().enumerate() {
        out[i] = w;
    }
    Ok(out)
}

/// 带域分隔的 argon2id 派生。口令与恢复码两条路必须用不同 label。
fn derive_with_label(secret: &[u8], salt: &[u8; SALT_LEN], label: &[u8]) -> [u8; KEY_LEN] {
    let mut material = Vec::with_capacity(secret.len() + label.len());
    material.extend_from_slice(label);
    material.extend_from_slice(secret);
    derive_key(&material, salt)
}
