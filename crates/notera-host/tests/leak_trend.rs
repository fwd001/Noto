//! 常驻循环的**泄漏趋势**长跑（§48 缺口 G10 的后半：PERF-10 里那条"30 min × 5 轮趋势"）。
//!
//! 为什么要有这一格：前面所有性能档量的都是"一次操作多少毫秒"，而泄漏是另一类失效 ——
//! 每轮都少还一点内存，单轮量不出来，跑一小时才塌。这一格量的就是那个"每轮都少一点"：
//! 两条常驻循环（文本 25 s / 附件 20 s）都在跑，中间每分钟做一次用户动作，
//! 每 `sample` 秒打一行账（笔记数 / 待发操作数 / 库字节）。
//!
//! RSS 不在这里读：**量具在 `scripts/measure-leak-trend.mjs`**，它按 PID 用 `Get-Process`
//! 采（与 `verify-perf` 那三档 RSS 同一个来源），所以采样这件事不占被测进程的堆。
//! 这里只负责"把生产那两条循环跑够久，并每分钟制造一点真活"。
//!
//! 跑法（默认 30 分钟；`NOTERA_LEAK_MINUTES` / `NOTERA_LEAK_SAMPLE_SECS` 可改）：
//! `cargo test -p notera-host --test leak_trend -- --ignored --nocapture --test-threads=1`
//!
//! **它证明不了什么**：服务器是仓库里的 `notera-test-webdav`（内存后端），所以这一格只覆盖
//! 我们这一侧的堆与账；真机上的 WebView2 那一块、以及真实 WebDAV 服务器的行为差异都不在里面
//! （那两条分别是 G13 与 G7/G8 的账）。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use notera_host::commands::{self, AccountDraftCmd};
use notera_host::App;
use notera_test_webdav::{Backend, TestServer};
use serde_json::json;

const SECRET: &str = "sup3r-s3cr3t";

fn data_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "notera-leak-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn doc(text: &str) -> serde_json::Value {
    json!({ "v": 1, "content": [
        { "id": "blk0000001", "type": "paragraph", "content": [{ "text": text }] }
    ] })
}

fn env_num(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(default)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "长跑基准：至少 1 分钟起，只在量常驻循环有没有漏时显式跑；数字进 TEST-PLAN 的 PERF-10"]
async fn resident_loops_leak_trend_run() {
    let minutes = env_num("NOTERA_LEAK_MINUTES", 30);
    let sample_secs = env_num("NOTERA_LEAK_SAMPLE_SECS", 30);

    let srv = TestServer::start(Backend::Mem).await;
    let dir = data_dir("dev");
    std::env::set_var("NOTERA_DEV_WEBDAV_SECRET", SECRET);
    let app = App::boot(&dir).expect("核心启动");
    let draft: AccountDraftCmd = serde_json::from_value(json!({
        "label": "长跑盘",
        "baseUrl": srv.base_url(),
        "username": "notera-leak",
    }))
    .expect("最小账户草案");
    app.configure_account(draft).expect("配置账户");

    // 现场：先攒出"用户已经用了一阵"的库 —— 200 条笔记 + 20 份附件，都还没传上去。
    // 空库跑 30 分钟是量不到东西的（每轮都是零工作）。
    let store = app.store();
    let folder = notera_core::EntityId::parse(notera_store::DEFAULT_FOLDER_ID).unwrap();
    for i in 0..200u32 {
        store
            .create_note(&folder, doc(&format!("长跑样本 {i}")))
            .unwrap();
    }
    for sha_id in 0..20u32 {
        let note = store.create_note(&folder, doc("带图的那条")).unwrap();
        let mut blob: Vec<u8> = (sha_id as u64).to_le_bytes().to_vec();
        blob.extend((8..32 * 1024).map(|k| (k % 251) as u8));
        store
            .attach_blob(&note.id, &blob, "image/png", Some("p.png"), "blk0000002")
            .unwrap();
    }
    println!(
        "LEAK-FIXTURE notes={} attachments_dir_exists={} minutes={minutes} sample_secs={sample_secs}",
        app.stats().unwrap().notes,
        dir.join("attachments").exists()
    );
    let baseline_notes = app.stats().unwrap().notes;

    // 与壳里那条路一模一样：探测 → 协商 → 两条循环各自 spawn（src-tauri/src/lib.rs §"首帧之后"）。
    let remote = app
        .remote_for_sync()
        .await
        .expect("装配适配器")
        .expect("已配置账户");
    app.negotiate(&remote).await.expect("协商通过");
    let stop = Arc::new(AtomicBool::new(false));

    let att = app.clone();
    let att_remote = Arc::clone(&remote);
    let att_stop = Arc::clone(&stop);
    let att_handle = tokio::spawn(async move { att.run_attachments(att_remote, att_stop).await });

    let sched = app.clone().start_sync(remote);
    let sched_handle = tokio::spawn(async move { sched.run().await });

    let started = Instant::now();
    // `NOTERA_LEAK_IDLE=1`：**不做任何用户动作**，只让两条循环自己空转。
    // 为什么要这一形：带着"每分钟建一条笔记"的那一形，30 分钟里库自己在长
    // （59 条笔记 / +295 KB），工作集的上涨就没法全算到循环头上。空转那一形把
    // 变量只剩"轮数"，才是"每轮漏一点"的干净读数 —— 两形一起看才分得清
    // "正当的工作集增长"和"真的不收敛"。
    let idle = std::env::var("NOTERA_LEAK_IDLE")
        .map(|v| v == "1")
        .unwrap_or(false);
    let mut samples = 0u64;
    while started.elapsed() < Duration::from_secs(minutes * 60) {
        samples += 1;
        // 每分钟一轮"用户动作"：新建 → 读回 → 改 → 列表 → 搜索 → 看同步态。
        // 泄漏多半就住在这几条的中间产物里（DTO、事件、待发队列），不是住在空闲轮里。
        if !idle {
            let created = commands::dispatch(
                &app,
                "create_note",
                json!({ "folderId": null, "doc": doc(&format!("长跑第 {samples} 轮")) }),
            )
            .expect("建笔记");
            let id = created["id"].as_str().unwrap().to_string();
            let _ = commands::dispatch(&app, "get_note", json!({ "id": id })).expect("读回");
            let rev = created["rev"].as_u64().unwrap();
            let _ = commands::dispatch(
                &app,
                "edit_note",
                json!({ "id": id, "doc": doc(&format!("长跑第 {samples} 轮·改")), "expectedRev": rev }),
            )
            .expect("改笔记");
            let _ = commands::dispatch(&app, "list_notes", json!({ "limit": 50 })).expect("列表");
            let _ = commands::dispatch(&app, "search", json!({ "text": "长跑" })).expect("搜索");
        }

        let s = app.stats().unwrap();
        let st = app.sync_status().unwrap();
        println!(
            "LEAK-SAMPLE n={samples} idle={} elapsed_s={} notes={} inflight_ops={} db_bytes={} pending_ops={} open_conflicts={} phase={}",
            if idle { 1 } else { 0 },
            started.elapsed().as_secs(),
            s.notes,
            s.inflight_ops,
            s.db_bytes,
            st.pending_ops,
            st.open_conflicts,
            st.phase,
        );
        tokio::time::sleep(Duration::from_secs(sample_secs)).await;
    }

    stop.store(true, Ordering::SeqCst);
    // 文本那条循环按 25 s 节拍走，附件那条按 20 s —— 收尾只给一个节拍 + 余量。
    // 这不是"等一个魔法数字"，是判据：stop 之后循环必须在这一拍内退出；退不出 = 卡在某一轮里。
    match tokio::time::timeout(Duration::from_secs(60), att_handle).await {
        Err(_) => panic!("附件循环在 stop 之后 60 s 还没退出：某一轮把它占住了"),
        Ok(Err(_)) => panic!("附件循环在长跑期间 panic 了"),
        Ok(Ok(())) => {}
    }
    assert!(
        !sched_handle.is_finished(),
        "文本循环提前退出了（panic 或某条 return）—— 长跑期间它必须一直在跑"
    );
    sched_handle.abort();
    let s = app.stats().unwrap();
    println!(
        "LEAK-END samples={samples} notes={} inflight_ops={} db_bytes={}",
        s.notes, s.inflight_ops, s.db_bytes
    );
    if idle {
        assert_eq!(
            s.notes, baseline_notes,
            "空转那一形不该新增笔记 —— 新增了就说明两形用的是同一份现场，趋势数对不上"
        );
    } else {
        assert!(
            s.notes >= baseline_notes + samples as u32,
            "长跑期间建的 {samples} 条笔记没都在账上：中间某一步静默丢了东西"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}
