//! DATA-MODEL §15 的备份与恢复。
//!
//! 备份用 `VACUUM INTO`：SQLite 自己产出**一致的单文件**快照，不需要拷活动的
//! `-wal`（那会连半截事务一起拷走）。选它而不是 rusqlite 的 `backup` 特性，
//! 是为了不为一件事加依赖。
//!
//! 恢复**不在进程内换库**：`Store` 活在 `Arc<Inner>` 里，换它等于重构整个最共享
//! 的对象。改成"点恢复 → 写待恢复标记 → 下次启动在 `Store::open` 之前落地"，
//! 顺带天然满足两条：替换前先备份当前库、恢复后走启动自检。

use crate::error::StoreError;
use crate::migrate;
use crate::store::Store;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const BACKUP_DIR_NAME: &str = "backups";
pub const PENDING_FILE_NAME: &str = "restore-pending.json";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupInfo {
    pub path: PathBuf,
    pub sha256: String,
    pub user_version: u32,
    pub bytes: u64,
    pub created_at: String,
}

/// 备份产物必须自检通过才算"有备份"。坏文件当备份是最危险的假安心。
pub fn inspect_backup(path: &Path) -> Result<BackupInfo, StoreError> {
    if !path.is_file() {
        return Err(StoreError::Rejected(format!(
            "备份不存在：{}",
            path.display()
        )));
    }
    let sha256 = notera_crypto::sha256_hex_file(path)?;
    let conn = open_read_only(path)?;
    let integrity: String = conn.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
    if integrity != "ok" {
        return Err(StoreError::Rejected(format!("备份自检未通过：{integrity}")));
    }
    let user_version = migrate::current_version(&conn)?;
    let supported = migrate::supported_version();
    if user_version > supported {
        return Err(StoreError::ReadOnly {
            db: user_version,
            supported,
        });
    }
    let bytes = std::fs::metadata(path)?.len();
    Ok(BackupInfo {
        path: path.to_path_buf(),
        sha256,
        user_version,
        bytes,
        created_at: backup_time_of(path),
    })
}

impl Store {
    /// 产出一份一致快照，返回它的自证信息（路径、sha256、user_version、字节数）。
    pub fn create_backup(&self, dest_dir: Option<&Path>) -> Result<BackupInfo, StoreError> {
        let dir = dest_dir
            .map(PathBuf::from)
            .unwrap_or_else(|| self.paths.db.with_file_name(BACKUP_DIR_NAME));
        std::fs::create_dir_all(&dir)?;
        let stamp = backup_time_of(&self.paths.db);
        // 同一秒里点两次备份是常事：要的是两份备份，不是一个报错。
        // 但绝不允许覆盖已有文件 —— 被覆盖掉的正好可能是用户想要的那一份。
        let target = free_name(&dir, &stamp);
        let tmp = dir.join(format!("notera-{stamp}.sqlite.writing"));
        let _ = std::fs::remove_file(&tmp);

        {
            let conn = self.write.lock().unwrap_or_else(|p| p.into_inner());
            // VACUUM INTO 不能在事务里跑，所以直接借连接而不走 write_tx。
            conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
            conn.execute(
                "VACUUM INTO ?1",
                rusqlite::params![tmp.to_string_lossy().as_ref()],
            )?;
        }
        std::fs::rename(&tmp, &target)?;
        inspect_backup(&target)
    }

    pub fn list_backups(&self) -> Result<Vec<BackupInfo>, StoreError> {
        let dir = self.paths.db.with_file_name(BACKUP_DIR_NAME);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("notera-") && n.ends_with(".sqlite"))
            {
                // 一份坏备份不该让"列出备份"整体失败：跳过它，恢复时再逐个校验。
                if let Ok(info) = inspect_backup(&path) {
                    out.push(info);
                }
            }
        }
        out.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(out)
    }

    /// 校验通过才立标记；标记是"下次启动要恢复"的唯一凭据。
    pub fn stage_restore(&self, path: &Path) -> Result<BackupInfo, StoreError> {
        let info = inspect_backup(path)?;
        let marker = pending_path(&self.paths.db);
        let body = serde_json::to_vec(&PendingRestore {
            path: info.path.clone(),
            sha256: info.sha256.clone(),
        })
        .map_err(|e| StoreError::Rejected(format!("待恢复标记序列化失败：{e}")))?;
        crate::store::write_atomic(&marker, &body)?;
        Ok(info)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingRestore {
    pub path: PathBuf,
    pub sha256: String,
}

fn pending_path(db_file: &Path) -> PathBuf {
    db_file.with_file_name(PENDING_FILE_NAME)
}

/// 校验用的连接**必须真的是只读**：`pool::open_readonly_conn` 其实会跑
/// `PRAGMA journal_mode=WAL`，那会把待校验的备份就地改写（还多出 `-wal`），
/// 于是"校验"本身破坏了被校验的东西，sha256 也对不上了。
fn open_read_only(path: &Path) -> Result<Connection, StoreError> {
    Ok(Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?)
}

/// 在 `Store::open` 之前调用：有标记就落地。返回替换前的当前库备份路径。
///
/// 顺序是刻意的：校验 → 备份现库 → 改名替换 → 最后删标记。
/// 崩在删标记之前只会导致"同一份备份再恢复一次"（幂等），
/// 反过来先删标记则可能出现"用户以为恢复了、其实没有"。
pub fn apply_pending_restore(data_dir: &Path) -> Result<Option<PathBuf>, StoreError> {
    let db = data_dir.join(crate::store::DB_FILE_NAME);
    let marker = pending_path(&db);
    if !marker.is_file() {
        return Ok(None);
    }
    let body = std::fs::read(&marker)?;
    let pending: PendingRestore = serde_json::from_slice(&body)
        .map_err(|e| StoreError::Rejected(format!("待恢复标记读不懂，已不动数据：{e}")))?;

    let info = inspect_backup(&pending.path)?;
    if info.sha256 != pending.sha256 {
        return Err(StoreError::Rejected(format!(
            "备份内容与标记不符（期望 {} 实际 {}），已停止恢复",
            &pending.sha256[..12.min(pending.sha256.len())],
            &info.sha256[..12.min(info.sha256.len())]
        )));
    }

    let mut replaced: Option<PathBuf> = None;
    if db.is_file() && std::fs::metadata(&db)?.len() > 0 {
        let here = migrate::current_version(&open_read_only(&db)?)?;
        let keep = db.with_file_name(format!(
            "notera.sqlite.pre-restore.{here}.{}",
            &info.sha256[..8]
        ));
        std::fs::copy(&db, &keep)?;
        replaced = Some(keep);
    }
    for suffix in ["-wal", "-shm"] {
        let side = file_with_suffix(&db, suffix);
        let _ = std::fs::remove_file(side);
    }
    let staged = db.with_file_name("notera.sqlite.restoring");
    std::fs::copy(&pending.path, &staged)?;
    std::fs::rename(&staged, &db)?;
    std::fs::remove_file(&marker)?;
    Ok(replaced)
}

fn file_with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let name = format!(
        "{}{}",
        path.file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default(),
        suffix
    );
    path.with_file_name(name)
}

/// 找一个没被占用的备份文件名。上限只是个保险，不是行为契约。
fn free_name(dir: &Path, stamp: &str) -> PathBuf {
    let first = dir.join(format!("notera-{stamp}.sqlite"));
    if !first.exists() {
        return first;
    }
    (2..=1000)
        .map(|n| dir.join(format!("notera-{stamp}-{n}.sqlite")))
        .find(|p| !p.exists())
        .unwrap_or(first)
}

fn backup_time_of(path: &Path) -> String {
    let mtime = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
    let secs = mtime
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs, 0)
        .map(|d| d.format("%Y%m%dT%H%M%SZ").to_string())
        .unwrap_or_else(|| "unknown".to_string())
}
