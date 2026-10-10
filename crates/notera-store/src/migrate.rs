//! 迁移引擎（ADR-0012 / DATA-MODEL §2）。
//!
//! * 迁移 SQL 是仓库根 `migrations/*.sql`，编译期 `include_str!` 嵌入 → 二进制自持迁移序列，
//!   不依赖运行时工作目录。
//! * 版本载体 `PRAGMA user_version`；**forward-only**，无 down 脚本。
//! * 每个迁移文件在单事务内执行；失败整体回滚且不推进 `user_version`。
//! * 迁移前先做物理备份 `notera.sqlite.pre-migration.<from_ver>`（先 checkpoint WAL，
//!   否则只拷主文件会丢掉未落盘的 WAL 帧）。
//! * `user_version > 本程序支持值` → [`StoreError::ReadOnly`]，不迁移、不降级写回。

use crate::error::StoreError;
use rusqlite::Connection;
use std::path::{Path, PathBuf};

pub(crate) struct Migration {
    pub ver: u32,
    /// 文件名（不含扩展名）：迁移失败时它是唯一能定位到 `migrations/NNNN_*.sql` 的线索。
    pub name: &'static str,
    pub sql: &'static str,
}

/// 迁移序列：编号必须与文件名前缀一致且严格递增（CI 有顺序断言）。
pub(crate) static MIGRATIONS: &[Migration] = &[
    Migration {
        ver: 1,
        name: "0001_init",
        sql: include_str!("../../../migrations/0001_init.sql"),
    },
    Migration {
        ver: 2,
        name: "0002_sync",
        sql: include_str!("../../../migrations/0002_sync.sql"),
    },
    Migration {
        ver: 3,
        name: "0003_search",
        sql: include_str!("../../../migrations/0003_search.sql"),
    },
    Migration {
        ver: 4,
        name: "0004_indexes",
        sql: include_str!("../../../migrations/0004_indexes.sql"),
    },
    Migration {
        ver: 5,
        name: "0005_views",
        sql: include_str!("../../../migrations/0005_views.sql"),
    },
    Migration {
        ver: 6,
        name: "0006_caps",
        sql: include_str!("../../../migrations/0006_caps.sql"),
    },
    Migration {
        ver: 7,
        name: "0007_remote_index_deleted_at",
        sql: include_str!("../../../migrations/0007_remote_index_deleted_at.sql"),
    },
    Migration {
        ver: 8,
        name: "0008_conflict_remote_wire",
        sql: include_str!("../../../migrations/0008_conflict_remote_wire.sql"),
    },
    Migration {
        ver: 9,
        name: "0009_attachment_reclaims",
        sql: include_str!("../../../migrations/0009_attachment_reclaims.sql"),
    },
];

pub(crate) fn supported_version() -> u32 {
    MIGRATIONS.last().map(|m| m.ver).unwrap_or(0)
}

pub(crate) fn current_version(conn: &Connection) -> Result<u32, StoreError> {
    let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    Ok(v.max(0) as u32)
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MigrateReport {
    pub from: u32,
    pub to: u32,
    pub applied: Vec<u32>,
    /// 迁移前物理备份文件名（`notera.sqlite.pre-migration.<from>`），未迁移时为 None。
    pub backup: Option<PathBuf>,
}

/// 应用 `from → HEAD` 的全部迁移。幂等：`from == HEAD` 时不动文件、不产生备份。
pub(crate) fn migrate(conn: &mut Connection, db_file: &Path) -> Result<MigrateReport, StoreError> {
    let supported = supported_version();
    let from = current_version(conn)?;
    if from > supported {
        // ADR-0012：只读闸门。这里刻意不做任何写操作（连备份都不做）。
        return Err(StoreError::ReadOnly {
            db: from,
            supported,
        });
    }
    let mut report = MigrateReport {
        from,
        to: from,
        applied: Vec::new(),
        backup: None,
    };
    if from == supported {
        report.to = supported;
        return Ok(report);
    }

    // 迁移前备份（仅在真要改 schema 时）。先 checkpoint，保证备份是自足的单文件。
    if db_file.exists() && std::fs::metadata(db_file).map(|m| m.len()).unwrap_or(0) > 0 {
        conn.query_row(
            "PRAGMA wal_checkpoint(TRUNCATE)",
            [],
            |r| -> rusqlite::Result<i64> { r.get::<_, i64>(0) },
        )
        .ok();
        let backup = backup_path(db_file, from);
        std::fs::copy(db_file, &backup).map_err(|e| StoreError::Migration {
            from,
            to: supported,
            detail: format!("备份 {} 失败: {e}", backup.display()),
        })?;
        report.backup = Some(backup);
    }

    for m in MIGRATIONS.iter().filter(|m| m.ver > from) {
        let tx = conn.transaction().map_err(sql_to_migration(m))?;
        tx.execute_batch(m.sql).map_err(sql_to_migration(m))?;
        // user_version 由迁移执行器唯一持有（CI grep 闸门的 allowlist 就是本文件）。
        tx.pragma_update(None, "user_version", m.ver as i64)
            .map_err(sql_to_migration(m))?;
        tx.commit().map_err(sql_to_migration(m))?;
        report.applied.push(m.ver);
        report.to = m.ver;
    }
    if report.to != supported {
        return Err(StoreError::Migration {
            from,
            to: supported,
            detail: format!("迁移后 user_version={} 与期望不符", report.to),
        });
    }
    Ok(report)
}

fn sql_to_migration(m: &Migration) -> impl Fn(rusqlite::Error) -> StoreError + '_ {
    move |e| StoreError::Migration {
        from: m.ver - 1,
        to: m.ver,
        detail: format!("{}: {e}", m.name),
    }
}

/// `notera.sqlite` + `from=3` → `notera.sqlite.pre-migration.3`
fn backup_path(db_file: &Path, from: u32) -> PathBuf {
    let name = db_file
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "notera.sqlite".into());
    db_file.with_file_name(format!("{name}.pre-migration.{from}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_numbers_are_strictly_increasing_and_match_file_names() {
        let mut prev = 0u32;
        for m in MIGRATIONS {
            assert!(
                m.ver > prev,
                "迁移编号必须严格递增: {} 在 {} 之后",
                m.ver,
                prev
            );
            prev = m.ver;
            assert!(
                m.name.starts_with(&format!("{:04}", m.ver)),
                "迁移文件名前缀必须与编号一致: {}",
                m.name
            );
            assert!(
                !m.sql.trim().is_empty(),
                "{} 内容为空（include_str! 路径错？）",
                m.name
            );
        }
        assert_eq!(supported_version(), prev);
    }

    #[test]
    fn backup_path_matches_data_model_naming() {
        let p = PathBuf::from("/data/notera.sqlite");
        assert_eq!(
            backup_path(&p, 2),
            PathBuf::from("/data/notera.sqlite.pre-migration.2")
        );
    }
}
