//! 命令成功载荷的形状契约：**成功是裸 DTO，不是 `{"Ok": …}`**。
//!
//! 这条为什么值得单独钉：两条通道（Tauri `invoke` / dev HTTP 桥）共用一条契约 ——
//! 成功时前端把 `dispatch` 返回的那个值**直接当 DTO 用**（`apps/desktop/src/api/bridge.ts`
//! 的 `unwrap`，devserver.rs:119 写着"成功是裸 DTO"）。而 `dispatch` 的臂是把值交给 `j()`
//! 序列化的；若臂写成 `j(app.to_dto(x))`（漏了传播 `to_dto` 自己那层 `Result` 的 `?`），
//! 序列化出来的是 `Result` 的外部标签形式 `{"Ok":{…}}`。HTTP 仍是 200、库里真的写进去了、
//! `unwrap` 照原样返回 —— 于是前端 `applyNoteUpdate` 在 `typeof note.id !== 'string'`
//! 那一行**静默 return**（`stores/notes.ts:266`）。用户看到的不是报错，是**点下去屏幕上
//! 什么都没发生**：「固定」按下去标记不翻面、键名不翻面，要换一页或重启才对上（G32）。
//!
//! TS 类型、前端 mock、组件单测都拦不住这一类（它们看不见真序列化输出），所以这一格用真
//! `dispatch` 的真 JSON 来钉，并把"每条臂都扫一遍"当默认形态，而不是只测当时坏掉的那三条。
//!
//! 跑法：`cargo test -p notera-host --test dto_envelope`
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
            std::env::temp_dir().join(format!("notera-envelope-{tag}-{}-{n}", std::process::id()));
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
    json!({
        "v": 1,
        "content": [{ "id": "blk000001", "type": "paragraph", "content": [{ "text": text }] }]
    })
}

/// 走真 `dispatch`：Err 直接红在错误码上（那是夹具不对，不是载荷不对）。
fn call(app: &App, name: &str, args: Value) -> Value {
    match dispatch(app, name, args) {
        Ok(v) => v,
        Err(e) => panic!("{name} 没成功：code={} detail={:?}", e.code, e.detail),
    }
}

/// 这一格的全部内容：成功的 JSON 里不允许出现 `Result` 的外部标签。
fn assert_bare_dto(name: &str, v: &Value) {
    let Some(obj) = v.as_object() else {
        panic!("{name} 的成功载荷不是对象：{v}");
    };
    assert!(
        obj.get("Ok").is_none() && obj.get("Err").is_none(),
        "{name} 的成功载荷是 Result 的外部标签形式（外面套了一层 Ok），前端 unwrap 会把整个\
         封套当 DTO → note.id undefined → 更新被静默丢掉。实际载荷：{v}"
    );
}

#[test]
fn success_payload_is_a_bare_dto_not_a_tagged_result() {
    let dir = Tmp::new("bare");
    let app = App::boot(dir.path()).expect("核心启动");

    let note = call(
        &app,
        "create_note",
        json!({ "folderId": Value::Null, "doc": doc("封套") }),
    );
    assert_bare_dto("create_note", &note);
    let note_id = note["id"]
        .as_str()
        .expect("create_note 该回 id")
        .to_string();

    let folder = call(
        &app,
        "create_folder",
        json!({ "parentId": Value::Null, "name": "封套本" }),
    );
    assert_bare_dto("create_folder", &folder);
    let folder_id = folder["id"]
        .as_str()
        .expect("create_folder 该回 id")
        .to_string();

    // 当时坏掉的三条，连**值**一起钉：界面画的就是这两个字段。
    let pinned = call(
        &app,
        "set_note_pinned",
        json!({ "id": &note_id, "pinned": true }),
    );
    assert_bare_dto("set_note_pinned", &pinned);
    assert_eq!(
        pinned["pinned"],
        json!(true),
        "固定后回包的 pinned 没翻面，列表那格无从更新"
    );
    assert_eq!(pinned["id"], json!(&note_id));

    let unpin = call(
        &app,
        "set_note_pinned",
        json!({ "id": &note_id, "pinned": false }),
    );
    assert_bare_dto("set_note_pinned(unpin)", &unpin);
    assert_eq!(unpin["pinned"], json!(false));

    let moved = call(
        &app,
        "set_note_folder",
        json!({ "id": &note_id, "folderId": &folder_id }),
    );
    assert_bare_dto("set_note_folder", &moved);
    assert_eq!(
        moved["folderId"],
        json!(&folder_id),
        "移动后回包的 folderId 不是目标文件夹"
    );

    let inner = call(
        &app,
        "create_folder",
        json!({ "parentId": Value::Null, "name": "待移动" }),
    );
    let inner_id = inner["id"].as_str().unwrap().to_string();
    let moved_folder = call(
        &app,
        "move_folder",
        json!({ "id": &inner_id, "parentId": Value::Null }),
    );
    assert_bare_dto("move_folder", &moved_folder);
    assert_eq!(moved_folder["id"], json!(&inner_id));

    // rev 从真读回来的那条取，不猜：编辑那一支要的是当前 rev。
    let fresh = call(&app, "get_note", json!({ "id": &note_id }));
    let rev = fresh["rev"].as_u64().expect("get_note 该回 rev");

    // 剩下的臂同一条规矩，一起扫：新加一条臂写错形状，这里就该红。
    let cases: Vec<(&str, Value)> = vec![
        ("daily_note", json!({})),
        (
            "edit_note",
            json!({ "id": &note_id, "doc": doc("封套 2"), "expectedRev": rev }),
        ),
        ("get_note", json!({ "id": &note_id })),
        (
            "list_notes",
            json!({ "folderId": Value::Null, "trash": false }),
        ),
        (
            "rename_folder",
            json!({ "id": &folder_id, "name": "封套本 2" }),
        ),
        ("list_folders", json!({})),
        ("search", json!({ "text": "封套", "limit": 10 })),
        ("stats", json!({})),
        ("get_prefs", json!({})),
        ("sync_status", json!({})),
        ("account", json!({})),
        ("open_conflicts", json!({})),
        ("platform_caps", json!({})),
        ("list_backups", json!({})),
    ];
    let mut swept = 0usize;
    let mut no_payload: Vec<&str> = Vec::new();
    for (name, args) in cases {
        let v = call(&app, name, args);
        if v.is_object() {
            assert_bare_dto(name, &v);
            swept += 1;
        } else if let Some(first) = v.as_array().and_then(|a| a.first()) {
            // 回数组的那几条（list/search）：元素也不允许被套一层。
            assert_bare_dto(&format!("{name}[0]"), first);
            swept += 1;
        } else {
            no_payload.push(name);
        }
    }
    // 扫到几条是有预期的：空着的那三条在"离线 + 全新库"这一档本来就没东西可回
    // （account 没配账户 / open_conflicts 没冲突 / list_backups 没备份）。
    // 用**名单**而不是数量下限，是因为名单两头都管：某条本来该有载荷的臂退化成 null，
    // 或某条本来空着的臂突然有东西，这里都会红 —— 而一个 `>= 11` 那种下限只会默默放过。
    no_payload.sort_unstable();
    assert_eq!(
        no_payload,
        vec!["account", "list_backups", "open_conflicts"],
        "这一扫读到的载荷名单和台账不一致（要么有条臂退化成了 null，要么夹具变了）"
    );
    assert_eq!(swept, 11, "这一扫只读到 {swept} 条载荷，门禁在空转");
}

/// 反向哨兵：真 DTO 的键是前端 `types.ts` 那一套 camelCase。空对象、或只含 `Ok` 的对象，
/// 都到不了这里 —— 钉住"这条门禁确实在读真序列化输出"。
#[test]
fn the_gate_reads_real_serialized_output() {
    let dir = Tmp::new("sentinel");
    let app = App::boot(dir.path()).expect("核心启动");
    let note = call(
        &app,
        "create_note",
        json!({ "folderId": Value::Null, "doc": doc("哨兵") }),
    );
    let note_id = note["id"].as_str().unwrap().to_string();
    let pinned = call(
        &app,
        "set_note_pinned",
        json!({ "id": &note_id, "pinned": true }),
    );
    for key in ["id", "folderId", "title", "pinned", "rev", "updatedAt"] {
        assert!(
            pinned.get(key).is_some(),
            "回包缺 {key}（缺了界面就画不出来）：{pinned}"
        );
    }
}

/// `systemKind` 必须在**线格式**里看得见，而且默认本的值就是 `"default"`。
///
/// 这一格此前核心一直在发、前端类型没声明也没往下带，于是"默认本"上长出
/// 改名/移动/删除三颗按钮 —— 点下去得到的全是 `assert_folder_writable` 的拒绝。
/// 界面唯一的凭据就是这个键，所以钉在真 `dispatch` 的输出上，而不是 Rust 字段名。
#[test]
fn folder_wire_carries_system_kind_for_the_default_folder() {
    let dir = Tmp::new("syskind");
    let app = App::boot(dir.path()).expect("核心启动");
    let mine = call(
        &app,
        "create_folder",
        json!({ "parentId": Value::Null, "name": "工作" }),
    );
    let folders = call(&app, "list_folders", json!({}));
    let all = folders.as_array().expect("list_folders 该回数组");

    let default = all
        .iter()
        .find(|f| f["systemKind"].as_str() == Some("default"))
        .expect("树里找不到 systemKind=\"default\" 的默认本：界面就没法藏那三颗按钮");
    assert_eq!(
        default["id"].as_str(),
        Some(notera_store::DEFAULT_FOLDER_ID),
        "默认本的 id 不是写死的那一个（角色实体不该由开机时间决定）"
    );

    let mine_id = mine["id"].as_str().unwrap();
    let normal = all
        .iter()
        .find(|f| f["id"].as_str() == Some(mine_id))
        .expect("刚建的文件夹没出现在树里");
    assert!(
        normal.get("systemKind").is_some(),
        "普通文件夹的载荷里没有 systemKind 这一格：前端只能凭名字猜默认本"
    );
    assert!(
        normal["systemKind"].is_null(),
        "普通文件夹的 systemKind 不是 null：{normal}"
    );

    // 核心那条守卫也得真的在：删默认本必须被拒（否则界面藏按钮只是遮丑）。
    let err = dispatch(&app, "delete_folder", json!({ "id": default["id"] }))
        .expect_err("默认本居然删得掉，界面藏按钮只是遮丑");
    assert_eq!(
        err.code, "constraint",
        "拒绝默认本删除的错误码变了：{err:?}"
    );
}

/// §3.3：「搜索分档是新能力：后端已经算出"精准/模糊"…但被丢掉了。精准排前，模糊在后。」
///
/// "被丢掉"发生在过桥这一刀，不在检索里 —— `notera_store::SearchHit.match_kind` 一直有值，
/// `SearchHitDto` 以前只带四个键（真 dev 桥实测：`noteId,score,snippetHtml,title`）。
/// 所以这一格钉的是**线格式**：键名、两档各一条的真值、以及"精准在前"这个顺序承诺。
/// TS 类型和前端 mock 都看不见这一类漂移（本仓库踩过两次同一形状）。
#[test]
fn search_wire_carries_the_tier_flag_with_exact_first() {
    let dir = Tmp::new("tier");
    let app = App::boot(dir.path()).expect("核心启动");

    // 同一句查询：连着写完的是精准档；把"数据同步"和"协议"隔开写的，只有三字串档能抓到。
    let exact = call(
        &app,
        "create_note",
        json!({ "folderId": Value::Null, "doc": doc("这一篇写着数据同步协议的边界条件") }),
    );
    let fuzzy = call(
        &app,
        "create_note",
        json!({ "folderId": Value::Null, "doc": doc("先做数据同步的预演，同步协调另开一条线，下一步协议的边界还没定") }),
    );
    let exact_id = exact["id"].as_str().unwrap().to_string();
    let fuzzy_id = fuzzy["id"].as_str().unwrap().to_string();

    let hits = call(
        &app,
        "search",
        json!({ "text": "数据同步协议", "limit": 10 }),
    );
    let all = hits.as_array().expect("search 该回数组");
    assert_eq!(all.len(), 2, "夹具该命中两条，实际 {all:?}");

    let first = &all[0];
    for key in ["noteId", "score", "snippetHtml", "title", "exact"] {
        assert!(first.get(key).is_some(), "命中载荷缺 {key}：{first}");
    }
    assert_eq!(
        first["noteId"].as_str(),
        Some(exact_id.as_str()),
        "精准档没排在前面——§3.3 的顺序承诺是'精准排前，模糊在后'，界面那两句读数以这个顺序为前提"
    );
    assert_eq!(
        first["exact"],
        json!(true),
        "精准那一条没报成精准：界面会显示「精准 0」"
    );
    assert_eq!(all[1]["noteId"].as_str(), Some(fuzzy_id.as_str()));
    assert_eq!(
        all[1]["exact"],
        json!(false),
        "模糊那一条报成了精准：界面会说谎"
    );
}
