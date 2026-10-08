//! §6「版本历史浏览」的读侧契约（缺口 G100）。
//!
//! 为什么又用真 `dispatch` 的真 JSON：这一格跨那道桥两次（列表 + 单版正文），而键名与
//! "哪一位是谁算的"就是契约本身（`stats` 那次 snake_case 直发让设置页三行恒为 `—`）。
//!
//! 三条不那么显然、但界面上每句都要依赖的事，本文件各自钉一条：
//!  1. **`contentHash` 不许过桥** —— 界面要的是"这一版跟现在一样吗"，那就由核心把这句话算完
//!     （`sameAsNow`）。§4.5 那句"绝不能用一串哈希代替内容"管的就是这一族。
//!  2. **列表不带正文** —— 保留窗口是每篇 200 行（DATA-MODEL §4.4），一次列表发 200 份正文
//!     就是把一篇笔记的体积乘 200 端上桥。所以"点开那一行才读那一版"是契约，不是优化。
//!  3. **回滚走 `edit_note` 那唯一一条写出口**，新那一行的 origin 是 `local` 而不是 `restored` ——
//!     `restored` 这个词已经被"从回收站回来"占用了（`Store::restore_note`），
//!     两个意思共用一个标记，历史里就再也分不出"这篇被恢复过"还是"这篇被退回旧版"。
//!
//! 跑法：`cargo test -p notera-host --test note_revisions`
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
            std::env::temp_dir().join(format!("notera-revisions-{tag}-{}-{n}", std::process::id()));
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

fn err_code(app: &App, name: &str, args: Value) -> String {
    match dispatch(app, name, args) {
        Ok(v) => panic!("{name} 本该失败，却成功了：{v}"),
        Err(e) => e.code,
    }
}

fn new_note(app: &App, text: &str) -> String {
    let folder = app.default_folder_id().unwrap();
    app.create_note(&folder, doc(text)).unwrap().id
}

/// 改一次正文（走界面在用的同一条命令，不是绕过去写库）。
fn edit(app: &App, id: &str, text: &str, expected_rev: u64) {
    call(
        app,
        "edit_note",
        json!({ "id": id, "doc": doc(text), "expectedRev": expected_rev }),
    );
}

fn revisions(app: &App, id: &str) -> Value {
    call(app, "note_revisions", json!({ "id": id }))
}

/// 列表里第 `i` 行（越界就 panic 成整份 JSON —— 判据红的时候要先看得见现场）。
fn row(got: &Value, i: usize) -> Value {
    got["rows"]
        .as_array()
        .unwrap_or_else(|| panic!("rows 不是数组：{got}"))
        .get(i)
        .cloned()
        .unwrap_or_else(|| panic!("第 {i} 行不存在：{got}"))
}

#[test]
fn list_is_newest_first_meta_only_and_the_hash_never_crosses_the_bridge() {
    let dir = Tmp::new("list");
    let app = App::boot(dir.path()).expect("核心启动");
    let id = new_note(&app, "第一版");
    edit(&app, &id, "第二版", 1);
    edit(&app, &id, "第三版", 2);

    let got = revisions(&app, &id);
    let rows = got["rows"].as_array().expect("rows 要的是数组");
    assert_eq!(rows.len(), 3, "三版，返回 {rows:?}");
    assert_eq!(
        rows.iter()
            .map(|r| r["rev"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        vec![3, 2, 1],
        "列表必须**倒序**（界面要说最近这一版；正序就得让前端再排一次 = 第二套真相）"
    );
    assert_eq!(got["currentRev"], json!(3), "{got}");
    // 这台设备从没同步过 ⇒ 没有任何一版被双方确认过。
    assert_eq!(got["syncRev"], json!(0), "{got}");
    assert_eq!(got["truncated"], json!(false), "{got}");

    let newest = row(&got, 0);
    let second = row(&got, 1);
    assert_eq!(
        newest["sameAsNow"],
        json!(true),
        "最新那一版就是现在：{newest}"
    );
    assert_eq!(
        second["sameAsNow"],
        json!(false),
        "旧版不许说自己和现在一样：{second}"
    );
    assert_eq!(newest["origin"], json!("local"), "{newest}");
    assert!(
        !newest["deviceId"].as_str().unwrap_or_default().is_empty(),
        "{newest}"
    );
    assert!(
        !newest["createdAt"].as_str().unwrap_or_default().is_empty(),
        "{newest}"
    );

    // 键名整排钉一次，并**反向**钉住"哈希没过来"。
    let mut names: Vec<String> = newest.as_object().unwrap().keys().cloned().collect();
    names.sort();
    assert_eq!(
        names,
        vec![
            "createdAt".to_string(),
            "deviceId".to_string(),
            "origin".to_string(),
            "rev".to_string(),
            "sameAsNow".to_string()
        ],
        "行上的键名漂了（多出来的那位如果是 contentHash，就是 §4.5 那句话被违反了）：{names:?}"
    );

    // 正文是另一次读：列表里不许夹带 doc（一次 200 份正文 = 把这篇乘 200 端上桥）。
    let raw = serde_json::to_string(&got).unwrap();
    assert!(
        !raw.contains("blk000001"),
        "列表里出现了正文块 ⇒ 契约破了：{raw}"
    );

    let one = call(&app, "note_revision", json!({ "id": &id, "rev": 1 }));
    assert_eq!(one["rev"], json!(1), "{one}");
    assert_eq!(
        one["doc"]["content"][0]["content"][0]["text"],
        json!("第一版"),
        "点开第 1 版要读回第 1 版的正文：{one}"
    );
}

#[test]
fn a_missing_revision_is_not_found_rather_than_an_empty_doc() {
    let dir = Tmp::new("missing");
    let app = App::boot(dir.path()).expect("核心启动");
    let id = new_note(&app, "只有一版");
    // 第 9 版从没存在过。回空 doc 会让界面画出"这一版什么都没写"，
    // 而事实是"这一版已经不在了"（GC 收走了）—— 两句话在用户那儿完全不同。
    assert_eq!(
        err_code(&app, "note_revision", json!({ "id": &id, "rev": 9 })),
        "not_found"
    );
    assert_eq!(
        err_code(&app, "note_revisions", json!({ "id": "不是个 id" })),
        "bad_id"
    );
    // 这一篇不在了 ⇒ **空账**，不是 400。列表是编辑器挂载时发的背景读，而那个 id 可能刚在别处
    // 被删掉（`get_note` 对同一种情形回的是 null 而不是错）—— 发 400 只会变成界面上一个
    // 没人看得懂的红请求。单版正文那一发仍然是 `not_found`：那是用户点出来的，两件事不同。
    let gone = revisions(&app, "3f000000-0000-4000-8000-000000000000");
    assert_eq!(
        gone["rows"].as_array().map(Vec::len).unwrap_or_default(),
        0,
        "{gone}"
    );
    assert_eq!(gone["currentRev"], json!(0), "{gone}");
    assert_eq!(gone["truncated"], json!(false), "{gone}");
}

#[test]
fn rolling_back_goes_through_the_one_write_door_and_keeps_both_histories() {
    let dir = Tmp::new("rollback");
    let app = App::boot(dir.path()).expect("核心启动");
    let id = new_note(&app, "第一版");
    edit(&app, &id, "第二版", 1);

    // 界面上的"用这一版覆盖现在" = 读第 1 版正文 + 走 `edit_note`（CAS 与唯一写出口都在那儿）。
    let old = call(&app, "note_revision", json!({ "id": &id, "rev": 1 }));
    let doc_v1 = old["doc"].clone();
    call(
        &app,
        "edit_note",
        json!({ "id": &id, "doc": doc_v1, "expectedRev": 2 }),
    );

    let after = revisions(&app, &id);
    // 建篇 = rev 1，改一次 = rev 2，覆盖 = rev 3：历史里三行都在，**没有一行被抹掉**。
    assert_eq!(
        after["rows"].as_array().unwrap().len(),
        3,
        "覆盖不该抹掉历史：{after}"
    );
    assert_eq!(after["currentRev"], json!(3), "{after}");
    let newest = row(&after, 0);
    assert_eq!(newest["rev"], json!(3), "{newest}");
    assert_eq!(newest["sameAsNow"], json!(true), "{newest}");
    assert_eq!(
        newest["origin"],
        json!("local"),
        "回滚在同步协议看就是一次本机改动；origin 写回 restored 会和\"从回收站恢复\"撞车"
    );
    // 第 1 版与第 3 版内容相同 ⇒ 两行都在，且都各自说清自己是不是"现在"。
    let first = row(&after, 2);
    assert_eq!(first["rev"], json!(1), "{first}");
    assert_eq!(
        first["sameAsNow"],
        json!(true),
        "第 1 版与现在逐字节相同，这句话说得出口：{first}"
    );
    assert_eq!(row(&after, 1)["rev"], json!(2), "{after}");
    assert_eq!(
        row(&after, 1)["sameAsNow"],
        json!(false),
        "被覆盖掉的那一版从此不是\"现在\"，这句也得说对：{}",
        row(&after, 1)
    );

    // 陈旧的那一发必须被拒（覆盖走的就是这条 CAS —— 两个窗口都开着时不许后一个悄悄盖掉前一个）。
    assert_eq!(
        err_code(
            &app,
            "edit_note",
            json!({ "id": &id, "doc": doc("并发的第三发"), "expectedRev": 2 })
        ),
        "stale_edit",
        "expectedRev 过期还要能写，回滚就是一条绕过 CAS 的旁路"
    );
}

#[test]
fn restore_from_the_trash_is_the_thing_that_actually_writes_restored() {
    let dir = Tmp::new("origin");
    let app = App::boot(dir.path()).expect("核心启动");
    let id = new_note(&app, "要被删掉又回来");
    call(&app, "delete_note", json!({ "id": &id }));
    call(&app, "restore_note", json!({ "id": &id }));
    let got = revisions(&app, &id);
    let newest = row(&got, 0);
    assert_eq!(
        newest["origin"],
        json!("restored"),
        "回收站那一族才用这个标记：{newest}"
    );
    assert_eq!(newest["sameAsNow"], json!(true), "{newest}");
}

#[test]
fn a_long_history_is_capped_and_says_so_instead_of_quietly_dropping_the_old_ones() {
    let dir = Tmp::new("cap");
    let app = App::boot(dir.path()).expect("核心启动");
    let id = new_note(&app, "第 1 版");
    for rev in 2..=105 {
        edit(&app, &id, &format!("第 {rev} 版"), rev - 1);
    }
    let got = revisions(&app, &id);
    assert_eq!(
        got["rows"].as_array().expect("rows").len(),
        100,
        "一次最多回 100 行：{}",
        got["rows"].as_array().map(Vec::len).unwrap_or_default()
    );
    assert_eq!(
        got["truncated"],
        json!(true),
        "截断了就要说出来，不许让更早的版本静默消失"
    );
    assert_eq!(row(&got, 0)["rev"], json!(105), "{}", row(&got, 0));
    assert_eq!(
        row(&got, 99)["rev"],
        json!(6),
        "留下的是**最近** 100 版：{}",
        row(&got, 99)
    );
}
