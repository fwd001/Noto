//! 设备身份（§6 第 12 格）的**两台真设备**断言：B 改的那一篇，A 读回来必须说"是 B 改的"。
//!
//! 为什么单独钉这一条：第 45 刀把 `updated_device` 接到了 DTO 与文档头（「改于 …」旁那一格），
//! 判据有 DTO 键名/值（等于本机 device_id）+ 前端单测 + 腿 55 的注入 —— 但**"对面那台设备的 id
//! 真的过了线"这一条没有任何测试**（`grep updated_device crates/*/tests` 零命中）。而它是这条链上
//! 最容易悄悄断的一节：`apply_remote` 里 `env.device.clone().unwrap_or_else(|| self.device …)`
//! 一旦退回本机 id，两台设备在屏幕上就永远显示"本机改的"，界面看起来"一切都对"。
//!
//! 两台**真** App（两个数据目录、各自真 SQLite）经**真 HTTP** 对同一台 `notera-test-webdav` 收敛：
//! ① A 写（正对照：那一格必须是 A 的 id）；② B 拉下来看到 A、改完推回；③ A 再拉 —— 那一格必须
//! **翻成 B 的 id**、正文是 B 那一版、rev 跟着前进；④ A 再自己改一下 —— 那一格翻回 A
//! （证明的不只是"线通着"，还有"每次写都会刷新"）。
//!
//! 跑法：`cargo test -p notera-host --test device_identity_e2e`

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use notera_core::Rev;
use notera_host::commands::{dispatch, AccountDraftCmd};
use notera_host::App;
use notera_test_webdav::{Backend, TestServer};
use serde_json::{json, Value};

const SECRET: &str = "sup3r-s3cr3t";
static SEQ: AtomicUsize = AtomicUsize::new(0);

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("notera-device-id-{tag}-{}-{n}", std::process::id()));
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
            "label": "device-id", "baseUrl": base_url, "username": "notera-test",
        }))
        .unwrap();
        app.configure_account(draft).expect("配置账户");
        Self { app, _dir: dir }
    }

    fn id(&self) -> String {
        self.app.stats().unwrap().device_id
    }

    /// 推到本机没有欠账为止（拉的一侧本来就干净，所以只看本机这一侧的账）。
    async fn settle(&self, cap: usize) {
        for _ in 0..cap {
            self.app.sync_once().await.expect("一轮同步");
            let st = self.app.store().stats().unwrap();
            if st.dirty_notes == 0 && st.outbox_pending == 0 {
                return;
            }
        }
        panic!("{cap} 轮之后还有欠账：没有收敛");
    }

    fn note_id(&self, title_part: &str) -> String {
        self.app
            .store()
            .list_notes(&notera_store::NoteQuery {
                folder: None,
                trash: false,
                limit: 50,
                offset: 0,
            })
            .unwrap()
            .into_iter()
            .find(|n| n.title.contains(title_part))
            .map(|n| n.id.to_string())
            .unwrap_or_else(|| panic!("列表里没有标题含「{title_part}」的笔记"))
    }

    fn detail(&self, id: &str) -> Value {
        dispatch(&self.app, "get_note", json!({ "id": id })).expect("读详情")
    }

    fn edit(&self, id: &str, text: &str, expected: u64) {
        self.app
            .edit_note(
                &notera_core::EntityId::parse(id).unwrap(),
                doc(text),
                Rev(expected),
            )
            .expect("编辑");
    }
}

fn doc(text: &str) -> Value {
    json!({ "v": 1, "content": [{ "id": "blk000001", "type": "paragraph", "content": [{ "text": text }] }] })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_other_devices_id_crosses_the_wire_and_shows_up_in_the_detail() {
    let srv = TestServer::start(Backend::Mem).await;
    let url = srv.base_url();
    let a = Device::boot("a", &url);
    let b = Device::boot("b", &url);
    let a_id = a.id();
    let b_id = b.id();
    assert_ne!(a_id, b_id, "两台设备必须各自有自己的 device_id（夹具前提）");

    // ① A 写：正对照 —— 本机改的，详情里就是 A 自己的 id。
    let folder = a.app.default_folder_id().unwrap();
    let made = a.app.create_note(&folder, doc("第一版：A 写的")).unwrap();
    a.settle(6).await;
    let seen = a.detail(&made.id);
    assert_eq!(
        seen["updatedDevice"],
        json!(a_id),
        "A 自己写的那一版必须标 A：{seen}"
    );

    // ② B 拉下来：同一篇、同一个 id，且**那一格已经是 A**（线带过来了）。
    b.settle(6).await;
    let id_b = b.note_id("第一版：A 写的");
    assert_eq!(id_b, made.id, "两边必须是同一篇（同一个 id）");
    let seen_b = b.detail(&id_b);
    assert_eq!(
        seen_b["updatedDevice"],
        json!(a_id),
        "B 看到的「最后改的人」是 A（insert 那一路的线带过来了）：{seen_b}"
    );

    // ③ B 改完推回，A 再拉 —— 这一刀最核心的读数：那一格必须翻成 B。
    let rev_b = seen_b["rev"].as_u64().unwrap();
    b.edit(&id_b, "第二版：B 改的", rev_b);
    b.settle(6).await;
    a.settle(6).await;
    let seen_after = a.detail(&made.id);
    assert_ne!(
        seen_after["updatedDevice"],
        json!(a_id),
        "B 改完拉回来还显示本机改的 ⇒ 线断了，或 apply 退回了本机 id：{seen_after}"
    );
    assert_eq!(
        seen_after["updatedDevice"],
        json!(b_id),
        "那一格必须是**改它的那台设备**（B）：{seen_after}"
    );
    let text = seen_after["doc"]["content"][0]["content"][0]["text"]
        .as_str()
        .unwrap_or_default();
    assert_eq!(
        text, "第二版：B 改的",
        "正文没跟着对面那一版过来：{seen_after}"
    );
    assert_eq!(
        seen_after["rev"],
        json!(rev_b + 1),
        "rev 要跟着对面的编辑前进：{seen_after}"
    );

    // ③′ G111 的第二个可观察面：**版本史那一格也吃同一个署名**。
    // `RevisionMeta.device_id` 以前同样被盖成本机（commit_edit 里 revision 与 updated_device 是同一处硬编），
    // 而它就在 `note_revisions` 的契约里（前端 `NoteRevisionRow.deviceId`）。
    let revs = dispatch(&a.app, "note_revisions", json!({ "id": made.id })).expect("读版本列表");
    let row = revs["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["rev"] == json!(rev_b + 1))
        .expect("拉回来的那一版要在历史里");
    assert_eq!(row["origin"], json!("remote"), "{row}");
    assert_eq!(
        row["deviceId"],
        json!(b_id),
        "版本史那一格也必须是改它的那台设备（B）：{row}"
    );

    // ④ A 再自己改一下：那一格翻回 A —— 证明的不只是"线通着"，还有"每次写都会刷新"。
    let rev_now = seen_after["rev"].as_u64().unwrap();
    a.edit(&made.id, "第三版：A 又改了", rev_now);
    let seen_final = a.detail(&made.id);
    assert_eq!(
        seen_final["updatedDevice"],
        json!(a_id),
        "A 自己改回来之后必须重新标 A：{seen_final}"
    );
}
