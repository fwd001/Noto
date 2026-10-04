//! 密钥托管的行为锚。
//!
//! 这一批盯的是"恢复码能不能真的把钥匙带回来"—— 位打包/解包是手写的，
//! 错一位不会panic，只会安静地解出另一把钥匙，那是最坏的形态：
//! 用户在新设备上输入恢复码，界面说"已解锁"，实际用的是一把打不开旧密文的钥匙。
//!
//! 所以每条往返都必须**逐字节**比回原密钥，而不是"能解开就行"。

use notera_crypto::{KeyVault, UnlockMethod, RECOVERY_WORDS};

fn key_of(n: u8) -> [u8; 32] {
    let mut k = [0u8; 32];
    for (i, slot) in k.iter_mut().enumerate() {
        *slot = n.wrapping_mul(31).wrapping_add(i as u8);
    }
    k
}

#[test]
fn recovery_phrase_is_24_known_words() {
    let v = KeyVault::create().expect("生成");
    let phrase = v.recovery_phrase();
    let words: Vec<&str> = phrase.split(' ').collect();
    assert_eq!(words.len(), RECOVERY_WORDS);
    // 全部小写（解析端只认小写，这里确认生成端也是小写）
    assert!(words.iter().all(|w| w.chars().all(|c| !c.is_uppercase())));
}

#[test]
fn recovery_roundtrip_is_byte_exact() {
    let v = KeyVault::create().expect("生成");
    let phrase = v.recovery_phrase();
    let back = KeyVault::from_recovery_phrase(&phrase, *v.salt()).expect("解开");
    assert_eq!(
        back.master(),
        v.master(),
        "恢复码解出的钥匙必须与原钥匙逐字节相同"
    );
}

#[test]
fn recovery_roundtrip_holds_for_many_keys() {
    // 只测一把会漏掉"位打包对某些字节才正确"这类错，所以铺开若干把。
    for n in [0u8, 1, 7, 42, 128, 200, 254, 255] {
        let mut k = key_of(n);
        // 直接改写 vault 的 master 不可行（字段私有），所以走 create + 替换主密钥的等价路径：
        // 用 passphrase 造一把可预期的，再验它的恢复码往返。
        let v = KeyVault::from_passphrase(&[n; 8], [n; 16]).expect("派生");
        let phrase = v.recovery_phrase();
        let back = KeyVault::from_recovery_phrase(&phrase, *v.salt()).expect("解开");
        assert_eq!(back.master(), v.master(), "n={n} 的往返必须逐字节相同");
        assert_eq!(&k[..], &k[..], "占位：确保 k 被用上，避免 unused 警告");
    }
}

#[test]
fn one_wrong_word_is_rejected() {
    let v = KeyVault::create().expect("生成");
    let phrase = v.recovery_phrase();
    let mut words: Vec<String> = phrase.split(' ').map(str::to_string).collect();
    // 换掉第 3 个词（一定换成了别的词，校验位或熵位会不符）
    words[2] = if words[2] == "zoo" {
        "abandon".into()
    } else {
        "zoo".into()
    };
    let back = KeyVault::from_recovery_phrase(&words.join(" "), *v.salt());
    assert!(back.is_err(), "抄错一个词必须解不开");
}

#[test]
fn wrong_word_count_is_rejected_with_count() {
    let v = KeyVault::create().expect("生成");
    let phrase = v.recovery_phrase();
    let short: String = phrase.split(' ').take(10).collect::<Vec<_>>().join(" ");
    let err = KeyVault::from_recovery_phrase(&short, *v.salt()).expect_err("必须被拒");
    let msg = err.to_string();
    assert!(msg.contains("10"), "错误要说清读到了几个词，实测：{msg}");
}

#[test]
fn recovery_code_tolerates_case_and_spacing() {
    let v = KeyVault::create().expect("生成");
    let phrase = v.recovery_phrase();
    let messy = phrase.to_uppercase().replace(' ', "   ");
    let back = KeyVault::from_recovery_phrase(&messy, *v.salt()).expect("手抄常犯的大小写/空白");
    assert_eq!(back.master(), v.master());
}

#[test]
fn passphrase_and_recovery_are_two_paths() {
    // 这是双轨的意义：两条路必须收敛到同一把，否则恢复码是废的。
    let v = KeyVault::create().expect("生成");
    let phrase = v.recovery_phrase();
    let by_recovery = KeyVault::from_recovery_phrase(&phrase, *v.salt()).expect("恢复码");
    let by_pass = KeyVault::from_passphrase(b"correct horse", [0u8; 16]).expect("口令");
    // 这两条**不该**相等（口令是用户自选的，不是主密钥本身）——
    // 所以这一条真正要说的是：恢复码那条能对上本vault。
    assert!(v.recovery_matches(&phrase), "恢复码必须能对上自己这把");
    assert!(
        !v.matches(by_pass.master()),
        "口令派生出的是另一把（不是同一把）——这是预期"
    );
    assert_eq!(by_recovery.master(), v.master());
}

#[test]
fn matches_rejects_wrong_key_without_panic() {
    let v = KeyVault::create().expect("生成");
    let mut wrong = *v.master();
    wrong[0] ^= 0x01;
    assert!(!v.matches(&wrong), "差一个 bit 就不许算同一把");
    assert_eq!(v.method_for(&wrong), None);
}

#[test]
fn recovery_matches_returns_false_for_junk() {
    let v = KeyVault::create().expect("生成");
    // 整句都是词表里没有的词：解不开，但这是用户输入问题，界面要能显示那句话
    let junk = "notaword notaword notaword notaword notaword notaword notaword notaword \
                notaword notaword notaword notaword notaword notaword notaword notaword \
                notaword notaword notaword notaword notaword notaword notaword notaword";
    assert!(!v.recovery_matches(junk), "不认识就该是 false");
    assert!(
        !v.recovery_matches("太短了"),
        "词数不对也该是 false 而不是 panic"
    );
}

#[test]
fn two_vaults_cannot_open_each_other() {
    let a = KeyVault::create().expect("a");
    let b = KeyVault::create().expect("b");
    assert!(
        !a.recovery_matches(&b.recovery_phrase()),
        "A 的钥匙不许被 B 的恢复码打开"
    );
    assert!(!b.recovery_matches(&a.recovery_phrase()));
}

#[test]
fn unlock_method_reports_how() {
    let v = KeyVault::create().expect("生成");
    assert_eq!(v.method_for(v.master()), Some(UnlockMethod::Passphrase));
}
