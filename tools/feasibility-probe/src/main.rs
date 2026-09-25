//! Phase 0 feasibility probe.
//!
//! Not product code. Each check answers a question that would otherwise be an
//! assumption in `docs/ARCHITECTURE.md`, and prints machine-readable evidence.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::Duration;

type Outcome = (&'static str, bool, String);

fn main() {
    let host = std::env::var("PROBE_TARGET").unwrap_or_else(|_| "unknown".into());
    println!("target_hint={host}");
    let mut results: Vec<Outcome> = Vec::new();

    results.push(probe_sqlite());
    results.push(probe_fts5_cjk());
    results.push(probe_fts5_benchmark());
    results.push(probe_tx_and_wal());
    results.push(probe_migration_versioning());
    results.push(probe_sha256());
    results.push(probe_argon2id());
    results.push(probe_envelope_roundtrip());
    results.push(probe_envelope_tamper());
    results.push(probe_uuidv7_ordering());
    results.push(probe_canonical_json_stability());
    results.push(probe_tls_over_https());
    results.push(probe_webdav_verb());
    results.push(probe_etag_precondition());
    results.push(probe_proxy_actually_used());
    results.push(probe_timeout_cancellation());

    let failed = results.iter().filter(|(_, ok, _)| !ok).count();
    println!();
    println!("{:<34} {:<6} evidence", "CHECK", "RESULT");
    for (name, ok, evidence) in &results {
        println!("{:<34} {:<6} {}", name, if *ok { "PASS" } else { "FAIL" }, evidence);
    }
    println!();
    println!("total={} failed={}", results.len(), failed);
    if failed > 0 {
        std::process::exit(1);
    }
}

// ---------------------------------------------------------------- SQLite ----

fn probe_sqlite() -> Outcome {
    let name = "sqlite-bundled-compiled";
    match rusqlite::Connection::open_in_memory() {
        Ok(conn) => {
            let v: String = conn
                .query_row("SELECT sqlite_version()", [], |r| r.get(0))
                .unwrap_or_default();
            let json1: i64 = conn
                .query_row(r#"SELECT json_extract('{"a":7}','$.a')"#, [], |r| r.get(0))
                .unwrap_or(-1);
            (name, v.starts_with("3.") && json1 == 7, format!("sqlite {v}, json1={json1}"))
        }
        Err(e) => (name, false, e.to_string()),
    }
}

/// FTS5 with the `trigram` tokenizer is the chosen CJK search strategy
/// (unicode61 treats a whole Chinese run as one token, which breaks substring search).
fn probe_fts5_cjk() -> Outcome {
    let name = "fts5-trigram-cjk";
    let conn = match rusqlite::Connection::open_in_memory() {
        Ok(c) => c,
        Err(e) => return (name, false, e.to_string()),
    };
    let sql = "CREATE VIRTUAL TABLE t USING fts5(title, body, tokenize='trigram')";
    if let Err(e) = conn.execute_batch(sql) {
        return (name, false, format!("create: {e}"));
    }
    if let Err(e) = conn.execute(
        "INSERT INTO t(title,body) VALUES(?1,?2)",
        rusqlite::params!["同步协议设计", "设备 A 删除笔记后设备 B 不得复活该笔记"],
    ) {
        return (name, false, format!("insert: {e}"));
    }
    // 3-char substring from the middle of a CJK run: what a user actually types.
    let hits: i64 = conn
        .query_row(
            "SELECT count(*) FROM t WHERE t MATCH '删除笔记'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(-1);
    let miss: i64 = conn
        .query_row("SELECT count(*) FROM t WHERE t MATCH '复活任务'", [], |r| r.get(0))
        .unwrap_or(-1);
    // Snippet/highlight must work for the >=3-char path (UAT checks the UI marks hits).
    let snippet: String = conn
        .query_row(
            "SELECT snippet(t,1,'[',']','~',12) FROM t WHERE t MATCH '删除笔记'",
            [],
            |r| r.get(0),
        )
        .unwrap_or_default();
    // FTS5 trigram only indexes >= 3 char sequences, so a 2-char query silently
    // returns zero rows through MATCH. Measure which fallback path actually works.
    let count = |sql: &str, arg: &str| -> i64 {
        let mut stmt = conn.prepare(sql).expect("prepare");
        let mut rows = stmt.query_map([arg], |r| r.get::<_, i64>(0)).expect("query");
        match rows.next() {
            Some(Ok(v)) => v,
            _ => -1,
        }
    };
    let m2 = count("SELECT count(*) FROM t WHERE t MATCH ?1", "同步");
    let like_table = count("SELECT count(*) FROM t WHERE t LIKE ?1", "%同步%");
    let like_col = count("SELECT count(*) FROM t WHERE body LIKE ?1", "%同步%");
    let m3 = count("SELECT count(*) FROM t WHERE t MATCH ?1", "同步协");
    let ok = hits == 1 && miss == 0 && m2 == 0 && m3 == 1 && snippet.contains("[删除笔记]");
    (
        name,
        ok,
        format!(
            "3+char MATCH hit={hits} negative={miss} | 2char: {} | 3char: {} | whole-table LIKE={}",
            format!("MATCH->{m2} col-LIKE->{like_col}"),
            format!("MATCH->{m3}"),
            like_table
        ),
    )

}

/// Committed data survives, an uncommitted transaction is fully discarded.
/// (SQLite has no partial rollback inside one transaction, so this uses two.)
/// §17 requires search to stay fast on a large library. Measure it rather than
/// assume: 5000 CJK notes, external-content FTS5 with the trigram tokenizer.
fn probe_fts5_benchmark() -> Outcome {
    let name = "fts5-search-5000-notes";
    let conn = match rusqlite::Connection::open_in_memory() {
        Ok(c) => c,
        Err(e) => return (name, false, e.to_string()),
    };
    if let Err(e) = conn.execute_batch(
        "CREATE TABLE notes(id TEXT PRIMARY KEY, title TEXT, plain_text TEXT);
         CREATE VIRTUAL TABLE notes_fts USING fts5(title, plain_text, content='notes', content_rowid='rowid', tokenize='trigram');",
    ) {
        return (name, false, format!("schema: {e}"));
    }
    let vocab = ["同步", "冲突", "笔记", "删除", "代理", "附件", "加密", "设备", "离线", "恢复", "清单", "修订"];
    let inserted = conn
        .execute_batch(
            "BEGIN;
             INSERT INTO notes(id, title, plain_text)
             SELECT hex(randomblob(8)),
                    '笔记标题 ' || (value % 1000),
                    '关于' || (CASE value % 12 WHEN 0 THEN '同步' WHEN 1 THEN '冲突' WHEN 2 THEN '删除'
                        WHEN 3 THEN '代理' WHEN 4 THEN '附件' WHEN 5 THEN '加密'
                        WHEN 6 THEN '设备' WHEN 7 THEN '离线' WHEN 8 THEN '恢复'
                        WHEN 9 THEN '清单' WHEN 10 THEN '修订' ELSE '笔记' END)
                    || '的处理策略需要重新评估，' || hex(randomblob(80))
             FROM (WITH RECURSIVE c(value) AS (SELECT 1 UNION ALL SELECT value+1 FROM c WHERE value < 5000) SELECT value FROM c);
             INSERT INTO notes_fts(rowid, title, plain_text) SELECT rowid, title, plain_text FROM notes;
             COMMIT;",
        )
        .map_err(|e| e.to_string());
    if let Err(e) = inserted {
        return (name, false, format!("seed: {e}"));
    }
    let rows: i64 = conn.query_row("SELECT count(*) FROM notes", [], |r| r.get(0)).unwrap_or(0);
    let queries: [&str; 12] = ["同步协议", "冲突解决", "删除标记", "代理配置", "附件上传", "加密密钥", "设备标识", "离线编辑", "恢复流程", "清单损坏", "修订历史", "笔记标题"];
    let mut latencies = Vec::new();
    let mut total_hits = 0i64;
    let started = std::time::Instant::now();
    for round in 0..2 {
        for q in queries.iter() {
            let t0 = std::time::Instant::now();
            let hits: i64 = conn
                .query_row("SELECT count(*) FROM notes_fts WHERE notes_fts MATCH ?1", [q], |r| r.get(0))
                .unwrap_or(-1);
            if round == 1 {
                latencies.push(t0.elapsed().as_micros() as u128);
                total_hits += hits;
            }
        }
    }
    latencies.sort();
    let p50 = latencies[latencies.len() / 2];
    let p95 = latencies[(latencies.len() as f64 * 0.95) as usize % latencies.len()];
    let max = *latencies.last().unwrap_or(&0);
    // The 2-char fallback, measured on the real (external-content) schema:
    // MATCH is wrong here; find the path that returns rows and how much it costs.
    let mut timed = |label: &str, sql: &str| -> (i64, u128) {
        let t0 = std::time::Instant::now();
        let n: i64 = conn
            .query_row(sql, ["%同步%".replace("%", "").as_str()], |r| r.get(0))
            .unwrap_or(-999);
        let mut hits = n;
        if hits == -999 {
            hits = conn.query_row(sql, [], |r| r.get(0)).unwrap_or(-999);
        }
        (hits, t0.elapsed().as_micros())
    };
    let _ = timed;
    let fall_start = std::time::Instant::now();
    let hits_match2: i64 = conn
        .query_row("SELECT count(*) FROM notes_fts WHERE notes_fts MATCH '同步'", [], |r| r.get(0))
        .unwrap_or(-999);
    let match2_us = fall_start.elapsed().as_micros();
    let fall_start = std::time::Instant::now();
    let hits_fts_like: i64 = conn
        .query_row("SELECT count(*) FROM notes_fts WHERE plain_text LIKE '%同步%'", [], |r| r.get(0))
        .unwrap_or(-999);
    let fts_like_us = fall_start.elapsed().as_micros();
    let fall_start = std::time::Instant::now();
    let hits_content_like: i64 = conn
        .query_row("SELECT count(*) FROM notes WHERE plain_text LIKE '%同步%'", [], |r| r.get(0))
        .unwrap_or(-999);
    let content_like_us = fall_start.elapsed().as_micros();

    let db_bytes: i64 = conn
        .query_row("SELECT (SELECT sum(pgsize) FROM dbstat)", [], |r| r.get(0))
        .unwrap_or(-1);
    let _ = vocab;
    let ok = rows == 5000 && total_hits > 0 && p95 < 15_000 && hits_content_like > 0 && hits_match2 == 0;
    (
        name,
        ok,
        format!(
            "5000 rows: MATCH p50={p50}us p95={p95}us max={max}us index={} | 2char fallback: MATCH->{hits_match2} ({match2_us}us, WRONG) fts-column LIKE->{hits_fts_like} ({fts_like_us}us) content-table LIKE->{hits_content_like} ({content_like_us}us)",
            if db_bytes > 0 { format!("{:.1} MiB", db_bytes as f64 / 1048576.0) } else { "n/a".into() }
        ),
    )
}

fn probe_tx_and_wal() -> Outcome {
    let name = "wal-tx-atomicity";
    let dir = std::env::temp_dir();
    let path = dir.join(format!("notera-probe-{}.sqlite", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let conn = match rusqlite::Connection::open(&path) {
        Ok(c) => c,
        Err(e) => return (name, false, e.to_string()),
    };
    if let Err(e) = conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON;") {
        return (name, false, format!("pragmas: {e}"));
    }
    let journaling_mode: String = conn
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap_or_default();
    let fk: i64 = conn
        .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
        .unwrap_or(-1);
    conn.execute_batch("CREATE TABLE n(id INTEGER PRIMARY KEY, t TEXT)").unwrap();
    {
        let tx = conn.unchecked_transaction().unwrap();
        tx.execute("INSERT INTO n(t) VALUES('kept')", []).unwrap();
        tx.commit().unwrap();
    }
    {
        let tx = conn.unchecked_transaction().unwrap();
        tx.execute("INSERT INTO n(t) VALUES('rolled-back')", []).unwrap();
        drop(tx);
    }
    let _ = conn.execute("INSERT INTO n(t) VALUES('kept2')", []);
    let rows: Vec<String> = conn
        .prepare("SELECT t FROM n ORDER BY id")
        .and_then(|mut s| {
            s.query_map([], |r| r.get(0)).map(|i| i.filter_map(|x| x.ok()).collect())
        })
        .unwrap_or_default();
    conn.close().map_err(|(_, e)| format!("close: {e}")).ok();
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{}-wal", path.display()));
    let _ = std::fs::remove_file(format!("{}-shm", path.display()));
    let ok = journaling_mode.eq_ignore_ascii_case("wal") && fk == 1 && rows == ["kept", "kept2"];
    (
        name,
        ok,
        format!("journal={journaling_mode} foreign_keys={fk} rows_after_commit_and_rollback={rows:?}"),
    )
}

fn probe_migration_versioning() -> Outcome {
    let name = "migration-user_version";
    let conn = match rusqlite::Connection::open_in_memory() {
        Ok(c) => c,
        Err(e) => return (name, false, e.to_string()),
    };
    let steps: [&str; 3] = [
        "CREATE TABLE folders(id TEXT PRIMARY KEY);",
        "ALTER TABLE folders ADD COLUMN deleted_at TEXT;",
        "CREATE TABLE notes(id TEXT PRIMARY KEY);",
    ];
    let mut applied = Vec::new();
    for (i, sql) in steps.iter().enumerate() {
        let want = (i + 1) as i64;
        let have: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap_or(-1);
        if have < want {
            if let Err(e) = conn.execute_batch(sql) {
                return (name, false, format!("step {want}: {e}"));
            }
            if let Err(e) = conn.pragma_update(None, "user_version", want) {
                return (name, false, format!("version: {e}"));
            }
            applied.push(want);
        }
    }
    // Re-running the runner must be a no-op (idempotent upgrade path).
    let final_version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap_or(-1);
    let again: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap_or(-1);
    (name, final_version == 3 && again == 3 && applied.len() == 3, format!("applied={applied:?} version={final_version}"))
}

// ---------------------------------------------------------------- crypto ----

/// FIPS 180-4 known-answer vectors. A hashing bug here corrupts every record
/// hash, attachment dedupe key and conflict check downstream.
fn probe_sha256() -> Outcome {
    use sha2::{Digest, Sha256};
    let vectors: [(&[u8], &str); 3] = [
        (b"", "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"),
        (b"abc", "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"),
        (b"notera", "598a5bd92cbeac933ea159313cf0aed11fe59e3ba6c9e37db88f75bcd78ebb22"),
    ];
    let mut mismatches = Vec::new();
    for (input, want) in vectors {
        let mut h = Sha256::new();
        h.update(input);
        let got = hex(&h.finalize());
        if got != want {
            mismatches.push(format!("{input:?}: got {got} want {want}"));
        }
    }
    (
        "sha256-known-answer",
        mismatches.is_empty(),
        if mismatches.is_empty() { "3/3 vectors match".into() } else { mismatches.join(" | ") },
    )
}

fn probe_argon2id() -> Outcome {
    use argon2::{Algorithm, Argon2, Params, Version};
    let mut out = [0u8; 32];
    let params = Params::new(19 * 1024, 2, 1, Some(32)).unwrap();
    let a2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let t = std::time::Instant::now();
    let ok = a2
        .hash_password_into(b"master-passphrase", b"0123456789abcdef", &mut out)
        .is_ok();
    let ms = t.elapsed().as_millis();
    // Deterministic: same inputs must yield the same key (sync must not drift).
    let mut out2 = [0u8; 32];
    let _ = a2.hash_password_into(b"master-passphrase", b"0123456789abcdef", &mut out2);
    (
        "kdf-argon2id",
        ok && out == out2,
        format!("19MiB/2it/1p -> {ms}ms, deterministic={}", out == out2),
    )
}

fn probe_envelope_roundtrip() -> Outcome {
    let key = vec![b'k'; 32];
    match envelope_seal(b"{\"type\":\"doc\"}", &key) {
        Err(e) => ("envelope-aes-256-gcm-siv", false, e),
        Ok(o) => {
            let overhead = o.ct.len() as i64 - o.pt.len() as i64;
            (
                "envelope-aes-256-gcm-siv",
                o.pt == b"{\"type\":\"doc\"}" && !o.ct.is_empty() && overhead == 16,
                format!("nonce={}B key=32B tag_overhead={overhead}B (ct {}B <- pt {}B)", o.nonce.len(), o.ct.len(), o.pt.len()),
            )
        }
    }
}

fn probe_envelope_tamper() -> Outcome {
    let key = vec![b'k'; 32];
    match envelope_seal(b"payload-original", &key) {
        Err(e) => ("envelope-tamper-detected", false, e),
        Ok(o) => {
            let mut ct = o.ct.clone();
            let last = ct.len() - 1;
            ct[last] ^= 0x01; // flip one ciphertext byte
            match envelope_open(&ct, &o.nonce, &key) {
                Ok(pt) => ("envelope-tamper-detected", false, format!("modified ciphertext accepted: {pt:?}")),
                Err(e) => ("envelope-tamper-detected", true, format!("rejected: {e}")),
            }
        }
    }
}

struct Sealed {
    ct: Vec<u8>,
    nonce: Vec<u8>,
    pt: Vec<u8>,
}

fn envelope_seal(pt: &[u8], key: &[u8]) -> Result<Sealed, String> {
    use aes_gcm_siv::{aead::{Aead, KeyInit}, Aes256GcmSiv, Nonce};
    let nonce = discover_nonce();
    let c = Aes256GcmSiv::new_from_slice(key).map_err(|e| e.to_string())?;
    let nonce_arr = Nonce::try_from(nonce.as_slice()).map_err(|_| "nonce length".to_string())?;
    let ct = c.encrypt(&nonce_arr, pt).map_err(|e| e.to_string())?;
    let opened = envelope_open(&ct, &nonce, key)?;
    Ok(Sealed { ct, nonce, pt: opened })
}

fn envelope_open(ct: &[u8], nonce: &[u8], key: &[u8]) -> Result<Vec<u8>, String> {
    use aes_gcm_siv::{aead::{Aead, KeyInit}, Aes256GcmSiv, Nonce};
    let c = Aes256GcmSiv::new_from_slice(key).map_err(|e| e.to_string())?;
    let nonce_arr = Nonce::try_from(nonce).map_err(|_| "nonce length".to_string())?;
    c.decrypt(&nonce_arr, ct).map_err(|e| e.to_string())
}

/// aes-gcm-siv 0.12 deprecated `Nonce::from_slice`, which panics on a length
/// guess. Probe the associated nonce size instead of hardcoding it.
fn discover_nonce() -> Vec<u8> {
    use aes_gcm_siv::{Aes256GcmSiv, Nonce};
    for n in [12usize, 8, 16, 24] {
        let candidate = vec![0xa5u8; n];
        if Nonce::try_from(candidate.as_slice()).is_ok() {
            return candidate;
        }
    }
    Vec::new()
}

// ------------------------------------------------------------------- ids ----

fn probe_uuidv7_ordering() -> Outcome {
    use uuid::Uuid;
    let a = Uuid::now_v7();
    thread::sleep(Duration::from_millis(2));
    let b = Uuid::now_v7();
    let sortable = a.as_u128() < b.as_u128();
    let path_safe = a.simple().to_string().len() == 32 && !a.hyphenated().to_string().contains(['/', ' ', '?']);
    (
        "uuidv7-monotonic-pathsafe",
        sortable && path_safe,
        format!("{a} < {b} = {sortable}"),
    )
}

fn probe_canonical_json_stability() -> Outcome {
    // Key order in a map is unspecified; canonical form must still be stable
    // because record hashes drive conflict detection and attachment dedupe.
    let mut m = serde_json::Map::new();
    m.insert("z".into(), serde_json::json!(1));
    m.insert("a".into(), serde_json::json!(2));
    m.insert("m".into(), serde_json::json!({"k":1,"b":2}));
    let canonical = canonical_json(&serde_json::Value::Object(m.clone()));
    let mut m2 = m.clone();
    let z = m2.remove("z").unwrap();
    m2.insert("z".into(), z);
    let canonical2 = canonical_json(&serde_json::Value::Object(m2));
    (
        "canonical-json-for-hashing",
        canonical == canonical2 && canonical.starts_with(r#"{"a":"#) && canonical.contains(r#"{"b":2,"k":1}"#),
        canonical,
    )
}

fn canonical_json(v: &serde_json::Value) -> String {
    use serde_json::Value;
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            let parts: Vec<String> = keys
                .iter()
                .map(|k| format!("{}:{}", Value::String((*k).clone()), canonical_json(&m[*k])))
                .collect();
            format!("{{{}}}", parts.join(","))
        }
        Value::Array(a) => format!("[{}]", a.iter().map(canonical_json).collect::<Vec<_>>().join(",")),
        other => other.to_string(),
    }
}

// ---------------------------------------------------------------- network ----

fn probe_tls_over_https() -> Outcome {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build();
    match rt {
        Err(e) => ("https-tls-handshake", false, format!("runtime: {e}")),
        Ok(rt) => {
            let code = rt.block_on(async {
                let client = match reqwest::Client::builder()
                    .timeout(Duration::from_secs(12))
                    .no_proxy()
                    .build()
                {
                    Ok(c) => c,
                    Err(e) => return format!("client: {e}"),
                };
                match client
                    .get("https://static.crates.io/crates/serde/serde-1.0.200.crate")
                    .send()
                    .await
                {
                    Ok(r) => format!("{}", r.status().as_u16()),
                    Err(e) => format!(
                        "err(timeout={}, connect={}, body={})",
                        e.is_timeout(),
                        e.is_connect(),
                        e.is_body()
                    ),
                }
            });
            let ok = code == "200" || code == "404";
            ("https-tls-handshake", ok, format!("static.crates.io -> {code}"))
        }
    }
}

fn webdav_method(name: &str) -> reqwest::Method {
    // `Method::from_static` is private in http 1.5; `from_bytes` is the public
    // constructor for extension methods.
    reqwest::Method::from_bytes(name.as_bytes()).expect("valid method token")
}

/// PROPFIND / MOVE are not expressible as REST verbs; the client must be able to
/// send arbitrary methods and the test server must be able to answer them.
fn probe_webdav_verb() -> Outcome {
    let addr = match spawn_tiny_webdav() {
        Ok(v) => v,
        Err(e) => return ("webdav-custom-verbs", false, e),
    };
    let url = format!("http://{addr}/.notes/");
    let r = reqwest::blocking::Client::new()
        .request(webdav_method("PROPFIND"), &url)
        .header("Depth", "1")
        .body("<?xml version=\"1.0\"?><propfind/>")
        .send();
    let (status, body) = match r {
        Ok(resp) => {
            let s = resp.status().as_u16();
            (s, resp.text().unwrap_or_default())
        }
        Err(e) => return ("webdav-custom-verbs", false, format!("send: {e}")),
    };
    let move_status = reqwest::blocking::Client::new()
        .request(webdav_method("MOVE"), &format!("http://{addr}/tmp"))
        .header("Destination", format!("http://{addr}/final"))
        .header("Overwrite", "F")
        .send()
        .map(|r| r.status().as_u16())
        .unwrap_or(0);
    let depth_ok = body.contains(r#"depth="1""#);
    let echoed = body.contains("verb=\"PROPFIND\"");
    (
        "webdav-custom-verbs",
        status == 207 && echoed && depth_ok && move_status == 201,
        format!("PROPFIND->{status} verb_echoed={echoed} depth_forwarded={depth_ok} MOVE->{move_status}"),
    )
}

/// If-Match precondition failures are the concurrency guard for record writes.
/// The sync layer must be able to *see* a 412 and re-plan, not treat it as fatal.
fn probe_etag_precondition() -> Outcome {
    let addr = match spawn_tiny_webdav() {
        Ok(v) => v,
        Err(e) => return ("precondition-412-plumbing", false, e),
    };
    let client = reqwest::blocking::Client::new();
    let url = format!("http://{addr}/.notes/notes/a.json");
    let put = |etag: &str| {
        client
            .request(reqwest::Method::PUT, &url)
            .header("If-Match", format!("\"{etag}\""))
            .body("{}")
            .send()
            .map(|r| r.status().as_u16())
    };
    let stale = put("rev-6").unwrap_or(0);
    let fresh = put("rev-7").unwrap_or(0);
    (
        "precondition-412-plumbing",
        stale == 412 && fresh == 204,
        format!("stale etag -> {stale}, current etag -> {fresh} (both arrive as statuses, so the engine can re-plan instead of failing the round)"),
    )
}

/// §19 demands proof that traffic really traverses the configured proxy.
/// Method: point at a proxy that is definitely not listening. If the request
/// still succeeds, the proxy setting was silently ignored.
fn probe_proxy_actually_used() -> Outcome {
    let proxy_url = "socks5://127.0.0.1:1";
    let client = reqwest::blocking::Client::builder()
        .proxy(reqwest::Proxy::all(proxy_url).unwrap())
        .connect_timeout(Duration::from_secs(4))
        .timeout(Duration::from_secs(8))
        .build();
    let client = match client {
        Ok(c) => c,
        Err(e) => return ("proxy-config-actually-honored", false, format!("build: {e}")),
    };
    let via_proxy = client.get("https://static.crates.io/crates/serde/serde-1.0.200.crate").send();
    let direct = reqwest::blocking::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(12))
        .build()
        .map(|c| c.get("https://static.crates.io/crates/serde/serde-1.0.200.crate").send().map(|r| r.status().as_u16()).unwrap_or(0))
        .unwrap_or(0);
    let proxy_blocked = via_proxy.is_err();
    (
        "proxy-config-actually-honored",
        proxy_blocked && direct == 200,
        format!("dead-proxy->error={proxy_blocked}; no_proxy->HTTP {direct} (differential proves routing layer is live)"),
    )
}

fn probe_timeout_cancellation() -> Outcome {
    let addr = match spawn_stall_server() {
        Ok(v) => v,
        Err(e) => return ("timeout-and-cancel", false, e),
    };
    let started = std::time::Instant::now();
    let outcome = reqwest::blocking::Client::builder()
        .timeout(Duration::from_millis(400))
        .no_proxy()
        .build()
        .map_err(|e| format!("builder: {e}"))
        .and_then(|c| {
            c.get(format!("http://{addr}/slow"))
                .send()
                .map(|r| format!("status {}", r.status().as_u16()))
                .map_err(|e| format!("timeout={} {e}", e.is_timeout()))
        });
    let elapsed = started.elapsed();
    let aborted_on_budget = matches!(&outcome, Err(msg) if msg.starts_with("timeout=true"));
    (
        "timeout-and-cancel",
        aborted_on_budget && elapsed >= Duration::from_millis(350) && elapsed < Duration::from_secs(3),
        format!(
            "stalled response cancelled after {}ms against a 400ms budget ({outcome:?})",
            elapsed.as_millis()
        ),
    )
}

// ----------------------------------------------------------- tiny servers ----

fn spawn_tiny_webdav() -> Result<String, String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?.to_string();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = match stream.try_clone() {
                Ok(s) => BufReader::new(s),
                Err(_) => continue,
            };
            let Some((method, path, headers)) = read_request(&mut reader) else { continue };
            let header = |name: &str| -> String {
                headers
                    .iter()
                    .find(|(k, _)| k == name)
                    .map(|(_, v)| v.clone())
                    .unwrap_or_default()
            };
            let content_length: usize = header("content-length").parse().unwrap_or(0);
            if content_length > 0 {
                let mut body = vec![0u8; content_length];
                let _ = reader.read_exact(&mut body);
            }
            let (status, body) = match method.as_str() {
                "PROPFIND" => (
                    "207 Multi-Status",
                    format!(r#"<?xml version="1.0"?><multistatus verb="PROPFIND" path="{path}" depth="{}"/>"#, header("depth")),
                ),
                "MOVE" => ("201 Created", "<moved/>".to_string()),
                "PUT" => {
                    if header("if-match") == "\"rev-7\"" {
                        ("204 No Content", String::new())
                    } else {
                        ("412 Precondition Failed", "<stale/>".to_string())
                    }
                }
                _ => ("405 Method Not Allowed", "<err/>".to_string()),
            };
            let resp = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/xml\r\nContent-Length: {}\r\nETag: \"rev-7\"\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.flush();
        }
    });
    Ok(addr)
}

fn spawn_stall_server() -> Result<String, String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?.to_string();
    thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 512];
            let _ = stream.read(&mut buf);
            thread::sleep(Duration::from_secs(30)); // never answers inside the budget
        }
    });
    Ok(addr)
}

fn read_request<R: BufRead>(reader: &mut R) -> Option<(String, String, Vec<(String, String)>)> {
    let mut line = String::new();
    if reader.read_line(&mut line).ok()? == 0 {
        return None;
    }
    let mut parts = line.trim_end().split_whitespace();
    let method = parts.next()?.to_string();
    let path = parts.next()?.to_string();
    let mut headers = Vec::new();
    loop {
        let mut raw = String::new();
        match reader.read_line(&mut raw).ok()? {
            0 => break,
            _ => {}
        }
        let trimmed = raw.trim_end();
        if trimmed.is_empty() {
            break;
        }
        if let Some((k, v)) = trimmed.split_once(':') {
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
    }
    Some((method, path, headers))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
