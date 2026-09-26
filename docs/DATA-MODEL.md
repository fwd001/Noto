# DATA-MODEL

Notera 本地数据模型与富文本规格。Phase 0 产物，Phase 1 实现依据。

关联：[SYNC-PROTOCOL.md](./SYNC-PROTOCOL.md) · [CONFLICT-RESOLUTION.md](./CONFLICT-RESOLUTION.md) · [ARCHITECTURE-MAP.md](./ARCHITECTURE-MAP.md) · ADR-0005 / 0006 / 0008 / 0012 / 0015

---

## 0. 不变式（先于任何字段）

违反其中任何一条即为 P0 缺陷，无论功能是否表现正常。

| # | 不变式 | 由谁保证 |
|---|---|---|
| I1 | 每个可同步实体拥有稳定 ID，跨设备不变，删除后永不复用 | UUIDv7 在创建设备上一次性生成 |
| I2 | 每次产生内容变化的提交，实体 `rev` 严格递增 | `rev = max(rev, remote_rev) + 1` |
| I3 | 删除是**状态**，不是行的消失；删除事实必须比数据活得更久 | `deleted_at` + `tombstones`（不自动 GC） |
| I4 | 同步判定不依赖 UI、不依赖文件 mtime、不依赖墙上时钟相等 | `rev` + `content_hash`（见 §4） |
| I5 | 派生数据（title / plain_text / fts / char_count）与权威数据在同一事务内更新 | `notera-store` 唯一写入口 |
| I6 | 校验失败（哈希不符、JSON 非法、协议版本过高）的数据永不进入权威表 | apply 前的 `verify()` 闸门 |
| I7 | 未知字段、未知节点类型、未知属性必须原样保留 | canonical 序列化 + `preserve-unknown` |
| I8 | 任何本地写入不依赖网络可达 | UI → store 直写，同步全在后台 |

> I4 明确禁止"比较修改时间决定谁新"。墙上时钟在多设备间不可信，是本项目禁止的实现方式。

---

## 1. 记法约定

| 项 | 约定 |
|---|---|
| 时间 | UTC，RFC 3339 毫秒：`2026-09-25T10:40:24.123Z`；TEXT 存储，字典序即时间序 |
| ID | UUIDv7 字符串，36 字符小写带连字符。单调有序，文件名安全（实测通过） |
| 哈希（权威） | `sha256:<64hex>` |
| 哈希（清单内） | `<12hex>`（sha256 前 12 位，仅作快速路径提示；权威记录内始终存全量） |
| 布尔 | INTEGER 0/1 |
| 枚举 | TEXT + `CHECK(... IN ...)`，可读性优先于 1 字节节省 |
| JSON | TEXT 列 + `CHECK(json_valid(col))`（SQLite 3.53.2 自带 JSON1，已实测） |
| 外键 | 每条 FK 显式声明删除行为；连接级 `PRAGMA foreign_keys=ON` |
| 命名 | 表复数 snake_case；列为语义名；索引 `idx_<表>_<列>`；视图 `v_` 前缀 |

`<12hex>` 截断的碰撞概率：20 000 条实体下约 7×10⁻⁷，且只可能造成"误判相同"→ 由权威记录全量哈希在 apply 时二次校验兜底。截断收益是每条清单条目省 52 B（实测 162→110 B/entry）。

---

## 2. 迁移机制

* 文件：`migrations/0001_init.sql` … `NNNN_name.sql`，纯 forward-only，编号单调。
* 版本：`PRAGMA user_version` 表示**已应用到的编号**（实测 1→3 顺序应用与幂等重跑均通过）。
* 每个迁移文件在**单事务**内执行（SQLite 不支持的 `ALTER` 变体在文件头显式标注为 `-- no-transaction` 并使用重建表流程）。
* 迁移前必须做物理文件备份：`notera.sqlite.pre-migration.<from_ver>`，成功后保留 1 份。
* 禁止运行时隐式 `ALTER TABLE`（CI 以 grep 闸门强制，见 CI-CD.md §Migration 契约）。
* 遇到 `user_version > 本版本支持值` → **进入只读模式并停止同步**，绝不降级写回。

Phase 1 落地文件：`0001_init` `0002_sync` `0003_search` `0004_indexes` `0005_views`。

---

## 3. 表清单

| 组 | 表 | 是否同步 | 归属 crate |
|---|---|---|---|
| 权威内容 | `folders` `notes` `note_revisions` `attachments` `note_attachments` | 是 | `notera-store` |
| 删除事实 | `tombstones` | 是 | `notera-store` |
| 同步元数据 | `sync_accounts` `sync_state` `sync_remote_index` `sync_operations` `sync_conflicts` | 否（本地推导缓存） | `notera-store` + `notera-sync` |
| 本地配置 | `settings` `meta` | 否 | `notera-config` |
| 检索 | `notes_fts`（FTS5 虚表，external content） | 否（可重建派生） | `notera-store` |

**没有 `notes` 与 `note_data` 的分离**：`doc` 直接内联在 `notes`，因为 Notera 的读模式是"列表→打开单条"，不需要为列表加载正文（列表只读派生列）。若 Phase 7 实测显示列表扫描因溢出页拖慢，再按 ADR 流程拆分为 `note_docs` 旁表——不提前设计。

---

## 4. Revision 模型（本文件最关键的一节）

### 4.1 字段

每个可同步实体携带：

```text
rev          本地/远端已知的最新修订号（权威头部）
sync_rev     本地与远端**最后一次确认一致**的 rev —— 即三方合并的 base
sync_hash    该一致点的内容哈希
remote_rev   清单中记录的服务器当前 rev
content_hash 当前 doc 的 sha256（权威）
```

> **收敛说明（Phase 0 决策）**：早期草案含 `base_rev` 与 `synced_rev` 两列。逐场景推演（push 成功、pull 成功、脏、落后、冲突）后两者在所有可达状态下恒等，故合并为单一 `sync_rev`。少一个字段 = 少一类不一致 bug。

### 4.2 规则

```text
提交本地修改:  rev ← max(rev, remote_rev) + 1      -- Lamport 式，无需协调
push 成功:     sync_rev ← rev ;  sync_hash ← content_hash
pull 生效:     rev ← remote_rev ;  doc ← 远端内容 ;  sync_rev ← rev
脏:            rev ≠ sync_rev
待拉取:        remote_rev ≠ sync_rev
冲突候选:      rev ≠ sync_rev  且  remote_rev ≠ sync_rev  且  两侧哈希均异于 sync_hash
```

### 4.3 共同祖先定义（为什么不需要向量时钟）

`sync_rev` 是**双端都确认过的内容点**，因此：

* 本地可用 `note_revisions(note_id, rev = sync_rev)` 取回 base 文档；
* 服务器侧 `rev = sync_rev` 的内容与本地 base 内容必然相同（否则当初不会被确认为一致）；
* 于是"base / local / remote"三方齐备，`max+1` 保证编号跨设备单调，无需向量时钟即可判定真并发。

代价：无法区分"谁的并发"这类因果细节——Notera 不需要，需要的是**不丢内容**（见 CONFLICT-RESOLUTION.md）。

### 4.4 保留窗口

`note_revisions` 的保留规则必须保证 base 可取：

1. `rev ≥ sync_rev` 的行**永不删除**；
2. `rev = notes.rev` 的行永不删除；
3. 其余每笔记保留最近 200 行；
4. GC 在后台执行，且永不进入用户可感知路径。

第 1 条是硬约束：违反它 = 冲突时找不回 base = 退化为"猜谁对"，直接违反 I3/数据安全优先级。

---

## 5. DDL

### 5.1 `0001_init.sql`

```sql
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
  updated_at TEXT NOT NULL,
  PRIMARY KEY (key, scope, COALESCE(account_id, ''))
) WITHOUT ROWID;
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
  sha256       TEXT PRIMARY KEY
               -- 必须是 64 位小写十六进制。注意 GLOB 匹配的是**整个**值：
               -- 写 '[0-9a-f][0-9a-f]' 会把长度限成恰好 2 字符，真实 sha256 全部插不进去。
               CHECK (length(sha256) = 64 AND sha256 NOT GLOB '*[^0-9a-f]*'),
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
```

### 5.2 `0002_sync.sql`

```sql
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
  created_at     TEXT NOT NULL,
  cap_mask       INTEGER,      -- 0006：§5 探测到的能力位图；NULL = 从未探测（≠ 0 = 全不支持）
  caps_probed_at TEXT          -- 0006：上次探测时刻，驱动"每日一次"
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
-- `entity_type` 用的是**库里长标记**（note/folder/attachment），而同步线上飘的是
-- **短标记**（n/f/a，见 SYNC-PROTOCOL §2 的路径 `/.notes/n/<id>.json`）。两套词汇之间
-- 的唯一翻译点是 `notera-host` 的适配器；`Store::outbox_settle` 因此收 `EntityKind`
-- 而不是字符串 —— 收字符串时传错词汇编译能过，只是 UPDATE 匹配 0 行、待办静默停在
-- inflight（实测坏过：设置页"待同步"永远不掉，`sync_operations` 只增不清）。
--
-- 结清按 `(account_id, entity_type, entity_id, payload_rev)` 精确定位，且只动
-- pending/inflight；匹配不到就返回 false 并留 warn，绝不"猜一行"标完成。
--
-- 「待发操作」计数（设置页那行）只统计**启用中账户**的行：`local` 哨兵账户
-- （enabled=0）是"提交即入 outbox"的留痕账，引擎永不消费它，算进来这个数永远归不了零。

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
```

### 5.3 `0003_search.sql`

```sql
CREATE VIRTUAL TABLE notes_fts USING fts5(
  title,
  plain_text,
  content='notes',
  content_rowid='rowid',
  tokenize = 'trigram case_sensitive 0'
);
-- 不使用触发器：派生文本需要 richtext→plain_text 转换，必须在 Rust 侧完成后同事务写入。
-- 维护点唯一：notera-store::SearchIndexer（见 §7）。
```

---

## 6. 同步字段 vs 本地字段

| 字段类别 | 例子 | 是否上服务器 | 冲突时 |
|---|---|---|---|
| 内容 | `doc` `folder_id` `pinned` `color` `name` `parent_id` | 是 | 走 CONFLICT-RESOLUTION |
| 元数据 | `created_at` `sha256` `size` `media_type` | 是 | 不可变，取较早者 |
| 设备态 | `sync_rev` `remote_rev` `lease_token` `seq_applied` | 否（本地推导） | — |
| UI 态 | 侧栏折叠、每设备列宽、滚动位置、排序方式（设备作用域） | 否 | — |
| 凭据 | WebDAV 密码 / token | **永不进 SQLite**，存系统钥匙串（PROXY.md §凭据） | — |

规则：**新增字段必须先归类**。归为"同步"即承诺跨设备可合并，否则不得同步。归类记录在 ARCHITECTURE-MAP.md 的字段表，并在本文件 DDL 落地。

---

## 7. 派生列与搜索

### 7.1 派生

`doc` 变更后，**同一事务内**由 `notera-richtext` 单向导出：

| 派生列 | 规则 |
|---|---|
| `title` | 第一个 heading 或第一个非空 text 节点，截 200 字符；无则空串 |
| `plain_text` | 所有 text 按块顺序拼接，块间 `\n`，去除零宽字符 |
| `summary` | `plain_text` 跳过标题取前 200 字符（列表预览用） |
| `char_count` | Unicode 码点数，排除空白 |
| `block_count` `has_attachment` | 顶层块数 / 是否存在 attachment 引用 |

派生列**只读**：任何写入路径必须经过 `Store::commit_note_edit()`（唯一写入口），绕过者由架构测试拦截。

### 7.2 检索策略（实测决定，非偏好）

Windows x64 + rusqlite 0.40.2（bundled SQLite 3.53.2）实测：

| 查询 | 路径 | 结果 | 耗时 |
|---|---|---|---|
| 3+ 字符中文 `MATCH` | FTS5 trigram | 正确 | p50 = 32 µs，p95 = 1404 µs（5000 条中文笔记，索引 6.0 MiB） |
| 2 字符中文 `MATCH` | FTS5 trigram | **静默返回 0 行** | 22 µs |
| 2 字符中文 `LIKE` | `notes.plain_text LIKE '%同步%'` | 正确（416/5000） | ≈ 4.1 ms |
| 2 字符中文 `LIKE` | `notes_fts.plain_text LIKE` | 正确（416/5000） | ≈ 5.8 ms |

结论（写死为策略，Phase 1 实现）：

```text
len(query) >= 3   → FTS5 MATCH（快，带 snippet 高亮）
len(query) <= 2   → content 表 LIKE + COLLATE NOCASE（慢一个数量级但正确）
混合长查询        → 按空格切分，每段独立选择路径，取交集
```

> **禁止**把 2 字查询直接交给 `MATCH`：那会向用户显示"没有结果"，而数据其实存在——在优先级表里属于"同步正确性"级的错误，测试矩阵必须有对应用例行（TEST-PLAN.md 功能矩阵"搜索/两字中文"）。

`LIKE` 路径需要 `plain_text` 上的扫描，5000 条 ≈ 4 ms，可接受；20 000 条需重测，超预算则引入 2 字 bigram 辅助表（Phase 7 待验证项，不提前实现）。

### 7.3 完整性

* `meta.search_generation` 每次全量重建 +1；
* `Store::verify_search()`：抽样比对 `count(FTS MATCH 高频词)` 与 `count(LIKE)`，不一致即标记需重建；
* 启动时若 `search_generation` 落后于 `max(notes.rowid)` 变化点 → 后台增量补齐；
* 重建期间旧索引继续可查（影子表 + 原子改名语义，或双表切换，Phase 1 定案）。

---

## 8. 附件生命周期

```
插入图片/文件
  ↓  取字节：前端 file input → base64，或壳里选好的 localPath（二者恰好二选一，都给/都不给都拒）
  ↓  核心算 sha256 → 落盘 <data>/attachments/xx/<sha>（tmp + rename 原子写）；单文件上限 32 MiB（读字节之前就拒）
  ↓  INSERT attachments(local_state='available', remote_state='unknown')
  ↓  INSERT note_attachments(...)
  ↓  outbox: op='upload'（键是 sha256，不是笔记 UUID）
  ↓  首次挂载翻转 notes.has_attachment → 走 commit_edit，**笔记 rev +1** 并留一条 revision
后台上传队列（独立于文本轮次，见 SYNC-PROTOCOL.md §9）
  ↓  清单里已存在同 sha → 直接 remote_state='present'，不重复上传
  ↓  上传后校验 size + 服务器 ETag → 'present'
```

* **`attach_file` 必须把推进后的新 rev 回给调用方**：编辑器手里若还是旧 rev，它随后那次自动保存
  就被判成 `stale_edit` —— 用户插一张图，看到的却是"这条笔记在别处被改动了"（端到端实测踩过）。
  正确顺序：先落编辑器自己的改动 → 核心写附件 → 接住新 rev → 才把附件块写进正文。
* **附件块本身进正文是编辑器的下一次保存做的事**：`attach_blob` 只写 `attachments` /
  `note_attachments` / 派生列，不改 `notes.doc`。显示要的字节走 `attachment_data`（按 sha 取，
  先校验是 64 位小写 hex —— 它会被拼进 blob 路径），且**只存在内存的 URL 表里**：写进块属性
  就等于写进 doc、随同步把附件在正文里再存一份 base64。

* 引用计数由 `note_attachments` 派生，**不存 `ref_count` 列**（缓存会漂移；需要时用一次索引扫描）。
* blob 物理删除条件：引用计数为 0 **且** 该 sha 无 `pending/inflight` 上传 **且** `local_state='available'`。
* 回收站内笔记仍持有引用 → 30 天窗口内 blob 不会被删。
* 下载缺失 blob（换设备）：`local_state='missing'` 且远端**没被否定**（`present` 或 `unknown`）→ 入 download 队列，UI 显示占位而非报错。
  口径为什么含 `unknown`：外来笔记登记出来的行起步就是 `unknown`（谁也没确认过），只等 `present` 就等于永远不去问服务器一次。
  404 走既有的一条路 —— 标 `absent` 后收手，不会每轮空转。
* **收到一条笔记 = 登记它引用的附件**：`apply_remote` 写笔记时在**同一事务**里把 doc 里的块级引用（`Image`/`Attachment` 及任何带
  64 位小写 hex `sha256` 的块）抄进 `attachments` + `note_attachments`。漏了这一步，第二台设备就没有下载任务、引用计数恒为 0
  （GC 敢删还在用的 blob）、按文件夹导出的附件集合也是空的。形态不合法的 `sha256` **不入库**：主键上的 CHECK 会让整批同步回滚。
* **从备份包还原走 `Store::restore_blob`，不是同步的 `ingest_blob`**：包里的字节没经过服务器，所以登记行、远端态留在
  `unknown`（→ 排进上传队列补传）。`ingest_blob` 会写 `present`，那等于宣布"服务器已经有了"，还原出来的附件就永远不再上传。

---

## 9. 删除与恢复语义

两级删除，对应两个不同用户意图：

| 用户动作 | 数据效果 | 传播 | 可逆 |
|---|---|---|---|
| 删除 | `deleted_at = now`，`rev` +1 | `upsert`（带 `deleted_at` 的记录） | 是（"最近删除"，30 天） |
| 恢复 | `deleted_at = NULL`，`rev` +1 | `upsert` | — |
| 永久删除 | 行移入 `tombstones(purged=1)`，`notes` 行删除 | `upsert`(purged) → 之后本地清行 | 否 |
| 30 天到期 | 自动等价于"永久删除" | 同上 | 否 |

关键设计：**文件夹删除不级联删除笔记**。子文件夹上移一级、笔记移入默认本，只为文件夹本身写一条 tombstone。理由：① 一次误触不应连带摧毁内容；② 级联会产生 N 条 tombstone 与 N 次远端写，把删除变成最重的操作。此决策见 ADR-0006。

`purged_at` 与 tombstone 必须传播，否则会出现"设备 A 永久删除 → 设备 B 从缓存重新上传 → 笔记复活"（§10 明令禁止的复活路径）。

---

## 10. 富文本模型（权威内容格式）

### 10.1 原则

* 内部格式**不是 HTML**，是与编辑器无关的块级树；HTML/Markdown 只是导出视图。
* 换编辑器不得触碰同步层（I7 + 版本闸门保证）。
* 每个顶层块拥有稳定 `id`，块级三方合并依赖它（见 CONFLICT-RESOLUTION.md §3）。

### 10.2 结构

```json
{ "v": 1, "content": [ Block ] }
```

```text
Block   = { id: string(8..32), type, attrs?, content?: Inline[] }

type    = paragraph | heading | blockquote | codeBlock
        | orderedList | bulletList | checklistItem | taskSection?   (列表为扁平 + indent 属性)
        | image | attachment | horizontalRule | table | tableRow | tableCell
        | unknown:*                                                  (前向兼容容器)

Inline  = { text: string, marks?: Mark[] }

Mark    = bold | italic | underline | strike | code | highlight{color}
        | link{href, rel?} | fontSize{n} | color{hex} | attachmentRef{sha256,role}
```

Block `attrs`（节选）：`level`(heading 1–6)、`lang`(codeBlock)、`checked`(checklist bool)、`indent`(0–8)、`align`、`ref`+`sha256`(image/attachment)、`width`、`alt`。

### 10.3 规范化与稳定序列化

写入时按顺序执行，任一失败即拒绝提交（不写坏数据）：

1. `normalize()`：拆零宽字符、剥离空 marks、丢弃无 text 的空 inline、按 `id` 去重、补齐必备 attr 默认值；
2. `validate()`：类型合法、嵌套合法（如 `tableCell` 不得直接含 `table`）、`id` 文档内唯一、`v ≤ 支持版本`；
3. `canonical()`：对象键按 Unicode 码点升序、无数组顺序变化、无无意义空白、UTF-8、**整数不用浮点表示**；
4. `content_hash = sha256(canonical(doc))`。

实测：同一逻辑文档在插入顺序不同的 map 下 canonical 输出一致（`{"a":2,"m":{"b":2,"k":1},"z":1}`）。

### 10.4 前向兼容（防止旧客户端摧毁新内容）

* 未知 `type` → 整体保留进 `unknown:<type>` 容器，原样写回；
* 未知 `attrs` 键 → 保留；
* `doc.v > 本客户端支持版本` → 笔记**只读打开**，禁止任何写回，UI 提示"请升级以编辑"；
* 未知 mark → 保留并降级渲染（不丢样式数据）。

这条规则是"换编辑器/灰度升级"期间的数据保险，测试矩阵必须有对应行（TEST-PLAN.md 富文本矩阵"未知节点往返"）。

---

## 11. 记录信封（与同步层的边界）

`notes.doc` 是明文文档；**信封只在离开本机时构造**（详见 SYNC-PROTOCOL.md §5）：

```json
{ "protocol":1, "kind":"note", "id":"<uuid>", "rev":7, "sync_rev":6,
  "hash":"sha256:<64hex>", "updated_at":"…Z", "device":"<uuid>",
  "deleted_at":null, "purged":false,
  "enc": { "alg":"none" },
  "payload": { "v":1, "content":[ … ] },
  "ct": null }
```

`enc.alg ∈ {"none","aes-256-gcm-siv"}`；`alg="none"` 时 `payload` 非空、`ct` 为 null。启用 E2EE 时反之。Phase 0 已定：v1 即带 `enc` 字段，未来开启加密是**次版本**而非破坏性重写（ADR-0002）。实测：AES-256-GCM-SIV nonce 12 B、认证标签固定 16 B、篡改必被拒。

---

## 12. 连接与并发模型

```text
PRAGMA journal_mode = WAL            (实测生效)
PRAGMA synchronous  = FULL           -- 数据安全 > 性能；自动保存已 debounce，可承受
PRAGMA foreign_keys = ON             (实测生效；注意它是连接级)
PRAGMA busy_timeout = 5000
PRAGMA cache_size   = -16384         -- 16 MiB
```

* **单写者**：一个专用写线程持有写连接；读用连接池（WAL 允许读者与写者并发）。
* 一次用户编辑 = 一个事务：`notes` + `note_revisions` + 派生列 + `notes_fts` + `sync_operations`。任何一处失败整体回滚。
* 自动保存：停止输入 1200 ms 或失焦或窗口关闭请求时提交；提交即入 outbox，**不等网络**。
* 崩溃恢复：WAL 检查点 + 事务原子性保证不出现半条笔记（实测：committed/rolled-back 分离正确）。
* `synchronous=FULL` 的实测代价需在 Phase 1 出数字；若单次提交 > 16 ms，再评估"仅删除/覆盖类操作 FULL、普通编辑 NORMAL"的分层方案（需 ADR）。

---

## 13. 常用视图

```sql
-- 侧栏列表：不读 doc，避免正文溢出页拖慢
CREATE VIEW v_note_list AS
SELECT n.id, n.folder_id, f.name AS folder_name, n.title, n.summary, n.pinned,
       n.updated_at, n.char_count, n.has_attachment,
       (n.rev <> n.sync_rev) AS dirty
FROM notes n JOIN folders f ON f.id = n.folder_id
WHERE n.deleted_at IS NULL;

CREATE VIEW v_trash AS
SELECT id, title, deleted_at,
       CAST(julianday('now') - julianday(deleted_at) AS INTEGER) AS days_left
FROM notes WHERE deleted_at IS NOT NULL;

-- 文件夹树：递归 CTE，path 为派生展示值，绝不参与同步语义
CREATE VIEW v_folder_tree AS
WITH RECURSIVE tree(id, parent_id, name, depth, path) AS (
  SELECT id, parent_id, name, 0, '/' || name FROM folders
    WHERE parent_id IS NULL AND deleted_at IS NULL
  UNION ALL
  SELECT f.id, f.parent_id, f.name, t.depth + 1, t.path || '/' || f.name
  FROM folders f JOIN tree t ON f.parent_id = t.id WHERE f.deleted_at IS NULL
)
SELECT * FROM tree;
```

> `parent_id IS NULL` 允许存在多棵树（未来多账户/多分区需要），不强制单根：单根约束会让首次引导多出一个"伪文件夹"概念，污染 UX。

---

## 14. 容量与性能预算

| 指标 | 目标 | 现有证据 |
|---|---|---|
| 冷启动 → 可输入 | ≤ 400 ms（桌面 SSD） | 待 Phase 4 实测；架构禁止启动路径等待网络 |
| 5000 条笔记搜索 p95 | ≤ 20 ms | 实测 1.4 ms（≥3 字）/ 4.1 ms（2 字 LIKE） |
| 20 000 条笔记搜索 p95 | ≤ 60 ms | 待 Phase 7 实测（2 字 LIKE 线性外推 ≈ 17 ms） |
| 单库体积（20 000 条纯文本笔记） | ≤ 120 MiB | FTS 索引外推：5000 条 6.0 MiB → 20 000 条 ≈ 24 MiB |
| 一次自动保存提交 | ≤ 16 ms | 待测（`synchronous=FULL` 影响未知） |
| 清单缓存条目成本 | 89 B/条（gzip 后摊薄） | 实测 |
| 后台同步 CPU 占用（空轮） | ≈ 0 | 依赖 ETag 304 快路径，待测 |

所有"待测"项必须在对应 Phase 出口前给出实测数字，不接受"应该够快"。

---

## 15. 导出 / 导入 / 备份

* **导出**：自描述 ZIP —— `manifest.json`（含 `exported_at`、`app_version`、`protocol`、`counts`、`partial`）+ `notes/<id>.json`（信封）+ `folders.json` + `attachments/<sha>` + `tombstones.json`。导入时可勾选"保留删除事实"，默认保留（防复活）。
* **按文件夹导出 = 两个集合两种用途**：`Store::folder_closure`（子树 + 祖先链）只决定哪些**文件夹行**进包（祖先只是外键骨架，缺了就是导不回去的废包）；`Store::folder_subtree`（子树，不含祖先）决定哪些**内容**算这一棵 —— 笔记按父本是否在子树里筛，附件按这些笔记筛。曾用同一个闭包筛内容，勾一个子层就会把祖先（往往是默认本）里那篇无关笔记连它的附件字节一起带走。包必须自己声明
  `partial: true` —— `tombstones` 不记父本，笔记的**永久删除公告无法归属到文件夹**，因此这种包**禁止**用于"仅在空库时导入"
  的整库还原（那样一导，之后与服务器同步时已删的笔记会被别人的副本带回来，违反 §8 硬性要求 6）。`partial` 带
  `#[serde(default)]`：老包缺这个键也必须读得回来，升级把自己的旧备份读坏等于我们自己制造数据丢失。
* **导入**：视为一次 `bootstrap_push` —— 目标库为空则直接落库；非空则**逐条冲突求解**，绝不静默覆盖（I3/I6）。ID 冲突但内容不同 → 生成副本并记 `sync_conflicts`。
* **备份**：`VACUUM INTO` —— 与 Online Backup API 同等的**一致单文件快照**（含未 checkpoint 的 WAL 内容，实测过），但不需要给 `rusqlite` 开 `backup` 特性。产物含 `sha256` 校验与 `user_version`。同一秒内重复备份各得一份，绝不覆盖已有文件。
* **恢复**：备份文件 → 校验哈希 → 校验 `user_version ≤ 当前支持` → 校验 `integrity_check` → **替换前先留一份当前库**（`notera.sqlite.pre-restore.<ver>`）→ 原子替换（先写 `.restoring` 再 rename）→ 启动自检。
  * 落地时机是**下次启动** `Store::open` 之前，而不是进程内换库：`Store` 活在 `Arc<Inner>` 里，进程内替换等于重构最共享的对象。UI 点"恢复"因此只回"已排期，重启生效"。
  * 校验用的连接必须是真只读（`SQLITE_OPEN_READ_ONLY`）：走常规连接池会执行 `PRAGMA journal_mode=WAL`，把**待校验的备份就地改写**并留下 `-wal` 边车 —— 那样"校验"这一步本身就破坏了被校验的东西。
* 验收：创建 → 导出 → 删除 → 重新导入 → 内容哈希逐条一致（TEST-PLAN.md 功能矩阵"导出/导入/恢复"）。

---

## 16. 本阶段未决

| 项 | 状态 | 需要 |
|---|---|---|
| 本地静态加密（SQLCipher） | 未决 | 若公安网要求"落盘必须加密"，需 ADR 并替换 SQLite 方案；与 E2EE 信封是两件事 |
| `synchronous=FULL` 的提交延迟 | 待测 | Phase 1 基准 |
| 2 字查询在 20 000 条下的成本 | 待测 | Phase 7 基准；可能触发 bigram 辅助表 ADR |
| 表格块（table/tableRow/tableCell） | 已入模型，Phase 1 后实现 | 需产品确认是否 v1 需要 |
| `notes.doc` 旁表拆分 | 不做（§3 说明） | 列表性能出现实测退化时再议 |
