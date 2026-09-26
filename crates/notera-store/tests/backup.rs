//! 备份与恢复（DATA-MODEL §15）。崩溃/落盘类断言一律用真临时目录，不 mock。
mod common;

use common::*;
use notera_store::{apply_pending_restore, inspect_backup, NoteQuery, Store, StoreError};

fn default_folder(store: &Store) -> notera_core::EntityId {
    store.list_folders().unwrap().into_iter().next().expect("默认本").id
}

#[test]
fn backup_is_a_self_describing_consistent_snapshot() {
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);
    let note = create(&store, &folder, "备份前写的正文");

    let info = store.create_backup(None).unwrap();
    assert!(info.path.is_file(), "备份文件必须真的在盘上");
    assert_eq!(info.sha256.len(), 64, "sha256 是裸 64 hex");
    assert!(info.bytes > 0);
    assert_eq!(info.user_version, 5, "备份要带上 schema 版本，恢复闸门靠它");

    // 备份产物自己必须能通过 integrity_check —— 坏文件当备份是最危险的假安心
    let again = inspect_backup(&info.path).unwrap();
    assert_eq!(again.sha256, info.sha256, "校验必须不改写被校验的文件");
    // 校验若把备份转成 WAL，会留下 -wal 边车：那时"一个文件的备份"就不再自足了
    let dir = info.path.parent().unwrap();
    let side = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.ends_with("-wal") || n.ends_with("-shm"))
        .collect::<Vec<_>>();
    assert!(side.is_empty(), "备份目录里不该出现 WAL 边车：{side:?}");

    // 内容对得上：把备份当新库打开，笔记还在
    let restore_dir = tempfile::tempdir().unwrap();
    std::fs::copy(&info.path, restore_dir.path().join("notera.sqlite")).unwrap();
    let reopened = Store::open(restore_dir.path(), fx.device.clone()).unwrap();
    let got = reopened.get_note(&note.id).unwrap().expect("备份里应有这条笔记");
    assert_eq!(got.title, "备份前写的正文");
}

#[test]
fn backup_captures_wal_frames_not_yet_flushed() {
    // VACUUM INTO 的价值就在这条：只拷主文件会丢掉未落盘的 WAL 帧。
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);
    let note = create(&store, &folder, "只在 WAL 里");

    let info = store.create_backup(None).unwrap();
    let restore_dir = tempfile::tempdir().unwrap();
    std::fs::copy(&info.path, restore_dir.path().join("notera.sqlite")).unwrap();
    let reopened = Store::open(restore_dir.path(), fx.device.clone()).unwrap();
    let rows = reopened.list_notes(&NoteQuery::all()).unwrap();
    assert!(rows.iter().any(|r| r.id == note.id), "备份必须包含刚写入、尚未 checkpoint 的改动");
}

#[test]
fn corrupt_backup_is_refused_rather_than_restored() {
    let fx = Fix::new();
    let store = fx.open();
    let info = store.create_backup(None).unwrap();

    // 截断：文件声称的页数与实际长度不符，integrity_check 必然报错
    let bytes = std::fs::read(&info.path).unwrap();
    std::fs::write(&info.path, &bytes[..bytes.len() / 3]).unwrap();

    let err = inspect_backup(&info.path).expect_err("截断的备份不能算备份");
    assert!(matches!(err, StoreError::Rejected(_) | StoreError::Sql(_)), "实际 {err:?}");
    let stage = store.stage_restore(&info.path).expect_err("校验不过就不该立恢复标记");
    assert!(matches!(stage, StoreError::Rejected(_) | StoreError::Sql(_)), "实际 {stage:?}");
    assert!(!fx.dir.join(notera_store::PENDING_FILE_NAME).exists(), "失败的恢复不得留下标记");
}

#[test]
fn staged_restore_applies_on_next_boot_and_keeps_the_replaced_db() {
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);
    let kept = create(&store, &folder, "备份里的笔记");
    let info = store.create_backup(None).unwrap();

    // 备份之后再改一笔：恢复必须把它抹掉（这正是用户点恢复的意图），
    // 但被抹掉的库要留下文件，否则一次误点就是不可逆数据丢失。
    create(&store, &folder, "备份之后的改动");
    assert!(list_titles(&store).iter().any(|t| t.contains("备份之后")));
    drop(store);

    fx.open().stage_restore(&info.path).unwrap();
    let after = fx.open();
    let titles = list_titles(&after);
    assert!(titles.iter().any(|t| t.contains("备份里的笔记")), "恢复后应回到备份内容：{titles:?}");
    assert!(!titles.iter().any(|t| t.contains("备份之后")), "恢复必须回到备份时点：{titles:?}");
    assert!(after.get_note(&kept.id).unwrap().is_some());

    let pre = std::fs::read_dir(&fx.dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.starts_with("notera.sqlite.pre-restore."))
        .collect::<Vec<_>>();
    assert_eq!(pre.len(), 1, "替换前必须留一份当前库，实际 {pre:?}");
    assert!(!fx.dir.join(notera_store::PENDING_FILE_NAME).exists(), "落地后标记必须删掉");
}

#[test]
fn apply_without_marker_is_a_noop() {
    let fx = Fix::new();
    fx.open();
    let before = std::fs::read(fx.db_file()).unwrap();
    assert!(apply_pending_restore(&fx.dir).unwrap().is_none(), "没有标记就不该动任何文件");
    assert_eq!(std::fs::read(fx.db_file()).unwrap(), before);
}

#[test]
fn marker_that_does_not_match_the_file_stops_the_restore() {
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);
    create(&store, &folder, "现库内容");
    let info = store.create_backup(None).unwrap();
    let current = std::fs::read(fx.db_file()).unwrap();
    drop(store);

    // 手写一个 sha256 对不上的标记：模拟备份文件在恢复前被换掉/被改坏
    let body = serde_json::json!({ "path": info.path, "sha256": format!("{:064}", 0u8) }).to_string();
    std::fs::write(fx.dir.join(notera_store::PENDING_FILE_NAME), body).unwrap();

    let err = apply_pending_restore(&fx.dir).expect_err("内容不符必须报错");
    assert!(format!("{err:?}").contains("与标记不符"), "错误要能说清为什么：{err:?}");
    assert_eq!(std::fs::read(fx.db_file()).unwrap(), current, "报错时一个字节都不该动");
    assert!(fx.dir.join(notera_store::PENDING_FILE_NAME).exists(), "标记保留，让用户自己决定");
}

#[test]
fn repeated_backups_never_overwrite_an_earlier_one() {
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);
    let first = store.create_backup(None).unwrap();
    let first_bytes = std::fs::read(&first.path).unwrap();

    // 同一秒连点两次备份是常态：必须各得一份，而不是报错、更不是覆盖掉上一份
    create(&store, &folder, "第二份备份之前写的");
    let second = store.create_backup(None).unwrap();
    assert_ne!(first.path, second.path, "两份备份不能指向同一个文件");
    assert_eq!(std::fs::read(&first.path).unwrap(), first_bytes, "后一次备份改写了前一份的内容");
    assert_eq!(inspect_backup(&first.path).unwrap().sha256, first.sha256, "前一份的自证哈希必须仍然成立");

    let all = store.list_backups().unwrap();
    assert_eq!(all.len(), 2, "两份都该在列表里：{all:?}");
}

#[test]
fn list_backups_sorts_newest_first_and_skips_unreadable() {
    let fx = Fix::new();
    let store = fx.open();
    let a = store.create_backup(None).unwrap();
    // 第二份必须落在同一个 backups 目录里才会被列出（换目录是"另存"，不是列表的一部分）
    let second = a.path.with_file_name("notera-20200101T000000Z.sqlite");
    std::fs::copy(&a.path, &second).unwrap();
    let b = inspect_backup(&second).unwrap();
    std::fs::write(fx.dir.join("backups").join("notera-垃圾.sqlite"), b"not a database").unwrap();

    let all = store.list_backups().unwrap();
    assert_eq!(all.len(), 2, "坏文件应被跳过而不是让整张列表失败：{all:?}");
    assert!(all.iter().any(|i| i.path == a.path));
    assert!(all.iter().any(|i| i.path == b.path));
    assert!(all.windows(2).all(|w| w[0].created_at >= w[1].created_at), "必须新的在前");
}

fn list_titles(store: &Store) -> Vec<String> {
    store.list_notes(&NoteQuery::all()).unwrap().into_iter().map(|r| r.title).collect()
}
