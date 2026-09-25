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
        loop {
            let popped = self.idle.lock().unwrap_or_else(|p| p.into_inner()).pop();
            let Some(c) = popped else { return open_readonly_conn(&self.db) };
            // 借出前必须确认连接处于 autocommit。
            //
            // 未耗尽的 query_map / 提前 return 会让连接停在一个**只读事务**里，
            // 而 WAL 下只读事务会把它开始时刻的快照钉住：这条连接下次被借出时，
            // 读到的是过期数据而不是已提交的新状态。实测表现为"同一个 COUNT(*)
            // 先返回 1、紧接着返回 0"——取决于这次借到哪个连接。
            if !c.is_autocommit() {
                if c.execute_batch("ROLLBACK;").is_err() {
                    // 回滚不掉就丢弃它，宁可新开一条，也不能把脏快照交出去
                    continue;
                }
            }
            return Ok(c);
        }
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
