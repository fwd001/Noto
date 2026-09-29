//! CONFLICT-RESOLUTION §4 那一格「文件夹 `parent_id`：两侧都改 → 采纳 remote，**并检测环**」
//! 的机器证据 —— 用两台真设备各做一次"单独看合法、合起来才成环"的移动。
//!
//! 为什么必须两台设备：本机那条路径 (`Store::move_folder`) **一直**有环检测，单设备写什么
//! 都不会坏；坏的是**远端那一支** —— `apply_folder` 过去只查"父存在不存在"，不查"这个父
//! 是不是我的后代"。于是：
//!
//! ```text
//! A 把 X 移到 Y 下面      （A 本机：合法）
//! B 把 Y 移到 X 下面      （B 本机：合法）
//! 两边各自同步            →  每台都收到对面那一支，本机于是 X.parent=Y 且 Y.parent=X
//! ```
//!
//! 后果不是报错，是**两个看不见的坏**：
//! 1. 下一次用户移动任何文件夹都要跑那条递归 CTE（`descendant_ids`，`UNION ALL` 且无深度上限），
//!    有环时它**不返回** —— 界面卡在"正在移动"，进程还在，但谁也不知道为什么；
//! 2. 导出/按文件夹删除那几条走的是同一棵子树，成环之后那棵子树的成员算不出来。
//!
//! 跑法：`cargo test -p notera-host --test folder_cycle`
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use notera_host::commands::AccountDraftCmd;
use notera_host::App;
use notera_store::EntityId;
use notera_test_webdav::{Backend, TestServer};
use serde_json::json;

const SECRET: &str = "sup3r-s3cr3t";
static SEQ: AtomicUsize = AtomicUsize::new(0);

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("notera-cycle-{tag}-{}-{n}", std::process::id()));
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

struct Device {
    app: App,
    _dir: Tmp,
}

impl Device {
    fn boot(tag: &str, base_url: &str) -> Self {
        let dir = Tmp::new(tag);
        let app = App::boot(dir.path()).expect("核心启动");
        std::env::set_var("NOTERA_DEV_WEBDAV_SECRET", SECRET);
        let draft: AccountDraftCmd = serde_json::from_value(json!({
            "label": "环检测盘", "baseUrl": base_url, "username": "notera-test",
        }))
        .unwrap();
        app.configure_account(draft).expect("配置账户");
        Self { app, _dir: dir }
    }

    /// 从每个文件夹往上走 parent 链：**再次走到见过的节点就是环**。
    /// 只看盘上的父子关系，不依赖任何递归查询 —— 查询本身正是被测对象之一。
    fn cycle(&self) -> Option<String> {
        let parent: HashMap<String, Option<String>> = self
            .app
            .store()
            .list_folders()
            .unwrap()
            .into_iter()
            .map(|f| {
                (
                    f.id.as_str().to_string(),
                    f.parent_id.as_ref().map(|p| p.as_str().to_string()),
                )
            })
            .collect();
        for start in parent.keys() {
            let mut seen: HashSet<String> = HashSet::new();
            let mut cur = Some(start.clone());
            while let Some(id) = cur {
                if !seen.insert(id.clone()) {
                    return Some(format!("从 {start} 出发又回到 {id}"));
                }
                cur = parent.get(&id).cloned().flatten();
            }
        }
        None
    }

    fn folder(&self, name: &str) -> EntityId {
        self.app
            .store()
            .list_folders()
            .unwrap()
            .into_iter()
            .find(|f| f.name == name)
            .unwrap_or_else(|| panic!("这台设备上没有文件夹 {name}"))
            .id
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_devices_moving_two_folders_into_each_other_never_create_a_cycle() {
    let srv = TestServer::start(Backend::Mem).await;
    let url = srv.base_url();
    let a = Device::boot("cyc-a", &url);
    let b = Device::boot("cyc-b", &url);
    a.app.sync_once().await.expect("A 入伙");
    b.app.sync_once().await.expect("B 入伙");

    let root = a.app.default_folder_id().unwrap();
    let x = a.app.store().create_folder(Some(&root), "甲夹").unwrap();
    let y = a.app.store().create_folder(Some(&root), "乙夹").unwrap();
    a.app.sync_once().await.expect("A 建两夹");
    for _ in 0..3 {
        b.app.sync_once().await.expect("B 追平建夹");
    }
    assert_eq!(b.folder("甲夹").as_str(), x.id.as_str(), "id 要跟着走");

    // 各自本机都合法：A 把甲移到乙下面，B 把乙移到甲下面。
    a.app
        .store()
        .move_folder(&x.id, Some(&y.id))
        .expect("A 的移动本身合法");
    b.app
        .store()
        .move_folder(&y.id, Some(&x.id))
        .expect("B 的移动本身合法");

    for _ in 0..6 {
        a.app.sync_once().await.expect("A 追平");
        b.app.sync_once().await.expect("B 追平");
    }

    // ① 盘上的父子关系必须无环（这是那条文档承诺本身）。
    if let Some(why) = a.cycle() {
        panic!("A 这台设备被同步写出了一个文件夹环：{why}");
    }
    if let Some(why) = b.cycle() {
        panic!("B 这台设备被同步写出了一个文件夹环：{why}");
    }

    // ② 判据不许只写在"环不存在"上：用户下一个动作就是移动文件夹，而那一步要跑那条
    //    无深度上限的递归 CTE。这里**真的调它**，并要求它给出正确成员（能返回 = 没卡死）。
    let subtree = a
        .app
        .store()
        .folder_subtree(std::slice::from_ref(&root))
        .expect("子树查询必须能返回（有环时那条 UNION ALL 递归永不结束）");
    assert!(
        subtree.contains(x.id.as_str()) && subtree.contains(y.id.as_str()),
        "默认本的子树要把两个子夹都算进来：{subtree:?}"
    );

    // ③ 用户在本机仍然能移动文件夹（这一步内部就是成环检测，坏过一次就会永不返回）。
    let moved = a
        .app
        .store()
        .move_folder(&y.id, Some(&root))
        .expect("追平之后本机移动仍要可用");
    assert_eq!(moved.parent_id.as_ref(), Some(&root));
}
