-- 远端视图要能真正被复用，必须带删除时间戳。
--
-- `sync_remote_index` 的建表注释写着它的目的就是"清单的本地缓存：让每轮同步免于全量
-- 下载"，但生产路径上一直没有写入者 —— 原因不只是没接线：引擎的远端视图
-- (`notera_sync::LocalPort`/`RemoteView`) 带 `deleted_at` 时间戳，而这张表只有
-- deleted/purged 两个标志位。少了时间戳就没法还原 P8/P11 那类判据（删除先后、
-- 删后又改），把视图存进这张表反而会丢信息。所以先补列，再谈持久化。
--
-- 第一版未发布：直接加列，不写历史兼容层（用户 2026-09-27 拍板）。
ALTER TABLE sync_remote_index ADD COLUMN deleted_at TEXT;
