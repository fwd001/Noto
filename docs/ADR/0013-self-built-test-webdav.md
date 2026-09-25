# ADR-0013: 自建真实 HTTP 测试 WebDAV 服务器

## 状态
日期：2026-09-25
状态：Accepted

## 背景
需求禁止用假 API 测同步。本项目的主要风险恰好落在协议细节上：`MOVE` 的 `Overwrite:F` 行为、`If-Match` 是否真返回 412、`Depth:infinity` 是否支持、半上传后服务器留什么。mock HTTP 层测不到这些，等于把风险留在唯一没被覆盖的地方。

## 决策
- 新建 crate `notera-test-webdav`：**真 TCP HTTP/1.1**（127.0.0.1），非 trait mock。
- 动词：`GET/HEAD/PUT/DELETE/MOVE/COPY/PROPFIND/PROPPATCH/OPTIONS/LOCK/UNLOCK`。
- 请求头与机制：`Depth`、`Destination`、`Overwrite`、`ETag`、`If-Match`、`If-None-Match`、`Range`、chunked 请求体。
- 控制面：`/_control/{reset,inject,stop,restart,log}`；状态面：`/_fs/dump`（服务端权威文件清单 + 内容 hash）。
- 两后端：`mem`（快，进程内，L1/L2 用）与 `fs`（跨进程重启保留状态，L3/L4 用）。
- 故障注入：延迟、断连、超时、401/403/404/409/412/500/507、半上传（截断 body）、清单损坏（坏 JSON / hash 不符）、服务器重启。
- 仅 dev/test 依赖，不得被产品 crate 依赖。

## 备选方案与被否决的原因
- mock HTTP 层（trait + 假实现）：测不到协议细节，而协议细节正是本项目的风险所在；还会让"客户端以为服务器支持条件 PUT"这类错误无声通过。
- 用现成 WebDAV 服务器（Apache / Nextcloud）做 CI：故障注入能力不足（无法稳定制造"已落盘但返回 500"），CI 依赖重（容器 + 服务启动时序）。保留为"真实兼容矩阵"的**补充**而非替代（SYNC-PROTOCOL §16 U1、CI-CD D5）。
- 依赖用户的真实服务器：不可重复、有数据风险、CI 无法运行，且真实凭据一旦入 CI 即不可召回。

## 后果
正面：S1/S2/S3 三种写入策略、崩溃点矩阵 C1–C10、错误分类与退避全部可在本机确定性复现；`STATS` 可断言"先写记录再写清单"。
代价：
- 要自己实现并维护一个 DAV 子集（约 1–2k 行），其**正确性本身需要被测试**——用 PROPFIND 一致性 / 文件系统快照双视图断言。
- 自建服务器行为可能与真实服务器不一致，导致"过了 test-webdav 却过不了坚果云"；因此兼容性结论必须等真实端点（当前 BLOCKED，B5/U1）。
- 控制面本身是测试基建的一部分，坏了会伪装成产品缺陷，排查成本高。

## 验证方式
- 注入必须确定可复现：同一 scenario 两次运行产生相同的 `STATS` 请求序列与 `/_fs/dump` 快照（除 `device`/时间字段外逐字节一致）。
- `/_fs/dump` 是**唯一允许**的服务端状态断言手段（禁止测试代码直接读 `mem` 内部结构），CI grep 检查测试代码不 import 其内部类型。
- 自测与产品测试分离编写：`notera-test-webdav` 的 L0/L1 用例（verb 覆盖、`Depth`/`Destination`/`Overwrite`、ETag、412/304、chunked、`mem`/`fs` 两模式）不得复用 `notera-webdav` 的断言代码。
- `fs` 模式专项：`RESTART` 后 `DUMP` 快照不变，且客户端不依赖服务端内存态（SY-INT 系列）。
- 探针已验证自定义动词与 412 语义可通：`PROPFIND->207 verb_echoed=true depth_forwarded=true MOVE->201`、`stale etag -> 412, current etag -> 204`。

## 关联
- TEST-PLAN.md §记法（test-webdav 控制调用：`FAIL/OFF/RESTART/DUMP/STATS/RESET/CRASH`）、L3/L4 层定义
- CI-CD.md §非目标（不做 HTTP mock、不接触真实账号）、§交接协议
- ARCHITECTURE-MAP.md §2 注册表（`notera-test-webdav` 行）、§5 禁止模式（把 mock 当真实 HTTP）
- SYNC-PROTOCOL.md §5 能力探测、§12 错误分类、§16 U1（真实服务器兼容矩阵 BLOCKED）
- 实测：`docs/evidence/probe-windows-gnu.txt` 的 `webdav-custom-verbs`、`precondition-412-plumbing`
- ADR-0010（代理独占模式由此服务器提供）· ADR-0012（迁移期崩溃注入）
