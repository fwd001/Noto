//! 文件夹只允许一层（缺口 G60 定案：口径要在**核心**强制，不只是文档里的一句话）。
//!
//! 为什么打在真 `dispatch` 上而不是 store：这条规则的意义就是"用户点不出第二层"，
//! 而命令层是唯一的用户入口。store 仍然收嵌套（对面的同步记录走 `apply_remote`，
//! 那是协议的可接受输入，收紧它是另一次决定 —— 见下面 `store_still_accepts_nested_*`），
//! 所以只有命令层能证明"这一格到底拒了没有"。
//!
//! 跑法：`cargo test -p notera-host --test folder_depth`
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
            std::env::temp_dir().join(format!("notera-depth-{tag}-{}-{n}", std::process::id()));
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

fn code(app: &App, name: &str, args: Value) -> Result<Value, String> {
    match dispatch(app, name, args) {
        Ok(v) => Ok(v),
        Err(e) => Err(e.code.to_string()),
    }
}

fn folders(app: &App) -> Vec<Value> {
    match dispatch(app, "list_folders", json!({})) {
        Ok(v) => v.as_array().cloned().unwrap_or_default(),
        Err(e) => panic!("list_folders 不该失败：{}", e.code),
    }
}

#[test]
fn user_side_cannot_create_or_move_a_folder_into_another() {
    let dir = Tmp::new("reject");
    let app = App::boot(dir.path()).expect("核心启动");

    // 最外层建一个：仍然要能建（这一刀不是"不许建文件夹"）。
    let top = code(
        &app,
        "create_folder",
        json!({ "parentId": null, "name": "最外层" }),
    )
    .unwrap_or_else(|e| panic!("建最外层文件夹被拒了：{e}"));
    let top_id = top["id"].as_str().expect("要回新建的那一个").to_string();
    let before = folders(&app).len();

    // 往里再建一层：当场拒，并且**一个都没多**。
    let nested = code(
        &app,
        "create_folder",
        json!({ "parentId": top_id, "name": "第二层" }),
    );
    assert_eq!(
        nested,
        Err("folder_nested".to_string()),
        "嵌套新建必须回具名码 folder_nested（回通用码就等于把这条口径又变回一句话）：{nested:?}"
    );
    assert_eq!(folders(&app).len(), before, "被拒的那一发不许留下半拉子行");

    // 把已有文件夹搬进去：同样拒，且 parent 没被偷偷改掉。
    let second = code(
        &app,
        "create_folder",
        json!({ "parentId": null, "name": "另一个最外层" }),
    )
    .unwrap_or_else(|e| panic!("建第二个最外层被拒：{e}"));
    let second_id = second["id"].as_str().expect("id").to_string();
    let moved = code(
        &app,
        "move_folder",
        json!({ "id": second_id, "parentId": top_id }),
    );
    assert_eq!(
        moved,
        Err("folder_nested".to_string()),
        "搬进别的文件夹就是造第二层：{moved:?}"
    );
    let after = folders(&app);
    let still_top = after
        .iter()
        .find(|f| f["id"] == json!(second_id))
        .expect("那一个还要在清单里");
    assert_eq!(
        still_top["parentId"],
        Value::Null,
        "被拒的移动不许把 parentId 改掉：{still_top}"
    );

    // 反向：搬到最外层（`parentId: null`）是**收拢**，不是加深 —— 必须仍然允许。
    code(
        &app,
        "move_folder",
        json!({ "id": second_id, "parentId": null }),
    )
    .unwrap_or_else(|e| panic!("搬到最外层被拒了：{e}"));
}

#[test]
fn store_layer_still_accepts_nested_so_the_sync_side_is_unchanged() {
    let dir = Tmp::new("store");
    let app = App::boot(dir.path()).expect("核心启动");
    // 这条断言守的是**边界**：这一刀只收紧用户那一条边。对面同步回来的嵌套走
    // `apply_remote`，今天仍然落得下 —— 把协议那侧一起改掉需要另一次决定。
    let top = app
        .store()
        .create_folder(None, "最外层")
        .expect("store 建最外层");
    let child = app
        .store()
        .create_folder(Some(&top.id), "第二层")
        .expect("store 那一层不许被这一刀顺手改掉");
    assert_eq!(child.parent_id.as_ref(), Some(&top.id));
}
