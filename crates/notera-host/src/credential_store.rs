//! OS 凭据库：口令只存在系统的凭据存储里，配置只留一个引用。
//!
//! 为什么这一格必须由系统接管：`credential_ref` 从 Phase 0 就是"指向钥匙串的引用"这个语义，
//! 而引用一旦被写成 `keychain:{id}` 而背后**什么都没存**，界面上就会出现"口令已设置"这种
//! 说了假话的指示（`has_credential` 就是照它算的），更糟的是发布版因此根本拿不到凭据 ——
//! `secret_for` 在 release 下只认一个开发用环境变量，于是"配了服务器却永远同步不了"。
//!
//! Windows 侧用的是凭据管理器（Credential Manager）的 generic credential：
//! * 不需要任何 UI 授权、按用户隔离、进程退出后仍在（`CRED_PERSIST_LOCAL_MACHINE` 这个名字
//!   指的是"跟着这个用户登录态持久"，不是全机器可见）；
//! * blob 上限是 **512 字节**，而 blob 装的是 UTF-16 ⇒ 最多 256 个 UTF-16 单元。
//!   超限**报错**，绝不截断 —— 截断后的口令存进去，用户会拿到一个"配好了但 401"的账户。
//!
//! 非 Windows 平台还没接系统凭据库。**2026-10-02 用户拍板走"口令只留本次会话内存"（缺口 G38 选 B）**，
//! 所以这一格现在不再是"保存被拒"，而是**退到进程内的会话表**：
//! * 存：OS 后端不可用时写进 `SESSION`，`put` 返回 `Ok` ⇒ 保存不再被顶回来；
//! * 取：`get` 先看 OS，再看到会话表 ⇒ 这一轮同步真拿得到口令；
//! * **绝不落盘**：口令不进 SQLite、不进 `AppConfig`、不进日志（配置里那个 `credential_ref`
//!   只是引用），进程一退出就没了 —— 下一次启动 `get` 返回 `None`，界面按"需要凭据"提示重填。
//!
//! 这条退化不是"静默兜底"：它由 DTO 上 `credentialLive` / `credentialPersistent` 两位说出来，
//! 设置页在用户敲口令**之前**就写明"只在这次运行里有效"（判据见 `notera-host/tests/credential_session.rs`
//! 与 `docs/ADR/0020-*.md` 的追记）。§46：不许用"理论通过"糊这一格。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

/// 本次会话内的口令表。用 `OnceLock` 而不是 `thread_local`：读它的有两个壳与同步线程，
/// 而"会话"的边界是**进程**（退出即失效），不是线程。
static SESSION: OnceLock<Mutex<HashMap<String, (String, String)>>> = OnceLock::new();
/// 测试缝：把 OS 后端当成"这个平台没有"，好在本机（Windows）真跑一遍上面那条退路。
/// 只在**自己的测试二进制**里翻（cargo 每个集成测试文件一个进程），否则会跟别的凭据测试抢全局态。
static SESSION_ONLY: AtomicBool = AtomicBool::new(false);

fn session() -> &'static Mutex<HashMap<String, (String, String)>> {
    SESSION.get_or_init(Default::default)
}

/// 测试用：之后本进程内 `available()` 一律报 false，`put/get/remove` 全走会话表。
#[doc(hidden)]
pub fn force_session_only() {
    SESSION_ONLY.store(true, Ordering::SeqCst);
}

/// 引用前缀与目标名的关系：配置里写 `keychain:{id}`，系统里那条叫
/// `notera:webdav:{id}`。分成两个字符串是因为前者会被下发到界面的等价物
/// （`has_credential` 的判据），后者是本进程的命名空间，不该泄漏出去。
pub const WEBDAV_TARGET_PREFIX: &str = "notera:webdav:";
/// 代理凭据同样走这一套（`net_proxy` 以前遇到引用就直接报 `proxy_credentials_pending`）。
pub const PROXY_TARGET_PREFIX: &str = "notera:proxy:";
/// 配置里 `credential_ref` 的标记前缀（语义：**真的存进去了**，不是"用户填过"）。
pub const REF_PREFIX: &str = "keychain:";

/// generic credential 的 blob 上限按 **UTF-16 单元** 算：512 字节 = 256 单元。
pub const MAX_UNITS: usize = 256;

#[derive(Debug, PartialEq, Eq)]
pub enum SecretError {
    /// 口令太长（带实际单元数，界面上要说清上限是多少）。
    TooLong(usize),
    /// 系统调用失败，带 last-error 码。
    ///
    /// 以前这里还有一个 `Unavailable`（"这个平台没接入"）—— 缺口 G38 选 B 之后没有平台会
    /// "存不了"了（退到会话表），那条码就没有出口了，于是整条删掉：留着一条永远不会发生的
    /// 错误码，界面上就永远有一段永远不会被看到的文案。
    Store(u32),
}

/// 日志用的写法（`tracing` 的 `%e`）。**不许带出口令本身**，只说哪一步、什么码。
///
/// 这里**不**再放一个 `code()`：错误码必须在 `CmdError::of("…")` 那个位置以字面量出现，
/// 才会被"错误码必须登记文案"那条门禁看到（算出来的码它扫不到，漏登记就静默退成通用兜底）。
/// 两条码现在住在 `App::secret_err` 的两个分支里。
impl std::fmt::Display for SecretError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLong(units) => write!(
                f,
                "口令 {} 个 UTF-16 单元，超过系统凭据 blob 的上限 {} 字节（换算 {limit} 单元）",
                units,
                MAX_UNITS * 2,
                limit = MAX_UNITS
            ),
            Self::Store(code) => write!(f, "系统凭据调用失败（Win32 错误码 {code}）"),
        }
    }
}

pub fn webdav_target(account_id: &str) -> String {
    format!("{WEBDAV_TARGET_PREFIX}{account_id}")
}

/// 代理凭据按账户分一条：同一台机器上换服务器不会串用代理口令。
pub fn proxy_target(account_id: &str) -> String {
    format!("{PROXY_TARGET_PREFIX}{account_id}")
}

pub fn ref_of(account_id: &str) -> String {
    format!("{REF_PREFIX}{account_id}")
}

/// `credential_ref` 背后的账户 id（不是 `keychain:` 开头的返回 None —— 未来可能有别的方案）。
pub fn account_of(reference: &str) -> Option<&str> {
    reference.strip_prefix(REF_PREFIX)
}

/// 这个平台上到底有没有**系统**凭据库。只有 Windows 接了（且测试缝没把它关掉）；
/// 报 false 不再意味着"口令存不了"，而是"退到本次会话的内存表"（见文件头，缺口 G38 选 B）。
///
/// 这里以前写着"界面的『记住口令』那颗开关要照它显示"，而界面里从来没有那颗开关（全仓零命中）——
/// 注释承诺了一个不存在的控制，按现状改回来。界面上现在说的是另一件事：
/// `credentialPersistent` 那位（"退出后要不要重填"）。
pub fn available() -> bool {
    cfg!(windows) && !SESSION_ONLY.load(Ordering::SeqCst)
}

/// 存（或替换）一条口令。返回值只说成不成 —— 内容不进配置，也不进日志。
/// OS 后端不可用时**退到会话表**并返回 `Ok`（这就是 G38 选 B 的那条退路）。
pub fn put(target: &str, user: &str, secret: &str) -> Result<(), SecretError> {
    let units: Vec<u16> = secret.encode_utf16().collect();
    if units.len() > MAX_UNITS {
        return Err(SecretError::TooLong(units.len()));
    }
    if available() {
        #[cfg(windows)]
        return win::put(target, user, &units);
    }
    // 中毒的锁只会是别处 panic 的现场，表本身还是那份 HashMap —— 拿它继续跑比丢掉口令安全。
    let mut map = session()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    map.insert(target.to_string(), (user.to_string(), secret.to_string()));
    tracing::debug!(target, "系统凭据库不可用：这条口令只留在本次进程的内存里");
    Ok(())
}

/// 取。`Ok(None)` = 这一轮拿不到（系统里没有、或进程刚重启过）。
/// 顺序是**先系统后会话**：接了 OS 后端的平台上会话表永远是空的，反过来则会让旧值盖住新值。
pub fn get(target: &str) -> Result<Option<(String, String)>, SecretError> {
    if available() {
        #[cfg(windows)]
        return win::get(target);
    }
    let found = session()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(target)
        .cloned();
    Ok(found)
}

/// 这一轮真拿得到吗（不管在系统里还是在会话表里）—— 界面上"能不能就这么同步"的判据。
pub fn live(target: &str) -> bool {
    matches!(get(target), Ok(Some(_)))
}

/// 这条口令是不是落在**系统**凭据库里（= 重启后还在）。会话表里的不算，
/// 所以非 Windows 上永远是 false —— 设置页那句"退出后要重填"就是照它说的。
pub fn persistent(target: &str) -> bool {
    if !available() {
        return false;
    }
    #[cfg(windows)]
    {
        matches!(win::get(target), Ok(Some(_)))
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// 删。**幂等**：系统里本来没有这一条算成功（删账户、回滚、重试都会走到这里）。
/// 两个后端都清 —— 换过平台或测试里翻过缝之后，不该留一份还能读到的旧口令。
pub fn remove(target: &str) -> Result<(), SecretError> {
    if available() {
        #[cfg(windows)]
        win::remove(target)?;
    }
    // 会话表也清一份：换过平台、或测试里翻过那条缝之后，不该留一条还能读到的旧口令。
    session()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(target);
    Ok(())
}

/// 测试与清理用：一条只在本进程生命周期里有意义的目标名（不会撞用户的真凭据）。
pub fn test_target(tag: &str) -> String {
    let unique = uuid::Uuid::now_v7().to_string();
    format!("notera-test:{tag}:{unique}")
}

#[cfg(windows)]
mod win {
    use super::SecretError;
    use std::mem;
    use std::ptr;
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_NOT_FOUND, FALSE};
    use windows_sys::Win32::Security::Credentials::{
        CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE,
        CRED_TYPE_GENERIC,
    };

    /// Windows 的宽字符串：末尾要有一个 0。空串也必须是"只有一个 0"，不能是空切片
    /// （`PCWSTR` 指过去就是野指针）。
    fn wide(s: &str) -> Vec<u16> {
        let mut v: Vec<u16> = s.encode_utf16().collect();
        v.push(0);
        v
    }

    pub fn put(target: &str, user: &str, units: &[u16]) -> Result<(), SecretError> {
        let mut target_w = wide(target);
        let mut user_w = wide(user);
        // blob 是"口令的 UTF-16 原样字节"，不含结尾 0 —— 大小按字节算。
        let mut blob: Vec<u16> = units.to_vec();
        let mut cred: CREDENTIALW = unsafe { mem::zeroed() };
        cred.Flags = 0;
        cred.Type = CRED_TYPE_GENERIC;
        cred.TargetName = target_w.as_mut_ptr();
        cred.CredentialBlobSize = (blob.len() * 2) as u32;
        cred.CredentialBlob = blob.as_mut_ptr() as *mut u8;
        cred.Persist = CRED_PERSIST_LOCAL_MACHINE;
        cred.UserName = user_w.as_mut_ptr();
        let ok = unsafe { CredWriteW(&raw const cred, 0) };
        if ok == FALSE {
            return Err(SecretError::Store(unsafe { GetLastError() }));
        }
        Ok(())
    }

    pub fn get(target: &str) -> Result<Option<(String, String)>, SecretError> {
        let target_w = wide(target);
        let mut ptr: *mut CREDENTIALW = ptr::null_mut();
        let ok = unsafe { CredReadW(target_w.as_ptr(), CRED_TYPE_GENERIC, 0, &raw mut ptr) };
        if ok == FALSE {
            let e = unsafe { GetLastError() };
            if e == ERROR_NOT_FOUND {
                return Ok(None);
            }
            return Err(SecretError::Store(e));
        }
        // 读回来了就必须负责 CredFree —— 先把手里的两段复制成 owned，再释放，
        // 中途 return 也不会漏（漏了就是句柄泄漏，长驻的后台轮会一点点吃掉 Handles）。
        let out = unsafe {
            let cred = &*ptr;
            let user = if cred.UserName.is_null() {
                String::new()
            } else {
                String::from_utf16_lossy(&wide_to_vec(cred.UserName))
            };
            let units = (cred.CredentialBlobSize as usize) / 2;
            let blob = std::slice::from_raw_parts(cred.CredentialBlob as *const u16, units);
            (user, String::from_utf16_lossy(blob))
        };
        unsafe { CredFree(ptr as *mut _) };
        Ok(Some(out))
    }

    pub fn remove(target: &str) -> Result<(), SecretError> {
        let target_w = wide(target);
        let ok = unsafe { CredDeleteW(target_w.as_ptr(), CRED_TYPE_GENERIC, 0) };
        if ok == FALSE {
            let e = unsafe { GetLastError() };
            if e == ERROR_NOT_FOUND {
                return Ok(());
            }
            return Err(SecretError::Store(e));
        }
        Ok(())
    }

    /// 把系统给的 `PWSTR` 读到结尾 0 为止，复制成 owned。
    /// 上限那道判断是给"指针指到坏了的内存"兜底的：UserName 按文档最长 512 字符。
    unsafe fn wide_to_vec(p: *const u16) -> Vec<u16> {
        let mut out = Vec::new();
        unsafe {
            while *p.add(out.len()) != 0 {
                out.push(*p.add(out.len()));
                if out.len() > 4096 {
                    break;
                }
            }
        }
        out
    }
}
