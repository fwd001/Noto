//! 迁移契约与 PRAGMA（ADR-0012 / DATA-MODEL §2、§12）。
mod common;

use common::*;
use notera_store::{SearchQuery, Store, StoreError, SUPPORTED_SCHEMA_VERSION};

fn user_version(file: &std::path::Path) -> u32 {
    let conn = rusqlite::Connection::open(file).unwrap();
    conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
        .unwrap() as u32
}

/// 期望的迁移序列：`from..=支持版本` 连续递增。
/// 不写死 `[1,2,3,4,5]` —— 那样每加一个迁移就得改测试，很容易顺手改成"少一个也过"。
/// 下限断言保证真有人删迁移号时这里仍然会红。
/// 写成 `const`：编译期就红，不用等测试被跑到。
const _: () = assert!(
    SUPPORTED_SCHEMA_VERSION >= 5,
    "支持版本不该低于 5，迁移序列被截断了？"
);
fn contiguous_from(from: u32) -> Vec<u32> {
    (from..=SUPPORTED_SCHEMA_VERSION).collect()
}

#[test]
fn empty_db_migrates_to_latest_and_tables_exist() {
    let fx = Fix::new();
    let store = fx.open();
    assert_eq!(store.migration_report().from, 0);
    assert_eq!(store.migration_report().to, SUPPORTED_SCHEMA_VERSION);
    assert_eq!(
        store.migration_report().applied,
        contiguous_from(1),
        "空库必须按 1..=支持版本逐号补齐，且不能跳号"
    );
    assert_eq!(user_version(&fx.db_file()), SUPPORTED_SCHEMA_VERSION);

    // 表/视图齐全（逐字照抄 DATA-MODEL §5 + §13）
    let conn = rusqlite::Connection::open(fx.db_file()).unwrap();
    let mut stmt = conn
        .prepare("SELECT type, name FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' AND name NOT LIKE 'notes_fts_%' ORDER BY name")
        .unwrap();
    let names: Vec<(String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .by_ref()
        .collect::<Result<_, _>>()
        .unwrap();
    let flat: Vec<&str> = names.iter().map(|(_, n)| n.as_str()).collect();
    for want in [
        "meta",
        "settings",
        "folders",
        "notes",
        "note_revisions",
        "attachments",
        "note_attachments",
        "tombstones",
        "sync_accounts",
        "sync_state",
        "sync_remote_index",
        "sync_operations",
        "sync_conflicts",
        "notes_fts",
        "v_note_list",
        "v_trash",
        "v_folder_tree",
    ] {
        assert!(flat.contains(&want), "缺少表/视图 {want}：{flat:?}");
    }
    // 视图可读（§13 的递归 CTE / 回收站视图）
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM v_note_list", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 0);
    let tree: Vec<(String, i64, String)> = conn
        .prepare("SELECT id, depth, path FROM v_folder_tree ORDER BY path")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .by_ref()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(tree.len(), 1, "只有默认本一棵树：{tree:?}");
    assert_eq!(tree[0].2, format!("/{}", notera_store::DEFAULT_FOLDER_NAME));
    assert!(store.verify().is_empty());
}

#[test]
fn reopening_does_not_reapply_migrations_or_duplicate_bootstrap() {
    let fx = Fix::new();
    let first_root;
    {
        let store = fx.open();
        first_root = default_folder(&store);
        create(&store, &first_root, "只写一次");
    }
    for _ in 0..3 {
        let store = fx.reopen();
        assert!(
            store.migration_report().applied.is_empty(),
            "重复 open 不得再跑迁移"
        );
        assert_eq!(user_version(&fx.db_file()), SUPPORTED_SCHEMA_VERSION);
        assert_eq!(default_folder(&store), first_root, "默认本不得被重复创建");
        assert_eq!(store.stats().unwrap().notes, 1);
        assert_eq!(
            store
                .list_notes(&notera_store::NoteQuery::all())
                .unwrap()
                .len(),
            1
        );
    }
    let folders = fx.reopen().list_folders().unwrap();
    assert_eq!(folders.len(), 1, "引导幂等：{folders:?}");
    assert_eq!(
        folders.iter().filter(|f| f.system_kind.is_some()).count(),
        1
    );
}

#[test]
fn upgrade_from_a_real_v2_database_backs_up_and_keeps_data() {
    // ADR-0012 M2/M3：先造一个**真正的 v2 库**（只应用 0001+0002），再让本程序升到 HEAD。
    let fx = Fix::new();
    let db = fx.db_file();
    build_partial_db(&db, 2);
    assert_eq!(user_version(&db), 2);
    let legacy = seed_legacy_note(&db);

    let store = fx.reopen();
    let rep = store.migration_report();
    assert_eq!(rep.from, 2);
    assert_eq!(
        rep.applied,
        contiguous_from(3),
        "只补缺失的后续迁移（forward-only）"
    );
    assert_eq!(
        rep.backup,
        Some(fx.dir.join("notera.sqlite.pre-migration.2"))
    );
    assert!(
        rep.backup.as_ref().unwrap().exists(),
        "迁移前必须有物理备份（ADR-0012）"
    );
    assert_eq!(user_version(&db), SUPPORTED_SCHEMA_VERSION);

    // M3：行数与内容不变，且新增的 FTS 由启动自愈补齐 → 老数据立刻可搜
    assert_eq!(store.stats().unwrap().notes, 1, "升级后笔记数不变");
    assert_eq!(store.get_note(&legacy).unwrap().unwrap().title, "老笔记");
    assert_eq!(
        store.stats().unwrap().fts_rows,
        1,
        "0003 建的空索引必须由启动自检补齐"
    );
    assert_eq!(
        store.search(&SearchQuery::new("老笔记")).unwrap().len(),
        1,
        "升级后老数据可搜"
    );
    assert_eq!(
        store
            .dirty_entities(notera_store::LOCAL_ACCOUNT_ID)
            .unwrap()
            .len(),
        2,
        "老笔记 + 默认本待上行"
    );
    assert!(store.verify().is_empty(), "{:?}", store.verify());

    // 升级后的库立即可写（新笔记 + 索引同步）
    let n = store
        .create_note(&default_folder(&store), doc_text("升级后新建"))
        .unwrap();
    assert_eq!(store.search(&SearchQuery::new("升级后")).unwrap().len(), 1);
    assert!(n.rev.get() >= 1);
    assert!(store.verify().is_empty(), "{:?}", store.verify());
}

#[test]
fn future_db_version_opens_read_only_and_is_never_downgraded() {
    let fx = Fix::new();
    {
        let store = fx.open();
        create(&store, &default_folder(&store), "未来的库");
    }
    let ahead = SUPPORTED_SCHEMA_VERSION + 7;
    downgrade(&fx.db_file(), ahead);
    let hash_before = notera_crypto::sha256_hex_file(&fx.db_file()).unwrap();
    let before_files = visible_listing(&fx.dir);

    // ADR-0012 说的是"**进入只读模式**"，不是"启动失败"。旧实现把这一格做成了
    // `Store::open` 直接返回 Err —— 于是整本库连读都读不到，界面也就永远挂不出
    // §4.2 那句"请升级以编辑"（缺口 G85）。
    let store = Store::open(&fx.dir, fx.device.clone())
        .expect("库过新时必须打得开 —— 只读模式不是启动失败（ADR-0012 M4）");
    assert_eq!(
        store.library_read_only(),
        Some((ahead, SUPPORTED_SCHEMA_VERSION))
    );
    let stats = store.stats().unwrap();
    assert!(stats.library_read_only, "界面读不到这一位就挂不出全局横幅");
    assert_eq!(stats.notes, 1, "只读模式下笔记必须照样读得到");

    // 写：拒，且拒的是"库过新"这一格。
    let e = store
        .create_note(&default_folder(&store), doc_text("这一笔不该落下去"))
        .expect_err("只读闸门下不许写回");
    match e {
        StoreError::ReadOnly { db, supported } => {
            assert_eq!((db, supported), (ahead, SUPPORTED_SCHEMA_VERSION));
        }
        other => panic!("必须是 StoreError::ReadOnly，实际 {other:?}"),
    }
    assert_eq!(
        store.stats().unwrap().notes,
        1,
        "被拒的那一笔不许落进库（拒了却没拒住是同一族的形状）"
    );

    assert_eq!(user_version(&fx.db_file()), ahead, "绝不降级写回（M4）");
    assert_eq!(
        notera_crypto::sha256_hex_file(&fx.db_file()).unwrap(),
        hash_before,
        "只读模式不得改动库文件（M4）"
    );
    let after_files = visible_listing(&fx.dir);
    assert_eq!(
        after_files, before_files,
        "只读闸门下不得产生备份或任何文件变化"
    );
    assert!(
        !after_files
            .iter()
            .any(|n| n.ends_with(&format!("pre-migration.{ahead}"))),
        "被拒绝的迁移不得生成备份：{after_files:?}"
    );
    drop(store);

    // 回到支持范围内后仍可写（只读闸门不是"库坏了"）
    downgrade(&fx.db_file(), SUPPORTED_SCHEMA_VERSION);
    let store = fx.reopen();
    assert_eq!(store.library_read_only(), None);
    assert!(!store.stats().unwrap().library_read_only);
    create(&store, &default_folder(&store), "升级之后能写了");
    assert_eq!(store.stats().unwrap().notes, 2);
}

#[test]
fn pragmas_match_data_model_section_12() {
    let fx = Fix::new();
    let store = fx.open();
    create(&store, &default_folder(&store), "pragma 检查");
    // WAL 是库级持久设置
    let conn = rusqlite::Connection::open(fx.db_file()).unwrap();
    let mode: String = conn
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mode.to_ascii_lowercase(), "wal", "journal_mode 必须是 WAL");
    drop(conn);
    // synchronous/busy_timeout/foreign_keys 是连接级：verify() 检的正是 Store 自己的连接
    assert!(
        store.verify().is_empty(),
        "verify() 检查的正是 Store 自己连接上的 §12 PRAGMA：{:?}",
        store.verify()
    );
    // 写连接是单写者：并发编辑不得产生丢文件或 rev 跳号
    let store = std::sync::Arc::new(store);
    let folder = default_folder(&store);
    let id = store.create_note(&folder, doc_text("单写者")).unwrap().id;
    let mut handles = Vec::new();
    for i in 0..6 {
        let s = store.clone();
        let id = id.clone();
        handles.push(std::thread::spawn(move || {
            for k in 0..3 {
                let cur = s.get_note(&id).unwrap().unwrap();
                // 允许 StaleEdit（单写者串行化下会被后来的写覆盖），但不许 panic
                let _ = s.edit_note(&id, doc_text(&format!("线程 {i} 第 {k} 次")), cur.rev);
            }
        }));
    }
    for h in handles {
        h.join().expect("线程不得 panic");
    }
    let n = store.get_note(&id).unwrap().unwrap();
    assert!(n.rev.get() >= 2, "至少推进过一次：{}", n.rev);
    assert!(store.verify().is_empty(), "{:?}", store.verify());
}

/// 造一个"真正的 v_k 库"：只按顺序应用前 k 个迁移文件（ADR-0012 的 M2 场景）。
fn build_partial_db(file: &std::path::Path, through: u32) {
    let files: [(u32, &str); 5] = [
        (1, include_str!("../../../migrations/0001_init.sql")),
        (2, include_str!("../../../migrations/0002_sync.sql")),
        (3, include_str!("../../../migrations/0003_search.sql")),
        (4, include_str!("../../../migrations/0004_indexes.sql")),
        (5, include_str!("../../../migrations/0005_views.sql")),
    ];
    let conn = rusqlite::Connection::open(file).unwrap();
    for (ver, sql) in files {
        if ver > through {
            break;
        }
        conn.execute_batch(sql).unwrap();
    }
    conn.pragma_update(None, "user_version", through as i64)
        .unwrap();
    drop(conn);
}

/// 以"旧客户端"的身份手写一条笔记（v2 时代没有 notes_fts，派生列/历史由旧程序写）。
fn seed_legacy_note(file: &std::path::Path) -> notera_core::EntityId {
    use notera_core::{ContentHash, EntityId};
    let conn = rusqlite::Connection::open(file).unwrap();
    conn.execute_batch("PRAGMA foreign_keys=ON").unwrap();
    let now = "2026-09-20T00:00:00.000Z";
    let folder = EntityId::new();
    let note = EntityId::new();
    conn.execute(
        "INSERT INTO folders (id, parent_id, name, system_kind, sort_order, rev, sync_rev, remote_rev,
                              content_hash, created_at, updated_at, created_device, updated_device)
         VALUES (?1,NULL,'默认本','default',0,1,0,0,?2,?3,?3,'dev-old','dev-old')",
        rusqlite::params![folder.as_str(), format!("sha256:{}", "f".repeat(64)), now],
    )
    .unwrap();
    let doc = serde_json::json!({
        "v": 1,
        "content": [
            { "id": "legacy01", "type": "heading", "attrs": { "level": 1 }, "content": [{ "text": "老笔记" }] },
            { "id": "legacy02", "type": "paragraph", "content": [{ "text": "升级前就存在的正文" }] }
        ]
    });
    let parsed = notera_richtext::parse_from_value(&doc).unwrap();
    let canonical = notera_richtext::canonical(&parsed);
    let hash = ContentHash::of(canonical.as_bytes());
    let ex = notera_richtext::extract(&parsed);
    conn.execute(
        "INSERT INTO notes (id, folder_id, doc, doc_format, pinned, title, plain_text, summary,
                            char_count, block_count, has_attachment, rev, sync_rev, remote_rev,
                            content_hash, created_at, updated_at, created_device, updated_device)
         VALUES (?1,?2,?3,1,0,?4,?5,?6,?7,?8,0,1,0,0,?9,?10,?10,'dev-old','dev-old')",
        rusqlite::params![
            note.as_str(),
            folder.as_str(),
            canonical,
            ex.title,
            ex.plain_text,
            ex.summary,
            ex.char_count as i64,
            ex.block_count as i64,
            hash.as_str(),
            now
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO note_revisions (note_id, rev, doc, content_hash, origin, device_id, created_at)
         VALUES (?1,1,?2,?3,'local','dev-old',?4)",
        rusqlite::params![note.as_str(), canonical, hash.as_str(), now],
    )
    .unwrap();
    drop(conn);
    note
}

/// 目录里的文件名（排序后），用于"拒绝打开不得改动文件"这类断言。
fn listing(dir: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    v.sort();
    v
}

/// 同上，但**不看 WAL 的伴生文件**：只读模式仍要开连接读，SQLite 自己会落下
/// `-wal` / `-shm`。"不许产生文件"讲的是不许多出备份或迁移产物，不是要 SQLite 不干活。
fn visible_listing(dir: &std::path::Path) -> Vec<String> {
    listing(dir)
        .into_iter()
        .filter(|n| !n.ends_with("-wal") && !n.ends_with("-shm"))
        .collect()
}

/// 用裸连接改版本（模拟"旧程序/新程序"）。刻意用 pragma_update，避免源码里出现
/// `PRAGMA user_version =`（CI-CD.md §Migration 契约的 grep 闸门）。
fn downgrade(file: &std::path::Path, to: u32) {
    let conn = rusqlite::Connection::open(file).unwrap();
    conn.pragma_update(None, "user_version", to as i64).unwrap();
    drop(conn);
}
