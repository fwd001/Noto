-- 附件「已回收」账（§6-9 第 4 项数据「回收字节数」的"已回收"那一读；缺口 G102 的修法）。
--
-- 一行 = 一次销毁落账（那一批销毁了几份、多少字节）。写在与
-- `DELETE FROM attachments` 同一笔事务里 —— 崩在中间时两边同生共死，
-- 不会出现"行没了、回收数没记"的漂账。
--
-- 只记**销毁**（GC 那一步不可逆的删除）；隔离、撤销隔离、重试取回都不落这笔账 ——
-- 那些事发生时字节还在（或又回来了），记进去就是把"已回收"说成了一件没发生的事。
CREATE TABLE attachment_reclaims (
  id    INTEGER PRIMARY KEY,
  at    TEXT NOT NULL,
  files INTEGER NOT NULL CHECK (files > 0),
  bytes INTEGER NOT NULL CHECK (bytes >= 0)
);
