//! 连接与并发（DATA-MODEL §12）。
//!
//! ```text
//! PRAGMA journal_mode = WAL      -- 读者与写者并发
//! PRAGMA synchronous  = FULL     -- 数据安全 > 性能
//! PRAGMA foreign_keys = ON       -- 连接级：每个新连接都要设
//! PRAGMA busy_timeout = 5000
//! PRAGMA cache_size   = -16384   -- 16 MiB
//! ```
//!
//! 单写者：写连接由 `Store` 用 `Mutex` 独占（所有权威表写入串行化）。
//! 读侧是一个小连接池：取不到空闲连接就直接新开（开连接在此规模下比阻塞更便宜），
//! 归还时超过 `MAX_READERS` 的连接丢弃。

use crate::error::StoreError;
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub(crate) const MAX_READERS: usize = 4;

/// 新连接一律走这里，保证 §12 的 PRAGMA 一个不漏（`foreign_keys` 是连接级）。
pub(crate) fn open_readonly_conn(path: &Path) -> Result<Connection, StoreError> {
    let conn = Connection::open(path)?;
    apply_pragmas(&conn)?;
    Ok(conn)
}

pub(crate) fn open_write_conn(path: &Path) -> Result<Connection, StoreError> {
    let conn = Connection::open(path)?;
    apply_pragmas(&conn)?;
    // WAL 是库级持久设置，但只有写连接负责建立；返回的必须是 "wal" 才算生效。
    let mode: String = conn.query_row("PRAGMA journal_mode", [], |r| r.get(0))?;
    if !mode.eq_ignore_ascii_case("wal") {
        return Err(StoreError::Migration {
            from: 0,
            to: 0,
            detail: format!("journal_mode 未能切到 WAL（实际 {mode}）"),
        });
    }
    Ok(conn)
}

pub(crate) fn apply_pragmas(conn: &Connection) -> Result<(), StoreError> {
    // journal_mode=WAL 会返回一行，必须用 query 侧接口执行。
    let mut stmt = conn.prepare("PRAGMA journal_mode=WAL")?;
    let jm: String = stmt.query_row([], |r| r.get(0))?;
    drop(stmt);
    if !jm.eq_ignore_ascii_case("wal") && !jm.eq_ignore_ascii_case("memory") {
        return Err(StoreError::Migration {
            from: 0,
            to: 0,
            detail: format!("journal_mode=WAL 未生效（得到 {jm}）"),
        });
    }
    conn.pragma_update(None, "synchronous", "FULL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    conn.pragma_update(None, "cache_size", -16_384)?;
    Ok(())
}

pub(crate) fn journal_mode(conn: &Connection) -> Result<String, StoreError> {
    Ok(conn.query_row("PRAGMA journal_mode", [], |r| r.get::<_, String>(0))?)
}

pub(crate) fn foreign_keys_on(conn: &Connection) -> Result<bool, StoreError> {
    let v: i64 = conn.query_row("PRAGMA foreign_keys", [], |r| r.get(0))?;
    Ok(v != 0)
}

pub(crate) fn synchronous_value(conn: &Connection) -> Result<i64, StoreError> {
    Ok(conn.query_row("PRAGMA synchronous", [], |r| r.get(0))?)
}

/// 读连接池。空了就新开，归还时超出上限则丢弃。
pub(crate) struct ReaderPool {
    idle: Mutex<Vec<Connection>>,
    db: PathBuf,
}

impl ReaderPool {
    pub(crate) fn new(db: PathBuf) -> Self {
        Self { idle: Mutex::new(Vec::new()), db }
    }

    fn take(&self) -> Result<Connection, StoreError> {
        if let Some(c) = self.idle.lock().unwrap_or_else(|p| p.into_inner()).pop() {
            return Ok(c);
        }
        open_readonly_conn(&self.db)
    }

    fn put(&self, conn: Connection) {
        let mut idle = self.idle.lock().unwrap_or_else(|p| p.into_inner());
        if idle.len() < MAX_READERS {
            idle.push(conn);
        }
        // 超出上限：直接 drop（WAL 下连接很轻）。
    }
}

/// 借出的读连接，`Drop` 时归池。
pub(crate) struct ReadGuard {
    conn: Option<Connection>,
    pool: Arc<ReaderPool>,
}

impl ReadGuard {
    fn new(conn: Connection, pool: Arc<ReaderPool>) -> Self {
        Self { conn: Some(conn), pool }
    }
}

impl std::ops::Deref for ReadGuard {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        // `conn` 只在 Drop 里 take，故此处必为 Some。
        self.conn.as_ref().expect("ReadGuard 已释放")
    }
}

impl Drop for ReadGuard {
    fn drop(&mut self) {
        if let Some(c) = self.conn.take() {
            self.pool.put(c);
        }
    }
}

pub(crate) struct Readers {
    pool: Arc<ReaderPool>,
}

impl Readers {
    pub(crate) fn new(db: PathBuf) -> Self {
        Self { pool: Arc::new(ReaderPool::new(db)) }
    }
    pub(crate) fn get(&self) -> Result<ReadGuard, StoreError> {
        Ok(ReadGuard::new(self.pool.take()?, self.pool.clone()))
    }
}
