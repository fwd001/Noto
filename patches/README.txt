PARKED WORK — 已落地，留作证据（2026-09-27）。
 主干状态：`cargo test --workspace` 514/0（61 个测试二进制）、clippy -D warnings 0/0、fmt 干净。

 这两份补丁是同一个决定的两级，现在**两级一起进了主干**：
   remote-view-persistence-step2.patch   远端视图可持久化（读/写 sync_remote_index）
   segment-cache-needs-remote-view.patch 基线分段内容哈希缓存 + 每轮最多下一次的门禁

 落地后的判据（TEST-PLAN SY-INT-14 的 ⑦）：千条库追平期间每个分段最多整份下载一次，
 同时 1000/1000 照样追平。变异自证三次，都红在真地方：
   ① 摘掉分段缓存写入 → "seg-0000.json 被整份下载了 6 次"；
   ② 把视图落盘条件写成 `!capped` → 第二台设备停在 198/1000；
   ③ 写侧按 `"note"` 而不是线上标签 `"n"` 匹配（整表被"未知类型"静默跳过）→ 同样 198/1000。
 ②③ 同一条教训：**跳过一次下载的前提是那份内容还在别处读得回来**（判据是"索引投影读全了"，
 不是"本轮没被截断"），而视图读写的两侧必须共用 `kind_tag`/`tag_kind` 这一对函数。

 当时"不划算所以回退"的判断是对的：单独进第 2 级确实是纯成本，且那条 `set_cached_remote`
 调用没有门禁守着。半修不交，这条规矩继续留着。

 —— keepboth-p11-repro.patch（2026-09-27，红着的复现，不是待应用的补丁）——
测试 `keep_both_on_a_p11_card_keeps_both_copies_and_reverts_the_remote_delete` 断言 P11 卡片上
按“保留两份”之后的三件事：① 正文仍是本机那一版 ② 卡片承诺的那份副本读得出同一版
③ 两边追平后笔记在对面那台也回来（“保留内容”=不采纳删除）。
第一条跑法：把补丁 apply 回 conflict_payload_e2e.rs，然后
  cargo +stable-x86_64-pc-windows-gnu test -p notera-host --test conflict_payload_e2e -- keep_both_on_a_p11
红在第几条与结论见本文件末尾的状态行。
