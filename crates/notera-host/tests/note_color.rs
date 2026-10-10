//! §6「颜色」的笔记那一半：`set_note_color` 的边界（2026-10-10 第 43 刀）。
//!
//! 用户拍的口径：**笔记级颜色当「标签」用**（列表行前一颗小点）。与文件夹那颗同一条路，
//! 但笔记这边有一个**必须写进断言**的差别：笔记的 `content_hash` 只覆盖正文 ——
//! 颜色改了它**不动**。那条"改了哈希才会传出去"的直觉在这里是反的：
//! 真凭据是 **`rev` 前进**（同步按 `rev`/`sync_rev` 判这一行脏不脏，记录载荷里的 `color`
//! 由 `syncml::note_wire` 带上）。不把这件事钉住，下一个改同步条件的人会以为"哈希没变 ⇒ 不用传"。
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use notera_core::EntityId;
use notera_host::{commands::dispatch, App};
use serde_json::{json, Value};

static SEQ: AtomicUsize = AtomicUsize::new(0);

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("notera-notecolor-{tag}-{}-{n}", std::process::id()));
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

fn call(app: &App, name: &str, args: Value) -> Result<Value, String> {
    match dispatch(app, name, args) {
        Ok(v) => Ok(v),
        Err(e) => Err(e.code.to_string()),
    }
}

/// 库里的真身。`color` 不在 `list_notes` 那一格……**从第 43 刀起它在**，
/// 但要问"这一行会不会传出去"只能问存储层那一份（DTO 是给界面看的，不是同步的信封）。
fn stored(app: &App, id: &EntityId) -> (i64, String, Option<String>) {
    let n = app
        .store()
        .get_note(id)
        .expect("读笔记不该失败")
        .expect("这一行要在");
    (n.rev.get() as i64, n.content_hash, n.color)
}

/// 列表投影里那一行（界面拿到的就是它）。
fn list_row(app: &App, id: &EntityId) -> Value {
    let rows = call(app, "list_notes", json!({ "limit": 500 })).expect("list_notes 不该失败");
    rows.as_array()
        .expect("list_notes 要的是数组")
        .iter()
        .find(|r| r["id"] == json!(id.as_str()))
        .cloned()
        .unwrap_or_else(|| panic!("{id} 不在列表里：{rows}"))
}

fn new_note(app: &App, text: &str) -> EntityId {
    let created = call(
        app,
        "create_note",
        json!({ "folderId": null, "doc": doc(text) }),
    )
    .unwrap_or_else(|e| panic!("建笔记被拒：{e}"));
    EntityId::parse(created["id"].as_str().expect("id")).expect("合法 id")
}

#[test]
fn setting_a_color_round_trips_moves_the_rev_and_reaches_the_list_projection() {
    let dir = Tmp::new("set");
    let app = App::boot(dir.path()).expect("核心启动");
    let nid = new_note(&app, "调色板");
    let (rev0, hash0, color0) = stored(&app, &nid);
    assert_eq!(color0, None, "新建时不该带颜色");
    assert_eq!(
        list_row(&app, &nid)["color"],
        Value::Null,
        "列表投影也要说没有（第 43 刀之前这一格压根不在投影里）"
    );

    // 大写进来 ⇒ 统一成小写存（界面对比与哈希只认一种形状）。
    let got = call(
        &app,
        "set_note_color",
        json!({ "id": nid.as_str(), "color": "#C2410C" }),
    )
    .unwrap_or_else(|e| panic!("设颜色被拒：{e}"));
    assert_eq!(got["color"], json!("#c2410c"), "命令回的那份要是小写");

    let (rev1, hash1, color1) = stored(&app, &nid);
    assert_eq!(color1.as_deref(), Some("#c2410c"), "颜色没落库");
    assert_eq!(rev1, rev0 + 1, "改色要抬一格 rev（它是一次真改动）");
    // **颜色不进 content_hash**（它只覆盖正文）。真凭据是上面那格 rev：
    // 同步按 rev 判脏，记录载荷里 color 由 `note_wire` 带上。
    assert_eq!(
        hash1, hash0,
        "笔记的 content_hash 只该跟着正文走 —— 这条断言是写给「以后改同步条件的人」看的"
    );
    assert_eq!(
        list_row(&app, &nid)["color"],
        json!("#c2410c"),
        "列表投影那条边：界面拿到的行里要带颜色，否则就是「库里设了、列表上看不见」"
    );

    // 幂等：同一个色再设一次，rev 一步不动（否则每点一次同一颗色就白传一轮）。
    call(
        &app,
        "set_note_color",
        json!({ "id": nid.as_str(), "color": "#c2410c" }),
    )
    .expect("同色再设不该失败");
    let (rev2, _, _) = stored(&app, &nid);
    assert_eq!(rev2, rev1, "幂等：设成同一个颜色不许抬 rev");

    // 空串 = 清掉。
    call(
        &app,
        "set_note_color",
        json!({ "id": nid.as_str(), "color": "" }),
    )
    .expect("空串该被当成清掉，却被拒了");
    let (rev3, _, color3) = stored(&app, &nid);
    assert_eq!(color3, None, "空串没把颜色清掉");
    assert_eq!(rev3, rev2 + 1, "清掉也是一次真改动，要抬 rev");
}

#[test]
fn garbage_color_is_refused_and_leaves_the_column_untouched() {
    let dir = Tmp::new("bad");
    let app = App::boot(dir.path()).expect("核心启动");
    let nid = new_note(&app, "别写垃圾进来");
    call(
        &app,
        "set_note_color",
        json!({ "id": nid.as_str(), "color": "#c2410c" }),
    )
    .expect("先给它一个好颜色");
    let (rev0, hash0, color0) = stored(&app, &nid);

    for bad in ["red", "#12345", "#1234567", "#gggggg", "rgb(1,2,3)"] {
        let got = call(
            &app,
            "set_note_color",
            json!({ "id": nid.as_str(), "color": bad }),
        );
        assert!(got.is_err(), "「{bad}」该被拒，却被收了：{got:?}");
        assert_eq!(got.unwrap_err(), "bad_args", "拒的理由要可机读");
    }
    let (rev1, hash1, color1) = stored(&app, &nid);
    assert_eq!(color1, color0, "被拒了却还是把颜色改了");
    assert_eq!(rev1, rev0, "被拒了却还是抬了 rev");
    assert_eq!(hash1, hash0, "被拒了却还是动了哈希");
}

#[test]
fn a_trashed_note_cannot_be_recolored_and_a_missing_one_is_not_found() {
    let dir = Tmp::new("trash");
    let app = App::boot(dir.path()).expect("核心启动");
    let nid = new_note(&app, "进回收站");
    call(
        &app,
        "set_note_color",
        json!({ "id": nid.as_str(), "color": "#3f6212" }),
    )
    .expect("先给它一个颜色");
    let (_, hash_before, color_before) = stored(&app, &nid);

    call(&app, "delete_note", json!({ "id": nid.as_str() })).expect("删除该成");
    let refused = call(
        &app,
        "set_note_color",
        json!({ "id": nid.as_str(), "color": "#9d174d" }),
    );
    assert!(refused.is_err(), "在回收站里的笔记不该被上色");
    let (_, hash_after, color_after) = stored(&app, &nid);
    assert_eq!(color_after, color_before, "被拒了却还是写进去了");
    assert_eq!(hash_after, hash_before, "被拒了却还是换了哈希");

    // 不存在的 id：是"没找到"，不是"参数错" —— 界面据此说不同的话。
    let missing = call(
        &app,
        "set_note_color",
        json!({ "id": EntityId::new().to_string(), "color": "#c2410c" }),
    );
    assert_eq!(
        missing.unwrap_err(),
        "not_found",
        "缺的那一篇要给 not_found"
    );
}
