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
remote_rev   清单中记录的服务器当前 rev（**按实**：每轮远端视图落盘时单调写回，见 ADR-0021 D1）
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
  -- 0007 补了 deleted_at：引擎的远端视图带删除时间戳，P8/P11 那类"删除先后、删后又改"
  -- 的判据要读得回来。只有 deleted/purged 两个布尔位时，把视图持久化下去是有损的 ——
  -- 所以先补列，然后才让引擎真的往里写。
  account_id  TEXT NOT NULL REFERENCES sync_accounts(id) ON DELETE CASCADE,
  kind        TEXT NOT NULL CHECK (kind IN ('note','folder','attachment')),
  entity_id   TEXT NOT NULL,
  rev         INTEGER NOT NULL,
  hash12      TEXT,
  size        INTEGER,
  deleted     INTEGER NOT NULL DEFAULT 0,
  purged      INTEGER NOT NULL DEFAULT 0,
  seg         TEXT,                            -- 所属清单分段名
  deleted_at  TEXT,                            -- 远端声明的删除时间（0007 补；见下）
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
  resolved_at   TEXT,
  -- 迁移 0008：服务器那一版的**原始记录信封**（不是摘要）。NULL = 这一轮没取回来
  -- （请求预算用尽 / 记录 404 / 网络失败）—— 界面退回显示哈希并照实说明，
  -- 冲突本身绝不因为取料失败而消失。见 CONFLICT-RESOLUTION §5.1.1。
  remote_wire   TEXT
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
| 凭据 | WebDAV 密码 / token | **永不进 SQLite**。0.0.29 起真的进系统凭据库（Windows 凭据管理器，目标名 `notera:webdav:<账户 id>`，代理那条是 `notera:proxy:<账户 id>`，ADR-0020）；配置里那列 `credential_ref` 的语义因此收紧成**「系统里真有一条」**——只有写成功才落引用，落配置失败还要把刚存的抹掉。旧实现是「口令收到就丢掉、引用照写」，于是界面的「口令已设置」是假的，而发布版永远拿不到凭据 | — |

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
  ↓  清单/HEAD 判存说已有 → 跳过上传（省一次搬运；这是**提示**不是收据，见 SYNC-PROTOCOL §13）
  ↓  tmp → MOVE → **整份读回来复算 sha256** → 'present'（判据只有内容哈希，服务器发什么 ETag 都算不上数）
```

* **`attach_file` 必须把推进后的新 rev 回给调用方**：编辑器手里若还是旧 rev，它随后那次自动保存
  就被判成 `stale_edit` —— 用户插一张图，看到的却是"这条笔记在别处被改动了"（端到端实测踩过）。
  正确顺序：先落编辑器自己的改动 → 核心写附件 → 接住新 rev → 才把附件块写进正文。
* **附件块本身进正文是编辑器的下一次保存做的事**：`attach_blob` 只写 `attachments` /
  `note_attachments` / 派生列，不改 `notes.doc`。显示要的字节走 `attachment_data`（按 sha 取，
  先校验是 64 位小写 hex —— 它会被拼进 blob 路径），且**只存在内存的 URL 表里**：写进块属性
  就等于写进 doc、随同步把附件在正文里再存一份 base64。

* 引用计数由 `note_attachments` 派生，**不存 `ref_count` 列**（缓存会漂移；需要时用一次索引扫描）。
* **blob 回收（GC）：安全的那一半已实现（2026-09-28，用户决定第 2 条；§48 缺口 G1 就此收口）**。
  它是本仓唯一一段"代码主动让用户的字节离开磁盘"的逻辑，所以销毁被拆成两步，中间留一段可撤销的现场：
  1. **隔离**（`App::reclaim_unreferenced_blobs`，每轮上界 200 条）：把 `<attachments>/xx/<sha>`
     **挪**进 `<data>/attachments-quarantine/xx/<sha>`（同一套两层寻址，只差根目录），账上写
     `local_state='missing'` + `deleted_at=now`。字节一个都不少，只是换了地方。
     准入三条，每一条挡一类数据丢失：
     * `NOT EXISTS note_attachments` —— 零引用才算。**回收站里的笔记仍算引用**（笔记行还在，链接就在），
       只有永久删除（`notes` 被 CASCADE 带走链接）才归零。
     * `remote_state='present'` —— 只收"服务器已确认有副本"的行。没传上去过的字节是本机独家的一份，
       而隔离会把它同时从**上传队列**里摘掉（那条队列也带 `deleted_at IS NULL`），于是"省磁盘"就变成
       "判死一份独家副本"。数据安全排在性能前面：这类行今天不收，它照常走上传队列，传成功了才变成候选。
     * `local_state='available'` —— 只有"账上说本机有"的行才谈得上回收字节。
     认领下来之后，这一行剩余的附件待办在**同一个写事务**里一起结掉，状态含 `failed`（0.0.24，判据
     FT-ATT-35）：`outbox_pending` 的口径本来就数 `('pending','inflight','failed')`，而这一行已经离开
     两个队列的取活范围 —— 留下一条没人再会消费的 `failed`，界面上就是"还有 1 项待发"永远不掉。
     范围只卡在本轮真被隔离的那些 sha：仍被引用的行那条 `failed` 一字不动，GC 不是替用户
     吞掉同步失败的那只手（这一点与引擎的 `outbox_settle` 相反，那条**不许**动 `failed`）。
* **附件待办的生命周期：`failed` 不是"有人在管的待重试"，而是一条没有消费者的账**（0.0.26，判据 FT-ATT-38）。
  以前 `outbox_settle` 的注释写着 failed 行"归退避逻辑管"，这是**错的**：文本引擎按 `local_views` 规划、
  附件轮按 `attachments` 的状态挑活，而 `Store::outbox_take` 在生产里没有调用方 —— 没有任何东西会回去重试
  一支 failed。`outbox_pending` 的口径又含 `failed`（`Store::stats`），所以它是一笔永远不会掉的数。
  三条真实出路写清楚：**下一次变更把它重新入队**（`rows::enqueue` 的 `ON CONFLICT(dedupe_key)` 把状态打回
  `pending`）、**附件侧两个收口器**（`settle_satisfied_attachment_ops` 只收"状态已满足"的，
  `mark_attachments_quarantined` 只收"本轮真被 GC 认领的"）、**用户点界面上的重试动作**。
  两个收口器的方向都是双向的，各挡一种错法：状态还没满足就去结 = 把用户真实的失败藏起来；
  状态已经满足了还不结 = 计数永久虚高。
  2. **真删**（`App::confirm_still_remote(remote, cutoff, cap)` + `App::purge_verified_blobs(&verified)`，
     同一套上界）：`deleted_at` 早于宽限期界
     （**常量 30 天**，`quarantine_cutoff` 算，`cutoff` 由调用方给，这样"到期没有"是可核对的而不是
     藏在 SQL 里的一个数）、**仍然**零引用、而且**这一轮刚从服务器问到的"还在"**，三者同时成立才动手。
     * 为什么要第三次确认（0.0.25，判据 FT-ATT-36）：账上那个 `remote_state='present'` 是**隔离那一刻**
       写下的结论，而隔离到销毁之间隔着整个宽限期。"本机这份是仅存的一份"是可达的 —— 别的设备可能还在
       画这张图（引用在对面那台的账上，本机这条 SQL 看不见），服务器也可能在中间被清过一段。
       销毁是本仓唯一不可逆的那一步，它的凭据必须是当下的事实，不能是 30 天前的一个信念。
     * 三种回答分工不同，这一条与 §27 早就划好的那条边界接得上：**HEAD 200** = 放行；
       **HEAD 404** = 关于远端事实的结论 → 一个字节都不碰，把这一行记成 `absent`；
       **问不到**（5xx / 超时 / 连接断了）= **不下结论**，账与字节都不动，下一轮再问。
       把 503 当成"没有"就是拿猜代替问，而猜错的代价是不可恢复的。
     * 判过的结论落在账上就不再每轮重问：`gc_ready_to_purge` 只收 `remote_state='present'` 的行，
       所以写成 `absent` 的那一行自己离开候选集（§27/§28 那句"不许 20 秒一轮敲同一扇门"在这一格的样子）。
       **代价照实写着**：这一格那份字节从此不会由 GC 销毁 —— 它是本机仅存的一份，而"服务器到底有没有"
       这个事实只有服务器能回答。用户点「重新上传本机这份」把它换回 `error`、真上传成功后回到 `present`，
       才重新变成候选；或者引用回来了，本地恢复那一步把它接回正式位置。宁可多占磁盘，不可少问一句。
     * 动手那一步的顺序没变：先 `DELETE` 那一行（`WHERE NOT EXISTS` 自己
       问第二遍，`note_attachments.sha256` 上的 `ON DELETE RESTRICT` 是机器兜底），**只有真删掉行的那些
       sha** 才去删隔离区里的文件。顺序反过来一旦断电就留下"账在、字节没了"的死链；按这个顺序，最坏残留
       是"行没了、字节多占一份"，那是可安全重跑的清理，不是数据损坏。
     * **"问"与"动手"分成两个函数**是有意的：生产里唯一的调用方是 `run_attachment_round`，它把前者的
       返回值原样交给后者；而 GC 的规模基准（`attachment_gc_scale.rs`）没有服务器，它量的是本地那两步
       （删行 + 删文件）的代价，**不含** HEAD 那一问 —— 读那个数字的人需要知道它没包含什么。
  * **顺序是"先挪字节、后写账"**：反过来若挪失败（跨卷、被占用）而账已写成"已隔离"，这一行就成了
    两边都不认领的死角（三个后台口径都被 `deleted_at` 挡住）。按现在的顺序，"挪成功而写账被拒"的残留是
    正式位置空了而账上还写 `available` —— 读侧回 `attachment_missing`，磁盘体检把它降级，下载那一轮
    先看隔离区就把字节挪回来了。它与体检是**一台状态机**的两半，不是两个抢同一行的循环。
    **这个窗口不是靠推理站住的**：`after_quarantine_move` 是一个真的崩溃注入点（子进程用
    `process::exit` 死在"挪与写之间"，不是 panic 所以不跑析构），重启后一轮附件轮必须自己把字节
    放回正式位置、把账纠回 `available`、且**为该 sha 一次请求都不发**，再一轮 GC 才正常认领并写上账
    —— 也就是**收敛到设计里的状态**，而不是在"补/收"之间来回摆。判据见 TEST-PLAN 的 CI-CRASH-11。
  * **搬那一步不许"先腾位置"**（0.0.27）：隔离区的目的地 `<quarantine>/<2hex>/<sha>` 是**两家共用的
    同一个落点**，而 `rename` 自己就会原子替换目标（Unix 与 Windows 都是），所以搬之前那一句
    `remove_file(to)` 除了制造窗口没有别的作用 —— 当对手（另一个 OS 进程，或 GC 与「重试取回」在
    同一份字节上交错）已经把它那份**唯一**的字节放进落点、而我的源文件正好在它手里没了的时候，
    "腾位置"删掉的就是那份仅存的东西，紧接着我自己的 rename 因为源不存在而失败。两个位置同时空着，
    账上却写着"已隔离"。判据 FT-ATT-39（真两进程在同一数据目录上抢同一批 sha，25 份重叠，
    红在"字节既不在正式位置也不在隔离区"），修前实测 4/5 轮命中、单轮最多 14 份。
    这条也顺带把"跨进程只有同进程两 store 的证据"那一格换成了现场：这个项目**没有单实例锁**
    （`store/pool.rs` 只有 `PRAGMA busy_timeout`），"应用被开了两次"是一个能发生的形状。
  * **撤销期内的恢复走本地，一次网络都不打**：重新引用同一 sha（外来笔记登记 / 备份包还原 / 再插一次图）
    会把 `deleted_at` 清回 NULL 并按盘上事实写 `missing`，于是这一行重新进下载队列；下载那一轮在做任何
    网络动作之前先 `App::restore_quarantined_blob` —— 读隔离区那份、**复算 sha256**、对得上才挪回正式位置，
    并把账写回 `available`（`set_attachment_states` 里那条 `deleted_at = CASE WHEN 'available' THEN NULL`
    就是为这一步存在的：不清掉标记，这份字节永远排不进上传队列）。跳过那次拉取有**读侧的持久来源**
    （隔离区里那个文件本身），不是猜的。落回正式位置后隔离区那份同名副本被清掉，同一份字节不许占两处。
  * 手动那颗「重试取回」也认隔离区（`App::retry_attachment` 先试本地恢复）。如果隔离区里那份**已经不可用**
    （位腐、或那个目录被磁盘清理连根删了），本地补回失败时核心会 `release_attachment_quarantine` **撤掉标记**
    —— 带着标记的行在三个后台口径里都是隐形的，只改远端态 + 入队的结果是：用户点了按钮、一次请求都不会发、
    而那条待办永远关不掉。撤标记的代价（宽限期结束）正是用户在那一刻要的东西。
  * 每轮量有上界的理由与体检同一条（§48 G4）：用户刚清空回收站时零引用的行可以有成百上千，一轮搬完就是把
    常驻循环那一格变成一段长 IO + 一段长写锁，而用户那次保存正排在后面。被认领过的行**立刻**离开候选集
    （`deleted_at` 有值 + 不再是 `available`），按 `sha256` 稳定排序取前 N 条不会饿死后面的行。
  * **仍然没实现的那一半，按 §40 记着**：
    * 远端（服务器上）孤儿对象的回收没做 —— 只回收本机盘。
    * 回收的触发点在附件轮里（常驻 20 s 一轮），所以**没配同步账户的设备不会跑 GC**：
      影响是本机盘上零引用的 blob 仍然只增不减；解除条件是把它挂到一个与同步无关的清理节律上。
    * **登记现在覆盖四条写路径，prune 仍然没有**：`note_attachments` 由 `attach_blob`（挂载）、
      本机 `create_note` / `edit_note`（走 `commit_edit`）与外来笔记的 apply 三处从 **doc 派生**插入，
      只有笔记行被删除（CASCADE）才减少。本机编辑不 prune（`apply.rs` 那侧同样只登记不 prune）。
      影响是 GC 的实际收益目前集中在"永久删除笔记"这一类，"从笔记里删掉一张图"那一类要等链接表
      随 doc 一起重算；判据因此是**偏保守**的（宁可少收，绝不错收）。
      **方向不能反**这件事在 0.0.22 之前是假的：本机 create/edit 那两条路当时不登记，而它们在生产里
      每天都在走（冲突副本是拿服务器那一版的正文直接 `create_note` 的，采纳冲突 / 把图片块搬进另一条
      笔记走的是 `edit_note`），于是"正文引用着、账上没链接"是可达状态，而 GC 把零引用当作
      "这份字节可以销毁"的**唯一**判据 —— 少算引用就是丢数据。判据：FT-ATT-34。
    * 断电在"删行"与"删字节"之间留下的孤儿文件不自动清（清它就没有宽限期依据了）：影响是隔离区
      可能多占一份，下一轮该行再被认领时会被同名覆盖顺带抹平。
    * **隔离区里的同名覆盖不复算内容**（`quarantine_move` 遇到同名文件先删再挪；重新挂载/收下同一份字节时
      也会把隔离区那份清掉）。这里删得掉的理由是**目录准入**，不是"内容寻址所以内容一定一样"那句空话：
      这个目录里的文件只可能由 GC 的候选集写入，而候选集要求 `remote_state='present'` —— 服务器上有副本，
      所以被覆盖/清掉的那一份**永远不会是唯一的一份**（本机坏字节走 `<blobs>/<sha>.corrupt` 那个后缀，
      不在这个目录里，也碰不到）。剩下的窗口是"正式位置那份同长度位腐、隔离区那份是好的"：不造成
      不可恢复的丢失，代价是多一次下载，而读侧的哈希校验保证坏的那份不会被画进界面。
* 回收站内笔记仍持有引用 → 它引用的 blob 不会被回收（回收站**没有到期日**，见上面的两级删除表：
  今天放着不管不会自动变成"永久删除"，所以那条 blob 会一直算被引用，直到用户真的点永久删除）。
* 下载缺失 blob（换设备）：`local_state='missing'` 且远端**没被否定**（`present` 或 `unknown`）→ 入 download 队列，UI 显示占位而非报错。
  口径为什么含 `unknown`：外来笔记登记出来的行起步就是 `unknown`（谁也没确认过），只等 `present` 就等于永远不去问服务器一次。
  404 走既有的一条路 —— 标 `absent` 后收手，不会每轮空转。
* **`absent` / `error` 只能由用户的意图重开（2026-09-28 的决定，两条命令）**：收手换来的是"不空转"，
  代价是那一格永远不会自愈，而用户手里没有任何能点的东西 —— 所以界面在占位上给两颗按钮，命令面是
  `attachment_retry` 与 `attachment_reupload`（`App::retry_attachment` / `App::reupload_attachment`）。
  * 「重试取回」把 `remote_state` 从 `absent`/`error` 撤成 `unknown`，并补一条 download 待办
    （只改行不落待办的话，界面上那个"待同步"不动，用户会以为点了没反应）。**`local_state` 一字不动**
    —— 本机有没有那份字节是磁盘上的事实，不是意见。本机已 `available` 时它回 `nothing_to_retry`：
    对着一次不会发生的下载说"重试成功"，比报错更坏。
  * 「重新上传本机这份」把 `remote_state` 写成 **`error`**，这就是本仓第二个写 `error` 的地方
    （第一个是下载复验不符）。为什么用账而不是内存标志：上传队列的远端口径本来就含 `error`
    （不用新加一条队列），而 `run_attachment_round` 对 `error` 行**不许**走 HEAD 跳过、要走覆盖式
    MOVE + 复读（SYNC-PROTOCOL §13 那条例外就此有了边界）；落账意味着进程重启、这台设备离线、
    这一轮 64 MiB 预算用完，用户点的那一下都还在。前置是本机那份真在且哈希就是它 —— 否则
    `nothing_to_upload`（把对不上号的字节传上去会污染所有引用同一 sha 的笔记）。
  * 三个失败码（`attachment_not_registered` / `nothing_to_retry` / `nothing_to_upload`）各有自己的文案，
    因为这三格看起来一样、用户下一步该做的事不一样。判据分别见 FT-ATT-25 / 26 / 27 / 28。
* **`absent`/`error` 是收手的结论，只能由**用户的一次意图**重开（2026-09-28 补，决定于"要不要给手动动作"那一问）**：
  这两格里那两个后台动作（`App::retry_attachment` / `App::reupload_attachment`，命令面 `attachment_retry` /
  `attachment_reupload`）各自只做一件事 —— 改这一张表，然后催一轮同步；下载与覆盖仍由既有的附件轮去做，
  **没有第二条传输路径**。
  * 「重试取回」把 `remote_state` 从 `absent`/`error` 撤成 `unknown`，并补一条 download 待办（界面上那个
    "待同步"要有动静）。`local_state` **一字不动** —— 本机有没有那份字节是磁盘上的事实，不是意见。
    本机已经 `available` 时它拒绝（`nothing_to_retry`）：说"重试成功"会让用户等一次不会发生的下载。
  * 「重新上传本机这份」把 `remote_state` 写成 `error`，含义正是"这一份被内容比对否定过" —— 于是
    `run_attachment_round` 对它**不许**再走 HEAD 跳过，要走覆盖式 MOVE（见 SYNC-PROTOCOL §13）。
    意图落在**账上**而不是内存标志：进程重启、这台设备离线、这一轮 64 MiB 预算用完，点下去那一下都不能丢。
    前置是本机那份真的在且哈希就是它 —— 内容与 sha 不符的一份字节传上去会污染所有引用同一 sha 的笔记，
    那比传失败严重得多，所以这里必须拒（`nothing_to_upload`）。拒绝的代价止于"没传"：这条命令不删不改本机文件、
    也不动账（挪开/销毁坏字节是磁盘体检那条路的事，它有自己那套"没有替代就不销毁"的凭据要求）。
  * 三个失败码各有自己的文案（`attachment_not_registered` / `nothing_to_retry` / `nothing_to_upload`），
    因为这三格看起来一样、用户下一步该做的事完全不一样。
* **`available` → `missing` 的反向迁移（磁盘体检，2026-09-27 补）**：账与盘会分叉 —— 磁盘清理、杀毒隔离、
  误删 `attachments/` 目录、换盘没搬完。这种行**两个队列都看不见**（`available` 不进下载队列、远端已 `present`
  不进上传队列），于是那张图永久打不开而系统以为自己修好了。所以附件轮开工前先扫一遍
  `local_state='available' AND remote_state='present'`：文件不在 → 降级；长度与登记不符 → **复算 sha256**，
  相符就**把登记尺寸回填成盘上实测长度**、字节一个不碰（哈希才是身份，而"哈希相符"意味着这个长度就是真值；
  不回填的代价是这一行此后**每一轮**都被整份读进内存再哈希一遍，且偏大的尺寸会继续排进上传预算 ——
  `upsert_attachment_row` 的规则是 `MAX(旧, 新)`，只许涨不许落，所以登记那条路纠不掉它，体检是唯一能纠的地方），
  不符就**挪开**再降级重下。
  挪开而不是删除是数据安全那条序决定的：坏文件改名成 `<sha>.corrupt`（那里已有文件就带毫秒戳）留在原地 ——
  万一 `remote_state='present'` 本身是假的（旧版本那个 412 分支就记出过悬空 present），这台机器上最后一份现场
  还有得查。但也不能留着不动：`ingest_blob` 见目标已存在就不覆盖，坏字节会一直用下去。等重下拿到**哈希对得上**
  的替代之后，下载那条分支才把 `.corrupt` 清掉 —— 销毁要有凭据。
  成本：每条候选一次 `stat`，哈希只在长度对不上时才算 —— 而算过一次就把尺寸改对，于是这一行下一轮回到
  只 `stat` 的快路（慢路不是常态循环）。**每轮的量有上界**（2026-09-28 补，§48 G4）：候选查询带 `LIMIT`
  （常驻循环里传 200），降级是**一次**写事务（`set_attachments_locally_missing`）而不是逐行提交；
  尺寸回填同样是**一次**写事务（`set_attachment_sizes`，FT-ATT-40）—— 这一半 G4 当时漏下了，逐行提交
  一直是它的形状，直到同一天把每轮的毫秒数量出来才看见：一轮慢路 ~320 ms 里"读 1 KiB×200"与
  "读 64 KiB×100"（IO 差 30 倍）耗时一样，而同样条数的逐行写提交是 100 次 106 ms / 1000 次 1151 ms
  （≈1.1 ms/次）—— 那一轮的钱花在提交上，不在哈希上。回填还有一条**顺序**账：它落在
  `lost.is_empty()` 那个早退**之前**，因为健康库最常见的形状就是"一条都没降级、只有尺寸是偏的"。
  实际每轮的代价已经量了（`--test attachment_gc_scale`，debug 构建；**下面这组是"回填还逐行提交"那一版**，
  改成批量之后同一把量具的读数记在 PERF-10）：100 份 × 64 KiB ⇒ 快路 11 ms、
  慢路 315 ms、爆发（100 条降级）12 ms；1000 份 × 1 KiB ⇒ 快路 24 ms、慢路 323 ms、爆发（200 条）25 ms。
  常驻循环是 20 s 一轮，所以快路那 24 ms 占的是 0.12%。另一条口径顺带被钉出来：**上界 200 意味着
  全库覆盖要 `ceil(N/200)` 轮** —— 一千份附件的机器被删空之后，最后一张图要到 100 s 之后才被记成
  `missing` 并进下载队列（这是产品口径，不是缺陷，但以前台账里没写过）。
  这两条各挡一件事：不分页 = 整个 `attachments/` 目录被删时一轮里几千次 `stat` 挤在同一个 20 s 格里；
  逐行提交 = 那么多次提交排队占住写锁，而用户那次保存正排在锁后面。
  取前 N 条不会饿死后面的行 —— 降完的行立刻离开候选集（不再 `available`），下一轮自然浮上来。
  **边界要说清**：只判"在不在 + 长度对不对 + 不符时算一次哈希"，**不是整库周期性重哈希**（那代价是每轮 O(库大小) 读）。
  所以"长度分毫不差、内容被逐字节改坏"的静默位腐**不在主动修复覆盖内**（做成周期性 scrub 要按 `verified_at` 轮转，
  另立工作项，别把那句当已实现）。**但读侧没有这个盲区**：`attachment_data` 每次读盘都复算一次 sha256，
  对不上就报 `attachment_corrupt`，界面留占位而不是画出那张错图 —— sha256 是身份这句话要一路管到给界面那条路，
  否则内存 URL 表按 sha 存，整个会话都在显示一份不属于这个 sha 的内容而没有任何地方说它坏了。
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
| 删除 | `deleted_at = now`，`rev` +1 | `upsert`（带 `deleted_at` 的记录） | 是（"最近删除"，**没有到期日**） |
| 恢复 | `deleted_at = NULL`，`rev` +1 | `upsert` | — |
| 永久删除 | 行移入 `tombstones(purged=1)`，`notes` 行删除 | `upsert`(purged) → 之后本地清行 | 否 |
| ~~30 天到期自动等价于"永久删除"~~ | **未实现，且这一版不打算自动替用户做** | — | — |

> **这一行原来写的是"30 天到期 → 自动永久删除"，那是设计意图不是行为**（§45 的对账要求把它改过来）：
> 今天回收站里的笔记**不会**因为放着不管而消失，只有用户点"永久删除"才会。实现"到期自动销毁"意味着
> 应用**主动、无人签核地**删用户数据 —— 与 GC 那一格同一条理由（这一步需要人先决定，见 §48/§52 交给
> 用户的那一项），而且它一旦落地就会连带影响 blob 回收：笔记被自动 purge 之后链接归零，blob 才会进回收候选。
> 影响面：长期用回收站当"归档"的人，本机盘与服务器上的墓碑都不会自己收缩。

关键设计：**文件夹删除不级联删除笔记**。子文件夹上移一级、笔记移入默认本，只为文件夹本身写一条 tombstone。理由：① 一次误触不应连带摧毁内容；② 级联会产生 N 条 tombstone 与 N 次远端写，把删除变成最重的操作。此决策见 ADR-0006。

`purged_at` 与 tombstone 必须传播，否则会出现"设备 A 永久删除 → 设备 B 从缓存重新上传 → 笔记复活"（§10 明令禁止的复活路径）。

默认本（`system_kind='default'`）的 **id 是写死的**（`DEFAULT_FOLDER_ID = 00000000-0000-7000-8000-6e6f74657261`），不是"本机第一次开机时新生成一个"。清单按 `(kind, id)` 认条目，所以"每个账户有且只有一个"这种**角色实体**必须各台设备算出同一个 id，否则它在协议里就是两个条目：各自都合法、各自都会被公告与分发，两台设备的侧栏最后各长出两个「默认本」，笔记散在两个里面（1000 条库上实测清单从 1001 涨到 1002，且复制是双向传播的）。用户手工建的文件夹仍用随机 id —— 它们本来就是各自独立的实体。

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
* **回读一条笔记之前，必须先等掉这条笔记自己的在飞写**（界面侧 `editor.flush()`，判据 FT-SAVE-04）。
  这条不是"顺手等一下"：`open()` 的形状是"先 flush 再 `get_note`"，如果 flush 只等待发的那一支 debounce、
  放过已经在飞的一支，回读就会拿到"这次写之前"的快照并盖掉本地正文；随后那支写落地时会发现"正文又变了"，
  于是照着被盖掉的旧快照**再写一次** —— 用户看到的是一次加粗刷新后消失。丢的是已提交的编辑，
  所以它排在"数据安全"那一格，不是"用户体验"。
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
