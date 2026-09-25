# ADR-0003: 远端布局与"先实体后清单"

## 状态
日期：2026-09-25
状态：Accepted

## 背景
WebDAV 只提供文件语义：没有事务、没有可靠的 CAS（多数服务器不实现条件 PUT）、回收站行为不受控。协议必须把"写坏了会怎样"收敛到少数几个可判定的崩溃点上，因此目录布局与写入顺序属于架构决策，不是实现细节。

## 决策
`.notes/`（`root_prefix` 可配）下的布局：

```text
protocol.json                 根自描述 + 版本协商
manifest/index.json           权威清单：seq + 分段目录 + 变更窗口
manifest/index.json.prev      上一版清单（损坏回退用）
manifest/seg-<NNNN>.json      基线分段
records/<kind>/<id>.json      一条实体 = 一个可变信封文件
attachments/<2hex>/<sha256>   内容寻址，存在即不可变
tmp/<device>-<nonce>.json     原子写暂存
locks/<device>.json           尽力而为租约（非正确性依赖）
```

- 写入顺序固定：**实体记录 → 附件 → 清单**（R2）。清单是公告板，最后一步。
- 记录文件是正确性来源，清单只是索引与公告（R1）：清单落后于实体是正常且安全的。
- 原子写三步：`PUT tmp/<唯一名>` → `GET` 回读校验（checksum/hash）→ `MOVE` 到正式路径。
- 路径构造集中在 `notera-webdav::RemotePath`，必须校验 `id` 字符集并拒绝任何 `..`。
- 附件目录一旦存在即不可变；记录文件可变，靠 `rev` 单调闸门保护。

## 备选方案与被否决的原因
- 直接同步 SQLite 文件：无法合并，崩溃即整库损坏，且每次改动重传整库体积。
- 每实体一个目录含多修订文件：请求数翻倍（列目录 + 取文件），`PROPFIND` 成本在万级实体下不可接受。
- 不可变修订文件 + 指针文件：需要额外一轮写来做"指针提交"，崩溃点从 1 个变 2 个，更难收敛。
- 用 WebDAV `LOCK` 保证并发：多数服务器实现残缺（见 `locks/` 仅作尽力而为租约），把它当正确性依赖等于赌服务器行为。

## 后果
正面：任何崩溃点都能用"记录文件的 hash + rev"单点判定；清单异常最多导致多拉一次或晚一轮发现，不导致数据丢失。
代价：
- 服务器文件数随实体数线性增长（万级实体 → 万级文件），部分网盘对单目录文件数与列举频率有限制。
- 依赖 `MOVE` / `Overwrite:F` / `If-Match` 语义，必须做能力探测并按 S1/S2/S3 选路；S3（盲写复验）存在覆盖窗口，只能靠复验检出。
- `tmp/` 残留需要维护轮清理（>24 h），多一件后台工作。

## 验证方式
- 崩溃点矩阵 C1–C10 逐点 `CRASH(<点>)` + 重启断言不变式（TEST-PLAN 崩溃点矩阵 CP-01..10），每点必须给出确定的远端状态。
- `cap_mask` 探测的三种写入策略 S1/S2/S3 均能出包：以 `notera-test-webdav` 分别关闭条件 PUT 与 `Overwrite:F` MOVE，断言仍能收敛。
- "清单领先实体"场景（`FAIL` 或直接 `DELETE records/...` 而清单仍引用）必须走 `missing_remote`（SY-INT-02）且**不删本地**：断言本地条目数与 hash 不变。
- `STATS` 断言每轮 PUT 序列中 `manifest/index.json` 是该轮最后一个写请求（R2 的直接证明）。
- 路径安全单元测试：`id` 含 `..`、`/`、`?`、空格时 `RemotePath` 必须返回错误而非拼出路径。

## 关联
- SYNC-PROTOCOL.md §1 远端布局、§4.4 原子提交、§5 能力探测与写入策略、§11.3 崩溃点矩阵
- ARCHITECTURE-MAP.md §3 R1/R2、§4 改动路由（"改写入原子性"一行）
- ADR-0002（信封）· ADR-0004（清单结构）· ADR-0009（附件寻址）· ADR-0013（测试服务器需支持 MOVE/Overwrite/ETag）
- 实测：`docs/evidence/probe-windows-gnu.txt` 的 `webdav-custom-verbs`（`PROPFIND->207 verb_echoed=true depth_forwarded=true MOVE->201`）、`precondition-412-plumbing`（`stale etag -> 412, current etag -> 204`）

## 待人工确认
- `records/<kind>/` 的目录名存在两种写法：SYNC-PROTOCOL §1 用 `note`/`folder`，TEST-PLAN 的注入路径用 `records/n/<id>.json`。需在 Phase 1 前统一（建议短名 `n`/`f`/`a`，与清单条目 `t` 字段一致），否则契约 fixture 与测试注入会各测一套路径。
