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
fn seed(store: &Store, total: usize, orphan: usize) -> Vec<String> {
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
        // 取 1 KiB：足够让 rename 成为"真的在搬东西"而不是空操作。
        let mut blob: Vec<u8> = (i as u64).to_le_bytes().to_vec();
        blob.extend((8..1024).map(|k| (k % 251) as u8));
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
