-- 0006_caps.sql —— 服务器能力探测结果的持久化位（SYNC-PROTOCOL.md §5）
-- §5 一直写着"结果存 sync_accounts.cap_mask"，但 0002 建表时没有这一列，
-- 于是探测结果无处可存、每次连接只能退回保守默认。这里补上那一列。

-- NULL = 从未探测过（此时适配器用 Caps::conventional()，宁可猜高：
-- 猜高了最坏是 412/405 后就地降级，猜低了会掉到 S3 盲写，那才有覆盖风险）。
-- 非 NULL = 实测位图，值即 notera_webdav::Caps::mask()。
ALTER TABLE sync_accounts ADD COLUMN cap_mask INTEGER;
-- 探测时间：§5 要求"首次连接与每日一次"，靠这个判断要不要重探。
ALTER TABLE sync_accounts ADD COLUMN caps_probed_at TEXT;
