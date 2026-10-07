//! 缺口 G85：库版本比本程序新时，**整机只读**这一格要真的可达。
//!
//! 旧形状不是"少了那条横幅"，而是横幅的**生产者根本不存在**：`Store::open` 在
//! `user_version > 支持值` 时直接返回 `Err` ⇒ `App::boot` 跟着失败 ⇒ 桌面壳的 `setup()`
//! 顶死、dev 桥整个进程退出。于是没有任何一条命令发得出 `db_too_new`（它在 Rust 里
//! 只出现在映射表和那张映射表自己的单测里），而 §4.2 要的是
//! 「全局横幅 请升级以编辑，绝不降级写」—— 用户看到的是"连不上本地服务"，
//! 查错方向查到网络上去了。
//!
//! ADR-0012 的 M4 写的是「**进入只读模式**并停止同步 …… 给出可操作文案」，
//! 所以下面这几条判据都打在命令面上：读要通、写要按 `db_too_new` 拒、
//! 不经过 `write_tx` 的那几条（改数据目录文件）要在门口就拒。
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use notera_host::App;
use serde_json::json;

static SEQ: AtomicUsize = AtomicUsize::new(0);

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("notera-libro-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn db(&self) -> PathBuf {
        self.0.join("notera.sqlite")
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// 把库的版本抬到"本程序还不认识"那一格。
///
/// 直接改文件头那四个字节（SQLite 的 `user_version` 就在这本库的偏移 60，大端 —— 实测：现役库里那一格读的到的正是迁移条数），而不是在这里发
/// `PRAGMA` —— ARCHITECTURE-MAP §5 不许 host 侧写 SQL，而"更新的那一版当年是怎么把这一位
/// 抬上去的"对本测试来说不是要复现的过程，只是要造出来的**形状**。
/// 前置条件是最后一个连接已经关掉（SQLite 那时会 checkpoint，页 1 已经在主文件里）。
fn bump_version(file: &Path, to: u32) {
    // 最后一个连接关掉时 SQLite 会做 checkpoint，但 `-wal` / `-shm` 可能还留着页 1 的副本 ——
    // 只改主文件就会被 WAL 里那一版盖掉（实测就是这样造不出"库过新"这一格）。删掉伴生文件，
    // 让主文件成为唯一权威；真丢了数据的话，下面那条"笔记还读得到"会红，不会静默。
    for suffix in ["-wal", "-shm"] {
        let mut p = file.as_os_str().to_os_string();
        p.push(suffix);
        let _ = std::fs::remove_file(Path::new(&p));
    }
    let mut bytes = std::fs::read(file).unwrap();
    bytes[60..64].copy_from_slice(&to.to_be_bytes());
    std::fs::write(file, bytes).unwrap();
}

fn doc(text: &str) -> serde_json::Value {
    json!({ "v": 1, "content": [{ "id": "b1", "type": "paragraph", "content": [{ "text": text }] }] })
}

fn code_of(e: notera_host::commands::CmdError) -> String {
    e.code
}

#[test]
fn a_library_from_a_newer_build_boots_read_only_reads_work_and_writes_are_refused() {
    let tmp = Tmp::new("mode");
    let note_id = {
        let app = App::boot(&tmp.0).expect("首次启动");
        let created = notera_host::commands::dispatch(
            &app,
            "create_note",
            json!({ "doc": doc("过新库里的这一篇") }),
        )
        .expect("create_note");
        created["id"].as_str().unwrap().to_string()
    };

    let ahead = notera_store::SUPPORTED_SCHEMA_VERSION + 3;
    bump_version(&tmp.db(), ahead);
    let hash_before = notera_crypto::sha256_hex_file(&tmp.db()).unwrap();

    // 第一格：boot 必须成功。这一条红 = 整个进程起不来 = 后面全是空谈。
    let app = App::boot(&tmp.0).expect("库过新时要进入只读模式，不是启动失败（ADR-0012 M4）");

    // 界面挂横幅靠的是 `stats` 这一位（首帧 `probeLink()` 就是它）。
    let stats = notera_host::commands::dispatch(&app, "stats", json!({}))
        .expect("stats 是读，只读模式下必须照常");
    assert_eq!(
        stats["libraryReadOnly"], true,
        "界面读不到这一位就挂不出§4.2 那句『请升级以编辑』"
    );

    // 读侧照常：笔记还得看得见，"只读"不能等于"没了"。
    let rows =
        notera_host::commands::dispatch(&app, "list_notes", json!({})).expect("list_notes 是读");
    assert_eq!(
        rows.as_array().unwrap().len(),
        1,
        "只读模式下笔记要读得到：{rows:?}"
    );

    // 写侧：按 `db_too_new` 拒。走 `write_tx` 的那几条由库层闸门管。
    for (name, args) in [
        (
            "edit_note",
            json!({ "id": note_id, "doc": doc("这一笔不许落"), "expectedRev": 1 }),
        ),
        ("create_note", json!({ "doc": doc("这一笔也不许") })),
        ("delete_note", json!({ "id": note_id })),
        ("set_note_pinned", json!({ "id": note_id, "pinned": true })),
    ] {
        let e = notera_host::commands::dispatch(&app, name, args)
            .err()
            .unwrap_or_else(|| panic!("{name} 在只读闸门下必须被拒"));
        assert_eq!(e.code, "db_too_new", "{name} 拒错了码：{}", e.code);
        assert_eq!(
            e.detail.unwrap()["db"],
            ahead,
            "{name} 要带出版本对，界面才说得出下一步"
        );
    }

    // 不经过 `write_tx` 的那几条（改数据目录里的文件 / 账户配置 / 起同步）：门口就拒。
    for (name, args) in [
        ("sync_now", json!({})),
        ("erase_all_data", json!({ "confirm": true })),
        ("restore_db", json!({ "path": "x" })),
        ("backup_db", json!({})),
    ] {
        let code = notera_host::commands::dispatch(&app, name, args)
            .err()
            .map(code_of)
            .unwrap_or_else(|| panic!("{name} 在只读闸门下必须被拒 —— 它改的是数据目录里的文件"));
        assert_eq!(code, "db_too_new", "{name} 拒错了码：{code}");
    }

    // 一整轮量下来：库文件一个字节没变，版本也没被降级写回。
    assert_eq!(
        notera_crypto::sha256_hex_file(&tmp.db()).unwrap(),
        hash_before,
        "只读模式不许改动库文件（M4）"
    );
    let bytes = std::fs::read(tmp.db()).unwrap();
    let now = u32::from_be_bytes(bytes[60..64].try_into().unwrap());
    assert_eq!(now, ahead, "绝不降级写回（M4）");
}
