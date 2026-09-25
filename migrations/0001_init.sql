-- 0001_init.sql —— 权威内容表（DATA-MODEL.md §5.1 逐字落地）
-- forward-only（ADR-0012）。本文件在单事务内执行，失败整体回滚且不推进 user_version。
--
-- 与 §5.1 的唯一偏离（已在此显式记录，见 notera-store 交付说明）：
--   `settings` 的 `PRIMARY KEY (key, scope, COALESCE(account_id, ''))` 无法建表——
--   SQLite 报 "expressions prohibited in PRIMARY KEY and UNIQUE constraints"
--   （实测 SQLite 3.49/3.53 均拒绝，且生成列也不得进主键）。
--   为保持**语义等价**：列定义原样保留（account_id 仍可空），把该主键改为
--   表达式唯一索引 `UNIQUE (key, scope, COALESCE(account_id,''))`，并去掉
--   WITHOUT ROWID（表达式索引只能建在 rowid 表上）。
--   唯一性、NULL 与 '' 同键这两条契约行为均与 §5.1 一致。

-- 本地自描述，永不参与同步
CREATE TABLE meta (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
-- 使用的 key: device_id, install_id, cached_root_id, last_migration_from,
--             search_generation, store_created_at

CREATE TABLE settings (
  key        TEXT NOT NULL,
  scope      TEXT NOT NULL CHECK (scope IN ('global','ui','device','account')),
  account_id TEXT,
  value      TEXT NOT NULL CHECK (json_valid(value)),
  updated_at TEXT NOT NULL
);
CREATE UNIQUE INDEX idx_settings_key_scope_account
  ON settings(key, scope, COALESCE(account_id, ''));
-- scope=device 的项（侧栏折叠、每设备排序方式）永不上传

CREATE TABLE folders (
  id             TEXT PRIMARY KEY CHECK (length(id) = 36),
  parent_id      TEXT REFERENCES folders(id) ON DELETE RESTRICT,
  name           TEXT NOT NULL,
  color          TEXT,
  system_kind    TEXT CHECK (system_kind IN ('default')),      -- 内置默认本，不可删/移/改名
  sort_order     INTEGER NOT NULL DEFAULT 0,
  rev            INTEGER NOT NULL DEFAULT 1,
  sync_rev       INTEGER NOT NULL DEFAULT 0,
  sync_hash      TEXT,
  remote_rev     INTEGER NOT NULL DEFAULT 0,
  content_hash   TEXT NOT NULL,
  created_at     TEXT NOT NULL,
  updated_at     TEXT NOT NULL,
  deleted_at     TEXT,
  purged_at      TEXT,
  created_device TEXT NOT NULL,
  updated_device TEXT NOT NULL,
  CHECK (NOT (system_kind IS NOT NULL AND deleted_at IS NOT NULL))  -- 系统本永不删除
);

CREATE TABLE notes (
  id             TEXT PRIMARY KEY CHECK (length(id) = 36),
  folder_id      TEXT NOT NULL REFERENCES folders(id) ON DELETE RESTRICT,
  doc            TEXT NOT NULL CHECK (json_valid(doc)),   -- 权威内容：RichText Document
  doc_format     INTEGER NOT NULL DEFAULT 1 CHECK (doc_format >= 1),
  pinned         INTEGER NOT NULL DEFAULT 0,
  color          TEXT,
  title          TEXT NOT NULL DEFAULT '',                -- 派生
  plain_text     TEXT NOT NULL DEFAULT '',                -- 派生
  summary        TEXT NOT NULL DEFAULT '',                -- 派生（列表预览）
  char_count     INTEGER NOT NULL DEFAULT 0,              -- 派生
  block_count    INTEGER NOT NULL DEFAULT 0,              -- 派生
  has_attachment INTEGER NOT NULL DEFAULT 0,              -- 派生
  rev            INTEGER NOT NULL DEFAULT 1,
  sync_rev       INTEGER NOT NULL DEFAULT 0,
  sync_hash      TEXT,
  remote_rev     INTEGER NOT NULL DEFAULT 0,
  content_hash   TEXT NOT NULL CHECK (content_hash GLOB 'sha256:[0-9a-f]*'),
  created_at     TEXT NOT NULL,
  updated_at     TEXT NOT NULL,
  deleted_at     TEXT,                                    -- 软删 = “最近删除”
  purged_at      TEXT,                                    -- 永久删除已传播
  created_device TEXT NOT NULL,
  updated_device TEXT NOT NULL,
  -- 打开中的笔记不得带 deleted_at：由应用层闸门 + 下列约束共同保证
  CHECK (doc_format >= 1),
  CHECK (NOT (deleted_at IS NOT NULL AND purged_at IS NOT NULL AND sync_rev > rev))
);

CREATE TABLE note_revisions (
  note_id      TEXT NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
  rev          INTEGER NOT NULL,
  doc          TEXT NOT NULL CHECK (json_valid(doc)),
  content_hash TEXT NOT NULL,
  origin       TEXT NOT NULL CHECK (origin IN ('local','remote','merged','conflict_copy','restored')),
  device_id    TEXT NOT NULL,
  created_at   TEXT NOT NULL,
  archived     INTEGER NOT NULL DEFAULT 0,   -- GC 候选标记（见 §4.4）
  PRIMARY KEY (note_id, rev)
) WITHOUT ROWID;

CREATE TABLE attachments (
  sha256       TEXT PRIMARY KEY CHECK (sha256 GLOB '[0-9a-f][0-9a-f]'),  -- 完整 64hex
  size         INTEGER NOT NULL CHECK (size >= 0),
  media_type   TEXT NOT NULL,
  filename     TEXT,
  width        INTEGER,          -- 图像元数据，可空
  height       INTEGER,
  duration_ms  INTEGER,          -- 音视频，可空
  local_state  TEXT NOT NULL CHECK (local_state IN ('missing','available','partial','error')),
  remote_state TEXT NOT NULL CHECK (remote_state IN ('unknown','absent','present','error')),
  verified_at  TEXT,             -- 上次本地内容哈希核对成功时间
  created_at   TEXT NOT NULL,
  deleted_at   TEXT
);
-- 内容寻址：blob 落盘路径由 sha256 推导，无独立 path 列
--   <data_dir>/attachments/<sha256[0..2]>/<sha256>

CREATE TABLE note_attachments (
  note_id  TEXT NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
  sha256   TEXT NOT NULL REFERENCES attachments(sha256) ON DELETE RESTRICT,
  block_id TEXT NOT NULL,        -- 指向 doc 中的 image/attachment 节点 id
  role     TEXT NOT NULL CHECK (role IN ('inline','file')),
  alt      TEXT,
  position INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (note_id, block_id)
) WITHOUT ROWID;

CREATE TABLE tombstones (
  entity_type   TEXT NOT NULL CHECK (entity_type IN ('note','folder','attachment')),
  entity_id     TEXT NOT NULL,
  rev           INTEGER NOT NULL,
  deleted_at    TEXT NOT NULL,
  purged        INTEGER NOT NULL DEFAULT 0,
  content_hash  TEXT,
  device_id     TEXT NOT NULL,
  title_snap    TEXT,            -- 便于向用户解释“这条曾经是什么”
  created_at    TEXT NOT NULL,
  PRIMARY KEY (entity_type, entity_id)
) WITHOUT ROWID;
-- 永久删除后 notes/folders 行消失，删除事实只存在于本表。
-- 本表行不自动 GC：见 ADR-0006。
