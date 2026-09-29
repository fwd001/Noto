//! 规模量具（`#[ignore]`，只显式跑）：0.0.38 给"本轮计划输入"加的那条查询
//! （`Store::remote_moved_entities`，见 ADR-0021 D4 / 缺口 G20）在 20000 行上到底花多少。
//!
//! 为什么单独一个文件而不是断言在门禁里：这条 lane 只**出数**，不判绿 ——
//! 与 `scripts/measure-startup-breakdown.mjs` 同一条口径（判绿归 `sync_cost` 那条 PERF-05 lane）。
//! 台账里那条"每轮代价未量"的 §40 记账，靠这里打出来的数结掉。
//!
//! 跑法：
//!   cargo test -p notera-store --test plan_input_scale -- --ignored --nocapture
mod common;

use common::*;
use notera_core::{EntityKind, Rev};
use notera_store::{RemoteIndexEntry, Store};
use std::time::{Duration, Instant};

const N: usize = 20_000;

/// 三种形状各跑 5 次，报最好与中位数（单次跑不算数 —— 这台机器的散布见过 44~94 ms）。
/// **两条查询分开计时**：合在一起量就分不出"本轮新加的那一条"到底值多少（先验量具再谈结论）。
fn time_five(store: &Store, label: &str) {
    let mut moved: Vec<Duration> = Vec::new();
    let mut dirty_runs: Vec<Duration> = Vec::new();
    let mut candidates = usize::MAX;
    let mut dirty = usize::MAX;
    for _ in 0..5 {
        let t = Instant::now();
        let c = store
            .remote_moved_entities(notera_store::LOCAL_ACCOUNT_ID)
            .unwrap();
        moved.push(t.elapsed());
        candidates = c.len();
        let t = Instant::now();
        let d = store
            .dirty_entities(notera_store::LOCAL_ACCOUNT_ID)
            .unwrap();
        dirty_runs.push(t.elapsed());
        dirty = d.len();
    }
    moved.sort();
    dirty_runs.sort();
    println!(
        "PLAN-INPUT {label}: 新加的那条 remote_moved_entities best {:.2} / 中位 {:.2} ms；原有 dirty_entities best {:.2} / 中位 {:.2} ms（候选 {} 行，脏 {} 行）",
        moved[0].as_secs_f64() * 1_000.0,
        moved[moved.len() / 2].as_secs_f64() * 1_000.0,
        dirty_runs[0].as_secs_f64() * 1_000.0,
        dirty_runs[dirty_runs.len() / 2].as_secs_f64() * 1_000.0,
        candidates,
        dirty
    );
}

#[test]
#[ignore = "规模量具：20000 行建库要几十秒，不进每次门禁（与 attachment_gc_scale 同一口径）"]
fn plan_input_query_cost_at_twenty_thousand_rows() {
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);
    let acct = notera_store::LOCAL_ACCOUNT_ID;

    let t = Instant::now();
    let mut ids = Vec::with_capacity(N);
    for i in 0..N {
        ids.push(create(&store, &folder, &format!("规模量具 {i}")).id);
    }
    let seeded = t.elapsed();
    // 全部确认到远端一致点 → 这些行都是**干净**的：脏集为空，正是 G20 那一类"进不了计划"的行。
    let t = Instant::now();
    for id in &ids {
        let rev = store.get_note(id).unwrap().unwrap().rev;
        store
            .mark_synced(EntityKind::Note, id, rev, "sha256:scale")
            .unwrap();
    }
    let fr = store.get_folder(&folder).unwrap().unwrap().rev;
    store
        .mark_synced(EntityKind::Folder, &folder, fr, "sha256:scale")
        .unwrap();
    let confirm = t.elapsed();

    let st = store.stats().unwrap();
    assert_eq!(st.notes as usize, N, "夹具没造对：库里的笔记数不是 {N}");
    assert_eq!(
        st.dirty_notes, 0,
        "全部确认之后脏集必须是空的（不然量的就不是这一格）"
    );
    assert!(
        st.notes_trash == 0,
        "夹具要求全是活行：回收站里有 {n} 条",
        n = st.notes_trash
    );
    println!(
        "PLAN-INPUT 建库 {:?} / 全部确认 {:?}（{} 行，DB {:.2} MiB）",
        seeded,
        confirm,
        N,
        st.db_bytes as f64 / 1_048_576.0
    );

    // ① 稳态：远端视图与本机确认点**逐条相同** → 候选集应为空（这就是"平时每轮多出来的那一点"）
    let same: Vec<RemoteIndexEntry> = ids
        .iter()
        .map(|id| RemoteIndexEntry {
            kind: EntityKind::Note,
            id: id.clone(),
            rev: Rev(1),
            hash12: Some("deadbeefcafe".into()),
            size: Some(220),
            deleted: false,
            purged: false,
            seg: None,
            sha256: None,
            deleted_at: None,
        })
        .collect();
    store.remote_index_replace(acct, &same).unwrap();
    assert_eq!(
        store.remote_moved_entities(acct).unwrap().len(),
        0,
        "视图与确认点相同就不该有候选（否则量的不是稳态）"
    );
    time_five(&store, "① 稳态（视图=确认点，候选 0）");

    // ② 最坏：整库都要追（视图全部领先一格）→ 候选 = 全表，这才是这条查询真正的活
    let ahead: Vec<RemoteIndexEntry> = same
        .iter()
        .cloned()
        .map(|mut e| {
            e.rev = Rev(2);
            e
        })
        .collect();
    store.remote_index_replace(acct, &ahead).unwrap();
    assert_eq!(
        store.remote_moved_entities(acct).unwrap().len(),
        N,
        "最坏形状要真的造出来：候选数不是 {N}"
    );
    time_five(&store, "② 全库待追（候选 = 20000）");

    // ③ 删除也要走同一格：把视图改成"对面删了"，候选仍应是全表（P10/P20 的输入）
    let deleted: Vec<RemoteIndexEntry> = ahead
        .iter()
        .cloned()
        .map(|mut e| {
            e.deleted = true;
            e.deleted_at = Some("2026-09-29T00:00:00Z".into());
            e
        })
        .collect();
    store.remote_index_replace(acct, &deleted).unwrap();
    time_five(&store, "③ 全库对面已删（候选 = 20000）");
}
