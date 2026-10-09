//! §6「附件管理器」那一格的契约：只读命令 `attachment_inventory`（缺口 G99 的前半）。
//!
//! 为什么用真 `dispatch` 的真 JSON 而不是类型/假数据：这条边上的键名就是契约
//! （`stats` 那次 snake_case 直发让设置页三行恒为 `—`，而前端测试喂的是 camelCase，全程绿灯）。
//! 界面要说的三句话各自读一个数，所以三个数**分别从哪一列来**必须钉住：
//!  * 「有几份字节不在这台设备上」= `local_state != 'available'`；
//!  * 「有几份在隔离区里、到期可释放多少」= `deleted_at IS NOT NULL`；
//!  * 两者**不是**同一件事（GC 隔离那一步会顺手把 `local_state` 写成 missing，
//!    但"隔离"与"本机没字节"是两个来源）—— 这一条不是推理，是本测试第 3 项的现场。
//!
//! 还有一条容易写错的：`refs` 是 `COUNT(DISTINCT note_id)` 且**含回收站里的笔记** ——
//! 同一段字节被两篇笔记用到 = 两篇引用而**一份**对象；被同一篇的两个块用到 = **一篇**。
//!
//! 第 35 刀（逐份清单）在这一格上新增两列，两列都各有口径，所以各有断言：
//!  · **`name`**：账上 `filename` 空白折成"没有名字"（`null`），界面才不许画出一个空白名字；
//!  · **`isImage`** 而不是 `mediaType`：`image/png` 是协议词汇，过桥只是把它递到屏幕上的一条路
//!    （§8 第一问）。真值只由"是不是 `image/` 开头"这一个事实决定。
//!
//! 跑法：`cargo test -p notera-host --test attachment_inventory`
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use notera_host::{commands::dispatch, App};
use serde_json::{json, Value};

static SEQ: AtomicUsize = AtomicUsize::new(0);

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("notera-inventory-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn doc(text: &str) -> Value {
    json!({ "v": 1, "content": [{ "id": "blk000001", "type": "paragraph", "content": [{ "text": text }] }] })
}

fn call(app: &App, name: &str, args: Value) -> Value {
    match dispatch(app, name, args) {
        Ok(v) => v,
        Err(e) => panic!("{name} 没成功：code={}", e.code),
    }
}

fn new_note(app: &App, text: &str) -> notera_core::EntityId {
    let folder = app.default_folder_id().unwrap();
    let note = app.create_note(&folder, doc(text)).unwrap();
    notera_core::EntityId::parse(&note.id).unwrap()
}

#[test]
fn inventory_reports_three_situations_two_counters_and_a_countdown() {
    let dir = Tmp::new("three");
    let app = App::boot(dir.path()).expect("核心启动");

    // 同一段字节被两篇笔记各用一块：对象一份、refs 两篇。
    let a = new_note(&app, "第一篇用图");
    let b = new_note(&app, "第二篇也用图");
    let sha_shared = app
        .store()
        .attach_blob(
            &a,
            b"shared-bytes",
            "image/png",
            Some("one.png"),
            "blk000002",
        )
        .unwrap()
        .sha256;
    app.store()
        .attach_blob(
            &b,
            b"shared-bytes",
            "image/png",
            Some("two.png"),
            "blk000003",
        )
        .unwrap();
    // 只有账、没有字节（对面同步过来的一张图）。
    let sha_absent = "b".repeat(64);
    app.store()
        .register_remote_attachment(&sha_absent, 4096, "image/png")
        .unwrap();
    // 第三份：本机有字节。它稍后会变成"没人引用"，从而正是 GC 第一步认领的那种行。
    let c = new_note(&app, "要被彻底删掉的那篇");
    let sha_gc = app
        .store()
        .attach_blob(
            &c,
            b"quarantined-bytes",
            "image/png",
            Some("three.png"),
            "blk000004",
        )
        .unwrap()
        .sha256;

    // 第四份：**同一篇**的两个块用同一段字节 ⇒ 两份链接、一篇引用。
    // 没有这一份，`COUNT(DISTINCT note_id)` 和 `COUNT(*)` 在这张表上根本分不出来
    // （前三份都是"一篇一块"，两种写法算出同一个数）—— 而它正是"能不能释放"的判据来源。
    // 这一份同时是"有名字的**非图片**"：界面上那一行的名字与"这张图 / 这份文件"那个词
    // 各自来自两列，绑在一起写死就会有一种永远没被量过。
    app.store()
        .attach_blob(
            &a,
            b"two-blocks",
            "application/pdf",
            Some("报告.pdf"),
            "blk000005",
        )
        .unwrap();
    let sha_twice = app
        .store()
        .attach_blob(
            &a,
            b"two-blocks",
            "application/pdf",
            Some("报告.pdf"),
            "blk000006",
        )
        .unwrap()
        .sha256;

    // 第五份：账上的名字是**三个空格**。空名字不算名字 —— 界面上「· 40.0 MB」前面挂一个空白
    // 比挂"这个文件"难懂得多，所以"空白要折成没有名字"是核心的口径，得钉在这里。
    let sha_blank = app
        .store()
        .attach_blob(&b, b"blank-name", "video/mp4", Some("   "), "blk000007")
        .unwrap()
        .sha256;

    let got = call(&app, "attachment_inventory", json!({}));
    let rows = got["rows"].as_array().expect("rows 要的是数组");
    assert_eq!(rows.len(), 5, "五份对象，返回 {rows:?}");
    let find = |sha: &str| {
        rows.iter()
            .find(|r| r["sha256"] == json!(sha))
            .cloned()
            .unwrap_or_else(|| panic!("{sha} 这一行不在：{rows:?}"))
    };
    // 名字与"是不是图片"：四格组合各来一次（有名字×图、有名字×非图、没名字×图、空白名×非图）。
    for (sha, want_name, want_image, why) in [
        (
            sha_shared.clone(),
            json!("one.png"),
            json!(true),
            "图片且有名字",
        ),
        (
            sha_twice.clone(),
            json!("报告.pdf"),
            json!(false),
            "非图片且有名字",
        ),
        (
            sha_absent.clone(),
            Value::Null,
            json!(true),
            "只有账、没名字",
        ),
        (
            sha_blank.clone(),
            Value::Null,
            json!(false),
            "名字是空白 ⇒ 折成没名字",
        ),
    ] {
        let row = find(&sha);
        assert_eq!(row["name"], want_name, "{why}：{row}");
        assert_eq!(row["isImage"], want_image, "{why}：{row}");
    }
    let twice = find(&sha_twice);
    assert_eq!(
        twice["refs"],
        json!(1),
        "两段链接、一篇笔记 ⇒ refs 是 1（`COUNT(*)` 会算成 2）：{twice}"
    );
    assert_eq!(twice["localState"], json!("available"), "{twice}");
    let shared = find(&sha_shared);
    let absent = find(&sha_absent);
    assert_eq!(
        shared["refs"],
        json!(2),
        "同一段字节被两篇用到 ⇒ refs 是 2：{shared}"
    );
    assert_eq!(
        absent["refs"],
        json!(0),
        "没被任何一篇引用也要出这一行：{absent}"
    );
    assert_eq!(shared["localState"], json!("available"), "{shared}");
    assert_eq!(absent["localState"], json!("missing"), "{absent}");
    assert_eq!(absent["bytes"], json!(4096), "{absent}");
    // 没进隔离区的行：那一格是 null，界面就不许说倒计时。
    assert_eq!(shared["quarantinedUntil"], Value::Null, "{shared}");

    let totals = &got["totals"];
    assert_eq!(totals["count"], json!(5), "{totals}");
    assert_eq!(totals["bytes"], json!(12 + 4096 + 17 + 10 + 10), "{totals}");
    assert_eq!(totals["unavailableCount"], json!(1), "{totals}");
    assert_eq!(totals["unavailableBytes"], json!(4096), "{totals}");
    assert_eq!(totals["quarantinedCount"], json!(0), "{totals}");
    assert_eq!(totals["quarantinedBytes"], json!(0), "{totals}");

    // 键名就是契约：整排钉一次，漂一个字母就红。
    let mut names: Vec<String> = shared.as_object().unwrap().keys().cloned().collect();
    names.sort();
    assert_eq!(
        names,
        vec![
            "bytes".to_string(),
            "isImage".to_string(),
            "localState".to_string(),
            "name".to_string(),
            "quarantinedUntil".to_string(),
            "refs".to_string(),
            "remoteState".to_string(),
            "sha256".to_string()
        ],
        "行上的键名漂了：{names:?}"
    );
    let mut tk: Vec<String> = totals.as_object().unwrap().keys().cloned().collect();
    tk.sort();
    assert_eq!(
        tk,
        vec![
            "bytes".to_string(),
            "count".to_string(),
            "quarantinedBytes".to_string(),
            "quarantinedCount".to_string(),
            "unavailableBytes".to_string(),
            "unavailableCount".to_string()
        ],
        "totals 的键名漂了：{tk:?}"
    );

    // 彻底删除那篇 ⇒ 链接随之消失 ⇒ refs 归零（GC 认领的前提就是这一位）。
    app.store().purge_note(&c).unwrap();
    let orphaned = call(&app, "attachment_inventory", json!({}));
    let orphan_row = orphaned["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["sha256"] == json!(sha_gc))
        .expect("笔记删干净了，那份对象的账还要在（字节还在盘上）");
    assert_eq!(
        orphan_row["refs"],
        json!(0),
        "彻底删除之后 refs 还要算错，就是 SQL 的口径错了：{orphan_row}"
    );
    assert_eq!(
        orphan_row["localState"],
        json!("available"),
        "隔离之前字节还在本机：{orphan_row}"
    );

    // 隔离：把两份都塞进去，只有**没人引用**那一份该落地。
    let marked = app
        .store()
        .mark_attachments_quarantined(&[sha_shared.clone(), sha_gc.clone()])
        .unwrap();
    assert_eq!(
        marked, 1,
        "仍被引用的那份不该被隔离（返回数就是这件事的读数）：{marked}"
    );
    let after = call(&app, "attachment_inventory", json!({}));
    let still_referenced = after["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["sha256"] == json!(sha_shared))
        .unwrap();
    assert_eq!(
        still_referenced["quarantinedUntil"],
        Value::Null,
        "账上不该出现仍被引用那一份的倒计时：{still_referenced}"
    );
    let row = after["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["sha256"] == json!(sha_gc))
        .unwrap();
    let until = row["quarantinedUntil"].as_str().unwrap_or_default();
    assert!(!until.is_empty(), "隔离之后要有到期时刻：{row}");
    let expiry = notera_core::Timestamp::parse(until)
        .and_then(|t| t.as_millis())
        .unwrap_or_else(|| panic!("到期时刻不是合法时间戳：{until}"));
    let now = notera_core::Timestamp::parse(&app.store().now())
        .and_then(|t| t.as_millis())
        .expect("now");
    let days = (expiry - now) / 86_400_000;
    assert!(
        (29..=30).contains(&days),
        "倒计时要落在宽限期（界面上那句「最早 N 天后」读的就是这个数）：{days} 天 / {until}"
    );
    let t2 = &after["totals"];
    assert_eq!(t2["quarantinedCount"], json!(1), "{t2}");
    assert_eq!(t2["quarantinedBytes"], json!(17), "{t2}");
    // 隔离把 local_state 写成了 missing ⇒ 两个数各自涨，谁也不覆盖谁。
    assert_eq!(t2["unavailableCount"], json!(2), "{t2}");
    assert_eq!(t2["unavailableBytes"], json!(17 + 4096), "{t2}");
    // totals 必须等于逐行之和：防"totals 另起一条查询"漂开。
    let sum: i64 = after["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["bytes"].as_i64().unwrap())
        .sum();
    assert_eq!(t2["bytes"], json!(sum), "totals.bytes 与逐行之和不等：{t2}");
}

#[test]
fn an_empty_library_reports_zeroes_not_a_missing_panel() {
    let dir = Tmp::new("empty");
    let app = App::boot(dir.path()).expect("核心启动");
    let got = call(&app, "attachment_inventory", json!({}));
    // "扫过且为空"与"这条命令不存在"在界面上必须分得开：前者是 0 份，后者是读不到。
    assert!(got["rows"].is_array(), "空库也要回 rows 数组：{got}");
    assert_eq!(got["rows"].as_array().unwrap().len(), 0, "{got}");
    assert_eq!(got["totals"]["count"], json!(0), "{got}");
    assert_eq!(got["totals"]["bytes"], json!(0), "{got}");
    assert_eq!(got["totals"]["quarantinedBytes"], json!(0), "{got}");
}
