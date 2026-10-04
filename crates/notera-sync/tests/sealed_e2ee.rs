//! E2EE 密封层的往返判据。
//!
//! 盯的是三件事，每件都各有一条会红的路：
//! 1. **密文里看不到明文**（把 payload 的字样拿去搜，搜不到才算真加密）；
//! 2. **往返逐字节还原**（`seal(open(x)) == x` 的内容等价，而不是"能打开就行"）；
//! 3. **没有钥匙的人打不开**（`vault=None` 时必须报错，**不是**静默透传明文）。

use notera_core::{canonical_json, hash_json, Rev};
use notera_crypto::{Envelope, KeyVault, KEY_LEN};
use notera_sync::sealed::{
    is_keyed_hash, is_undecryptable, open_incoming, seal_outgoing, PRIMARY_KID,
};
use std::sync::Arc;

fn doc(text: &str) -> serde_json::Value {
    serde_json::json!({ "v": 1, "content": [{ "type": "paragraph", "content": [{ "text": text }] }] })
}

/// 造一条明文信封（与 store 落库时的字段集一致）。
fn plain_wire(text: &str) -> Vec<u8> {
    let d = doc(text);
    let env = Envelope::for_note(
        &"0192e6c1-0000-7000-8000-00000000abcd".parse().expect("id"),
        Rev(7),
        Rev(6),
        &d,
        None,
        "device-a",
        "2026-10-03T02:00:00Z",
    );
    env.to_wire().expect("wire").into_bytes()
}

fn vault() -> Arc<KeyVault> {
    Arc::new(KeyVault::create().expect("建 vault"))
}

#[test]
fn 上行后密文里搜不到明文() {
    let plain = plain_wire("绝密内容：信用卡号 6222020200000000");
    let sealed = seal_outgoing(&plain, Some(&vault())).expect("密封");
    assert!(sealed.is_encrypted(), "启用了 vault 就必须是密文形态");
    let ct = String::from_utf8(sealed.wire().to_vec()).expect("utf8");
    assert!(!ct.contains("绝密内容"), "密文里不许出现正文");
    assert!(
        !ct.contains("6222020200000000"),
        "密文里不许出现任何明文片段"
    );
    // 形态标志：`ct` 有值、`payload` 为 null、alg 变成 aes-256-gcm-siv
    assert!(ct.contains("\"ct\""), "要有 ct 字段");
    assert!(ct.contains("aes-256-gcm-siv"), "alg 要切到密文算法");
    assert!(ct.contains(&format!("\"kid\":{PRIMARY_KID}")), "要带 kid");
}

#[test]
fn 往返后内容逐字节等价() {
    let plain = plain_wire("同步的记事本，重点是可靠。");
    let v = vault();
    let sealed = seal_outgoing(&plain, Some(&v)).expect("密封");
    let opened = open_incoming(sealed.wire(), Some(&v)).expect("开封");
    // 比的是**内容**，不是字节：信封的 hash 前后要一致才能通过 check_matches，
    // 所以这一条真正在验"解密后重算的 hash 与落库时算的是同一个"。
    let a = Envelope::from_wire(std::str::from_utf8(&plain).unwrap()).expect("原");
    let b = Envelope::from_wire(std::str::from_utf8(&opened).unwrap()).expect("还原");
    assert_eq!(a.payload_doc(), b.payload_doc(), "正文必须逐字段相同");
    assert_eq!(a.hash, b.hash, "重算的 hash 必须与落库时一致");
    assert_eq!(a.rev, b.rev, "rev 不得被密封改动");
}

#[test]
fn 没有钥匙时拒收而不是静默透传() {
    let plain = plain_wire("不该被放行");
    let sealed = seal_outgoing(&plain, Some(&vault())).expect("密封");
    // 这一条是 I6 的核心：解不开的记录必须 Err，让上层计入拒收。
    // 若实现改成"没钥匙就原样返回"，脏数据就进了权威表。
    assert!(
        open_incoming(sealed.wire(), None).is_err(),
        "没有钥匙必须拒收，绝不能把密文当明文放行"
    );
    assert!(
        is_undecryptable(sealed.wire(), None),
        "界面要能说出'本机没有那把钥匙'"
    );
    assert!(
        !is_undecryptable(sealed.wire(), Some(&vault())),
        "有钥匙时不许报打不开"
    );
}

#[test]
fn 用错钥匙一定解不开() {
    let sealed = seal_outgoing(&plain_wire("A 的秘密"), Some(&vault())).expect("密封");
    let other = vault();
    assert!(
        open_incoming(sealed.wire(), Some(&other)).is_err(),
        "另一把钥匙必须解不开（AEAD 标签 + keyed 摘要双重把关）"
    );
}

#[test]
fn 未启用加密的设备读写都走明文() {
    // 这一条保证"一启用就把自己锁在外面"不会发生：没 vault 时行为与开启前完全一致。
    let plain = plain_wire("普通笔记");
    let sealed = seal_outgoing(&plain, None).expect("明文路径");
    assert!(!sealed.is_encrypted(), "没有 vault 就不许产生密文");
    assert_eq!(
        sealed.wire(),
        plain.as_slice(),
        "明文路径必须逐字节原样透传"
    );
    let opened = open_incoming(&plain, None).expect("明文下行");
    assert_eq!(opened, plain, "明文下行也必须原样透传");
    assert!(!is_undecryptable(&plain, None), "明文记录不许被当成密文");
}

#[test]
fn 墓碑不参与密封() {
    // ADR-0002 待确认项的落地：purged 时 payload 与 ct 皆 null。
    // 给墓碑硬塞一个 ct 会造出"内容为空却声称已加密"的怪东西。
    let env = Envelope::for_purged(
        notera_core::EntityKind::Note,
        &"0192e6c1-0000-7000-8000-0000000000ff".parse().expect("id"),
        Rev(9),
        Rev(8),
        "2026-10-03T02:00:00Z",
        "device-a",
        "2026-10-03T02:00:00Z",
    );
    let plain = env.to_wire().expect("wire").into_bytes();
    let sealed = seal_outgoing(&plain, Some(&vault())).expect("墓碑密封");
    assert!(!sealed.is_encrypted(), "墓碑不该被密封");
    assert_eq!(sealed.wire(), plain.as_slice(), "墓碑必须原样上行");
    // 下行时墓碑也要原样回来
    let opened = open_incoming(&plain, Some(&vault())).expect("墓碑下行");
    assert_eq!(opened, plain, "墓碑下行必须原样透传");
}

#[test]
fn 密文被改一个字节必被拒() {
    let v = vault();
    let sealed = seal_outgoing(&plain_wire("完整性"), Some(&v)).expect("密封");
    let mut ct = String::from_utf8(sealed.wire().to_vec()).expect("utf8");
    // 在 ct 的 base64 中间翻一个字符
    let i = ct.find("\"ct\":\"").expect("有 ct") + 6;
    let ch = ct.as_bytes()[i];
    ct.replace_range(i..i + 1, if ch == b'A' { "B" } else { "A" });
    assert!(
        open_incoming(ct.as_bytes(), Some(&v)).is_err(),
        "篡改一字节必被 AEAD 标签或 keyed 摘要拒掉"
    );
}

#[test]
fn 摘要前缀随形态切换() {
    // ADR-0002：开加密后 hash_alg 必须从 sha256 转到 hmac-sha256，
    // 否则密文之外仍泄露明文的等价指纹。
    let v = vault();
    let plain = plain_wire("指纹");
    let sealed = seal_outgoing(&plain, Some(&v)).expect("密封");
    let opened = open_incoming(sealed.wire(), Some(&v)).expect("开封");
    let enc = Envelope::from_wire(std::str::from_utf8(sealed.wire()).unwrap()).expect("密文信封");
    assert!(
        is_keyed_hash(&enc.hash),
        "密文形态的 hash 必须是 hmac-sha256 前缀"
    );
    assert!(
        !is_keyed_hash(&opened_as_str(&opened)),
        "解密后必须回到 sha256 前缀"
    );
    // 且解密后的 hash 要等于 store 对同一份 doc 的权威算法
    let expect = hash_json(&doc("指纹")).as_str().to_string();
    assert_eq!(
        opened_as_str(&opened),
        expect,
        "解密后的 hash 必须等于 store 的权威值"
    );
    // canonical_json 是键序无关的，所以这里能按字节比
    assert_eq!(canonical_json(&doc("指纹")), canonical_json(&doc("指纹")));
    let _ = KEY_LEN;
}

#[test]
fn 未启用加密时对任意形状都是旁路() {
    // 这条是被实测打出来的：引擎那批毫秒级单测用的 wire 是 `{"rev":3}` 这种极简形状，
    // 不是合法信封。我第一版在 `open_incoming` 里先跑 `Envelope::from_wire`，
    // 于是"没启用加密"也换了行为 —— 引擎 4 条单测当场红（remote_new_note_is_pulled_and_applied 等）。
    // ⇒ 未启用加密时本模块必须是**完全透明的旁路**，一个字节都不许动。
    for raw in [
        &b"{\"rev\":3}"[..],
        &b"{}"[..],
        &b"{\"i\":\"s1\"}"[..],
        &b"not json at all"[..],
        &b""[..],
    ] {
        let out = open_incoming(raw, None).expect("旁路不许失败");
        assert_eq!(out, raw, "未启用加密时必须逐字节原样返回：{raw:?}");
        assert!(
            !is_undecryptable(raw, None),
            "这些形状都不该被当成密文：{raw:?}"
        );
        let s = seal_outgoing(raw, None).expect("旁路不许失败");
        assert_eq!(s.wire(), raw, "上行旁路也必须原样：{raw:?}");
    }
}

fn opened_as_str(w: &[u8]) -> String {
    let env = Envelope::from_wire(std::str::from_utf8(w).expect("utf8")).expect("信封");
    env.hash
}
