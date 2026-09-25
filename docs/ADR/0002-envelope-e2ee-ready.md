# ADR-0002: 记录信封 v1 即预留 E2EE 结构

## 状态
日期：2026-09-25
状态：Accepted

## 背景
远端记录一旦按明文 JSON 落盘，日后启用端到端加密等于重写 WebDAV 上的全部历史数据：需要双读兼容期、需要逐条下载-解密-重传，期间任何中断都会留下混合形态。结构决策必须在 v1 做出，因为 v1 之后面对的就是用户的真实数据。

## 决策
- 每条远端记录是一个信封：
  `{protocol, kind, id, rev, sync_rev, hash, updated_at, device, deleted_at, purged, enc{alg,kid,nonce,hash_alg}, payload, ct}`
- `payload` 与 `ct` 严格互斥：`enc.alg="none"` 时 `payload` 非空、`ct` 为 null；启用加密时反之。
- v1 固定 `enc.alg="none"`：结构预留，功能默认关闭。
- 开启加密是协议的**次版本升级**，不是目录布局重写。
- `enc.kid`（密钥标识）与 `enc.nonce` 一并预留，避免启用时再加字段。
- 启用加密后 `hash_alg` 必须从 `sha256` 转为 `hmac-sha256`，否则密文之外仍泄露明文内容的等价指纹。

实测（`docs/evidence/probe-windows-gnu.txt`）：AES-256-GCM-SIV `nonce=12B key=32B tag_overhead=16B (ct 30B <- pt 14B)`；篡改一字节必被拒（`rejected: aead::Error`）。

## 备选方案与被否决的原因
- v1 明文、v2 再加密：破坏性重写 + 需要双读兼容期，风险最高；兼容期内的半迁移状态是最难测的形态。
- v1 直接实装 E2EE：密钥丢失即数据全丢，必须先有恢复码/托管体系才能上线，超出第一版范围。经人工确认选择"预留但默认关闭"。
- 只在客户端设置里放加密开关、信封不加 `enc`：开关无法解释远端已有数据的形态，等于把 v1 布局写成"明文专用"，退回方案一的问题。

## 后果
正面：加密从"重写全库"降级为"逐条 payload→ct 的次版本迁移"；密文形态有独立的字节级契约可测。
代价：
- 每条信封约 100–200 字节元数据开销（含 `enc`、哈希、时间戳），短笔记的信封占比明显。
- 调试时不能直接肉眼读服务器文件，需经 `notera-cli manifest dump` / 记录 dump 才能还原内容。
- `hash_alg` 切换意味着历史记录 `hash` 全部重算，迁移工具必须与加密开关同期交付。

## 验证方式
- 契约测试双向兼容：`fixtures/envelope-v1-alg-none.json` 与 `fixtures/envelope-v1-alg-aes-256-gcm-siv.json` 两形态，新客户端读旧样本、旧客户端读新样本（只读降级）均通过。
- 属性测试：对任意 `payload`，`seal` 后断言 `len(ct) = len(payload) + 16`；`open(ct)` 逐字节还原；随机改一位密文必失败。
- 互斥校验断言：构造 `payload` 与 `ct` 同时非空的信封，断言被判为协议错误、丢弃响应且**不写库**（I6），outbox 不被清空。
- 信封字段集与 SYNC-PROTOCOL §3 约束表逐条对齐（`kind`/`id` 与路径一致、`rev` 单调、`hash` 与 canonical 一致）。

## 关联
- SYNC-PROTOCOL.md §3 记录信封、§15 版本演进规则
- DATA-MODEL.md §11 记录信封、§1 记法约定
- ADR-0003（远端布局）· ADR-0009（附件密文寻址未决 U5）· ADR-0016（本地静态加密，独立议题）
- 实测：`docs/evidence/probe-windows-gnu.txt` 的 `envelope-aes-256-gcm-siv`、`envelope-tamper-detected`

## 待人工确认
- SYNC-PROTOCOL §3 规定 `purged:true` 的墓碑公告 `payload` 为 null，与本 ADR "payload 与 ct 恰好一个非空"的互斥式冲突。建议把墓碑写成互斥规则的唯一例外（`purged=true` 时两者皆 null），否则 v1 自己的永久删除传播会被校验闸门判为协议错误。
