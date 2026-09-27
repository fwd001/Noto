 PARKED WORK — 不是待应用的补丁，是证据。
 状态：已回退，未进主干（主干 487/0 绿）。
 结论：分段下载跳过（LocalPort::cached_segment_hashes 接线）不能单独做 ——
   它依赖"远端视图可持久化"，而 HostLocalPort::cached_remote() 读的是一个从未被写入
   的 Mutex（恒空），sync_remote_index 表在生产路径上没有任何调用者，且缺
   deleted_at 列（引擎的 RemoteView 需要它）。实测：只接缓存不接视图 → 千条库
   第二台设备停在 198/1000 并报 NoOp（big_library.rs 的新断言当场抓到）。
 解除条件：先按 §9/§51 定"远端视图持久化"的迁移（含 deleted_at），再回头接这份缓存。
