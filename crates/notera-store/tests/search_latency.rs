//! 搜索延迟门禁（用户口径：「精准匹配和模糊匹配共同的去搜索，**效率一定要很高**」）。
//!
//! 这条测试的存在理由：以前"快"只是 DATA-MODEL §7.2 里 2026-09 的一次手工读数，
//! 代码改了没人再量 ⇒ 慢化不会有红。这里把量具钉进 `cargo test`：
//! 5000 篇真笔记（经真 `create_note` 写入 ⇒ 走完整的派生 + FTS 维护 + outbox），
//! 覆盖三档查询（≤2 字 LIKE、≥3 字 FTS 精准、模糊、无结果），每种重复若干次，
//! 断言打在 **p95** 上，并把全部读数原样打出来 —— 阈值一改，历史读数要还能对得上。
//!
//! 阈值怎么来的：2026-10-04 本机（Windows x64 / GNU 工具链 / bundled SQLite）实测记在下面
//! 那条 `notes` 输出里，阈值取"实测 p95 的若干倍"向上取整，为 CI 的慢机器留余量。
//! 这条判据不追求精确到毫秒，它追求的是**数量级回归一定红**（见文件末尾的变异说明）。
mod common;

use common::*;
use notera_store::{SearchQuery, Store};
use std::time::Instant;

const NOTES: usize = 5_000;
const REPS: usize = 12;

/// 三档都要有样本：漏掉任何一档，"慢"可能只发生在没测的那一档。
const QUERIES: &[&str] = &[
    "同步",              // ≤2 字 → LIKE 全表扫
    "附件",              // ≤2 字 → LIKE，命中多
    "同步机制",          // ≥3 字 → FTS 精准
    "数据同步协议",      // ≥3 字 → 精准 + 模糊两档
    "冲突解决策略",      // ≥3 字，语料里有连着的那篇
    "同步 机制",         // 两段：交集（LIKE × LIKE）
    "数据同步 独立队列", // 两段混合长度（FTS × LIKE）
    "sync",              // 拉丁短文
    "查无此词啊",        // 无结果 —— 空结果集也必须快
];

/// 语料：绝大多数篇不含查询词（真实库里"没有"才是常态），
/// 少数几篇专门喂给精准档与模糊档 —— 模糊档的那篇**故意把三段三字串拆开摆**。
fn seed(store: &Store, folder: &notera_core::EntityId) {
    for i in 0..NOTES {
        let body = format!(
            "第 {i} 篇会议记录：本期讨论了数据同步机制的两段式实现，冲突解决策略另行评审；\
             附件上传走独立队列，队列长度上限 {tail}。其余条目待补。备注备注备注备注备注备注备注备注备注备注备注备注。",
            tail = i % 9,
        );
        store.create_note(folder, doc_text(&body)).expect("seed");
    }
    // 精准档："数据同步协议"连着出现
    store
        .create_note(folder, doc_text("这一篇写着数据同步协议的边界条件"))
        .unwrap();
    // 模糊档：查询「数据同步协议」的四个三字串（数据同/据同步/同步协/步协议）全在，
    // 但**没有**连着出现的那一个串 ⇒ 精准档必须抓不到它，只有放宽能。
    store
        .create_note(
            folder,
            doc_text("先做数据同步的预演，同步协调另开一条线，下一步协议的边界还没定"),
        )
        .unwrap();
    // 拉丁样本
    store
        .create_note(folder, doc_text("A short English note about sync ordering"))
        .unwrap();
}

#[test]
fn search_latency_stays_within_the_measured_budget() {
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);

    let seeded = Instant::now();
    seed(&store, &folder);
    let stats = store.stats().unwrap();
    assert_eq!(stats.notes as usize, NOTES + 3, "夹具没种满：{stats:?}");
    assert_eq!(
        stats.fts_rows as usize,
        NOTES + 3,
        "FTS 行数必须与权威表一致（I5），否则量的不是同一份数据：{stats:?}"
    );

    // 先证明这些查询**真的命中**：空结果集的"快"是假绿（§45 那条不会红的断言比没有更糟）。
    let exact = store
        .search(&SearchQuery::new("数据同步协议"))
        .unwrap()
        .into_iter()
        .filter(|h| h.snippet_html.contains("<mark>数据同步协议</mark>"))
        .count();
    assert!(exact >= 1, "精准档没命中任何一篇，延迟数字量的是空转");
    let fuzzy = store
        .search(&SearchQuery::new("数据同步协议"))
        .unwrap()
        .into_iter()
        .filter(|h| matches!(h.match_kind, notera_store::MatchKind::Fuzzy))
        .count();
    assert!(
        fuzzy >= 1,
        "模糊档在 5000 篇的库上没捞出那篇拆开的，放宽没生效"
    );

    let mut samples: Vec<(String, u128)> = Vec::new();
    for q in QUERIES {
        // 每条先跑 2 次预热（页缓存、prepare_cached 的语句缓存都是首次贵）
        for _ in 0..2 {
            store.search(&SearchQuery::new(*q)).unwrap();
        }
        for _ in 0..REPS {
            let t = Instant::now();
            let hits = store.search(&SearchQuery::new(*q)).unwrap();
            let us = t.elapsed().as_micros();
            samples.push((format!("{q} hits={}", hits.len()), us));
        }
    }
    let mut only_us: Vec<u128> = samples.iter().map(|s| s.1).collect();
    only_us.sort_unstable();
    let pct = |p: usize| only_us[((only_us.len() - 1) * p / 100).min(only_us.len() - 1)];
    let (p50, p95, max) = (pct(50), pct(95), only_us[only_us.len() - 1]);

    let per_query: Vec<String> = QUERIES
        .iter()
        .map(|q| {
            let mut v: Vec<u128> = samples
                .iter()
                .filter(|s| s.0.starts_with(&format!("{q} ")))
                .map(|s| s.1)
                .collect();
            v.sort_unstable();
            format!(
                "  {q:<12} n={:>3} p50={:>6} µs  max={:>7} µs",
                v.len(),
                v[v.len() / 2],
                v[v.len() - 1]
            )
        })
        .collect();
    println!(
        "\n搜索延迟实测 · {NOTES} 篇库 · {} 次采样（预热未计入）\n{}  播种耗时 {:?}\n  p50 = {p50} µs\n  p95 = {p95} µs\n  max = {max} µs",
        samples.len(),
        per_query.join("\n"),
        seeded.elapsed(),
    );

    // 预算是**两个实测点**之间定的，不是拍的（都是 5000 篇、都是 debug 构建、本机 Windows x64）：
    //  · 今天这版（AND 下推进一条 SQL）：p50 = 10.4 ms / p95 = 20.9 ms / max = 22.1 ms
    //  · 上一版（每段各取不限条数的候选集再在 Rust 里交集）：p50 = 33.9 ms / p95 = 149 ms / max = 157 ms
    // p95 卡在 80 ms = 中间量级：正常波动（CI 机器慢两三倍）不红，而"把 LIMIT 丢了 / 每条候选都
    // 去取正文"这一类数量级的慢化一定红 —— 上面那版就是 149 ms，摆在这儿它自己会拦自己。
    const P95_BUDGET_US: u128 = 80_000;
    // 常用那一档（p50）单独再卡一道：尾部可以容忍，"平常那次击键"不行。
    const P50_BUDGET_US: u128 = 40_000;
    assert!(
        p50 <= P50_BUDGET_US,
        "p50 = {p50} µs / 预算 {P50_BUDGET_US} µs 超了（p95={p95} µs, max={max} µs）"
    );
    assert!(
        p95 <= P95_BUDGET_US,
        "p95 = {p95} µs / 预算 {P95_BUDGET_US} µs 超了（p50={p50} µs, max={max} µs）"
    );
    // 单条查询也不许长尾到"按下去没反应"的程度
    assert!(
        max <= P95_BUDGET_US * 3,
        "最慢的一条 = {max} µs，长尾失控（p95={p95} µs）"
    );
    assert!(
        store.verify().is_empty(),
        "跑完延迟夹具之后不变式仍要成立：{:?}",
        store.verify()
    );
}
