//! §13 附件队列的存储侧契约：谁该传、谁该下、下载回来的字节凭什么算数。
mod common;

use common::*;
use notera_store::{AttachmentJob, Store};

fn attach(store: &Store, folder: &notera_core::EntityId, bytes: &[u8], block: &str) -> String {
    let note = store.create_note(folder, doc_text("带附件")).unwrap();
    store.attach_blob(&note.id, bytes, "image/png", Some("a.png"), block).unwrap().sha256
}

#[test]
fn uploads_are_selected_by_state_and_sorted_by_size() {
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);
    let big = attach(&store, &folder, &vec![7u8; 4096], "blkbig01");
    let small = attach(&store, &folder, &vec![9u8; 16], "blksml01");
    store.set_attachment_states(&big, None, Some("present")).unwrap();

    let jobs = store.attachment_uploads(10).unwrap();
    assert_eq!(
        jobs.iter().map(|j| j.sha256.as_str()).collect::<Vec<_>>(),
        vec![small.as_str()],
        "远端已 present 的不得再排进上传队列"
    );
    let both = store.attachment_uploads(10).unwrap();
    assert_eq!(both.len(), 1);
    store.set_attachment_states(&big, None, Some("unknown")).unwrap();
    let two = store.attachment_uploads(10).unwrap();
    assert_eq!(two.len(), 2);
    assert!(two[0].size <= two[1].size, "必须按体积升序，先让小附件见效");
    assert_eq!(two[0].media_type, "image/png");
}

#[test]
fn downloads_only_pick_rows_the_remote_claims_to_have() {
    let fx = Fix::new();
    let store = fx.open();
    let sha = notera_core::ContentHash::of(b"ghost").as_str().replace("sha256:", "");
    assert!(store.attachment_downloads(5).unwrap().is_empty(), "没有登记就不该有活");
    store.register_remote_attachment(&sha, 128, "image/png").unwrap();
    let jobs: Vec<AttachmentJob> = store.attachment_downloads(5).unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].sha256, sha);
    assert_eq!(store.blob_path(&sha).exists(), false, "登记只写元数据，绝不造空 blob");
    // 幂等：重复登记不产生第二行，也不把已下载的态改回 missing
    store.set_attachment_states(&sha, Some("available"), None).unwrap();
    store.register_remote_attachment(&sha, 128, "image/png").unwrap();
    assert!(store.attachment_downloads(5).unwrap().is_empty(), "本地已 available 就不该再下载");
}

#[test]
fn ingest_verifies_sha256_before_touching_the_disk() {
    let fx = Fix::new();
    let store = fx.open();
    let bytes = "真实的图片内容".as_bytes().to_vec();
    let sha = notera_core::ContentHash::of(&bytes).as_str().replace("sha256:", "");
    store.register_remote_attachment(&sha, bytes.len() as i64, "image/png").unwrap();

    let wrong = store.ingest_blob(&sha, "完全不同的字节".as_bytes());
    assert!(matches!(wrong, Err(notera_store::StoreError::Constraint(_))), "哈希不符必须拒收：{wrong:?}");
    assert!(!store.blob_path(&sha).exists(), "拒收的东西不得留在盘上");
    assert_eq!(store.attachment_for_state(&sha).0, "error", "拒收必须留下可见的失败态");

    store.ingest_blob(&sha, &bytes).unwrap();
    assert_eq!(store.blob_path(&sha).exists(), true);
    assert_eq!(std::fs::read(store.blob_path(&sha)).unwrap(), bytes);
    let (local, remote) = store.attachment_for_state(&sha);
    assert_eq!((local.as_str(), remote.as_str()), ("available", "present"));
    assert!(store.attachment_downloads(5).unwrap().is_empty());
}

/// 备份包还原这条路（`restore_blob`）与同步下载那条路的区别就体现在这个测试里。
#[test]
fn restore_from_a_bundle_registers_the_row_and_still_offers_to_upload() {
    let fx = Fix::new();
    let store = fx.open();
    let bytes = b"backup blob bytes".to_vec();
    let sha = notera_core::ContentHash::of(&bytes).as_str().replace("sha256:", "");

    // 干净库里没有这一行：同步那条路的 `ingest_blob` 只会 UPDATE，在这里直接失败
    assert!(store.ingest_blob(&sha, &bytes).is_err(), "没有行的时候 UPDATE 打不到任何一行");
    store.restore_blob(&sha, &bytes).unwrap();

    assert_eq!(std::fs::read(store.blob_path(&sha)).unwrap(), bytes);
    let (local, remote) = store.attachment_for_state(&sha);
    assert_eq!(
        (local.as_str(), remote.as_str()),
        ("available", "unknown"),
        "包里的字节没经过服务器，远端态不能谎报 present"
    );
    let queued: Vec<String> = store.attachment_uploads(5).unwrap().into_iter().map(|j| j.sha256).collect();
    assert_eq!(queued, vec![sha.clone()], "还原出来的附件必须排进上传队列，否则第三台设备拿不到它");

    let tampered = store.restore_blob(&sha, b"other bytes".as_ref());
    assert!(matches!(tampered, Err(notera_store::StoreError::Constraint(_))), "哈希不符必须拒收：{tampered:?}");

    // 已经确认服务器有了，再还原一次不得把它退回 unknown（否则每次还原都全量重传）
    store.set_attachment_states(&sha, None, Some("present")).unwrap();
    store.restore_blob(&sha, &bytes).unwrap();
    assert_eq!(store.attachment_for_state(&sha).1, "present");
    assert!(store.attachment_uploads(5).unwrap().is_empty());
}

#[test]
fn finishing_ops_closes_the_outbox_rows_for_that_blob() {    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);
    let sha = attach(&store, &folder, b"payload", "blk00001");
    let acct = notera_store::LOCAL_ACCOUNT_ID;
    assert!(store.outbox_len(acct, &[notera_store::OpState::Pending]).unwrap() >= 1);
    store.finish_attachment_ops(&sha, true).unwrap();
    let left: Vec<_> = store
        .outbox_take(acct, 20)
        .unwrap()
        .into_iter()
        .filter(|o| o.kind == notera_core::EntityKind::Attachment)
        .collect();
    assert!(left.is_empty(), "附件行必须被关掉，否则 outbox_pending 永远虚高：{left:?}");
}
