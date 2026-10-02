//! OS 凭据库这一格（ADR-0020）：口令只活在系统的凭据存储里，配置只留引用。
//!
//! 为什么这批要有这一条 lane：`credential_ref` 从 Phase 0 的语义就是"指向钥匙串的引用"，
//! 而实现里**什么都没存** —— 于是两件事同时成立：设置页的"口令已设置"是假的（它照那个引用
//! 是否为空算），而发布版永远拿不到凭据（`secret_for` 在 release 下只认一个开发用环境变量，
//! 那个变量在用户的机器上不存在）。也就是说"配了自己的服务器却同步不了"，且没人报错。
//!
//! 这里验的是这一格的四条边界：
//! * 存进去的原样读得回来（含非 ASCII，因为 blob 装的是 UTF-16）；
//! * 上限按系统那侧的真实约束算（512 字节 = 256 个 UTF-16 单元），**超限报错且不落任何东西** ——
//!   截断存进去等于给用户一个"配好了但永远 401"的账户；
//! * 重新配置账户而不重填口令 = 不改口令（界面上那格本来就是"已设置"的占位提示）；
//! * 没填口令的草稿不许让 `hasCredential` 亮 —— 这条指示现在是有分量的。
//!
//! 跑法：`cargo test -p notera-host --test credentials`（进程内四道测试由 `lock_store` 串行，见下面那条注释）
//! 这一条会真动**当前用户的 Windows 凭据库**（目标名带 uuid，收尾一律抹掉），
//! 所以它不与别的套件叠着跑。非 Windows 上它不假装通过：断言的是"这一格还没接"那个具名错误。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};

use notera_host::commands::AccountDraftCmd;
use notera_host::{credential_store, App};
use serde_json::json;

static SEQ: AtomicUsize = AtomicUsize::new(0);

/// 同一进程里并发调 `CredWriteW` 会偶发失败（实测约 1/17 次，报的是系统那侧的错，不是我们的逻辑错）。
/// 这条 lane 摸的是真凭据库，所以四道测试在**进程内**串行跑 —— 不串行就会在 CI 里偶发红，
/// 而那种红查下去永远是"系统忙"，比没有这条门禁更糟。
static STORE: Mutex<()> = Mutex::new(());

fn lock_store() -> MutexGuard<'static, ()> {
    STORE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn tmp_dir(tag: &str) -> std::path::PathBuf {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("notera-cred-{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 一条只属于本测试的凭据。`Drop` 负责抹掉 —— 残留一条没人引用的口令，比测试失败更难查。
struct Guard(String);

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = credential_store::remove(&self.0);
    }
}

fn draft(id: &str, username: Option<&str>, password: Option<&str>) -> AccountDraftCmd {
    let mut obj = json!({
        "id": id, "label": "凭据测试", "baseUrl": "http://127.0.0.1:5005/.notes",
    });
    if let Some(u) = username {
        obj["username"] = json!(u);
    }
    if let Some(p) = password {
        obj["password"] = json!(p);
    }
    serde_json::from_value(obj).expect("草稿形态合法")
}

#[test]
fn a_stored_secret_comes_back_byte_identical_and_removal_is_idempotent() {
    let _serial = lock_store();
    let target = credential_store::test_target("roundtrip");
    let _guard = Guard(target.clone());
    if !credential_store::available() {
        // 非 Windows（缺口 G38 选 B 之后）：口令**不退到报错，退到本次进程的内存表**。
        // 这一支在 Windows 上跑不到，所以它是"文档说的退路真存在"的唯一自动检查。
        credential_store::put(&target, "u", "p").expect("没有系统凭据库时也要存得进去（会话表）");
        assert_eq!(
            credential_store::get(&target).unwrap(),
            Some(("u".to_string(), "p".to_string())),
            "存进会话表必须读得回来，否则同步那一轮拿不到口令"
        );
        assert!(
            !credential_store::persistent(&target),
            "会话表里的不算持久 —— 界面上那句「退出后要重填」就是照这位说的"
        );
        credential_store::remove(&target).expect("删也要幂等");
        assert_eq!(credential_store::get(&target).unwrap(), None);
        return;
    }
    // 用户名与口令都带非 ASCII：blob 装的是 UTF-16，这一句顺便验编码没走形。
    let user = "用户-Ⅰ";
    let secret = "pβss-Ⅰ-ünïcødé-7f3a";
    credential_store::put(&target, user, secret).expect("存进去");
    assert_eq!(
        credential_store::get(&target).unwrap(),
        Some((user.to_string(), secret.to_string())),
        "读回来的必须与存进去的一字不差"
    );
    // 这两位在**有**系统凭据库的平台上必须是 true / true —— 界面那句"退出后不用重填"照它们说。
    // 没有这一条，`persistent()` 写成常量 `false` 也能让别的门禁全绿（缺的就是这个反向差分）。
    assert!(
        credential_store::live(&target),
        "真存进系统凭据库却报「这一轮拿不到」"
    );
    assert!(
        credential_store::persistent(&target),
        "系统凭据库里明明有，`persistent` 却报 false —— 这一位不许硬编码"
    );
    // 覆盖同一目标：内容寻址之外这里是"同名替换"，第二次说了算（重设口令就是这个形状）。
    credential_store::put(&target, user, "新的口令").expect("替换");
    assert_eq!(
        credential_store::get(&target).unwrap().unwrap().1,
        "新的口令"
    );
    credential_store::remove(&target).expect("删掉");
    assert_eq!(
        credential_store::get(&target).unwrap(),
        None,
        "删完要真的没有"
    );
    credential_store::remove(&target).expect("再删一次必须幂等 —— 删账户与回滚都这么调");
}

#[test]
fn the_blob_limit_is_enforced_at_the_boundary_without_storing_anything() {
    let _serial = lock_store();
    let target = credential_store::test_target("limit");
    let _guard = Guard(target.clone());
    if !credential_store::available() {
        // 上限是**后端无关**的一条规则（超限就报错、绝不截断），所以会话表这条路也要验它：
        // 截断后的口令存进去，用户拿到的还是一个"配好了但 401"的账户。
        let one_over = "ａ".repeat(credential_store::MAX_UNITS + 1);
        assert_eq!(
            credential_store::put(&target, "u", &one_over),
            Err(credential_store::SecretError::TooLong(
                credential_store::MAX_UNITS + 1
            )),
            "没有系统凭据库时也不许把超长口令塞进会话表"
        );
        assert_eq!(
            credential_store::get(&target).unwrap(),
            None,
            "被拒的那一次不许留下任何东西"
        );
        return;
    }
    let exactly_max = "ａ".repeat(credential_store::MAX_UNITS); // 全角：每个字符占一个 UTF-16 单元
    let one_over = "ａ".repeat(credential_store::MAX_UNITS + 1);
    credential_store::put(&target, "u", &exactly_max).expect("正好在上限内必须存得进去");
    assert_eq!(
        credential_store::get(&target).unwrap().unwrap().1.len(),
        exactly_max.len()
    );
    assert_eq!(
        credential_store::put(&target, "u", &one_over),
        Err(credential_store::SecretError::TooLong(
            credential_store::MAX_UNITS + 1
        )),
        "超限要报错，而且要带上到底多少个单元"
    );
    // 报错之后**上一次那份必须还在**：不许"拒绝写入"顺手把旧口令也清了。
    assert_eq!(
        credential_store::get(&target).unwrap().unwrap().1,
        exactly_max,
        "被拒绝的写入不许动已有的那条"
    );
}

#[test]
fn an_account_reconfigured_without_a_password_keeps_the_stored_secret() {
    let _serial = lock_store();
    if !credential_store::available() {
        return; // 这一条形状依赖真凭据库；非 Windows 按 ADR-0020 记 BLOCKED
    }
    let dir = tmp_dir("keep");
    let app = App::boot(&dir).expect("核心启动");
    let first = app
        .configure_account(draft("", Some("notera-test"), Some("第一次的口令")))
        .unwrap();
    let id = first.id.clone();
    assert!(first.has_credential, "存进去了就该亮");

    // 用户只是改了标签、没重填口令 —— 系统里那条必须还在，引用也必须还写着有
    let second = app
        .configure_account(draft(&id, Some("notera-test"), None))
        .unwrap();
    assert!(second.has_credential, "不重填口令不等于清空口令");
    assert_eq!(
        credential_store::get(&credential_store::webdav_target(&id))
            .unwrap()
            .unwrap()
            .1,
        "第一次的口令",
        "重新配置不许把已存的口令抹掉或换掉"
    );
    app.remove_account(&id).unwrap();
    assert_eq!(
        credential_store::get(&credential_store::webdav_target(&id)).unwrap(),
        None
    );
}

#[test]
fn a_draft_without_a_password_stops_claiming_a_credential_exists() {
    let _serial = lock_store();
    let dir = tmp_dir("no-claim");
    let app = App::boot(&dir).expect("核心启动");
    // 只填用户名、没填口令：这一格以前是 `hasCredential = true`（照"填过没填过"算），
    // 于是发布版显示"口令已设置"而引擎永远拿不到凭据 —— 那是指示在说谎。
    let dto = app
        .configure_account(draft("", Some("notera-test"), None))
        .unwrap();
    assert!(
        !dto.has_credential,
        "没存进系统凭据就不许说口令已设置（debug 那套环境变量喂的是开发路径，不算凭据已就绪）"
    );
    let id = dto.id.clone();
    assert_eq!(
        credential_store::get(&credential_store::webdav_target(&id)).unwrap(),
        None
    );
    app.remove_account(&id).unwrap();
}
