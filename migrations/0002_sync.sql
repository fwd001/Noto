-- 0002_sync.sql —— 同步元数据表（DATA-MODEL.md §5.2 逐字落地）
-- 存储层不知道同步（ARCHITECTURE-MAP §2）：本表只是**持久化容器**，
-- 判定逻辑一律在 notera-sync。

CREATE TABLE sync_accounts (
  id             TEXT PRIMARY KEY,
  label          TEXT NOT NULL,
  base_url       TEXT NOT NULL,
  root_prefix    TEXT NOT NULL DEFAULT '/.notes',
  auth_kind      TEXT NOT NULL CHECK (auth_kind IN ('basic','token')),
  tls_policy     TEXT NOT NULL CHECK (tls_policy IN ('strict','pin','ca_bundle','insecure_local')),
  device_id      TEXT NOT NULL,
  protocol_min   INTEGER,
  protocol_max   INTEGER,
  enabled        INTEGER NOT NULL DEFAULT 1,
  created_at     TEXT NOT NULL
);

CREATE TABLE sync_state (                     -- 每账户一行，同步引擎权威状态
  account_id           TEXT PRIMARY KEY REFERENCES sync_accounts(id) ON DELETE CASCADE,
  root_id              TEXT,
  phase                TEXT NOT NULL CHECK (phase IN
                       ('unconfigured','provisioning','bootstrap_push','bootstrap_pull',
                        'online','read_only','needs_credentials','error')),
  seq_applied          INTEGER NOT NULL DEFAULT 0,   -- 已消费的清单 seq
  manifest_etag        TEXT,
  lease_token          TEXT,
  lease_expires_at     TEXT,
  last_round_at        TEXT,
  last_success_at      TEXT,
  next_attempt_at      TEXT,
  consecutive_failures INTEGER NOT NULL DEFAULT 0,
  needs_relist         INTEGER NOT NULL DEFAULT 0,   -- 清单不可信，需分段/全量重列
  last_error_code      TEXT,
  last_error_at        TEXT
);

CREATE TABLE sync_remote_index (              -- 清单的本地缓存：让每轮同步免于全量下载
  account_id  TEXT NOT NULL REFERENCES sync_accounts(id) ON DELETE CASCADE,
  kind        TEXT NOT NULL CHECK (kind IN ('note','folder','attachment')),
  entity_id   TEXT NOT NULL,
  rev         INTEGER NOT NULL,
  hash12      TEXT,
  size        INTEGER,
  deleted     INTEGER NOT NULL DEFAULT 0,
  purged      INTEGER NOT NULL DEFAULT 0,
  seg         TEXT,                            -- 所属清单分段名
  updated_at  TEXT NOT NULL,
  PRIMARY KEY (account_id, kind, entity_id)
) WITHOUT ROWID;

CREATE TABLE sync_operations (                -- 持久化 outbox：崩溃可恢复
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  account_id    TEXT NOT NULL REFERENCES sync_accounts(id) ON DELETE CASCADE,
  dedupe_key    TEXT NOT NULL UNIQUE,          -- hash(entity, op, rev)
  entity_type   TEXT NOT NULL CHECK (entity_type IN ('note','folder','attachment')),
  entity_id     TEXT NOT NULL,
  op            TEXT NOT NULL CHECK (op IN ('upsert','delete','purge','upload','download')),
  payload_rev   INTEGER,
  sha256        TEXT,
  state         TEXT NOT NULL CHECK (state IN
                  ('pending','inflight','done','failed','superseded','blocked')),
  attempts      INTEGER NOT NULL DEFAULT 0,
  next_retry_at TEXT,
  last_error    TEXT,
  created_at    TEXT NOT NULL,
  updated_at    TEXT NOT NULL
);

CREATE TABLE sync_conflicts (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  account_id    TEXT NOT NULL,
  entity_type   TEXT NOT NULL,
  entity_id     TEXT NOT NULL,
  base_rev      INTEGER NOT NULL,
  local_rev     INTEGER NOT NULL,
  remote_rev    INTEGER NOT NULL,
  local_hash    TEXT NOT NULL,
  remote_hash   TEXT NOT NULL,
  auto_merged   INTEGER NOT NULL DEFAULT 0,    -- 1 = 块级合并成功，无需用户介入
  copy_note_id  TEXT REFERENCES notes(id) ON DELETE SET NULL,
  state         TEXT NOT NULL CHECK (state IN ('open','resolved','dismissed')),
  resolution    TEXT CHECK (resolution IN ('kept_both','local','remote','merged','manual')),
  created_at    TEXT NOT NULL,
  resolved_at   TEXT
);
