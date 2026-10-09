//! §6「颜色」那一格的写入口（用户 2026-10-09 拍板：只做侧栏小色点）。
//!
//! 这一格历史上只有列、没有写入方，也没有任何地方渲染它。所以判据要问的不是"能不能存"，
//! 而是三件容易各修各的事：
//!  · 改颜色**必须换 `content_hash`** —— 同步靠它判等，沿用旧哈希会变成"库里颜色变了、
//!    对面却认为没变"，那次改动**永不传播**；
//!  · 改成同一个颜色要**幂等**（不许抬 rev、不许换哈希）；
//!  · 非法值当场拒，并且**库里那一位不许被改成半拉子**。
//!
//! 跑法：`cargo test -p notera-host --test folder_color`
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
            std::env::temp_dir().join(format!("notera-color-{tag}-{}-{n}", std::process::id()));
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

fn call(app: &App, name: &str, args: Value) -> Result<Value, String> {
    match dispatch(app, name, args) {
        Ok(v) => Ok(v),
        Err(e) => Err(e.code.to_string()),
    }
}

fn row(app: &App, folder_id: &str) -> Value {
    let rows = dispatch(app, "list_folders", json!({})).expect("list_folders 不该失败");
    rows.as_array()
        .expect("list_folders 要的是数组")
        .iter()
        .find(|f| f["id"] == json!(folder_id))
        .cloned()
        .unwrap_or_else(|| panic!("{folder_id} 不在清单里：{rows}"))
}

/// 库里的真身。`content_hash` 与 `rev` **不在 `list_folders` 的 DTO 上** —— 那是给用户看的
/// 清单，不是同步的信封；要问"这次改动会不会传播"，只能问存储层那一份。
fn stored(app: &App, folder_id: &str) -> (i64, String, Option<String>) {
    let eid = notera_core::EntityId::parse(folder_id).expect("合法 id");
    let f = app
        .store()
        .get_folder(&eid)
        .expect("读文件夹不该失败")
        .expect("这一行要在");
    (f.rev.get() as i64, f.content_hash, f.color)
}

fn new_folder(app: &App, name: &str) -> String {
    call(
        app,
        "create_folder",
        json!({ "parentId": null, "name": name }),
    )
    .unwrap_or_else(|e| panic!("建文件夹被拒：{e}"))["id"]
        .as_str()
        .expect("id")
        .to_string()
}

#[test]
fn setting_a_color_round_trips_and_moves_the_content_hash() {
    let dir = Tmp::new("set");
    let app = App::boot(dir.path()).expect("核心启动");
    let fid = new_folder(&app, "调色板");
    let (rev0, hash0, color0) = stored(&app, &fid);
    assert_eq!(color0, None, "新建时不该带颜色");
    assert_eq!(
        row(&app, &fid)["color"],
        Value::Null,
        "DTO 那一格也要说没有"
    );

    // 大写进来 ⇒ 统一成小写存（界面对比与哈希只认一种形状）。
    let got = call(
        &app,
        "set_folder_color",
        json!({ "id": fid, "color": "#C2410C" }),
    )
    .unwrap_or_else(|e| panic!("设颜色被拒：{e}"));
    assert_eq!(got["color"], json!("#c2410c"), "{got}");
    assert_eq!(
        row(&app, &fid)["color"],
        json!("#c2410c"),
        "读侧要回同一个值"
    );
    let (rev1, hash1, color1) = stored(&app, &fid);
    assert_eq!(color1.as_deref(), Some("#c2410c"), "{color1:?}");
    assert_ne!(
        hash1, hash0,
        "改了颜色却沿用旧 content_hash ⇒ 对面永远看不到这次改动"
    );
    assert!(rev1 > rev0, "真改了就要抬 rev：{rev0} → {rev1}");

    // 幂等：改成同一个颜色不许再抬一次 rev、也不许换哈希。
    call(
        &app,
        "set_folder_color",
        json!({ "id": fid, "color": "#C2410C" }),
    )
    .unwrap_or_else(|e| panic!("第二次设同一个颜色被拒：{e}"));
    let (rev2, hash2, _) = stored(&app, &fid);
    assert_eq!(
        (rev2, hash2.clone()),
        (rev1, hash1.clone()),
        "改成同一个颜色抬了 rev 或换了哈希：{rev1}/{hash1} → {rev2}/{hash2}"
    );

    // 清掉：`null` 是"没有颜色"，不是"这次没改"。
    let cleared = call(
        &app,
        "set_folder_color",
        json!({ "id": fid, "color": null }),
    )
    .unwrap_or_else(|e| panic!("清颜色被拒：{e}"));
    assert_eq!(cleared["color"], Value::Null, "{cleared}");
    let (rev3, hash3, color3) = stored(&app, &fid);
    assert_eq!(color3, None, "{color3:?}");
    assert_ne!(hash3, hash2, "清掉颜色也要换哈希：{hash2} → {hash3}");
    assert!(rev3 > rev2, "清掉是一次真改动，要抬 rev：{rev2} → {rev3}");
}

#[test]
fn garbage_color_is_refused_and_leaves_the_column_untouched() {
    let dir = Tmp::new("reject");
    let app = App::boot(dir.path()).expect("核心启动");
    let fid = new_folder(&app, "别乱写");
    let (rev0, hash0, _) = stored(&app, &fid);

    // 词、缺 #、位数不对、非十六进制 —— 每种都当场拒。
    for bad in ["red", "C2410C", "#c241", "#gggggg", "#c2410cdd"] {
        let got = call(&app, "set_folder_color", json!({ "id": fid, "color": bad }));
        assert_eq!(got, Err("bad_args".to_string()), "{bad} 该被拒：{got:?}");
        let (rev, hash, color) = stored(&app, &fid);
        assert_eq!(color, None, "被拒的那一发把颜色写脏了（{bad}）");
        assert_eq!(
            (rev, hash),
            (rev0, hash0.clone()),
            "被拒的那一发不该抬 rev / 换哈希（{bad}）"
        );
    }

    // 反向：`null` 与空串都算"清掉"，是合法值而不是坏值。
    call(&app, "set_folder_color", json!({ "id": fid, "color": "" }))
        .unwrap_or_else(|e| panic!("空串该被当成清掉，却被拒了：{e}"));
}

/// 2026-10-09 用户拍的第二问：内置那两本（默认本 / 回收站）**也允许打色标**。
///
/// 开的例外**只有颜色这一条**，所以同一本改名的判据必须照样是拒的 ——
/// 那才是"例外管到哪一级"的差分；只测"上色能成"的话，把整本放开也照样绿。
/// 反过来在回收站里那一本仍然拒：那棵整都不该被写，恢复出来的东西不该带着一轮没人确认过的改动。
#[test]
fn system_folders_are_colorable_while_renaming_them_still_is_not() {
    let dir = Tmp::new("sys");
    let app = App::boot(dir.path()).expect("核心启动");
    let rows = dispatch(&app, "list_folders", json!({})).expect("list_folders 不该失败");
    let list = rows.as_array().expect("list_folders 要的是数组");
    let default_id = list
        .iter()
        .find(|f| f["systemKind"] != Value::Null)
        .expect("默认本要在清单里")["id"]
        .as_str()
        .expect("id")
        .to_string();
    let name0 = row(&app, &default_id)["name"].clone();

    let (rev0, hash0, _) = stored(&app, &default_id);
    call(
        &app,
        "set_folder_color",
        json!({ "id": default_id, "color": "#1e40af" }),
    )
    .unwrap_or_else(|e| panic!("内置那一本上色被拒：{e}"));
    let (rev1, hash1, color1) = stored(&app, &default_id);
    assert_eq!(color1.as_deref(), Some("#1e40af"), "颜色没落库");
    assert_ne!(
        hash0, hash1,
        "颜色变了而 content_hash 没变 ⇒ 这条改动永远不会传到别的设备"
    );
    assert_eq!(rev1, rev0 + 1, "改色要抬一格 rev（它是一次真改动）");

    // 同一本：改名仍然一律拒，而且库里那个名字没被动过。
    let renamed = call(
        &app,
        "rename_folder",
        json!({ "id": default_id, "name": "改了名" }),
    );
    assert!(
        renamed.is_err(),
        "颜色开了例外不等于整本可写：内置本改名必须照样被拒"
    );
    assert_eq!(
        row(&app, &default_id)["name"],
        name0,
        "被拒的那次改名把名字留在库里了吗"
    );

    // 回收站里的那一本：拒，且颜色一位都没动。
    let fid = new_folder(&app, "进回收站");
    call(
        &app,
        "set_folder_color",
        json!({ "id": fid, "color": "#3f6212" }),
    )
    .expect("先给它一个颜色");
    let (_, hash_before, color_before) = stored(&app, &fid);
    call(&app, "delete_folder", json!({ "id": fid })).expect("删除该成");
    let trashed = call(
        &app,
        "set_folder_color",
        json!({ "id": fid, "color": "#9d174d" }),
    );
    assert!(trashed.is_err(), "在回收站里的文件夹不该被上色");
    let (rev_t, hash_after, color_after) = stored(&app, &fid);
    assert_eq!(color_after, color_before, "被拒了却还是写进去了");
    assert_eq!(hash_after, hash_before, "被拒了却还是换了哈希");
    let _ = rev_t;
}
