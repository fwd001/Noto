//! §4.4 / §4.8 两句**明说过的承诺**的判据（2026-10-10 完成度审计补的：实现与文案都在，判据没有）：
//!  ① 单附件 ≤32 MiB，「入口就拒并明说」—— 边界**两侧**各钉一次：限值+1 拒且什么都不落、
//!     恰限值必须收（判据是 `>` 不是 `≥`，错一格的实现要能被抓到）；
//!  ② 导出「目标位置永远不覆盖已存在的文件」—— 拒之外还要钉**原文件一个字节没动**，
//!     并配正对照（同一条导出、目标不存在时必须成功）—— 否则"永远拒绝一切导出"也能骗过它。
//!
//! 空文件拒绝（§4.4 后半句）不在这里重复：核心内联单测（`notera-host` 的 `tests` mod）与前端
//! `attachmentWire.spec` 各有一条。
//!
//! 跑法：`cargo test -p notera-host --test limits_and_overwrite`

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use notera_host::commands::dispatch;
use notera_host::App;
use serde_json::json;

static SEQ: AtomicUsize = AtomicUsize::new(0);

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("notera-limits-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn boot() -> (App, Tmp) {
    let dir = Tmp::new("app");
    let app = App::boot(dir.path()).expect("核心启动");
    (app, dir)
}

fn new_note(app: &App) -> (String, String) {
    let folder = app.default_folder_id().unwrap();
    let doc = json!({ "v": 1, "content": [{ "id": "blk000001", "type": "paragraph", "content": [{ "text": "附件的宿主" }] }] });
    let note = app.create_note(&folder, doc).unwrap();
    (note.id, "blk000001".to_string())
}

/// 数一遍某个目录下的普通文件（用来钉"什么都没落盘"）。
fn count_files(dir: &Path) -> usize {
    let mut n = 0;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            n += count_files(&p);
        } else {
            n += 1;
        }
    }
    n
}

#[test]
fn a_blob_over_the_limit_is_refused_at_the_door_and_nothing_lands() {
    let (app, dir) = boot();
    let (note, block) = new_note(&app);
    let big = dir.path().join("big.bin");
    std::fs::write(&big, vec![7u8; (32 * 1024 * 1024) + 1]).unwrap();

    let err = dispatch(
        &app,
        "attach_file",
        json!({
            "noteId": note, "blockId": block, "role": "file",
            "localPath": big.to_string_lossy(),
            "mediaType": "application/octet-stream", "filename": "big.bin",
        }),
    )
    .expect_err("超限必须拒");
    assert_eq!(err.code, "too_large", "码要能被界面翻译成那句话：{err:?}");

    // 「什么都不落」是这句话的另一半：账上没有那一行、盘上没有那个 blob。
    let inv = dispatch(&app, "attachment_inventory", json!({})).unwrap();
    assert_eq!(inv["totals"]["count"], json!(0), "{inv}");
    assert_eq!(
        count_files(&dir.path().join("attachments")),
        0,
        "被拒的字节不许在盘上留下任何一份"
    );
}

#[test]
fn a_blob_exactly_at_the_limit_is_accepted() {
    let (app, dir) = boot();
    let (note, block) = new_note(&app);
    // 恰 32 MiB：判据是 `>` 而不是 `≥` —— 把边界错一格（拒掉合法的一份）在这里红。
    let at_limit = dir.path().join("at-limit.bin");
    std::fs::write(&at_limit, vec![7u8; 32 * 1024 * 1024]).unwrap();

    dispatch(
        &app,
        "attach_file",
        json!({
            "noteId": note, "blockId": block, "role": "file",
            "localPath": at_limit.to_string_lossy(),
            "mediaType": "application/octet-stream", "filename": "at-limit.bin",
        }),
    )
    .expect("恰在限值上的一份必须收下");

    let inv = dispatch(&app, "attachment_inventory", json!({})).unwrap();
    assert_eq!(inv["totals"]["count"], json!(1), "{inv}");
    assert_eq!(inv["totals"]["bytes"], json!(32 * 1024 * 1024), "{inv}");
    assert_eq!(
        count_files(&dir.path().join("attachments")),
        1,
        "收下就要真落盘"
    );
}

#[test]
fn exporting_onto_an_existing_file_refuses_and_leaves_it_untouched() {
    let (app, dir) = boot();
    let _ = new_note(&app);

    // 目标已存在：放一份可辨认的"旧字节"。
    let target = dir.path().join("已有备份.zip");
    std::fs::write(&target, b"OLD-BYTES-NOT-A-ZIP").unwrap();
    let err = dispatch(
        &app,
        "export_data",
        json!({ "path": target.to_string_lossy() }),
    )
    .expect_err("目标已存在时不许覆盖");
    assert_eq!(err.code, "export_target_exists", "{err:?}");
    assert_eq!(
        std::fs::read(&target).unwrap(),
        b"OLD-BYTES-NOT-A-ZIP",
        "拒了但文件被动了 —— 那是比不拒更坏的一种"
    );

    // 正对照：同一条导出、目标不存在 ⇒ 必须成功并真写出一个非空的包。
    let fresh = dir.path().join("新的备份.zip");
    let ok = dispatch(
        &app,
        "export_data",
        json!({ "path": fresh.to_string_lossy() }),
    )
    .expect("换个不存在的位置就该成功");
    assert!(fresh.exists(), "{ok}");
    assert!(std::fs::metadata(&fresh).unwrap().len() > 0, "{ok}");
}
