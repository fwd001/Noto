//! §5 与 §6 的落点验收。跑在 `notera-test-webdav` 的真 TCP 服务器上，
//! 走的是 `App::sync_once` —— 产品调度器每 25 秒跑的同一条代码路径。
//!
//! 三件事必须被观测到，缺一件都算没接线：
//! 1. **探测发生在装配适配器之前**：装出来的那个 remote 必须已经带着实测位图。
//!    反过来说，如果先装再探，这次会话拿到的仍是保守默认，§5 的优化要等下次启动。
//! 2. **探测失败 ≠ 能力缺失**：连接被掐断时必须退回保守默认并如实提示，
//!    绝不能用"全 false 的结论"去写库（那会把好服务器降级成 S3 盲写）。
//! 3. 本地笔记真的落到服务器文件系统，并被**第二台设备**原样拉下来。
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use notera_host::commands::AccountDraftCmd;
use notera_host::{App, BusEvent};
use notera_store::NoteQuery;
use notera_test_webdav::{Backend, Injection, TestServer};
use notera_webdav::{Caps, WriteStrategy};
use serde_json::json;

const SECRET: &str = "sup3r-s3cr3t";
static SEQ: AtomicUsize = AtomicUsize::new(0);

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("notera-synconce-{tag}-{}-{n}", std::process::id()));
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

/// 一台设备 = 一个数据目录 + 一个 `App`。目录必须比 App 活得久，所以成对持有。
struct Device {
    app: App,
    _dir: Tmp,
}

impl Device {
    fn boot(tag: &str, base_url: &str) -> Self {
        let dir = Tmp::new(tag);
        let app = App::boot(dir.path()).expect("核心启动");
        // 凭据走 debug 专用的环境变量：OS 钥匙串是 PLATFORM.md §6 的活，
        // 在此之前 `secret_for` 只认这一个来源（发布版宁可显示"需要凭据"）。
        std::env::set_var("NOTERA_DEV_WEBDAV_SECRET", SECRET);
        // 用 JSON 构造而不是结构体字面量：顺便证明"前端发的那个形状"真的被接受。
        let draft: AccountDraftCmd = serde_json::from_value(json!({
            "label": "测试盘",
            "baseUrl": base_url,
            "username": "notera-test",
        }))
        .expect("最小账户草案");
        app.configure_account(draft).expect("配置账户");
        Self { app, _dir: dir }
    }
    fn account_id(&self) -> String {
        self.app
            .current_account()
            .unwrap()
            .expect("已配置账户")
            .id
            .to_string()
    }
}

fn doc(text: &str) -> serde_json::Value {
    json!({ "v": 1, "content": [{ "id": "blk000001", "type": "paragraph", "content": [{ "text": text }] }] })
}

#[tokio::test]
async fn boot_probe_happens_before_the_adapter_is_built() {
    let srv = TestServer::start(Backend::Mem).await;
    let a = Device::boot("order", &srv.base_url());
    let acct = a.account_id();

    // 从未探测：cap_mask 必须是 NULL，而不是 0（0 = "探过且什么都不支持"，含义完全不同）
    assert!(
        a.app.store().account_caps(&acct).unwrap().is_none(),
        "新账户不该带探测结果"
    );
    assert!(a
        .app
        .store()
        .account_caps_probed_at(&acct)
        .unwrap()
        .is_none());

    // 让服务器"把条件头当装饰"：实测结论因此**低于**保守默认，
    // 于是"适配器带的是哪个位图"就成了一次可观测的顺序判定 ——
    // 若先装后探，这里会是默认的 S1，而真实测出来的应该是 S2。
    srv.inject(Injection::status("PUT /.notes/probe/cput.json", 200))
        .await;

    let remote = a
        .app
        .remote_for_sync()
        .await
        .expect("启动期装配远端")
        .expect("配了账户就该有适配器");
    let mask = a
        .app
        .store()
        .account_caps(&acct)
        .unwrap()
        .expect("探测结果要落库");
    let caps = remote.caps();
    assert!(
        !caps.has(Caps::CONDITIONAL_PUT),
        "服务器不支持条件写，探测却没认出来：mask={mask:#06b}"
    );
    assert_eq!(
        caps.write_strategy(),
        WriteStrategy::S2,
        "适配器带的还是默认位图，探测结果没赶上本次会话"
    );
    assert_eq!(caps.mask(), mask, "落库的位图与真正生效的位图必须一致");
    srv.clear_injection().await;

    a.app
        .create_note(&a.app.default_folder_id().unwrap(), doc("探测之后的一轮"))
        .unwrap();
    let stats = a.app.sync_once().await.expect("第一轮要跑起来");
    assert!(stats.pushed >= 1, "本地新建要真的推上去：{stats:?}");
    // 请求日志是第二重证据：所有探测请求都要早于任何真实数据请求。
    // depth 探测打的是库根上的 PROPFIND，路径里不带 probe/，别把它漏判成"真实读写"。
    let log = srv.request_log();
    let is_probe = |r: &&notera_test_webdav::LoggedRequest| {
        r.path.contains("/probe/") || r.method == "PROPFIND"
    };
    let probe_last = log
        .iter()
        .filter(is_probe)
        .map(|r| r.seq)
        .max()
        .expect("探测请求要出现");
    let real_first = log
        .iter()
        .filter(|r| !is_probe(r))
        .map(|r| r.seq)
        .min()
        .expect("真实读写要出现");
    let trace = log
        .iter()
        .map(|r| format!("{} {} -> {}", r.seq, r.method, r.path))
        .collect::<Vec<_>>()
        .join(" | ");
    assert!(
        probe_last < real_first,
        "探测穿插在真实读写之间，说明不是『先探后装』：{probe_last} vs {real_first}\n{trace}"
    );

    // §5「每日一次」：同一天里的第二次入口不该再来一轮探测请求
    let probes_before = log.iter().filter(is_probe).count();
    a.app.remote_for_sync().await.expect("第二次装配");
    let probes_after = srv.request_log().iter().filter(is_probe).count();
    assert_eq!(
        probes_before, probes_after,
        "当天已探过就不该重复探测（每次启动 12+ 个请求）"
    );
    // 探测对象用完要清掉：留在库里会污染清单并跟着同步到每台设备
    let leftover = srv.fs_dump()["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .filter(|e| e["path"].as_str().unwrap_or_default().contains("/probe/"))
        .count();
    assert_eq!(leftover, 0, "探测对象没清理");
    srv.stop().await;
}

/// 探测期间连接被掐断 → 退回保守默认 + 一条可见提示，库里的 cap_mask 保持"从未探测"。
/// 这一条守的是方向性风险：把"没探到"写成"不支持"，服务器就永久掉到 S3 盲写复验。
#[tokio::test]
async fn a_failed_probe_falls_back_to_the_conservative_default_and_says_so() {
    let srv = TestServer::start(Backend::Mem).await;
    let a = Device::boot("probe-fails", &srv.base_url());
    let acct = a.account_id();
    let rx = a.app.subscribe();
    srv.inject(Injection {
        drop_after_n: Some(0),
        ..Default::default()
    })
    .await;

    let remote = a
        .app
        .remote_for_sync()
        .await
        .expect("探测失败不许挡住启动")
        .expect("服务器在，就该给出适配器");
    assert_eq!(
        remote.caps(),
        Caps::conventional(),
        "探测没成功就必须用保守默认，不能编造结论"
    );
    assert!(
        a.app.store().account_caps(&acct).unwrap().is_none(),
        "没探到就不该写 cap_mask"
    );

    let mut saw_deferred = false;
    while let Ok(ev) = rx.try_recv() {
        if let BusEvent::Toast { message_key, level } = &ev {
            assert_eq!(level, "warn", "探测未完成是可恢复状态，不该报成错误");
            if message_key == "sync.probeDeferred" {
                saw_deferred = true;
            }
        }
    }
    assert!(saw_deferred, "探测失败必须让用户看得见，不能静默按默认跑");
    srv.clear_injection().await;
    srv.stop().await;
}

/// 一轮同步的端到端闭环：A 推 → 服务器落盘 → B 拉 → 标题/正文逐字一致。
#[tokio::test]
async fn one_round_carries_a_local_note_to_a_second_device() {
    let srv = TestServer::start(Backend::Mem).await;
    let url = srv.base_url();
    let a = Device::boot("push", &url);
    let text = "只有真服务器才算数的正文";
    a.app
        .create_note(&a.app.default_folder_id().unwrap(), doc(text))
        .unwrap();

    let stats = a.app.sync_once().await.expect("A 的一轮");
    assert_eq!(
        stats.outcome,
        notera_sync::RoundOutcome::Converged,
        "{stats:?}"
    );
    assert!(stats.pushed >= 1);
    // 队列得真的清空：outbox 停在 inflight 的话，设置页的"待同步"计数永远不掉，
    // 而且这张表只会一直长。
    let acct = a.account_id();
    assert_eq!(
        a.app
            .store()
            .outbox_len(&acct, &[notera_store::OpState::Inflight])
            .unwrap(),
        0,
        "一轮跑完不许留下『进行中但没结清』的待办"
    );
    assert!(
        a.app
            .store()
            .outbox_len(&acct, &[notera_store::OpState::Done])
            .unwrap()
            >= 1,
        "推上去的那条要留下 done 痕迹，而不是凭空消失"
    );
    assert_eq!(
        a.app.store().stats().unwrap().outbox_pending,
        0,
        "同步成功后不该还有未完成操作"
    );
    // 本地哨兵账户（enabled=0）是"提交即入 outbox"的留痕账，引擎永不消费它。
    // 它留在这儿正是 outbox_pending 不许把它算进来的原因 —— 算进去，用户看到的
    // "待同步"就永远归不了零，长得像同步卡死。
    assert!(
        a.app
            .store()
            .outbox_len(
                notera_store::LOCAL_ACCOUNT_ID,
                &[notera_store::OpState::Pending]
            )
            .unwrap()
            >= 1,
        "哨兵账户的留痕行不该被同步顺手改掉"
    );

    let b = Device::boot("pull", &url);
    let stats_b = b.app.sync_once().await.expect("B 的一轮");
    assert!(stats_b.pulled >= 1, "B 必须从服务器拉到东西：{stats_b:?}");
    let notes = b.app.store().list_notes(&NoteQuery::all()).unwrap();
    assert_eq!(notes.len(), 1, "B 拉下来的就该是 A 那一条");
    assert_eq!(notes[0].title, text, "跨设备内容必须逐字一致");
    // B 也各自探过一次能力（同一台服务器，结论应相同），并且没把 A 的库当成第二个库
    assert_eq!(
        b.app
            .store()
            .account_caps(&b.account_id())
            .unwrap()
            .map(Caps::from_mask)
            .map(|c| c.write_strategy()),
        Some(WriteStrategy::S1)
    );
    srv.stop().await;
}

/// FT-ATT-02 的真服务器版本：A 挂一张图 → B 不只拿到笔记，还拿到那张图的**字节**。
///
/// 这条同时是"外来记录要登记附件"那处修复的证据。不登记的话：B 的下载队列是空的，
/// 界面永远是一张取不回来的占位图 —— 而文本轮照样 `Converged`、徽标写着"已同步"、
/// 设置页一个错误都看不到。两台设备各自读回的字节都与源一致，也就证明了服务器上那份
/// 不多不少（内容寻址，错一个字节所有引用同一 sha 的笔记都被污染）。
#[tokio::test]
async fn an_attachment_follows_its_note_to_a_second_device() {
    let srv = TestServer::start(Backend::Mem).await;
    let url = srv.base_url();
    let a = Device::boot("att-a", &url);
    let folder = a.app.default_folder_id().unwrap();
    let note = a.app.create_note(&folder, doc("带图的笔记")).unwrap();
    let blob: Vec<u8> = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, b'\x01'];
    let note_id = notera_core::EntityId::parse(&note.id).expect("命令层回带的 id 一定合法");
    let sha = a
        .app
        .store()
        .attach_blob(&note_id, &blob, "image/png", Some("shot.png"), "blk000002")
        .unwrap()
        .sha256;
    // 编辑器的真实顺序（`stores/editor.ts::attachFile`）：先落自己的编辑 → 核心写附件 →
    // 接住新 rev → **才把带 sha256 的附件块写进正文**。引用住在 doc 里，`note_attachments`
    // 只是本机账本 —— 只 `attach_blob` 而正文里没有引用，第二台设备根本无从知道要取哪个 blob。
    let head = a.app.store().get_note(&note_id).unwrap().unwrap();
    let with_image = json!({ "v": 1, "content": [
        { "id": "blk000001", "type": "paragraph", "content": [{ "text": "带图的笔记" }] },
        { "id": "blk000002", "type": "image", "attrs": {
            "sha256": &sha, "ref": &sha, "role": "inline", "pending": false,
            "size": blob.len(), "mediaType": "image/png", "name": "shot.png" } },
    ] });
    a.app
        .store()
        .edit_note(&note_id, with_image, head.rev)
        .expect("把附件引用写进正文");

    let ra = a
        .app
        .remote_for_sync()
        .await
        .unwrap()
        .expect("A 装了适配器");
    a.app.sync_once().await.expect("A 的文本轮");
    let round_a = a.app.run_attachment_round(&ra).await;
    assert_eq!(
        round_a,
        (1, 0, 0),
        "A 的附件轮要把那一个 blob 传上去：{round_a:?}"
    );
    assert_eq!(
        a.app.store().attachment_for_state(&sha).1,
        "present",
        "传成了就该记下服务器已有"
    );

    let b = Device::boot("att-b", &url);
    let rb = b
        .app
        .remote_for_sync()
        .await
        .unwrap()
        .expect("B 装了适配器");
    let stats_b = b.app.sync_once().await.expect("B 的文本轮");
    assert!(
        stats_b.pulled >= 1,
        "B 必须从服务器拉到那条记录：{stats_b:?}"
    );
    let queued: Vec<String> = b
        .app
        .store()
        .attachment_downloads(5)
        .unwrap()
        .into_iter()
        .map(|j| j.sha256)
        .collect();
    assert_eq!(
        queued,
        vec![sha.clone()],
        "收到记录却没登记附件 = 没有下载任务 = 图片永远停在占位"
    );

    let round_b = b.app.run_attachment_round(&rb).await;
    assert_eq!(
        round_b,
        (0, 1, 0),
        "B 的附件轮要把那一个 blob 取回来：{round_b:?}"
    );
    assert_eq!(
        std::fs::read(b.app.store().blob_path(&sha)).unwrap(),
        blob,
        "B 落盘的字节要和源一致"
    );
    // 走编辑器真正用的那条读路径（`attachment_data`），不是只看库里的账
    let data =
        notera_host::commands::dispatch(&b.app, "attachment_data", json!({ "sha256": &sha }))
            .expect("B 的界面读得出这张图");
    assert_eq!(
        notera_crypto::b64::decode(data["bytesBase64"].as_str().unwrap()).unwrap(),
        blob
    );
    assert_eq!(
        data["mediaType"], "image/png",
        "媒体类型随记录一起落地，否则显示只能退回二进制"
    );
    assert_eq!(
        b.app.store().attachment_refs(&sha).unwrap(),
        1,
        "引用计数不是 0，GC 才不会删掉还在用的 blob"
    );
    srv.stop().await;
}

/// 两台设备真的把同一条笔记改成分叉的两份（CONFLICT-RESOLUTION §6/§6.1 的验收）。
///
/// 这条测试是"冲突"这一块第一次被端到端跑起来 —— 之前只有手搓 `LocalView` 的单测，
/// 于是"冲突到底把什么留在了这台设备上"没人验证过。跑出来的事实是：**对面那台的编辑
/// 在这台设备上根本不存在**（只登记了两个哈希），面板左右两栏显示同一份本地内容，
/// 而本机脏 head 下一轮会把自己的版本推上去，把别人已确认的那一份静默盖掉。
/// 现在引擎在判出 UpdateUpdate 时真的去取远端记录并采纳为正文（本地那份先进副本）。
#[tokio::test]
async fn a_real_divergence_records_one_conflict_and_keeps_both_texts() {
    let srv = TestServer::start(Backend::Mem).await;
    let url = srv.base_url();
    let a = Device::boot("div-a", &url);
    let folder = a.app.default_folder_id().unwrap();
    let note = a.app.create_note(&folder, doc("共同起点")).unwrap();
    let id = notera_core::EntityId::parse(&note.id).unwrap();
    a.app.sync_once().await.expect("A 先把共同起点推上去");

    // A 本地改一份，先不推
    let a_head = a.app.store().get_note(&id).unwrap().unwrap();
    a.app
        .store()
        .edit_note(&id, doc("甲设备加的段落"), a_head.rev)
        .unwrap();

    // B 拉到起点，改成另一份，并推上服务器
    let b = Device::boot("div-b", &url);
    b.app.sync_once().await.expect("B 拉起点");
    let b_head = b.app.store().get_note(&id).unwrap().unwrap();
    b.app
        .store()
        .edit_note(&id, doc("乙设备加的段落"), b_head.rev)
        .unwrap();
    let pushed = b.app.sync_once().await.expect("B 推上去");
    assert!(pushed.pushed >= 1, "B 的改动要真的公告出去：{pushed:?}");

    // A 再同步：本地脏 + 远端也变了 → 必须进收件箱，且两份内容都不许没
    let stats_a = a.app.sync_once().await.expect("A 的第二轮");
    assert_eq!(stats_a.conflicts, 1, "这一轮该判出一次冲突：{stats_a:?}");
    let cards = notera_host::commands::dispatch(&a.app, "open_conflicts", json!({}))
        .expect("open_conflicts");
    let row = cards.as_array().expect("卡片数组");
    assert_eq!(row.len(), 1, "收件箱里就该一条：{row:?}");
    let card = &row[0];
    assert_eq!(card["noteId"], id.to_string(), "卡片得说清在跟谁打架");
    assert!(
        card["copyNoteId"].is_string(),
        "进箱的同一刻就该有本地副本（§6：副本不是提醒，是保底）"
    );

    let copy_id = notera_core::EntityId::parse(card["copyNoteId"].as_str().unwrap()).unwrap();
    let body = a.app.store().get_note(&id).unwrap().expect("正文还在");
    let copy = a.app.store().get_note(&copy_id).unwrap().expect("副本还在");
    // §6.1：正文是**已被别的设备确认的那一份**，本机那份以完整副本的形式活着。
    assert!(
        body.plain_text.contains("乙设备加的段落"),
        "正文该是服务器那一份：{}",
        body.plain_text
    );
    assert!(
        copy.plain_text.contains("甲设备加的段落"),
        "本机那一份必须原样在副本里：{}",
        copy.plain_text
    );
    // 采纳之后本机正文与远端一致 → 不许再把自己的那一版推上去盖掉别人的。
    // 这一轮**该**推的只有那篇副本（它是个新实体，别处还没有）。
    let again = a.app.sync_once().await.expect("A 的第三轮");
    assert_eq!(again.pushed, 1, "只该推上去那一篇副本：{again:?}");
    assert_eq!(
        again.conflicts, 0,
        "冲突已收敛，第二轮不该再造一张卡片：{again:?}"
    );
    let head = a.app.store().get_note(&id).unwrap().unwrap();
    assert_eq!(
        head.rev, head.sync_rev,
        "正文采纳后就该是已确认状态，否则下一轮它会把乙的版本盖掉"
    );
    assert_eq!(
        a.app.store().list_notes(&NoteQuery::all()).unwrap().len(),
        2,
        "一条正文 + 一条副本，不多也不少"
    );
    // 并排预览的右栏必须是真的那一份服务器内容
    let remote_preview = notera_host::commands::dispatch(
        &a.app,
        "preview_text",
        json!({ "id": id.to_string(), "rev": card["remoteRev"] }),
    );
    let shown = remote_preview
        .ok()
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .unwrap_or_default();
    assert!(
        shown.contains("乙设备加的段落"),
        "右栏显示的不是服务器那一版：{shown}"
    );
    // 左栏按 (copyNoteId, copyRev) 读 —— 卡片不带 copyRev 的话，前端只能拿正文顶替，两栏就同款了
    assert_eq!(card["copyRev"], 1, "卡片要给出副本的 rev：{card:?}");
    let local_preview = notera_host::commands::dispatch(
        &a.app,
        "preview_text",
        json!({ "id": card["copyNoteId"], "rev": card["copyRev"] }),
    );
    let mine = local_preview
        .ok()
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .unwrap_or_default();
    assert!(
        mine.contains("甲设备加的段落"),
        "左栏显示的不是本机那一版：{mine}"
    );
    srv.stop().await;
}

/// 删除 vs 修改（P11）：引擎**故意**不自动采纳这一类（保留哪一边该由用户决定），
/// 所以只要用户没处理，它每轮都会再被判出来。这条钉的是"重复判出同一件事"不许留下
/// 重复痕迹：收件箱每轮多一张一样的卡片是骚扰，每轮多造一篇副本笔记是往用户库里塞垃圾。
#[tokio::test]
async fn a_recurring_conflict_is_recorded_once_and_makes_exactly_one_copy() {
    let srv = TestServer::start(Backend::Mem).await;
    let url = srv.base_url();
    let a = Device::boot("dup-a", &url);
    let folder = a.app.default_folder_id().unwrap();
    let note = a.app.create_note(&folder, doc("共同起点")).unwrap();
    let id = notera_core::EntityId::parse(&note.id).unwrap();
    a.app.sync_once().await.expect("A 把起点推上去");

    let b = Device::boot("dup-b", &url);
    b.app.sync_once().await.expect("B 拉到起点");
    let b_head = b.app.store().get_note(&id).unwrap().unwrap();
    b.app
        .store()
        .edit_note(&id, doc("乙改过的一版"), b_head.rev)
        .unwrap();
    b.app.sync_once().await.expect("B 把改动推上去");

    a.app
        .store()
        .delete_note(&id)
        .expect("A 删掉它（删除还没传播）");
    let first = a.app.sync_once().await.expect("A 的第一轮");
    assert_eq!(first.conflicts, 1, "删除 vs 修改要进收件箱：{first:?}");
    let rows = a.app.store().open_conflicts().unwrap().len();
    let notes = a.app.store().list_notes(&NoteQuery::all()).unwrap().len();
    assert_eq!(rows, 1, "第一轮就该只有一张卡片：{rows}");

    // 后面几轮仍会判出同一件事（P11 由引擎故意不自动收敛：留哪一边要用户决定）。
    // 这里不问 outcome，只盯"重复判定不许留下重复痕迹"—— 见下面两条断言。
    for round in 2..=4 {
        a.app
            .sync_once()
            .await
            .unwrap_or_else(|e| panic!("第 {round} 轮不该失败：{e:?}"));
    }
    assert_eq!(
        a.app.store().open_conflicts().unwrap().len(),
        rows,
        "同一件事不许逐轮再登记一次"
    );
    assert_eq!(
        a.app.store().list_notes(&NoteQuery::all()).unwrap().len(),
        notes,
        "副本笔记不许逐轮多造一篇"
    );
    srv.stop().await;
}

/// 面板上"用我这一版"这颗按钮说到做到：两版**互换**，然后照常传播到另一台设备。
///
/// 关键性质不是"换了"，而是**换了也谁都没丢**：正文与副本各拿到对方的内容，两版始终各有
/// 一处存放；副本是普通笔记，会同步给别人。所以对面那台设备最终同时持有两份，而不是
/// 被覆盖掉一份。这条测试跑的是真分叉 → 用户决定 → 双方再同步的完整回路。
#[tokio::test]
async fn replacing_with_the_local_version_swaps_both_sides_and_propagates() {
    let srv = TestServer::start(Backend::Mem).await;
    let url = srv.base_url();
    let a = Device::boot("sw-a", &url);
    let folder = a.app.default_folder_id().unwrap();
    let note = a.app.create_note(&folder, doc("共同起点")).unwrap();
    let id = notera_core::EntityId::parse(&note.id).unwrap();
    a.app.sync_once().await.expect("A 推起点");

    let a_head = a.app.store().get_note(&id).unwrap().unwrap();
    a.app
        .store()
        .edit_note(&id, doc("甲设备加的段落"), a_head.rev)
        .unwrap();
    let b = Device::boot("sw-b", &url);
    b.app.sync_once().await.expect("B 拉起点");
    let b_head = b.app.store().get_note(&id).unwrap().unwrap();
    b.app
        .store()
        .edit_note(&id, doc("乙设备加的段落"), b_head.rev)
        .unwrap();
    b.app.sync_once().await.expect("B 推上去");
    a.app.sync_once().await.expect("A 判出冲突并采纳");

    let card = &notera_host::commands::dispatch(&a.app, "open_conflicts", json!({})).unwrap()[0];
    let copy_id = notera_core::EntityId::parse(card["copyNoteId"].as_str().unwrap()).unwrap();
    // 采纳之后：正文 = 乙，副本 = 甲
    assert!(
        a.app
            .store()
            .get_note(&id)
            .unwrap()
            .unwrap()
            .plain_text
            .contains("乙"),
        "前提：正文已被采纳成服务器那一版"
    );

    notera_host::commands::dispatch(
        &a.app,
        "resolve_conflict",
        json!({ "id": card["id"], "action": "replaceWithLocal" }),
    )
    .expect("用我这一版");
    let body = a.app.store().get_note(&id).unwrap().unwrap();
    let copy = a.app.store().get_note(&copy_id).unwrap().unwrap();
    assert!(
        body.plain_text.contains("甲设备加的段落"),
        "点完之后正文该是我这一版：{}",
        body.plain_text
    );
    assert!(
        copy.plain_text.contains("乙设备加的段落"),
        "对面那一版必须原样活着（这次决定不许吃掉任何人的输入）：{}",
        copy.plain_text
    );
    assert!(
        body.rev > body.sync_rev,
        "正文换了内容就得重新公告，否则另一台看到的还是旧的一版"
    );
    assert!(
        a.app.store().open_conflicts().unwrap().is_empty(),
        "处理完的卡片要关掉"
    );

    a.app.sync_once().await.expect("A 把决定推上去");
    let stats_b = b.app.sync_once().await.expect("B 拉回来");
    assert!(stats_b.pulled >= 1, "B 要能看到 A 的决定：{stats_b:?}");
    let mine: Vec<String> = b
        .app
        .store()
        .list_notes(&NoteQuery::all())
        .unwrap()
        .into_iter()
        .map(|r| format!("{} {}", r.title, r.summary))
        .collect();
    assert!(
        mine.iter().any(|t| t.contains("甲设备加的段落")),
        "B 上该有甲那一版：{mine:?}"
    );
    assert!(
        mine.iter().any(|t| t.contains("乙设备加的段落")),
        "B 上也该有乙那一版：{mine:?}"
    );
    srv.stop().await;
}

// ---------------------------------------------------------------- §11.4 租约 ---

/// 清单 index.json 的 sha（内容指纹）。公告了就会变，没公告就不该变。
fn manifest_sha(srv: &TestServer) -> String {
    srv.fs_dump()["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .find(|e| e["path"].as_str() == Some("/.notes/manifest/index.json"))
        .map(|e| e["sha256"].as_str().unwrap_or_default().to_string())
        .unwrap_or_default()
}

fn lock_writes(srv: &TestServer) -> Vec<String> {
    srv.request_log()
        .iter()
        .filter(|r| r.method == "PUT" && r.path.contains("/locks/"))
        .map(|r| r.path.clone())
        .collect()
}

/// 让探测认为"这台服务器没有强 ETag"：探测用的那次 GET 永远回 200，不给 304。
/// 没强 ETag ⇒ 清单 CAS 不可信 ⇒ §11.4 要求启用租约。
async fn pretend_no_strong_etag(srv: &TestServer) {
    srv.inject(Injection::status("GET /.notes/probe/etag.json", 200))
        .await;
}

#[tokio::test]
async fn a_weak_server_turns_the_lease_on_and_the_second_device_yields_the_announcement() {
    let srv = TestServer::start(Backend::Mem).await;
    let url = srv.base_url();
    pretend_no_strong_etag(&srv).await;

    let a = Device::boot("lease-a", &url);
    a.app
        .create_note(&a.app.default_folder_id().unwrap(), doc("A 的先手"))
        .unwrap();
    let st_a = a.app.sync_once().await.expect("A 的一轮");
    assert!(st_a.pushed >= 1, "A 要先推上去：{st_a:?}");
    assert!(
        matches!(a.app.lease_policy(), notera_sync::LeasePolicy::On { .. }),
        "探不到强 ETag 就该开租约"
    );
    // A 的租约：服务器上有一份、本地 sync_state 里也记了一份
    let locks_after_a = lock_writes(&srv);
    assert_eq!(
        locks_after_a.len(),
        1,
        "弱服务器下必须贴自己的租约：{locks_after_a:?}"
    );
    let acct_a = a.account_id();
    let state_a = a
        .app
        .store()
        .sync_state(&acct_a)
        .unwrap()
        .expect("同步状态行");
    assert!(
        state_a
            .lease_token
            .as_deref()
            .unwrap_or_default()
            .starts_with("01"),
        "租约 token 要落库：{state_a:?}"
    );
    assert!(state_a
        .lease_expires_at
        .as_deref()
        .unwrap_or_default()
        .ends_with('Z'));

    // 第二台设备：它也会贴自己的，然后看见 A 的还新鲜 → 让路
    let b = Device::boot("lease-b", &url);
    // 先入伙再改：§2 不许两个"各自有内容"的库并成一个，所以 B 的空库先协商。
    b.app.sync_once().await.expect("B 先入伙");
    b.app
        .create_note(&b.app.default_folder_id().unwrap(), doc("B 的后手"))
        .unwrap();
    let before = manifest_sha(&srv);
    let st_b = b.app.sync_once().await.expect("B 的一轮");
    assert!(st_b.pushed >= 1, "让路只挡公告，不挡上传：{st_b:?}");
    assert_eq!(
        manifest_sha(&srv),
        before,
        "B 让路期间不许提交清单（两份公告互相覆盖才是要防的事）"
    );
    let status = b.app.sync_status().unwrap();
    assert_eq!(
        status.message_key.as_deref(),
        Some("sync.leaseHeld"),
        "让路必须让用户看得见：{status:?}"
    );
    assert!(status.retryable, "让路是可重试状态");
    assert_eq!(
        b.app.store().stats().unwrap().dirty_notes,
        1,
        "没公告的改动必须还是 dirty，下一轮重发"
    );
    let both = lock_writes(&srv);
    // 每一轮都会续期，所以 PUT 次数会多于设备数；按路径去重才是"几台设备在贴"。
    let devices: std::collections::BTreeSet<&str> = both
        .iter()
        .map(|p| p.rsplit('/').next().unwrap_or_default())
        .collect();
    assert_eq!(
        devices.len(),
        2,
        "每台设备一份租约（续期是重复 PUT 同一路径）：{both:?}"
    );
    srv.clear_injection().await;
    srv.stop().await;
}

#[tokio::test]
async fn a_server_with_strong_etag_pays_nothing_for_the_lease() {
    let srv = TestServer::start(Backend::Mem).await;
    let a = Device::boot("lease-strong", &srv.base_url());
    a.app
        .create_note(&a.app.default_folder_id().unwrap(), doc("强 ETag 服务器"))
        .unwrap();
    let st = a.app.sync_once().await.expect("一轮");
    assert!(st.pushed >= 1);
    assert!(
        lock_writes(&srv).is_empty(),
        "有强 ETag + 条件写时 CAS 已经够用，不该白多两个请求"
    );
    assert!(
        matches!(a.app.lease_policy(), notera_sync::LeasePolicy::Off),
        "CAS 可信时不该开租约"
    );
    srv.stop().await;
}

/// 同一篇连改两次、只跑一轮同步：旧 rev 的那条待办必须被较新的那次公告结清。
///
/// 这条是被崩溃注入照出来的真实缺陷：`outbox_settle` 以前按**精确 rev** 匹配，
/// 于是 rev=2 的行在 rev=3 公告之后仍然 `pending`，永远没有下一轮会去结它 ——
/// 设置页的"待同步"计数就此卡住不动（§18 要求它诚实）。
#[tokio::test]
async fn two_edits_before_one_round_leave_nothing_pending() {
    let srv = TestServer::start(Backend::Mem).await;
    let a = Device::boot("two-edits", &srv.base_url());
    let folder = a.app.default_folder_id().unwrap();
    let note = a.app.create_note(&folder, doc("第一版")).unwrap();
    let id = notera_core::EntityId::parse(&note.id).unwrap();
    a.app
        .store()
        .edit_note(&id, doc("第二版"), notera_core::Rev(note.rev))
        .unwrap();
    a.app
        .store()
        .edit_note(&id, doc("第三版"), notera_core::Rev(note.rev + 1))
        .unwrap();

    a.app.sync_once().await.expect("一轮同步");
    let st = a.app.store().stats().unwrap();
    assert_eq!(st.dirty_notes, 0, "公告之后不该还有脏 head：{st:?}");
    assert_eq!(
        st.outbox_pending, 0,
        "旧 rev 的待办没被结清，计数会永远挂着：{st:?}"
    );
    srv.stop().await;
}

/// 一台删除，另一台**空闲**（这一行干净、没有待办）也必须接到这条删除。
///
/// 为什么单列一条：`dirty_entities` 按定义只报 `rev <> sync_rev`，空闲那一台的这一行**进不了
/// 本轮计划**；计划里那一格就变成 `decide(None, 远端删除)` → P2b"本地没有这一条，无事可做"，
/// 可它其实有这一条。两台真设备的形状是：A 删除并公告，B 几轮之后主行仍活在正常列表、
/// 徽标还说"已同步"。
#[tokio::test]
async fn a_delete_reaches_a_device_that_had_nothing_pending() {
    let srv = TestServer::start(Backend::Mem).await;
    let url = srv.base_url();
    let a = Device::boot("delidle-a", &url);
    let folder = a.app.default_folder_id().unwrap();
    let note = a.app.create_note(&folder, doc("要被删掉的一版")).unwrap();
    let id = notera_core::EntityId::parse(&note.id).unwrap();
    a.app.sync_once().await.expect("A 公告起点");
    let b = Device::boot("delidle-b", &url);
    b.app.sync_once().await.expect("B 拉到起点");
    assert!(
        b.app.store().get_note(&id).unwrap().is_some(),
        "夹具没造对：B 该先有这一条"
    );
    a.app.store().delete_note(&id).expect("A 删除");
    for _ in 0..3 {
        a.app.sync_once().await.expect("A 公告删除");
    }
    for _ in 0..4 {
        b.app.sync_once().await.expect("B 追删除");
    }
    let rows = b.app.store().list_notes(&NoteQuery::all()).unwrap();
    let st = b.app.store().stats().unwrap();
    assert!(
        b.app
            .store()
            .get_note(&id)
            .ok()
            .flatten()
            .and_then(|n| n.deleted_at)
            .is_some(),
        "A 删掉的那条在空闲的 B 上还活在正常列表：rows={rows:?} stats={st:?}"
    );
    srv.stop().await;
}

/// §10 那一格（清单说有、记录 404）今天**只有一句注释**，全套门禁没测过它。
/// 这一条钉的是"必须是真"的三半：本地内容一个字不许少、不许留下永久待同步、也不许因此把轮次跑失败。
///
/// **并且它顺手把缺口 G21 的机制拍成照片**：注入四轮之后，针对这条记录的 GET 次数由诊断行打出来
/// （2026-09-29 实测 **0**）—— `synced_heads`（"已经有了的那一版别再花请求去要"）在 pull 循环里
/// 先 `continue` 掉了，
/// 于是 §10 那条"连续 404 → 标 dirty 走 P3 补传"在**已收敛的设备上永远不可达**：
/// 服务器把记录文件弄丢之后，所有收敛过的设备都说"已同步"，谁也不会去补。
/// 这不是"界面说谎"（每台设备本地确实有那一版），而是"数据在自己服务器上"这句承诺静默失真 ——
/// 修法与代价记在 `PRODUCTION-READINESS` §7 G21（要做校验就得花请求，直接顶着 PERF-05 的空轮预算）。
/// 这条判据因此**只钉现状里必须是真的那三样**，不钉"永远不补"：G21 修好之后它照样绿。
#[tokio::test]
async fn a_vanished_remote_record_keeps_local_content_and_leaves_no_pending_state() {
    let srv = TestServer::start(Backend::Mem).await;
    let url = srv.base_url();
    let a = Device::boot("gone-a", &url);
    let folder = a.app.default_folder_id().unwrap();
    let note = a
        .app
        .create_note(&folder, doc("服务器后来把这份弄丢了"))
        .unwrap();
    let id = notera_core::EntityId::parse(&note.id).unwrap();
    for _ in 0..3 {
        a.app.sync_once().await.expect("A 公告");
    }
    let b = Device::boot("gone-b", &url);
    for _ in 0..4 {
        b.app.sync_once().await.expect("B 追平");
    }
    let before = b.app.store().get_note(&id).unwrap().expect("B 该有这一条");
    assert_eq!(
        before.rev, before.sync_rev,
        "夹具没造对：B 该是干净的（追平之后）"
    );

    // 只挡 GET 记录，PUT 依旧通 —— 于是"补传"这条路上不留外部阻碍，能不能补才看得清
    let log_before = srv.request_log().len();
    srv.inject(notera_test_webdav::Injection::status(
        format!("GET *{}", note.id),
        404,
    ))
    .await;
    let mut stats = None;
    for _ in 0..4 {
        b.app
            .sync_once()
            .await
            .expect("注入 404 的几轮不许把轮次跑失败");
        stats = Some(b.app.store().stats().unwrap());
    }
    srv.inject(notera_test_webdav::Injection::none()).await;

    let after = b
        .app
        .store()
        .get_note(&id)
        .unwrap()
        .expect("本地那条不许消失");
    let st = stats.expect("跑过几轮");
    let mut problems = Vec::new();
    if after.content_hash != before.content_hash
        || after.rev != before.rev
        || after.deleted_at.is_some()
    {
        problems.push(format!(
            "远端 404 之后本机那条被改了或进了回收站：rev {} → {} 删除={:?} 哈希 {} → {}",
            before.rev, after.rev, after.deleted_at, before.content_hash, after.content_hash
        ));
    }
    if st.dirty_notes != 0 || st.outbox_pending != 0 {
        problems.push(format!(
            "记录 404 之后留下永久待同步：dirty={} outbox={}",
            st.dirty_notes, st.outbox_pending
        ));
    }
    let hits: Vec<_> = srv
        .request_log()
        .into_iter()
        .skip(log_before)
        .filter(|r| r.method == "GET" && r.path.contains(&note.id))
        .collect();
    println!(
        "诊断 G21：本机那条={} 脏={} 待办={} 注入这四轮里针对这条记录的 GET 次数={}（其中 404 的可见次数={}）",
        if after.deleted_at.is_some() { "回收站" } else { "正常" },
        st.dirty_notes,
        st.outbox_pending,
        hits.len(),
        hits.iter().filter(|r| r.status == 404).count()
    );
    assert!(problems.is_empty(), "{}", problems.join(" ｜ "));
    srv.stop().await;
}

/// P13（别处把这条**永久删除**了）此前只有一条规划层单测看着 —— 规则审计里它红在 1 条，就是那一格。
/// 这一条补上两设备那一半：A 永久删除并公告之后，B（干净、空闲）那台这一条要**整个读不到**，
/// 并留下一张 purged 墓碑 —— 于是它以后既不会被对面那一版写活，也不会从本机历史里被翻出来。
/// 顺带把 §8 那句"永久删除的记录是 `purged:true, payload=null` 的墓碑公告"打在**服务器盘上那个文件本身**：
/// 正文留在服务器上就等于没删（这一台删掉了，别人换个设备还能把它取回来）。
#[tokio::test]
async fn a_permanent_delete_reaches_a_device_that_had_nothing_pending() {
    let dav = Tmp::new("purge-dav");
    let srv = TestServer::start(Backend::Fs(dav.path().to_path_buf())).await;
    let url = srv.base_url();
    let a = Device::boot("purge-a", &url);
    let folder = a.app.default_folder_id().unwrap();
    let note = a
        .app
        .create_note(&folder, doc("要被永久删掉的一版"))
        .unwrap();
    let id = notera_core::EntityId::parse(&note.id).unwrap();
    for _ in 0..3 {
        a.app.sync_once().await.expect("A 公告起点");
    }
    let b = Device::boot("purge-b", &url);
    for _ in 0..4 {
        b.app.sync_once().await.expect("B 追平");
    }
    assert!(
        b.app.store().get_note(&id).unwrap().is_some(),
        "夹具没造对：B 该先有这一条"
    );

    a.app.store().purge_note(&id).expect("A 永久删除");
    for _ in 0..4 {
        a.app.sync_once().await.expect("A 公告永久删除");
    }
    for i in 0..5 {
        let st = b.app.sync_once().await.expect("B 追永久删除");
        println!(
            "[diag4] B 第 {i} 轮：{:?} ｜ 本机这一行={}",
            st,
            b.app
                .store()
                .get_note(&id)
                .unwrap()
                .map(|n| format!(
                    "rev={}/删={}/purged表={}",
                    n.rev,
                    n.deleted_at.is_some(),
                    n.purged_at.is_some()
                ))
                .unwrap_or_else(|| "读不到".to_string())
        );
    }
    let st = b.app.store().stats().unwrap();
    let mut problems = Vec::new();
    if b.app.store().get_note(&id).unwrap().is_some() {
        problems.push("对面永久删除之后，B 那台还读得到这一条（回收站里也不许有）".to_string());
    }
    if st.notes != 0 || st.notes_trash != 0 {
        problems.push(format!(
            "B 那台还留着这一行的痕迹：正常 {} 回收站 {}",
            st.notes, st.notes_trash
        ));
    }
    if st.tombstones_purged == 0 {
        problems.push("B 那台没落下 purged 墓碑 —— 以后任何一份旧视图都可能把它写活".to_string());
    }
    if st.dirty_notes != 0 || st.outbox_pending != 0 {
        problems.push(format!(
            "追平之后账没结清：dirty={} outbox={}",
            st.dirty_notes, st.outbox_pending
        ));
    }
    // 服务器盘上那一条记录本身：必须是"墓碑公告"，正文一个字都不许留下（§8）。
    let rec = dav
        .path()
        .join(".notes")
        .join("records")
        .join("note")
        .join(format!("{}.json", note.id));
    let raw = std::fs::read_to_string(&rec).unwrap_or_default();
    if raw.is_empty() {
        problems.push(format!(
            "服务器上没有那条记录文件（{}）—— 清单还指着它，D2 重建就没料",
            rec.display()
        ));
    } else {
        let env: serde_json::Value = serde_json::from_str(&raw).unwrap_or_default();
        if env["purged"] != json!(true) {
            problems.push(format!(
                "服务器上那条记录不是永久删除公告：purged={} payload={}",
                env["purged"],
                if env["payload"].is_null() {
                    "null"
                } else {
                    "非空"
                }
            ));
        }
        if !env["payload"].is_null() {
            problems
                .push("永久删除之后服务器上的记录还带着 payload（正文可被别人取回）".to_string());
        }
        if raw.contains("要被永久删掉的一版") {
            problems.push("服务器那条记录里还读得到正文".to_string());
        }
    }
    println!(
        "诊断 G22：B 的 tombstones={}/purged={} ｜ A 的 purged={} ｜ 服务器记录文件={}（含正文={}）",
        st.tombstones,
        st.tombstones_purged,
        a.app.store().stats().unwrap().tombstones_purged,
        raw.chars().take(120).collect::<String>().replace('\n', ""),
        raw.contains("要被永久删掉的一版")
    );
    assert!(problems.is_empty(), "{}", problems.join(" ｜ "));
    srv.stop().await;
}

/// 量一件事（**`#[ignore]` 量具，只出数不判绿**）：一条**带附件**的笔记被永久删除之后，
/// 服务器上那些按 sha256 寻址的 blob 还在不在 —— 本机这一侧有 GC，远端这一侧今天没有判据。
///
/// 2026-09-29 用它量到的事实（`cargo test -p notera-host --test sync_once -- --ignored --nocapture`）：
/// 笔记记录被换成 369 B 的墓碑公告（`purged=true / payload=null`）、B 那台的引用数归 0、A 留下 purged 墓碑，
/// **而 `.notes/attachments/<sha前两位>/<sha>` 那份字节原样躺在服务器上**。
/// 口径没定之前不写断言（定了就要反过来断"不许在"），见 PRODUCTION-READINESS §7 的 **G23**。
#[tokio::test]
#[ignore = "量具：永久删除之后服务器上的附件 blob 走没走 —— 只出数，不当门禁"]
async fn probe_purge_and_remote_attachment_blobs() {
    let dav = Tmp::new("probe-purge-dav");
    let srv = TestServer::start(Backend::Fs(dav.path().to_path_buf())).await;
    let url = srv.base_url();
    let a = Device::boot("pp-a", &url);
    let folder = a.app.default_folder_id().unwrap();
    let note = a
        .app
        .create_note(&folder, doc("带图并且要被永久删掉"))
        .unwrap();
    let blob: Vec<u8> = vec![
        0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, b'X', b'Y', b'Z',
    ];
    let nid = notera_core::EntityId::parse(&note.id).unwrap();
    let sha = a
        .app
        .store()
        .attach_blob(&nid, &blob, "image/png", Some("shot.png"), "blk000002")
        .unwrap()
        .sha256;
    let head = a.app.store().get_note(&nid).unwrap().unwrap();
    let with_image = json!({ "v": 1, "content": [
        { "id": "blk000001", "type": "paragraph", "content": [{ "text": "带图并且要被永久删掉" }] },
        { "id": "blk000002", "type": "image", "attrs": {
            "sha256": &sha, "ref": &sha, "role": "inline", "pending": false,
            "size": blob.len(), "mediaType": "image/png", "name": "shot.png" } },
    ] });
    a.app.store().edit_note(&nid, with_image, head.rev).unwrap();
    let ra = a.app.remote_for_sync().await.unwrap().expect("A 适配器");
    a.app.sync_once().await.expect("A 文本轮");
    let round_a = a.app.run_attachment_round(&ra).await;
    println!("量具：A 的附件轮={round_a:?} sha={sha}");
    let b = Device::boot("pp-b", &url);
    let rb = b.app.remote_for_sync().await.unwrap().expect("B 适配器");
    for _ in 0..4 {
        b.app.sync_once().await.expect("B 文本追平");
        b.app.run_attachment_round(&rb).await;
    }
    let blob_on_b = b.app.store().attachment_for_state(&sha);
    println!("量具：B 那台读到的附件状态={blob_on_b:?}");
    let walk = |tag: &str| {
        let mut out = Vec::new();
        let mut stack = vec![dav.path().to_path_buf()];
        while let Some(dir) = stack.pop() {
            if let Ok(rd) = std::fs::read_dir(&dir) {
                for e in rd.flatten() {
                    let p = e.path();
                    if p.is_dir() {
                        stack.push(p);
                    } else if let Ok(m) = std::fs::metadata(&p) {
                        out.push(format!(
                            "{}:{}B",
                            p.strip_prefix(dav.path()).unwrap().display(),
                            m.len()
                        ));
                    }
                }
            }
        }
        out.sort();
        println!("量具 {tag} 服务器上的文件：{}", out.join(" | "));
        out.iter().any(|f| f.contains(&sha))
    };
    let before = walk("永久删除之前");
    a.app.store().purge_note(&nid).expect("A 永久删除");
    for _ in 0..4 {
        a.app.sync_once().await.expect("A 公告永久删除");
        a.app.run_attachment_round(&ra).await;
    }
    for _ in 0..3 {
        b.app.sync_once().await.expect("B 追永久删除");
        b.app.run_attachment_round(&rb).await;
    }
    let after = walk("永久删除之后");
    let refs_b = b.app.store().attachment_refs(&sha).unwrap();
    println!(
        "量具结论：服务器上带这个 sha 的文件 删除前={before} 删除后={after} ｜ B 那台的引用数={refs_b} ｜ A 的 purged 墓碑={}",
        a.app.store().stats().unwrap().tombstones_purged
    );
    srv.stop().await;
}

/// §8.4 那一格：**本机把一条笔记从回收站里恢复出来，而服务器上那条已经被对面永久删除** ——
/// 文档承诺"走冲突路径而非静默复活"。这一条钉的就是那两句：① **必须问用户**（B 上要出现一张卡片），
/// ② **服务器上那条必须还是墓碑公告**（悄悄把正文 PUT 回去就是"用同步复活永久删除的数据"，总指令明令禁止）。
#[tokio::test]
async fn restoring_a_purged_note_from_the_trash_asks_instead_of_resurrecting_it() {
    let dav = Tmp::new("revive-dav");
    let srv = TestServer::start(Backend::Fs(dav.path().to_path_buf())).await;
    let url = srv.base_url();
    let a = Device::boot("rs-a", &url);
    let folder = a.app.default_folder_id().unwrap();
    let note = a.app.create_note(&folder, doc("要被恢复的这一版")).unwrap();
    let id = notera_core::EntityId::parse(&note.id).unwrap();
    for _ in 0..3 {
        a.app.sync_once().await.expect("A 公告起点");
    }
    let b = Device::boot("rs-b", &url);
    for _ in 0..4 {
        b.app.sync_once().await.expect("B 追平");
    }
    // B 删（本机脏，还没公告），A 那边永久删除并公告
    b.app.store().delete_note(&id).unwrap();
    a.app.store().purge_note(&id).expect("A 永久删除");
    for _ in 0..4 {
        a.app.sync_once().await.expect("A 公告永久删除");
    }
    // 用户在 B 上明确点了"恢复到正常列表"
    b.app.store().restore_note(&id).expect("B 恢复");
    let mut rounds = Vec::new();
    for _ in 0..4 {
        let st = b.app.sync_once().await.expect("B 轮次");
        rounds.push((st.conflicts, st.rejections, st.outcome));
    }
    let row = b.app.store().get_note(&id).unwrap();
    let st = b.app.store().stats().unwrap();
    let cards = b.app.open_conflicts().unwrap_or_default();
    let mut problems = Vec::new();
    if cards.is_empty() {
        problems.push(
            "B 那台没被问一句：对面永久删除、本机从回收站恢复，§8.4 承诺的是走冲突路径而不是静默二选一"
                .to_string(),
        );
    }
    if row.is_none() {
        problems.push("本机用户明确恢复出来的那一版不见了（冲突期间必须留着等他决定）".to_string());
    }
    // 服务器上那条记录还是不是墓碑公告 —— 被 PUT 回正文就是"用同步复活永久删除的数据"。
    let rec = dav
        .path()
        .join(".notes")
        .join("records")
        .join("note")
        .join(format!("{}.json", note.id));
    let raw = std::fs::read_to_string(&rec).unwrap_or_default();
    let env: serde_json::Value = serde_json::from_str(&raw).unwrap_or_default();
    if raw.is_empty() || env["purged"] != json!(true) || !env["payload"].is_null() {
        problems.push(format!(
            "服务器上那条被复活回来了：purged={} payload={} 含正文={}",
            env["purged"],
            if env["payload"].is_null() {
                "null"
            } else {
                "非空"
            },
            raw.contains("要被恢复的这一版")
        ));
    }
    println!(
        "量具 §8.4：B 的轮次(冲突/拒收/结局)={rounds:?} ｜ 本机这一行={} ｜ 卡片={} 张 ｜ 未决账：脏={} 待办={} ｜ 服务器记录={} 字节、含正文={}",
        row.as_ref()
            .map(|n| format!("rev={}/sync_rev={}/删={}", n.rev, n.sync_rev, n.deleted_at.is_some()))
            .unwrap_or_else(|| "读不到".to_string()),
        cards.len(),
        st.dirty_notes,
        st.outbox_pending,
        raw.len(),
        raw.contains("要被恢复的这一版")
    );
    assert!(problems.is_empty(), "{}", problems.join(" ｜ "));
    srv.stop().await;
}

/// 同一形状的**下一格**：卡片弹出来之后，用户按「用服务器那一版」（也就是接受对面那次永久删除）——
/// 这一按必须真的把本机那条收掉（行消失 + purged 墓碑 + 账结清 + 卡片不再回来），
/// 而不是报一个永远消不掉的错。G24 把"问一句"补上之后，这一格才成为"永久删除"这条链的下一跳。
#[tokio::test]
async fn accepting_the_peers_purge_after_a_restore_actually_purges_locally() {
    let dav = Tmp::new("revive-accept-dav");
    let srv = TestServer::start(Backend::Fs(dav.path().to_path_buf())).await;
    let url = srv.base_url();
    let a = Device::boot("ra-a", &url);
    let folder = a.app.default_folder_id().unwrap();
    let note = a
        .app
        .create_note(&folder, doc("按下去要真收掉的一版"))
        .unwrap();
    let id = notera_core::EntityId::parse(&note.id).unwrap();
    for _ in 0..3 {
        a.app.sync_once().await.expect("A 公告起点");
    }
    let b = Device::boot("ra-b", &url);
    for _ in 0..4 {
        b.app.sync_once().await.expect("B 追平");
    }
    b.app.store().delete_note(&id).unwrap();
    a.app.store().purge_note(&id).expect("A 永久删除");
    for _ in 0..4 {
        a.app.sync_once().await.expect("A 公告永久删除");
    }
    b.app.store().restore_note(&id).expect("B 恢复");
    for _ in 0..3 {
        b.app.sync_once().await.expect("B 问出卡片");
    }
    let cards = b.app.open_conflicts().expect("收件箱");
    let here: Vec<_> = cards
        .iter()
        .filter(|c| c.note_id == id.to_string())
        .cloned()
        .collect();
    let mut problems = Vec::new();
    if here.is_empty() {
        problems.push("夹具没造对：B 上该有一张对面永久删除的卡片".to_string());
    } else {
        let card_id: i64 = serde_json::to_value(&here[0])
            .expect("卡片可序列化")
            .get("id")
            .and_then(|v| v.as_i64())
            .expect("卡片带行号");
        let res = b
            .app
            .resolve_conflict(notera_host::commands::ResolveConflictCmd {
                id: card_id,
                action: "replaceWithRemote".into(),
            });
        println!(
            "量具 G24：B 按「用服务器那一版」的结果={:?}",
            res.as_ref().err()
        );
        for _ in 0..6 {
            b.app.sync_once().await.expect("B 收尾");
        }
        let st = b.app.store().stats().expect("B 的统计");
        if b.app.store().get_note(&id).expect("读库").is_some() {
            problems.push("按了对面那一版，本机这条却还在".to_string());
        }
        if st.tombstones_purged == 0 {
            problems.push("按了对面那一版却没落下 purged 墓碑 —— 以后还能被写活".to_string());
        }
        if st.dirty_notes != 0 || st.outbox_pending != 0 {
            problems.push(format!(
                "裁决之后账没结清：dirty={} outbox={}",
                st.dirty_notes, st.outbox_pending
            ));
        }
        if !b
            .app
            .open_conflicts()
            .expect("收件箱")
            .iter()
            .any(|c| c.note_id == id.to_string())
        {
            // 卡片消掉才算真收口（这一句是反着写的：消不掉就是还挂着）
        } else {
            problems.push("裁决过的卡片又回到收件箱".to_string());
        }
    }
    assert!(problems.is_empty(), "{}", problems.join(" ｜ "));
    srv.stop().await;
}
