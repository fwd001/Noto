# SYNC-PROTOCOL

Notera ⇄ 用户自有 WebDAV 的同步协议 v1。本文是**规范性**文档：实现与本文冲突即为缺陷。

关联：[DATA-MODEL.md](./DATA-MODEL.md)（rev / sync_rev / 信封字段）· [CONFLICT-RESOLUTION.md](./CONFLICT-RESOLUTION.md) · [PROXY.md](./PROXY.md) · [ARCHITECTURE-MAP.md](./ARCHITECTURE-MAP.md) · ADR-0003 / 0004 / 0005 / 0006 / 0009

尺寸类结论均为**实测**（脚本与输出见 `docs/evidence/`），非估算。

---

## 0. 三条不可违反的规则

**R1 · 正确性来源是记录文件本身，清单（manifest）只是索引与公告。**
每条实体记录自带 `rev / hash / deleted_at / purged`，足以独立判定状态。清单损坏、落后或被并发覆盖，最多导致"多拉一次"或"晚一轮被发现"，**不会**导致数据丢失或错误覆盖。

**R2 · 写入顺序固定为：实体记录 → 附件 → 清单。**
清单是"公告板"。因此清单落后于实体是**正常且安全**的；清单领先于实体（引用了不存在的记录）是**异常**，按 §10 的 `missing_remote` 路径处理，**绝不因此删除本地数据**。

**R3 · 任何远端状态判定只用 `rev` + `content_hash`，禁止使用 mtime / 服务器时间 / 墙上时钟。**
多设备时钟不可信。`updated_at` 仅用于展示与诊断，不参与任何"谁更新"的判定。

---

## 1. 远端布局

```text
<base_url><root_prefix>/            root_prefix 默认 /.notes
├── protocol.json                   一次性写入的根自描述（版本协商）
├── manifest/
│   ├── index.json                  权威清单：seq + 分段目录 + 变更窗口
│   ├── index.json.prev             上一版清单（损坏回退用）
│   └── seg-<NNNN>.json             基线分段，不可变命名，内容随压实而变
├── records/
│   ├── note/<id>.json              一条笔记 = 一个信封文件（可变）
│   └── folder/<id>.json
├── attachments/<2hex>/<sha256>     内容寻址，一旦存在即不可变
├── locks/<device>.json             尽力而为租约（非正确性依赖，见 §11.4）
└── tmp/<device>-<nonce>.json       原子写暂存（>24h 由维护轮清理）
```

* 目录名一律小写；`<id>` 为 UUIDv7 字符串（实测路径安全，无 `/ ? space`）。
* `<2hex>` = sha256 前两个十六进制字符，用于摊平单目录文件数。
* 路径构造集中在 `notera-webdav::RemotePath`，**必须**校验 `id` 字符集并拒绝任何 `..`（目录穿越即安全事故）。

---

## 2. `protocol.json` 与版本协商

```json
{
  "protocol": 1, "min_protocol": 1, "layout": "v1",
  "root_id": "0192e6c1-…", "created_at": "2026-09-25T09:00:00.000Z",
  "created_by": "0192e6c1-…", "software": "notera 0.1.0",
  "segment_target_entries": 2000, "window_max_entries": 200,
  "capabilities_hint": { "conditional_put": null }
}
```

协商规则（每账户首次连接与每次 `seq` 变化时复核）：

| 条件 | 行为 |
|---|---|
| 服务器 `min_protocol` > 客户端 `max_protocol` | `phase=read_only`，停止一切写入，提示"请升级 Notera" |
| 服务器 `protocol` < 客户端 `min_protocol` | 同上，提示"服务器数据格式过旧，请升级服务器端或客户端" |
| 区间相交 | 取交集，写入 `sync_accounts.protocol_min/max` |
| `protocol.json` 存在但解析失败 | **不得**当作"空根"重新初始化（会摧毁现有库）→ `Failed{Protocol}` 并停轮 |
| `protocol.json` 不存在且根目录为空 | 允许初始化（§9 场景 A） |

`protocol.json` 写入用 `Overwrite:F`（创建语义）；若已存在则丢弃本地写入意图，改为协商。两个客户端同时初始化时，后写入者会发现 `root_id` 不同 → 立即停轮并提示"该 WebDAV 目录已被另一个 Notera 库使用"，**绝不**合并两库。

---

## 3. 记录信封（wire 形态）

`records/<kind>/<id>.json` 的内容即 DATA-MODEL §11 的 `Envelope`，此处规定约束：

```json
{
  "protocol": 1, "kind": "note", "id": "0192…", "rev": 7, "sync_rev": 6,
  "hash": "sha256:<64hex>", "updated_at": "2026-09-25T09:12:03.441Z",
  "device": "01a0…", "deleted_at": null, "purged": false,
  "enc": { "alg": "none", "hash_alg": "sha256" },
  "payload": { "v": 1, "content": [ … ] },
  "ct": null
}
```

| 约束 | 违反时 |
|---|---|
| `payload` 与 `ct` 恰好一个非空 | 丢弃该响应，`Failed{Protocol}`，不写库（I6） |
| `hash` 必须等于 `sha256(canonical(payload))`（`alg=none` 时） | 同上，计入 `dangling` 诊断 |
| `kind` 与路径目录一致 | 同上 |
| `id` 与文件名一致 | 同上 |
| `rev` 单调：新写 `rev` 必须 > 服务器当前 `rev` | 412 重算（§11.2） |
| `enc.alg=none` 且 `hash_alg=sha256` | v1 默认；开启 E2EE 后 `hash_alg` 必须转 `hmac-sha256`，否则泄露明文等价指纹 |

`purged=true` 的记录是**墓碑公告**：`payload` 为 `null`（即便 `alg=none`），只保留 `id/rev/hash/deleted_at/purged`。它使"永久删除"可传播（§8）。

---

## 4. 清单（manifest）

### 4.1 结构：基线分段 ⊕ 变更窗口

```json
{
  "protocol": 1, "root_id": "0192…", "seq": 1324,
  "generated_at": "2026-09-25T09:12:05.001Z", "generated_by": "01a0…",
  "software": "notera 0.1.0",
  "counts": { "note": 5000, "folder": 12, "attachment": 340 },
  "segments": [
    { "n": "seg-0000", "cover": ["0192…", "0193…"], "count": 2000, "hash12": "a1b2c3d4e5f6", "bytes": 158976 }
  ],
  "window": {
    "since_seq": 1290, "complete": true,
    "entries": [ { "i": "0192…", "t": "n", "r": 7, "h": "a1b2c3d4e5f6", "s": 1834, "d": null, "p": 0 } ]
  },
  "attachments": [ { "i": "<sha256>", "z": 2048576 } ],
  "checksum": "sha256:<64hex>"
}
```

* **有效远端状态 = 所有分段条目，被窗口条目覆盖（窗口优先）。**
* `checksum` = 去掉 `checksum` 字段后 canonical JSON 的 sha256（自校验，防半写与截断）。
* 条目字段：`i`=id，`t`=类型（`n`/`f`/`a`），`r`=rev，`h`=hash 前 12 hex，`s`=字节数，`d`=deleted_at，`p`=purged。
* 分段按 id 升序排列，`cover` 给出首尾 id，读端可二分定位而不必先拉分段。

### 4.2 实测尺寸（决定本设计）

| 形态 | 裸大小 | gzip | 用途 |
|---|---|---|---|
| 单条目 `{i,r,h,s,t}` | 89 B | — | 决定窗口/分段容量 |
| 全量 5000 条清单 | 438.7 KiB | 186 KiB | ❌ 不作为每轮载体 |
| 全量 20000 条清单 | 1 754 KiB | 742 KiB | ❌ 更不可接受 |
| 窗口 200 条 | 16 KiB | **7.8 KiB** | ✅ 每轮实际重写量 |
| 压实后的 `index.json`（5000 条 → 3 段，窗口清空） | **711 B** 实测 | — | ✅ 每轮重写量与库规模解耦；分段 [2000,2000,1000]，有效视图折回 5000 条不丢 |
| 分段 2000 条 | 175.6 KiB | 74.7 KiB | ✅ 仅压实或落后过多时拉取 |

> 若采用"每轮重写全量清单"，5000 条笔记的**每一次编辑**都要上传 186 KiB——这正是要用两段式结构避免的。窗口 7.8 KiB 与库规模无关，是"少请求、少流量"（§12）的落点。

### 4.3 压实（compaction）

触发条件（任一）：`window.entries.len > 200`；**窗口攒够 `COMPACT_OVERLAP_MIN`(=SEGMENT_TARGET/4=500) 条之后**，与某分段的 id 重叠率 > 20%；`seq` 距上次压实 > 5000。

> 重叠率那条为什么要有量下限：压实过一次之后，任何一条新改动的 id 都必然落在已有分段的 cover 里 —— 只看比率就是 100%，于是**每次编辑都重写整份基线分段**（2000 条 ≈ 159 KiB 上传 / 一次改动）。这条下限是 5000 条库的端到端跑测撞出来的，实测证据在 `manifest::tests::one_edit_after_a_compaction_does_not_rewrite_the_baseline` 与 `notera-host/tests/compaction.rs`。

压实动作：把窗口折进受影响分段 → 重写这些分段文件 → 生成新 `index.json`（`window.since_seq = 新 seq`，`window.entries = []`）→ `seq += 1`。

压实是**唯一**重写分段内容的动作，且与普通提交走同一 CAS 路径。未参与压实的客户端只会看到"若干分段 hash 变了"，按 §6.2 拉取变化分段。

### 4.4 原子提交与损坏恢复

清单写入（`index.json` 是本轮的**最后**一步，见 R2）：

```text
1. GET index.json，记下 etag E0
2. 生成新清单：seq = max(观测 seq)+1，checksum 自校验
3. PUT manifest/index.json.tmp-<device>-<nonce>   → GET 回读校验 checksum
4. MOVE index.json → index.json.prev              (If-Match: E0, Overwrite:T)
5. MOVE manifest/index.json.tmp-* → index.json    (Overwrite:F)
6. GET index.json 复算 checksum，比对 seq 与预期
```

> 步骤 4 与 5 的先后是修订过的：原文写的是"把 `.tmp` 移进 `.prev`"，那会把**新**清单
> 放进上一版的位置、并把真正的上一版覆盖掉，D1 回退时读到的就是自己刚写的那份，
> 等于没有回退点；而步骤 5 的 `Overwrite:F`（创建语义）在 `index.json` 仍存在时永远失败。
> 现在改成先让位（旧版进 `.prev`）、再落地（新建 `index.json`），CAS 判定就落在步骤 4 的
> `If-Match: E0` 上 —— 别的设备在我们读取之后改过清单，这一步直接 412，本轮重规划。

4 与 5 之间崩溃留下的窗口：`index.json` 缺失、`.prev` 是旧版、`.tmp-*` 是新版。
按 D1 读 `.prev` 继续工作，下一轮由持有新清单的设备重新提交；`missing_remote` 一律不删本地。

失败与损坏恢复阶梯（严格顺序，任一级成功即停）：

| 级 | 症状 | 处置 |
|---|---|---|
| D1 | `index.json` checksum 不符 / JSON 非法 | 读 `index.json.prev`，校验通过后以其为准并置 `needs_relist=true`（下轮补正） |
| D2 | `index.json` 404 且 `prev` 也不可用 | `PROPFIND Depth:infinity` 扫描 `records/` + `attachments/`，**从记录文件重建**清单（记录自带 rev/hash/deleted_at/purged，故信息无损） |
| D3 | 扫描结果与本地缓存严重背离（消失条目 > 30% 且 > 50 条） | **拒绝**据此删除本地；置 `phase=error`，要求用户确认。防"服务器被清空/挂错目录"导致误删 |
| D4 | 分段文件 checksum 不符 | 只丢弃该分段缓存，回退窗口 + 逐条 GET 受影响实体 |

D3 是数据安全的硬闸门：服务器侧异常（运维误删、挂载错目录、配额清空）不得被翻译成"用户删了 3000 条笔记"。

---

## 5. 服务器能力探测（不同 WebDAV 服务器行为差异是现实存在的）

首次连接与每日一次探测，结果存 `sync_accounts.cap_mask`：

> **as-built（2026-09-26）**：探测在 `notera-webdav/probe.rs`，启动路径是
> `App::remote_for_sync()` = **先按需探测 → 再装适配器**。顺序不能反：先装后探的话，
> 这次会话仍然按保守默认写，探到的能力要等下次启动才生效。`cap_mask IS NULL`
> 与 `cap_mask = 0` 含义不同（前者=从未探测→用 `Caps::conventional()`，后者=实测全不支持→S3）。
> 探测**未完成**（连接被掐/超时）时不写 `cap_mask`、不写"全 false 的结论"，只发一条
> `sync.probeDeferred` 提示并以保守默认继续 —— 猜低的代价是掉进 S3 盲写，那才有覆盖风险。
> 强制重探：`notera-cli dav-probe`。`CHUNKED` 一位**不探**（`RequestSpec` 的 body 是
> `Vec<u8>`，发不出真 chunked 请求，硬凑头部只会得到假阳性）。
> 证据：`crates/notera-webdav/tests/probe.rs`（逐项缺失单独可辨 + 不可达必须是报错）
> 与 `crates/notera-host/tests/sync_once.rs`（请求日志里探测全部早于任何真实读写）。

| 探测 | 方法 | 影响 |
|---|---|---|
| 强 ETag | `PUT` 后 `GET` 带 `If-None-Match` | 决定能否走 304 空轮快路径 |
| 条件 PUT | `PUT` + `If-Match:"bogus"` 期望 412 | 决定写入策略 S1 |
| `Overwrite:F` MOVE | MOVE 到已存在目标期望 412 | 决定写入策略 S2 |
| `Depth:infinity` | 一次 PROPFIND | 不支持则退化为逐目录递归列举 |
| Range | `GET` + `Range: bytes=0-0` 期望 206 | 决定附件断点续传 |
| 分块请求 | chunked body | 影响半上传检测方式 |

### 写入策略（按能力自动选择）

| 策略 | 前提 | 步骤 | 并发保护强度 |
|---|---|---|---|
| **S1 条件 PUT** | conditional_put=true | `PUT records/…` + `If-Match: <已知 etag>`；412 → 重读清单重算 plan | 强 |
| **S2 tmp+MOVE** | overwrite_f_move=true | `PUT tmp/…` → 回读校验 → `MOVE tmp→record`（`Overwrite:F`）；目标已存在 → 说明有人先写 → 412 路径 | 强 |
| **S3 盲写复验** | 都不支持 | `PUT record` → 立即 `GET` 复算 hash；与本地预期不符 → 判定被覆盖 → 回退为"远端领先"，重算 plan 并走冲突路径 | 弱（存在覆盖窗口，靠复验检出） |

S3 下**必须**启用租约（§11.4）以缩小覆盖窗口，并在 UI 上把该账户标为"服务器不支持并发保护，建议多设备串行编辑"。这是诚实降级，不是静默风险。

---

## 6. 一轮同步（round）

### 6.1 状态机

```text
idle
 └─(Trigger)→ acquire_lease?          尽力而为，失败不阻塞
                └→ read_manifest      GET index.json (If-None-Match)
                     ├─ 304 且无本地脏 → no_op_done           ← 空轮，1 请求 0 字节
                     ├─ 304 但有本地脏 → plan_from_local
                     └─ 200 → parse+verify → plan
                          └→ push_records   (实体，必要时附件另队)
                               └→ pull_records (并发 GET，上限 8)
                                    └→ apply_local      单事务，见 §7
                                         └→ commit_manifest  (CAS，见 §4.4)
                                              └→ release_lease → done(RoundStats)
```

任一阶段失败 → 记录 `sync_state.last_error_*` → 按 §12 分类退避 → **本地不受影响**（I8）。

### 6.2 读远端状态的三档成本

| 情形 | 动作 | 请求数 | 字节 |
|---|---|---|---|
| 无变化 | `GET index.json` → 304 | 1 | 0 正文 |
| 落后 ≤ 窗口 | 读 index，用 `window.entries` 与 `sync_remote_index` 比对 | 1 + 变更实体数 | ≈ 7.8 KiB + Σ记录 |
| 落后 > 窗口 | 比对 `segments[].hash12` 与缓存 → 只拉变化的分段 | 1 + 变化分段数 + 变更实体数 | 74.7 KiB/分段 |
| 全新设备 | 拉全部分段 + 窗口 | 1 + ⌈N/2000⌉ + N | 5000 条 ≈ 219 KiB gzip |

### 6.2.1 跳过一次下载的前提：那份内容还得读得回来（2026-09-27）

基线分段"内容哈希没变就不重下"是一项跳过，而跳过只有在**条目本身仍然读得回来**时才安全。
所以两条绑在一起，缺一不可：

1. 每轮把算出的**远端视图**（清单投影：`kind/id/rev/hash12/deleted_at/purged`）整份写回
   本机 `sync_remote_index`，读侧也从这张表读 —— 活在进程内存里等于每次开机重下一遍；
2. 分段内容缓存只在**核对过**时写：把拿到的条目重算 `segment_hash12` 与清单声明的 `hash12`
   比对，一致才算"本机这一段就是清单所指的那版"（写侧与读侧必须共用这一个函数）。

落盘的判据是"**这一轮把索引投影读全了**"，不是"本轮没被请求预算截断"：预算常常是花在逐条
取正文时耗尽的，那时索引投影本身完整，存下来安全；反过来，基线分段没读完就截断的那一轮
绝不能存 —— 半份视图配上一句"这段我有了"，缺的那些条目永久没人认领（实测第二台设备停在
198/1000）。压实之后不再被引用的分段名一并从缓存里清掉，否则无界长大。

### 6.3 每轮请求与字节预算（规范上限）

| 场景 | 请求 | 上行 | 下行 |
|---|---|---|---|
| 空轮（无改动、无新内容） | **1** | 0 | 0（304） |
| 本地改 1 条笔记 | ≤ 4 | 记录 ≈ 1 KiB + 清单 ≈ 8 KiB | 8 KiB |
| 远端有 1 条新笔记 | 2 | 0 | 8 KiB + 记录 |
| 压实轮 | ≤ 4 + 重写分段数 | 分段 74.7 KiB/个 | 同 |
| 单轮硬上限 | 200 请求 / 8 MiB | 超出 → 本轮收敛为部分完成，余量留下一轮 | — |

> 空轮必须是 1 请求 0 字节。任何"每轮下载全量清单"的实现都违反本节，属于性能与流量双缺陷（内网/移动网络尤其敏感）。

---

## 7. 计划判定表（完整枚举）

记号：`L.rev` = 本地头部；`L.sync_rev` = 双端上次一致点；`R.rev` = 清单/记录给出的远端 rev；`H(x)` = 内容哈希；`—` = 该实体不存在。

| # | 本地 | 远端 | 判定 | 动作 |
|---|---|---|---|---|
| P1 | `—` | `—` | 不可能 | 断言失败 |
| P2 | `—` | 有 | 远端新增 | `Pull` 记录 → 落库（`rev=sync_rev=R.rev`） |
| P3 | 有 | `—`（清单无） | **missing_remote** | 见 §10；**不删本地**，按脏处理并补传 |
| P4 | `rev==sync_rev` | `R.rev==sync_rev` | 已收敛 | NoOp |
| P5 | `rev!=sync_rev` | `R.rev==sync_rev` | 仅本地改 | `Push` |
| P6 | `rev==sync_rev` | `R.rev!=sync_rev` | 仅远端改 | `Pull` |
| P7 | 两侧都改 | `H(local)==H(remote)` | 内容相同（重复保存/回环） | 收敛：`rev=R.rev`，不产生冲突，不二次上传 |
| P8 | 两侧都改 | `H` 不同 | **真冲突** | 交 CONFLICT-RESOLUTION.md §3 |
| P9 | 本地 `deleted_at!=—` 且脏 | 远端有 | 本地删除待传播 | `Push`（信封带 `deleted_at`） |
| P10 | 本地有 | 远端 `deleted_at!=—` | 远端删除 | 本地软删（若本地更脏 → P11） |
| P11 | 本地在删除后又被编辑（`rev>sync_rev` 且 `updated_at>deleted_at`） | 远端已删 | **删除 vs 修改** | 冲突：保留内容 + 提示，绝不静默二选一 |
| P12 | 本地 `purged` 待传播 | 远端有 | 永久删除传播 | `Push`(`purged:true`) → 本地移入 `tombstones` |
| P13 | `—` | 远端 `purged:true` | 别处已永久删除 | 写 `tombstones(purged=1)`，本地若有行则删除 |
| P14 | 本地 `tombstones` 有 | 远端 `—` | 删除已生效或从未上传 | 若 `sync_rev` 曾 > 0 且无远端记录 → 补传墓碑 |
| P15 | 文件夹 `parent_id` 指向不存在/已删文件夹 | 有 | 悬空父级 | 重挂到默认本 + 记诊断，不删笔记（§8.3） |
| P16 | 记录 `doc.v` / `protocol` 高于本地支持 | 有 | 版本超前 | **只读**该实体，不改写、不降级（I7） |
| P17 | 记录校验失败（§3 任一约束） | 有 | 脏数据 | 丢弃响应，`Failed{Protocol}`，不写库（I6） |
| P18 | 附件 `local_state=missing` | 清单有该 sha | 缺本地副本 | 入 download 队列（不阻塞文本） |

**P7 值得强调**：内容哈希相同即判收敛，因此"两台设备各自打开又保存同一笔记"不会产生假冲突——这是 `hash` 参与判定而非只比 `rev` 的直接收益。

---

## 8. 删除、恢复与永久删除

### 8.1 三态

| 态 | 本地 | 远端记录 | 清单条目 |
|---|---|---|---|
| 正常 | `deleted_at=NULL` | `deleted_at=null` | `d=null` |
| 回收站 | `deleted_at=T` | `deleted_at=T` | `d=T` |
| 永久删除 | 行删除 + `tombstones(purged=1)` | `purged:true, payload=null` | `p=1` |

### 8.2 传播与反复活

删除是一次普通 `Push`（信封多一个字段），因此**与编辑共用同一条已验证路径**，不需要独立协议。

防复活（§10 明令禁止的行为）依赖三点：

1. 删除是**记录内容**，不是"文件消失"——服务器上的记录仍在，只是带 `deleted_at`；
2. `tombstones` 表**不自动 GC**（ADR-0006）：本地即使物理删了行，删除事实仍在，重新拉取时不会被视为"本地新增"；
3. `missing_remote`（P3）永不导致本地删除，只导致补传。

因此"设备 A 删除 → 文件不见 → 设备 B 重新上传"这条路径在协议层不存在。

### 8.3 文件夹删除不级联

删除文件夹只写它自己的墓碑；其子文件夹上移一级、笔记移入默认本（P15）。理由：① 一次误触不应连带摧毁内容；② 级联会把删除变成最重的操作（N 条墓碑 + N 次远端写）。

### 8.4 恢复

从回收站恢复 = 一次 `deleted_at=NULL` 的 `Push`（`rev` +1）。若恢复时远端该 id 已被永久删除（P13），走冲突路径而非静默"复活"——用户明确恢复才允许重建。

---

## 9. 首次同步（bootstrap）四场景

| 场景 | 本地 | 远端 | 序列 |
|---|---|---|---|
| A 全新用户 | 空库 | 空根 | 写 `protocol.json`（`Overwrite:F`）→ 空清单（seq=1）→ `phase=online` |
| B 新设备接老库 | 空库 | 有数据 | 协商 → 拉清单（分段 + 窗口）→ `bootstrap_pull` 全量落库（单事务分批，每批 500 条）→ `online` |
| C 首台设备上传老库 | 有数据 | 空根 | `bootstrap_push`：先写全部记录（分批、可断点续传）→ 最后写清单一次性公告 → `online` |
| D 两边都有数据 | 有数据 | 有数据 | **不覆盖任何一方**：以远端为基线做 P2..P18 全量 plan，冲突按 CONFLICT-RESOLUTION；UI 明确提示"检测到远端已有 Notera 库，正在合并" |

进度要求：`bootstrap_*` 期间 UI 显示"正在同步（x/y）"，但**编辑与查看不受阻**（I8）——用户可以先写笔记，队列在后面追。断点续传靠 `sync_operations` 的 outbox 状态，不靠内存。

---

## 10. `missing_remote`：最危险的分支

清单引用了记录、或本地认为已上传但远端 404 时：

```text
GET records/note/<id>.json → 404
  ├─ 本轮刚写过？ → 视为"服务器最终一致性延迟"，标 retry，下轮复验（不报错）
  ├─ 连续 3 轮 404 → 判定远端确实缺失
  │    ├─ 本地有该实体 → 标 dirty，走 P3 补传
  │    └─ 本地也无（仅清单引用） → 从清单剔除该条目（压实轮生效），记 dangling 诊断
  └─ 任何情况下：绝不因远端 404 删除本地数据
```

配额类（507）与权限类（403）导致的"写不进去"必须与"远端没有"区分开——前者保留 `dirty` 并提示用户，后者才是上面的路径。混淆二者会造成"服务器满了 → 客户端以为笔记不存在 → 清理本地"的灾难。

---

## 11. 并发、幂等与崩溃恢复

### 11.1 幂等

`sync_operations.dedupe_key = sha256(account ‖ kind ‖ id ‖ op ‖ rev)`，唯一索引。

上传成功但确认丢失（超时/断连）→ 重放：`GET` 远端记录，若 `H(remote)==H(local)` 且 `rev` 相等 → **直接判定成功**，不重复写。因此"至少一次"投递不会变成"重复覆盖"。

### 11.2 并发保护三层

1. **rev 单调闸门**：写入前比对观测到的 `R.rev`；`local.rev <= R.rev` 且 hash 不同 → 不写，转冲突判定；
2. **条件写**：S1/S2（§5），412 → 重读清单 → 重算 plan（最多 3 次）；
3. **租约**（尽力而为）：`locks/<device>.json` 记 `token/expires_at/seq`，写清单前若他人租约新鲜且本轮非压实 → 让路，延后一轮。租约**不是**正确性依赖（多数 WebDAV 服务器不实现 LOCK，见 ADR）。细则见 §11.4。

### 11.3 崩溃点矩阵

| 崩溃位置 | 远端状态 | 重启后判定与动作 |
|---|---|---|
| C1 写 tmp 前 | 无变化 | outbox `pending` 原样重放 |
| C2 tmp 写完、未 MOVE | 多一个 `tmp/*` 残留 | 24h 后维护轮清理；本轮重发新 tmp（nonce 不同） |
| C3 MOVE 中 | 记录要么旧要么新，无中间态 | `GET` 复算 hash：等于本地 → 视为已提交；否则重发 |
| C4 记录已提交、清单未提交 | 实体新、清单旧 | 清单重放（窗口幂等：同 id 同 rev 覆盖即可）→ 无数据损失（R1） |
| C5 清单 `prev` 已写、`index` 未写 | `index` 仍是旧版 | 正常：旧清单自洽，重算 plan 后重写 |
| C6 清单写完、本地未标记 | 远端新、本地 `dirty` | 复验：`H(remote)==H(local)` → P7 收敛，清 dirty，不二次上传 |
| C7 apply 事务中途 | 本地事务原子回滚 | WAL + 事务保证无半条笔记；outbox 仍 `pending` |
| C8 附件上传中途 | `tmp` 或半文件 | sha256 内容寻址：半文件哈希必不符 → 校验失败即删除重传，**永不**当作有效 |
| C9 下载中途 | 本地 `.part` | 校验后原子 rename；未完成不入 `available` |
| C10 压实中途 | 分段旧/新混合 | 清单是唯一入口：`index` 未换 → 旧分段仍有效；换了 → 新分段自洽（压实自身在 tmp 里先构造完整新清单） |

启动自检（`notera-host`）：`inflight → pending` 归位 + 逐条 `GET` 复验（P7）+ 清单 checksum 校验 + `user_version` 闸门。

### 11.4 尽力而为租约（§11.2 第三层的细则）

对象：`locks/<device>.json`，每台设备一个，内容

```json
{ "device": "<device_id>", "token": "<本次启动随机串>", "expires_at": "<RFC3339 UTC>", "seq": <清单 seq> }
```

| 规则 | 为什么 |
|---|---|
| **只在保护缺位时启用**：写入策略为 S3，**或** 探测不到强 ETag（清单 CAS 不可信） | 有 S1/S2 且强 ETag 时，服务器自己就会拦并发写，租约只是每轮多两个请求 |
| 每轮**先贴自己的租约**（无条件 PUT，覆盖自己上一份），再在 **⑥ 写清单之前**读别人的租约 | 记录已经推上去但清单没公告 = 别人看不见，也无害（清单是唯一入口，§4）；所以让路的代价只是延后一轮，不是丢数据 |
| 别人未过期 → 本轮**不提交清单**，本地改动保持 dirty，下一轮再试；状态必须可见（徽标 + `sync.leaseHeld`），不许静默不公告 | "静默地不同步"和"静默地同步错"一样不可接受 |
| 过期即视为无人持有；TTL 60s，而桌面轮次间隔 25s → 正常在线时租约一直在续 | 设备崩溃后留下的租约最坏挡住别人 60s，不需要清理进程 |
| 自己的旧租约（路径按 device 分）**总是覆盖**，不当成"别人占着" | 换 token 不代表换设备；同机重启不该被自己上次崩溃挡住 |
| 读别人的租约失败（拿不到列表 / 单个文件读不出）→ **当作无人持有**，继续提交 | 这是 best-effort 层：它坏了只能退回第 1、2 层的保护，绝不能变成"永远不同步"。反之探测**能力**失败不许当结论（§5）—— 两者不同：这里最坏是多一次 CAS 竞争，那里最坏是降级成盲写 |
| 时间比较用墙上时间 | 与 I4 不冲突：I4 禁止用墙上时间判**数据新旧**；这里判的是"某个提示还作不作数"，判错的方向是"少让一次路"，不会覆盖内容 |
| 不用 `LOCK`/`OPTIONS` 判定 | 多数服务器不实现（见 ADR-0013 附带实测：本仓库测试服务器对 LOCK 回 501），把它当正确性依赖会直接封死一批后端 |

**它防住的到底是什么**（说清边界，避免过度承诺）：两台 Notera 同时走到 ⑥ 时，弱 ETag 服务器上 CAS 形同虚设，清单公告会互相覆盖 —— 租约让其中一台晚一轮，从而把窗口从"两轮并行提交"缩到"顺序提交"。它**防不住**：非 Notera 客户端（不认识 `locks/`）、时钟严重偏差的设备、以及记录本身的并发写（那一层由 §11.2 第一、二层与 S1/S2 负责）。

> **as-built（2026-09-26）**：策略判定在 `notera-host`（`App::lease_policy`：`S3 || !STRONG_ETAG` 才开），
> 贴/看/让路在 `notera-sync`（轮次开始贴自己那份，⑥ 之前看别人），
> 端口实现在 `notera-webdav/lease.rs`。租约**没有**走 `LOCK`；`sync_state.lease_token/lease_expires_at`
> 存本轮贴出去的那份，只为出问题时能看出"当时我以为谁在写"。


---

## 12. 错误分类与退避

| HTTP / 传输 | 分类 | 本轮动作 | 对用户 | 影响本地写入 |
|---|---|---|---|---|
| 连接失败 / DNS / 超时 | `Offline` | 停止本轮，退避 | `○ 离线` | **否** |
| 401 / 407 | `AuthRequired` | 停轮，outbox 转 `blocked` | `! 需要重新登录 WebDAV` | 否 |
| 403 | `Forbidden` | 停 push，允许 pull | `! 无写入权限` | 否 |
| 404（预期外） | `NotFound` | §10 路径 | 不提示（内部自愈） | 否 |
| 409 / 412 | `Precondition` | 重读清单重算 plan，≤3 次 | 无 | 否 |
| 405 / 501 | `Unsupported` | 降级写入策略（S1→S2→S3）并记诊断 | 首次提示"服务器功能受限" | 否 |
| 423 Locked | `Locked` | 退避重试 | 无 | 否 |
| 507 / 509 | `QuotaFull` | 停 push，保留 dirty，继续 pull | `! 存储空间不足` | 否 |
| 5xx | `ServerUnavailable` | 指数退避 | `! 同步失败` | 否 |
| 跨源重定向带凭据 | `RedirectCrossOrigin` | **拒绝跟随** | `! 服务器地址异常` | 否 |
| checksum / 信封校验失败 | `Protocol` | 丢弃响应，不写库 | `! 服务器数据异常，已保护本地数据` | 否 |
| 取消（切后台/退出/新一轮） | `Cancelled` | 静默中止，不计失败 | 无 | 否 |

退避：`delay = min(15min, 2s × 1.85^n) × (1 ± 0.2 jitter)`；`Retry-After` 存在则优先服从。连续失败只影响重试节奏，**永不**清空或降级 outbox。

---

## 13. 附件：独立队列，不阻塞文本

```text
upload 队列（独立 tokio 任务，与文本轮次解耦）
  HEAD/PROPFIND 判存 → 清单已含该 sha → 直接 present（去重，零上传）
  否则 PUT attachments/xx/<sha>（流式，tmp→MOVE）→ GET Range 0-0 复验 → present
download 队列
  缺失 blob → GET → 校验 sha256 → 原子 rename → 更新 local_state
```

规范约束：

* 文本同步轮次**必须**能在附件全部失败时正常完成（TEST-PLAN 有专门用例：拔网线只针对附件端点）。
* 单轮附件预算：≤ 4 个文件 / ≤ 64 MiB，超出留下一轮，避免移动网络长占连接。
* sha256 是身份：内容相同即同一附件，跨笔记去重；因此"重复插入同一图片"不产生二次上传。
* 附件加密（E2EE）时密文寻址：`sha256(ciphertext)` 作为远端名，`attachments` 表另存明文 sha 用于本地去重与校验（未决项，见 §16）。

---

## 14. 定时与延迟目标

| 触发 | 桌面 | 移动 |
|---|---|---|
| 保存后 debounce | 2.5 s | 2.5 s |
| 网络恢复 | 立即 | 立即 |
| 回到前台 | 立即 | 立即 |
| 周期 | 25 s（留 5 s 余量满足 ≤30 s 目标） | WorkManager / BGAppRefreshTask，系统最小约 15 min |
| 退出前 flush | 1.5 s 预算内尽力 | 不适用 |

**明确不承诺**：移动端在后台被系统挂起时仍满足 30 s。iOS/Android 不允许常驻进程，任何声称做到的方案都在撒谎（见 PLATFORM.md §后台同步与诚实预算）。移动端的真实目标是"打开设备时已同步"。

---

## 15. 版本演进规则

| 变更类型 | 例子 | 协议动作 |
|---|---|---|
| 加可选字段 | 信封新增 `x` | 次版本内允许；读端必须忽略未知字段（I7） |
| 加必需字段 | 清单新增必填 `segments[].cover` | `protocol` +1，双端协商，老客户端只读 |
| 改语义 | `rev` 改为混合逻辑时钟 | 破坏性：`protocol` +1 + 迁移工具 + 冻结期 |
| 开启 E2EE | `enc.alg: none→aes-256-gcm-siv` | 次版本（信封结构 v1 已预留，ADR-0002）；需全量重加密迁移 |
| 目录布局变化 | `records/` 改名 | 破坏性：`layout` 字段 +1，双端协商 |

版本号单一来源：`crates/notera-sync/src/protocol.rs` 的 `SYNC_PROTOCOL_VERSION`，CI 校验其与 `protocol.json` 写入值、`Cargo.toml` 版本策略一致（CI-CD.md §版本与单一版本源）。

---

## 16. 未决与需要人工确认

| # | 事项 | 状态 | 需要 |
|---|---|---|---|
| U1 | 真实 WebDAV 服务器兼容矩阵 | **BLOCKED** | 用户提供一个可访问的真实服务器（内网 + 公网各一）；当前仅对自建 test-webdav 验证 |
| U2 | `Depth:infinity` 在目标服务器上的支持 | 待测 | 同上；不支持则退化为递归列举（多请求） |
| U3 | S3 盲写复验的覆盖窗口实测 | 待测 | 需真实服务器 RTT 数据；RTT > 300 ms 时覆盖窗口风险显著 |
| U4 | 压实阈值（200 / 20% / 5000）是否最优 | 待基线 | Phase 2 用 5000 条实测每轮字节后回调 |
| U5 | 附件 E2EE 的寻址方案（密文 sha vs 明文 sha 双索引） | 未决 | 若确认启用 E2EE，需 ADR |
| U6 | 多账户（同一设备接两个 WebDAV）是否 v1 支持 | 未决 | 影响 `sync_state` 主键已预留 `account_id`，但 UI 未设计 |
| U7 | 服务器端"回收站/软删"叠加导致的语义混淆 | 待测 | 部分服务器（Nextcloud）删除进回收站可被服务端恢复，需明确策略 |
