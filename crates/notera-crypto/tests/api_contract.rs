//! 公共 API 契约测试（跨 crate 冻结点，见 ARCHITECTURE-MAP §6）。
//!
//! richtext / store / sync / webdav 都按同一份签名并行开发。这里把契约钉成可机检的东西：
//! 函数指针类型标注钉签名，结构体字面量钉**公共字段集合**（少字段编译不过，**多加字段
//! 同样编译不过** —— 别人用字面量构造它），穷尽 match 钉枚举变体。

use notera_crypto::{
    derive_key, open, seal_plaintext, sha256_hex, verify_bytes, CryptoError, EncAlg, EncMeta,
    Envelope, HashAlg, PROTOCOL,
};
use notera_core::{EntityId, EntityKind, Rev};

#[test]
fn function_signatures_are_exactly_the_contract() {
    let _: u16 = PROTOCOL;
    let _: fn(&[u8]) -> String = sha256_hex;
    let _: fn(&[u8], &str) -> bool = verify_bytes;
    let _: fn(&[u8], &[u8; 16]) -> [u8; 32] = derive_key;
    let _: fn(&[u8], &[u8; 32]) -> Result<(Vec<u8>, [u8; 12]), CryptoError> = seal_plaintext;
    let _: fn(&[u8], &[u8; 12], &[u8; 32]) -> Result<Vec<u8>, CryptoError> = open;
}

#[test]
fn envelope_field_set_is_frozen() {
    let id = EntityId::parse("0192e6c1-0000-7000-8000-00000000abcd").unwrap();
    let payload = serde_json::json!({ "v": 1, "content": [] });
    let built = Envelope::for_note(
        &id,
        Rev(1),
        Rev(0),
        &payload,
        None,
        "0192e6c1-0000-7000-8000-00000000000d",
        "2026-09-25T09:12:03.441Z",
    );
    // 字面量写法（store/sync 也在用）必须能列出**全部**字段：少一个编译不过，多一个也编译不过。
    let mut e = Envelope {
        protocol: PROTOCOL,
        kind: EntityKind::Note,
        id: id.clone(),
        rev: Rev(1),
        sync_rev: Rev(0),
        hash: String::new(),
        updated_at: "2026-09-25T09:12:03.441Z".into(),
        device: "0192e6c1-0000-7000-8000-00000000000d".into(),
        deleted_at: None,
        purged: false,
        enc: EncMeta {
            alg: EncAlg::None,
            kid: None,
            nonce: None,
            hash_alg: HashAlg::Sha256,
        },
        payload: Some(payload.clone()),
        ct: None,
    };
    // hash 由构造函数算，字面量那份补上后两者必须是同一个东西。
    e.hash = built.hash.clone();
    assert_eq!(e, built);
    assert_eq!(
        built.hash,
        format!("sha256:{}", sha256_hex(br#"{"content":[],"v":1}"#)),
        "hash 必须是 sha256(canonical(payload))"
    );
    assert_eq!(e.protocol, PROTOCOL);
    assert_eq!(e.kind, EntityKind::Note);
    assert_eq!(e.rev, Rev(1));
    assert_eq!(e.sync_rev, Rev(0));
    let _: Option<u32> = e.enc.kid;
    let _: Option<String> = e.enc.nonce;
    let _: Option<&serde_json::Value> = e.payload_doc();
    let _: Result<String, CryptoError> = e.to_wire();
    let _: Result<Envelope, CryptoError> = Envelope::from_wire("{}");
    let _: Result<(), CryptoError> = e.check_matches(EntityKind::Note, &id);
}

#[test]
fn for_folder_mirrors_for_note_argument_order() {
    let id = EntityId::parse("0192e6c1-0000-7000-8000-00000000abcd").unwrap();
    let f = Envelope::for_folder(
        &id,
        Rev(3),
        Rev(2),
        &serde_json::json!({ "name": "夹" }),
        None,
        "0192e6c1-0000-7000-8000-00000000000d",
        "2026-09-25T09:12:03.441Z",
    );
    assert_eq!(f.kind, EntityKind::Folder);
    let wire = f.to_wire().unwrap();
    let v: serde_json::Value = serde_json::from_str(&wire).unwrap();
    assert_eq!(v["kind"], serde_json::json!("folder"), "wire 上 kind 用目录名");
    assert_eq!(v["rev"], serde_json::json!(3));
    assert_eq!(v["sync_rev"], serde_json::json!(2));
}

#[test]
fn crypto_error_variants_are_matchable_exhaustively() {
    let errs = [
        CryptoError::PayloadAndCtMutuallyExclusive,
        CryptoError::HashMismatch {
            expected: "a".into(),
            actual: "b".into(),
        },
        CryptoError::KindMismatch,
        CryptoError::IdMismatch,
        CryptoError::Malformed("x".into()),
        CryptoError::Auth("x".into()),
    ];
    assert_eq!(errs.len(), 6);
    for e in errs {
        let tag = match &e {
            CryptoError::PayloadAndCtMutuallyExclusive => "互斥",
            CryptoError::HashMismatch { expected, actual } => {
                assert!(!expected.is_empty() && !actual.is_empty(), "两个哈希都要带上");
                "哈希不符"
            }
            CryptoError::KindMismatch => "kind",
            CryptoError::IdMismatch => "id",
            CryptoError::Malformed(_) => "畸形",
            CryptoError::Auth(_) => "认证",
        };
        assert!(!e.to_string().is_empty(), "{tag} 必须有用户可读文案");
        assert_eq!(e.clone(), e);
    }
}

#[test]
fn enc_and_hash_alg_wire_spelling_is_frozen() {
    // 这两个字符串是 wire 契约的一部分：写错就等于换了协议（SYNC-PROTOCOL §3）。
    let j = |m: &EncMeta| serde_json::to_value(m).unwrap();
    assert_eq!(
        j(&EncMeta { alg: EncAlg::None, kid: None, nonce: None, hash_alg: HashAlg::Sha256 }),
        serde_json::json!({ "alg": "none", "hash_alg": "sha256" })
    );
    assert_eq!(
        j(&EncMeta {
            alg: EncAlg::Aes256GcmSiv,
            kid: Some(3),
            nonce: Some("AAAAAAAAAAAAAAAA".into()),
            hash_alg: HashAlg::HmacSha256,
        }),
        serde_json::json!({
            "alg": "aes-256-gcm-siv", "kid": 3, "nonce": "AAAAAAAAAAAAAAAA",
            "hash_alg": "hmac-sha256"
        })
    );
    // 反向也要吃进同一套拼写。
    let back: EncMeta = serde_json::from_value(serde_json::json!({
        "alg": "aes-256-gcm-siv", "hash_alg": "hmac-sha256", "kid": 1, "nonce": "AA=="
    }))
    .unwrap();
    assert!(matches!(back.alg, EncAlg::Aes256GcmSiv));
    assert_eq!(back.hash_alg, HashAlg::HmacSha256);
}

#[test]
fn kdf_parameters_are_the_measured_ones() {
    // "19 MiB / t=2 / p=1" 是实测出来的（≈400 ms），悄悄调弱 = 削弱口令抗暴力破解能力。
    use notera_crypto::{ARGON2_M_KIB, ARGON2_P, ARGON2_T, KEY_LEN, SALT_LEN};
    assert_eq!(ARGON2_M_KIB, 19 * 1024);
    assert_eq!(ARGON2_T, 2);
    assert_eq!(ARGON2_P, 1);
    assert_eq!(KEY_LEN, 32);
    assert_eq!(SALT_LEN, 16);
}
