//! GC 与磁盘体检加进**常驻附件轮**（每 20 s 一次）之后，每轮到底花多少时间 —— 千级附件规模下的实测。
//!
//! 为什么单独一个文件、而且是 `#[ignore]`：
//! * 这是**量级**证据，不是行为门禁。把它挂在默认跑的那一批里，CI 就会开始测机器的 SQLite
//!   手感（慢一点的 runner 红、快一点的绿），那是"抖动门禁"那一类（§40 / 台账里对 blackbox 立的规矩）。
//! * 但它必须被**跑过一次**并把数字写进台账：GC 与体检都往那一格里加了工作（一次候选查询 +
//!   最多 200 次 rename + 一次批量写事务），我只写了推理没量过 —— 推理不是证据。
//!
//! 跑法（会打印每一步的毫秒数）：
//! `cargo test -p notera-host --test attachment_gc_scale -- --ignored --nocapture`

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use notera_host::App;
use notera_store::Store;

static SEQ: AtomicUsize = AtomicUsize::new(0);

fn tmp_dir(tag: &str) -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let dir =
        std::env::temp_dir().join(format!("notera-gc-scale-{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 造 `total` 份各自独立的 blob（都标成"服务器已有副本"= GC 的准入条件），然后把其中
/// `orphan` 条笔记**永久删除** —— 剩下的那些仍被引用，用来证明"候选集不是整表扫"。
/// 默认每份 1 KiB（够让 rename 真的在搬东西）。
fn seed(store: &Store, total: usize, orphan: usize) -> Vec<String> {
    seed_len(store, total, orphan, 1024)
}

/// 同上，但每份字节是 `blob_len` 长。附件轮的代价分成两种付法：快路每条一次 `stat`
/// （跟 blob 多大无关），慢路每条**整份读 + 复算 SHA-256**（跟大小线性相关），所以量它
/// 必须能挑尺寸 —— "少而大"（一张照片）和"多而小"（一堆截图）是两笔不同的账。
fn seed_len(store: &Store, total: usize, orphan: usize, blob_len: usize) -> Vec<String> {
    let folder = notera_core::EntityId::parse(notera_store::DEFAULT_FOLDER_ID).unwrap();
    let mut shas = Vec::new();
    for i in 0..total {
        let doc = serde_json::json!({ "v": 1, "content": [
            { "id": "blk0000001", "type": "paragraph", "content": [{ "text": format!("规模样本 {i}") }] }
        ] });
        let note = store.create_note(&folder, doc).unwrap();
        // 每份字节都不同 ⇒ 不同的 sha。前 8 字节是这条的序号，其余按 k 铺开 —— 只用 `(k+i) % 251`
        // 会让周期撞车（251 是质数，i 相差 251 的两份内容一模一样），于是几条笔记共享同一个 sha，
        // "永久删除一条"就把引用数从 7 改成 6 而不是 0，前置直接不成立（实测就是这样红的）。
        let mut blob: Vec<u8> = (i as u64).to_le_bytes().to_vec();
        blob.extend((8..blob_len).map(|k| (k % 251) as u8));
        let sha = store
            .attach_blob(&note.id, &blob, "image/png", Some("p.png"), "blk0000002")
            .unwrap()
            .sha256;
        store
            .set_attachment_states(&sha, None, Some("present"))
            .unwrap();
        shas.push((note.id, sha));
    }
    let unique: std::collections::HashSet<&String> = shas.iter().map(|(_, s)| s).collect();
    assert_eq!(
        unique.len(),
        shas.len(),
        "前置：{} 份样本必须有 {} 个不同的 sha，否则一条笔记被删不会让引用归零",
        shas.len(),
        shas.len()
    );
    for (id, sha) in shas.iter().take(orphan) {
        store.purge_note(id).unwrap();
        assert_eq!(
            store.attachment_refs(sha).unwrap(),
            0,
            "前置：这一条要零引用"
        );
    }
    shas.into_iter().map(|(_, sha)| sha).collect()
}

fn ms(start: Instant) -> u128 {
    start.elapsed().as_millis()
}

/// 每一轮的实际开销：候选查询 + 最多 `cap` 次 rename + 一次批量写事务。
/// 断言只设**很宽**的界（防的是"量级跳了一个数量级"，不是"慢了几十毫秒"）。
#[test]
#[ignore = "规模基准：只在看 GC/体检代价时显式跑，数字进 TEST-PLAN 的 PERF-10"]
fn gc_and_sweep_per_round_cost_at_a_thousand_attachment_library() {
    const TOTAL: usize = 2000;
    const ORPHAN: usize = 1500;
    const CAP: usize = 200; // 与生产里 run_attachment_round 传的那两个上界一致

    let dir = tmp_dir("lib");
    let app = App::boot(&dir).expect("核心启动");
    let store = app.store();
    assert_eq!(app.list_folders().unwrap().len(), 1, "前置：默认本在");

    let started = Instant::now();
    let shas = seed(store, TOTAL, ORPHAN);
    let seeded = ms(started);
    println!("造库 {TOTAL} 条附件（永久删除 {ORPHAN} 条笔记）用时 {seeded} ms");

    // ① 体检的候选查询（available ∧ present，一千多条仍被引用的行都在它的扫描范围里）。
    let t = Instant::now();
    let repair = store.attachment_repair_candidates(CAP).unwrap();
    let repair_ms = ms(t);
    println!(
        "体检候选查询（cap={CAP}）：{} 条，{repair_ms} ms",
        repair.len()
    );

    // ② GC 的候选查询。
    let t = Instant::now();
    let candidates = store.gc_quarantine_candidates(CAP).unwrap();
    let cand_ms = ms(t);
    assert_eq!(candidates.len(), CAP, "前置：候选集要正好装满一轮的上界");
    println!("GC 候选查询（cap={CAP}）：{cand_ms} ms");

    // ③ 完整的一轮回收：最多 CAP 次 rename + 一次批量写事务。
    let t = Instant::now();
    let marked = app.reclaim_unreferenced_blobs(CAP);
    let round_ms = ms(t);
    assert_eq!(marked, CAP, "一轮要恰好认领 cap 条");
    println!("GC 一轮（{CAP} 次挪进隔离区 + 一次批量写）：{round_ms} ms");

    // ④ 跑完全部孤儿要几轮 —— 这才是"用户清空回收站之后常驻循环被占多久"的真答案。
    let t = Instant::now();
    let mut rounds = 0usize;
    while app.reclaim_unreferenced_blobs(CAP) > 0 {
        rounds += 1;
        assert!(rounds < 40, "轮数失控（上界没有真的生效）");
    }
    let drain_ms = ms(t);
    let total_rounds = ORPHAN.div_ceil(CAP);
    assert_eq!(
        rounds + 1,
        total_rounds,
        "{ORPHAN} 条 / 每轮 {CAP} 条 = {total_rounds} 轮（含上面那一步那一轮）"
    );
    println!(
        "把 {ORPHAN} 条全部挪进隔离区：{rounds} 轮，共 {drain_ms} ms（平均每轮 {} ms）",
        drain_ms / rounds as u128
    );

    // ⑤ 销毁那一侧：宽限期一过，1500 条要几轮、每轮多少毫秒（先删行再删字节）。
    // **这里量的是本地那两步（删行 + 删文件）**：生产在动手之前还会向远端确认一次
    // （`confirm_still_remote`，一条一个零字节体的 HEAD），那个网络代价不在下面这些数字里 ——
    // 这个基准没有服务器，硬把它算进来只会让"每轮多少毫秒"变成"本机磁盘的手感 + 一次往返"。
    // 写在这里是因为读这些数字的人需要知道它没包含什么。
    let t = Instant::now();
    let mut purged_total = 0usize;
    let mut prows = 0usize;
    loop {
        let ready = app
            .store()
            .gc_ready_to_purge("9999-01-01T00:00:00.000Z", CAP)
            .unwrap();
        if ready.is_empty() {
            break;
        }
        let n = app.purge_verified_blobs(&ready);
        purged_total += n;
        prows += 1;
        assert!(prows < 40, "销毁轮数失控");
    }
    let purge_ms = ms(t);
    assert_eq!(
        purged_total, ORPHAN,
        "回收了多少条就该销毁多少条（少的那部分是 GC 漏掉的）"
    );
    assert!(
        store.gc_quarantine_candidates(10).unwrap().is_empty(),
        "回收完了候选集该是空的"
    );
    println!(
        "销毁 {ORPHAN} 条（{prows} 轮，每轮一次批量删行 + {CAP} 次 remove）：共 {purge_ms} ms（平均每轮 {} ms）",
        purge_ms / prows as u128
    );

    // 仍在被引用的那些一份都不许少。
    for sha in shas.iter().skip(ORPHAN) {
        assert!(
            Path::new(&store.blob_path(sha)).exists(),
            "还有引用的那份字节被动过了：{sha}"
        );
    }
    println!("仍然被引用的 {} 条字节全部还在正式位置 ✓", TOTAL - ORPHAN);

    // ⑥ **空转那一格才是常驻循环每 20 s 的真代价**：没有东西可收时，一轮应该只是一次索引查询。
    // 上面那些 900 ms 量级的数字只在"用户刚清空回收站"那种爆发里出现，八轮就退场；
    // 而这一格是每天都付的钱。
    let t = Instant::now();
    for _ in 0..10 {
        assert_eq!(app.reclaim_unreferenced_blobs(CAP), 0, "无事可做时不该认领");
        assert_eq!(
            app.purge_verified_blobs(
                &app.store()
                    .gc_ready_to_purge("9999-01-01T00:00:00.000Z", CAP)
                    .unwrap()
            ),
            0,
            "账上没有已隔离的行时销毁清单必须是空的"
        );
    }
    let idle_ms = ms(t);
    println!(
        "空转一轮（回收 + 销毁都无事可做）：平均 {} ms（10 轮共 {idle_ms} ms）",
        idle_ms / 10
    );
    assert!(
        idle_ms / 10 < 50,
        "无事可做的一轮慢到 {} ms：那已经不是'顺手扫一眼'",
        idle_ms / 10
    );

    // 宽得多的界：量级跳了才红（这台机器上的实测数字进台账，不靠这三行说话）。
    assert!(
        cand_ms < 1_000,
        "GC 候选查询慢到 {cand_ms} ms：判据或索引出了问题"
    );
    assert!(
        round_ms < 5_000,
        "一轮回收慢到 {round_ms} ms：常驻循环那一格会被它占住"
    );
    assert!(
        purge_ms < 30_000,
        "整体销毁慢到 {purge_ms} ms：说明每轮上界没起作用"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// PERF-10 明确欠着的那个量：**附件轮开工前的磁盘体检**（`sweep_lost_local_blobs`）在
/// 100 / 1000 两份附件库下每轮花多少。加自愈（0.0.19）与上界（G4）时只写了成本推理，
/// 推理不是证据 —— 这一格把它量出来。
///
/// 三条路都要量，因为它们付的是三种不同的钱：
/// * **快路**（每天都付）：一次候选查询 + 每条候选一次 `stat`，一个字节都不读。
/// * **慢路**（刚导入 / 刚升级那一轮）：登记尺寸与实测长度对不上 ⇒ 整份读 + 复算 SHA-256，
///   相符则回填尺寸。回填这件事必须在千级规模下**真的生效**，否则每一天都在付慢路。
/// * **爆发**（整个 `attachments/` 目录没了）：一轮攒满 `cap` 条降级、一次写事务落账。
///
/// 跑法同上一个：`cargo test -p notera-host --test attachment_gc_scale -- --ignored --nocapture`
#[test]
#[ignore = "规模基准：只在量磁盘体检的每轮代价时显式跑，数字进 TEST-PLAN 的 PERF-10"]
fn disk_sweep_per_round_cost_at_100_and_1000_attachment_library() {
    // 与生产里 `run_attachment_round` 传给体检的那个上界同源（lib.rs 里的 `SWEEP_CAP`）。
    const SWEEP_CAP: usize = 200;
    const ROUNDS: usize = 10;

    // "少而大"（一张 64 KiB 的图，慢路要整份读）与"多而小"（1 KiB，一轮装满 cap）。
    for (total, blob_len) in [(100usize, 64 * 1024usize), (1000usize, 1024usize)] {
        println!("\n=== 附件库 {total} 份，每份 {blob_len} 字节（体检上界 {SWEEP_CAP} 条/轮）");
        let dir = tmp_dir("sweep");
        let app = App::boot(&dir).expect("核心启动");
        let store = app.store();

        let started = Instant::now();
        seed_len(store, total, 0, blob_len);
        println!("造库 {total} 份用时 {} ms", ms(started));

        // 前置：候选集真的装满这一轮的上界（100 那一档不足 cap，就是 total）。
        let expected = total.min(SWEEP_CAP);
        let seen = store.attachment_repair_candidates(SWEEP_CAP).unwrap();
        assert_eq!(
            seen.len(),
            expected,
            "前置：体检一轮该看到 {expected} 条候选，实际 {} 条 —— 夹具没造起来，下面的数不作数",
            seen.len()
        );

        // ① 快路：账与盘一致 ⇒ 每条一次 stat，降级 0 条。
        let mut fast_total = 0u128;
        let mut fast_worst = 0u128;
        for _ in 0..ROUNDS {
            let t = Instant::now();
            assert_eq!(
                app.sweep_lost_local_blobs(SWEEP_CAP),
                0,
                "前置：账盘一致的库不该有任何一条被降级（降了就是在改数据，不是在量时间）"
            );
            let one = ms(t);
            fast_total += one;
            fast_worst = fast_worst.max(one);
        }
        println!(
            "快路一轮（{expected} 次 stat）：平均 {} ms，最坏 {fast_worst} ms（{ROUNDS} 轮共 {fast_total} ms）",
            fast_total / ROUNDS as u128
        );

        // ② 慢路：把登记尺寸全部改成"比实测大 1"—— 这正是那条 `MAX(旧, 新)` 登记规则
        // 纠不掉的偏大声明值，所以生产里真的会出现（G3 写的是它，这一格量的是它）。
        // 夹具自己走批量那一条（`set_attachment_sizes`）：逐行提交的原值已经量过并记在
        // PERF-10 与方法的 doc 里（100 次 106 ms / 1000 次 1151 ms ≈1.1 ms/次），
        // 那正是把回填改成批量的理由，所以这里不再重复付那笔钱。
        let t = Instant::now();
        let stale: Vec<(String, i64)> = store
            .attachment_repair_candidates(total)
            .unwrap()
            .into_iter()
            .map(|(sha, size)| (sha, size + 1))
            .collect();
        assert_eq!(stale.len(), total, "前置：现场该把 {total} 条都列进候选集");
        store.set_attachment_sizes(&stale).unwrap();
        println!("（夹具）一次批量把 {total} 条登记尺寸写歪用时 {} ms", ms(t));
        let t = Instant::now();
        assert_eq!(
            app.sweep_lost_local_blobs(SWEEP_CAP),
            0,
            "慢路那一轮哈希相符 ⇒ 一条都不该降级（降级=误判用户的数据坏了）"
        );
        let slow_ms = ms(t);
        println!(
            "慢路一轮（{expected} 次整份读 {} MiB + 复算哈希）：{slow_ms} ms",
            (expected * blob_len) as f64 / 1048576.0
        );

        // 回填必须**真的生效**：这正是 G3 的卖点，千级规模下没生效就等于每天都在付慢路。
        let after = store.attachment_repair_candidates(SWEEP_CAP).unwrap();
        assert_eq!(after.len(), expected, "前置：候选集还是那么多条");
        for (sha, size) in &after {
            assert_eq!(
                *size, blob_len as i64,
                "这一条的登记尺寸没回填成实测长度：下一轮还要整份复算一次哈希（{sha}）"
            );
        }
        let t = Instant::now();
        assert_eq!(app.sweep_lost_local_blobs(SWEEP_CAP), 0);
        let back_to_fast = ms(t);
        println!(
            "回填之后紧接着的下一轮：{back_to_fast} ms（慢路是 {slow_ms} ms —— 慢路该只付这一次）"
        );

        // ③ 爆发：把**整个库**的字节从盘上拿走（"整个 attachments 目录没了"那一形），
        // 第一轮该降级 min(total, cap) 条，剩下的按每轮上界往前浮。
        let doomed: Vec<String> = store
            .attachment_repair_candidates(total)
            .unwrap()
            .into_iter()
            .map(|(sha, _)| sha)
            .collect();
        assert_eq!(doomed.len(), total, "前置：这一档该拿满 {total} 条");
        for sha in &doomed {
            let _ = std::fs::remove_file(store.blob_path(sha));
        }
        let t = Instant::now();
        let demoted = app.sweep_lost_local_blobs(SWEEP_CAP);
        let burst_ms = ms(t);
        assert_eq!(
            demoted, expected,
            "整目录没了的第一轮该把这一轮上界内的 {expected} 条都降级（一次写事务），实际 {demoted} 条"
        );
        println!("爆发一轮（{demoted} 条降级 = 一次写事务）：{burst_ms} ms");

        // 全库被删光要几轮才降级完 —— 常驻循环每 20 s 一轮，这个轮数就是"界面多久停止转圈"。
        let mut rounds = 1usize;
        while app.sweep_lost_local_blobs(SWEEP_CAP) > 0 {
            rounds += 1;
            assert!(
                rounds < 20,
                "轮数失控（每轮上界没生效，或候选集把已降级的行还算进来）"
            );
        }
        println!(
            "把 {total} 条全部降级：{rounds} 轮（每轮上界 {SWEEP_CAP}）⇒ 常驻循环按 20 s 一轮算，全库过一遍约 {} s",
            rounds * 20
        );
        assert_eq!(
            rounds,
            total.div_ceil(SWEEP_CAP),
            "{total} 条 / 每轮 {SWEEP_CAP} 条该是 {} 轮",
            total.div_ceil(SWEEP_CAP)
        );

        // 宽界：防的是"量级跳了一个数量级"。精确数字进台账，不靠这三行说话。
        assert!(
            fast_worst < 1_000,
            "快路最坏一轮 {fast_worst} ms：常驻循环那一格（每 20 s）不该付到毫秒级以上"
        );
        assert!(
            slow_ms < 10_000,
            "慢路一轮 {slow_ms} ms：回填之后还在每轮重读整库的话，这一格就成了常驻成本"
        );
        assert!(
            burst_ms < 10_000,
            "爆发一轮 {burst_ms} ms：一次批量写事务不该排到这么久（G4 那条门回归）"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
