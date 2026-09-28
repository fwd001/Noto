//! §13 附件队列的存储侧契约：谁该传、谁该下、下载回来的字节凭什么算数。
mod common;

use common::*;
use notera_store::{AttachmentJob, Store};

fn attach(store: &Store, folder: &notera_core::EntityId, bytes: &[u8], block: &str) -> String {
    let note = store.create_note(folder, doc_text("带附件")).unwrap();
    store
        .attach_blob(&note.id, bytes, "image/png", Some("a.png"), block)
        .unwrap()
        .sha256
}

#[test]
fn uploads_are_selected_by_state_and_sorted_by_size() {
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);
    let big = attach(&store, &folder, &vec![7u8; 4096], "blkbig01");
    let small = attach(&store, &folder, &[9u8; 16], "blksml01");
    store
        .set_attachment_states(&big, None, Some("present"))
        .unwrap();

    let jobs = store.attachment_uploads(10).unwrap();
    assert_eq!(
        jobs.iter().map(|j| j.sha256.as_str()).collect::<Vec<_>>(),
        vec![small.as_str()],
        "远端已 present 的不得再排进上传队列"
    );
    let both = store.attachment_uploads(10).unwrap();
    assert_eq!(both.len(), 1);
    store
        .set_attachment_states(&big, None, Some("unknown"))
        .unwrap();
    let two = store.attachment_uploads(10).unwrap();
    assert_eq!(two.len(), 2);
    assert!(two[0].size <= two[1].size, "必须按体积升序，先让小附件见效");
    assert_eq!(two[0].media_type, "image/png");
}

#[test]
fn downloads_only_pick_rows_the_remote_claims_to_have() {
    let fx = Fix::new();
    let store = fx.open();
    let sha = notera_core::ContentHash::of(b"ghost")
        .as_str()
        .replace("sha256:", "");
    assert!(
        store.attachment_downloads(5).unwrap().is_empty(),
        "没有登记就不该有活"
    );
    store
        .register_remote_attachment(&sha, 128, "image/png")
        .unwrap();
    let jobs: Vec<AttachmentJob> = store.attachment_downloads(5).unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].sha256, sha);
    assert!(
        !store.blob_path(&sha).exists(),
        "登记只写元数据，绝不造空 blob"
    );
    // 幂等：重复登记不产生第二行，也不把已下载的态改回 missing
    store
        .set_attachment_states(&sha, Some("available"), None)
        .unwrap();
    store
        .register_remote_attachment(&sha, 128, "image/png")
        .unwrap();
    assert!(
        store.attachment_downloads(5).unwrap().is_empty(),
        "本地已 available 就不该再下载"
    );
}

#[test]
fn ingest_verifies_sha256_before_touching_the_disk() {
    let fx = Fix::new();
    let store = fx.open();
    let bytes = "真实的图片内容".as_bytes().to_vec();
    let sha = notera_core::ContentHash::of(&bytes)
        .as_str()
        .replace("sha256:", "");
    store
        .register_remote_attachment(&sha, bytes.len() as i64, "image/png")
        .unwrap();

    let wrong = store.ingest_blob(&sha, "完全不同的字节".as_bytes());
    assert!(
        matches!(wrong, Err(notera_store::StoreError::Constraint(_))),
        "哈希不符必须拒收：{wrong:?}"
    );
    assert!(!store.blob_path(&sha).exists(), "拒收的东西不得留在盘上");
    assert_eq!(
        store.attachment_for_state(&sha).0,
        "error",
        "拒收必须留下可见的失败态"
    );

    store.ingest_blob(&sha, &bytes).unwrap();
    assert!(store.blob_path(&sha).exists());
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
    let sha = notera_core::ContentHash::of(&bytes)
        .as_str()
        .replace("sha256:", "");

    // 干净库里没有这一行：同步那条路的 `ingest_blob` 只会 UPDATE，在这里直接失败
    assert!(
        store.ingest_blob(&sha, &bytes).is_err(),
        "没有行的时候 UPDATE 打不到任何一行"
    );
    store.restore_blob(&sha, &bytes).unwrap();

    assert_eq!(std::fs::read(store.blob_path(&sha)).unwrap(), bytes);
    let (local, remote) = store.attachment_for_state(&sha);
    assert_eq!(
        (local.as_str(), remote.as_str()),
        ("available", "unknown"),
        "包里的字节没经过服务器，远端态不能谎报 present"
    );
    let queued: Vec<String> = store
        .attachment_uploads(5)
        .unwrap()
        .into_iter()
        .map(|j| j.sha256)
        .collect();
    assert_eq!(
        queued,
        vec![sha.clone()],
        "还原出来的附件必须排进上传队列，否则第三台设备拿不到它"
    );

    let tampered = store.restore_blob(&sha, b"other bytes".as_ref());
    assert!(
        matches!(tampered, Err(notera_store::StoreError::Constraint(_))),
        "哈希不符必须拒收：{tampered:?}"
    );

    // 已经确认服务器有了，再还原一次不得把它退回 unknown（否则每次还原都全量重传）
    store
        .set_attachment_states(&sha, None, Some("present"))
        .unwrap();
    store.restore_blob(&sha, &bytes).unwrap();
    assert_eq!(store.attachment_for_state(&sha).1, "present");
    assert!(store.attachment_uploads(5).unwrap().is_empty());
}

/// §27 磁盘体检的批量降级（`Store::set_attachments_locally_missing`）。
///
/// 一轮体检要把"账上 `available` 而盘上没有"的一批行交出去，**必须一次写事务做完**：
/// 逐行开事务的代价记在 §48 的 G4 —— 整个 `attachments/` 目录被删（换盘没搬完、杀毒整目录
/// 隔离）时，一轮里 N 次提交会把写锁占住好几秒，而用户那边的保存正排在这把锁后面。
///
/// 这条测试盯的是"批量"最容易悄悄做错的三件事：
/// * 列出来的**全部**降到 `missing`，而 `remote_state` 一字不动 —— 体检只证明本机没有，
///   远端有没有是另一回事（顺手改成 absent 就等于把这张图判死，永远不去问了）；
/// * **没列出来的不许顺手动到** —— 批量语句写成 `WHERE local_state='available'` 这种
///   "更省事"的形式会把好文件一起降级；
/// * 传进来的 sha 库里没有、或本来就已经不是 `available` 时**不许算进成功条数** ——
///   调用方（host 体检）就靠这个差值吵一声，静默吞掉就是 §39 禁的那种 fallback。
#[test]
fn a_batched_demotion_moves_exactly_the_listed_rows() {
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);
    let a = attach(&store, &folder, b"aa", "blk000001");
    let b = attach(&store, &folder, b"bb", "blk000002");
    let kept = attach(&store, &folder, b"cc", "blk000003");
    for sha in [&a, &b, &kept] {
        store
            .set_attachment_states(sha, Some("available"), Some("present"))
            .unwrap();
    }

    let ghost = notera_core::ContentHash::of("库里没有这一行".as_bytes())
        .as_str()
        .replace("sha256:", "");
    let changed = store
        .set_attachments_locally_missing(&[a.clone(), b.clone(), ghost.clone()])
        .expect("批量降级");
    assert_eq!(changed, 2, "库里没有的那条不许算成已降级：{changed}");
    for sha in [&a, &b] {
        assert_eq!(
            store.attachment_for_state(sha),
            ("missing".into(), "present".into()),
            "列出来的行必须降级，而远端态不许被顺手改"
        );
    }
    assert_eq!(
        store.attachment_for_state(&kept),
        ("available".into(), "present".into()),
        "没列出来的那行被批量语句顺手动到了"
    );
    // 降级出来的行必须**立刻**是下载队列的活，否则"降级"只是把状态改了一下而没接线
    let queued: Vec<String> = store
        .attachment_downloads(10)
        .unwrap()
        .into_iter()
        .map(|j| j.sha256)
        .collect();
    assert_eq!(queued.len(), 2, "降完两级必须马上排进下载队列：{queued:?}");

    // 幂等：同一批再降一次不该再算成功（host 那声 warn 的判据就是这个数）
    assert_eq!(
        store
            .set_attachments_locally_missing(&[a.clone(), b.clone()])
            .expect("重复降级"),
        0,
        "已经不是 available 的行被重复计入成功条数"
    );
}

/// 磁盘体检的**尺寸回填**也必须走批量那一条（`Store::set_attachment_sizes`）。
///
/// 这一格是 G4 的另一半：当年只把"降级"批量了，回填登记尺寸还是每条一次写事务。
/// 2026-09-28 量出来这不是理论账 —— 一轮慢路 ~320 ms，其中读 1 KiB×200 与读 64 KiB×100
/// （IO 差 30 倍）耗时一样，而夹具里同样条数的写提交是 100 次 106 ms / 1000 次 1151 ms
/// （≈1.1 ms/次）：那一轮的钱花在提交上，不在哈希上。刚导入 / 刚升级过的库可以有成百
/// 上千条声明尺寸是偏的，逐行提交就是成百上千次排队占住写锁。
///
/// 盯的是批量回填最容易悄悄做错的几件事：
/// * 点名的行**全部**改成给定的长度，没点名的那行一个字不动（语句漏掉 `WHERE sha256` 就是全表）；
/// * `size` 允许**往小**改 —— 这是唯一能纠掉"偏大的声明值"的一路（登记那条规则是 `MAX(旧,新)`，
///   只许涨不许落），写成 `MAX` 就白回填了，那一行此后每轮都要整份重读重哈希；
/// * 只改 `size` 那一列：两半状态与 `deleted_at` 不许被带跑（回填只证明"实测长度是这个数"，
///   不证明本机有没有、远端有没有）；
/// * 库里没有的那条**不计入成功数**，host 体检就靠这个差值吵一声；空输入是 `Ok(0)` 而不是错。
#[test]
fn a_batched_size_backfill_moves_exactly_the_listed_rows() {
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);
    let a = attach(&store, &folder, &[7u8; 9000], "blk000001");
    let b = attach(&store, &folder, &[9u8; 4000], "blk000002");
    let kept = attach(&store, &folder, &[3u8; 5000], "blk000003");
    for sha in [&a, &b, &kept] {
        store
            .set_attachment_states(sha, Some("available"), Some("present"))
            .unwrap();
    }
    // 前置：把两条的登记尺寸写成"偏大"，模拟正文里客户端各自声明的那个值
    store
        .set_attachment_sizes(&[(a.clone(), 32_000), (b.clone(), 32_000)])
        .expect("造现场");
    let size_of = |sha: &str| -> i64 {
        store
            .attachment_repair_candidates(50)
            .unwrap()
            .into_iter()
            .find(|(s, _)| s == sha)
            .map(|(_, sz)| sz)
            .expect("这一行该在体检候选里")
    };
    assert_eq!(
        size_of(&a),
        32_000,
        "前置：现场没造出来（登记尺寸没被写成偏大）"
    );

    let ghost = notera_core::ContentHash::of("库里没有这一行".as_bytes())
        .as_str()
        .replace("sha256:", "");
    let done = store
        .set_attachment_sizes(&[(a.clone(), 9000), (b.clone(), 4000), (ghost.clone(), 1234)])
        .expect("批量回填");
    assert_eq!(done, 2, "库里没有的那条不许算成已回填：{done}");
    assert_eq!(
        size_of(&a),
        9000,
        "偏大的登记尺寸没被纠掉：那一行此后每轮都要整份重读重哈希"
    );
    assert_eq!(size_of(&b), 4000, "偏大的登记尺寸没被纠掉");
    assert_eq!(
        size_of(&kept),
        5000,
        "没点名的那行被批量语句顺手动到了（漏掉 WHERE sha256 就是这个形状）"
    );

    // 只改 size：状态与远端态一个字都不动
    for sha in [&a, &b, &kept] {
        assert_eq!(
            store.attachment_for_state(sha),
            ("available".into(), "present".into()),
            "回填尺寸把两半状态之一带跑了：{sha}"
        );
    }
    // 空输入是"无事可做"，不是错（host 每轮都可能攒出零条）
    assert_eq!(store.set_attachment_sizes(&[]).expect("空批量"), 0);
}

#[test]
fn finishing_ops_closes_the_outbox_rows_for_that_blob() {
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);
    let sha = attach(&store, &folder, b"payload", "blk00001");
    let acct = notera_store::LOCAL_ACCOUNT_ID;
    assert!(
        store
            .outbox_len(acct, &[notera_store::OpState::Pending])
            .unwrap()
            >= 1
    );
    store.finish_attachment_ops(&sha, true).unwrap();
    let left: Vec<_> = store
        .outbox_take(acct, 20)
        .unwrap()
        .into_iter()
        .filter(|o| o.kind == notera_core::EntityKind::Attachment)
        .collect();
    assert!(
        left.is_empty(),
        "附件行必须被关掉，否则 outbox_pending 永远虚高：{left:?}"
    );
}

/// GC 结掉的附件待办**必须也含 `failed`**（独立审查抓出来的第 4 条）。
///
/// 为什么 `failed` 是一格的洞：`outbox_pending` 的口径是 `('pending','inflight','failed')`
/// （`Store::stats` 与 §18 那一处都这么写），而被隔离的行已经离开两个队列的取活范围 ——
/// 那一行既不在下载也不在上传的候选里，于是这条 `failed` 永远不会有人再碰它：界面写着
/// "还有 N 项待发"，而核心里一行可做活的都没有。§18 要的是这个数**诚实**，不是"差不多"。
/// 方向也要守住：只结掉本轮真被隔离的那些 sha 的待办，仍被引用的行那条 `failed` 归退避逻辑管，
/// 一条都不许顺手动（否则 GC 就成了吞掉同步失败的那只手）。
#[test]
fn the_quarantine_settles_failed_attachment_ops_too() {
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);
    let note = store
        .create_note(&folder, doc_text("马上永久删除的那条"))
        .unwrap();
    let sha = store
        .attach_blob(
            &note.id,
            b"orphan-bytes",
            "image/png",
            Some("o.png"),
            "blk0000c1",
        )
        .unwrap()
        .sha256;
    store
        .set_attachment_states(&sha, Some("available"), Some("present"))
        .unwrap();
    store.purge_note(&note.id).unwrap();
    // 另一条仍然有人引用的行，也给它一支 failed 待办 —— 它是"不许顺手动别人"的那半边判据
    let kept = attach(&store, &folder, b"kept-bytes", "blk0000c2");
    store
        .set_attachment_states(&kept, Some("available"), Some("present"))
        .unwrap();

    // 两支都标成 failed。**只取一次**：`outbox_take` 会把取到的行写成 inflight，
    // 分两次取的话第二次什么都取不到（第一版就这么假红过一次，报的是"前置缺待办"）。
    let taken = store
        .outbox_take(notera_store::LOCAL_ACCOUNT_ID, 50)
        .expect("取待办");
    for sha in [&sha, &kept] {
        let op = taken
            .iter()
            .find(|o| o.kind == notera_core::EntityKind::Attachment && o.entity_key == *sha)
            .unwrap_or_else(|| panic!("前置：{sha} 这一支该有一条附件待办"));
        store
            .outbox_state(op.id, notera_store::OpState::Failed, Some("上传失败"), None)
            .expect("标成 failed");
    }
    assert_eq!(
        store
            .outbox_len(
                notera_store::LOCAL_ACCOUNT_ID,
                &[notera_store::OpState::Failed]
            )
            .unwrap(),
        2,
        "前置：两行各自挂着一支 failed"
    );

    let changed = store
        .mark_attachments_quarantined(std::slice::from_ref(&sha))
        .expect("隔离");
    assert_eq!(changed, 1, "只该认领零引用的那一条");

    let left = store
        .outbox_len(
            notera_store::LOCAL_ACCOUNT_ID,
            &[notera_store::OpState::Failed],
        )
        .unwrap();
    assert_eq!(
        left, 1,
        "被隔离那行的 failed 待办必须一起结掉，否则「待发操作」永久虚高：还剩 {left} 条"
    );
    // 结掉的是这一条 sha，不是别人的：仍被引用的行那条 failed 要原样留着等退避重试
    let who: Vec<String> = store
        .outbox_take(notera_store::LOCAL_ACCOUNT_ID, 50)
        .unwrap()
        .into_iter()
        .filter(|o| o.kind == notera_core::EntityKind::Attachment)
        .map(|o| o.entity_key)
        .collect();
    assert_eq!(who, vec![kept], "只许动本轮真被隔离的那些 sha 的待办");
}

/// GC 的账上那一步（§8）：**一次写事务**里同时做完三件事 —— 只认领"仍然零引用"的行、
/// 把它们写成 `missing ∧ deleted_at`、关掉它们还挂着的附件待办。
///
/// 为什么要在这一层再挡一次引用：候选集是上一句 SQL 查出来的，而"用户在这一瞬把笔记
/// 还原了 / 另一台设备的记录刚把同一份字节引用进来"就落在那两条语句之间。判据写在
/// `UPDATE ... WHERE` 里，检查与写入就是同一个事务，不需要外面再加一把锁。
#[test]
fn the_quarantine_mark_only_takes_rows_that_are_still_unreferenced() {
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);
    let live = attach(&store, &folder, b"live-bytes", "blk0000a1");
    let orphan_note = store
        .create_note(&folder, doc_text("要永久删除的那条"))
        .unwrap();
    let orphan = store
        .attach_blob(
            &orphan_note.id,
            b"orphan-bytes",
            "image/png",
            None,
            "blk0000a2",
        )
        .unwrap()
        .sha256;
    for sha in [&live, &orphan] {
        store
            .set_attachment_states(sha, Some("available"), Some("present"))
            .unwrap();
    }

    // 先证明候选集这条 SQL 本身问的是引用，而不是别的什么：两条都 available ∧ present，
    // 唯一差别就是有没有笔记还指着它。
    store.purge_note(&orphan_note.id).unwrap();
    let candidates = store.gc_quarantine_candidates(50).unwrap();
    assert_eq!(
        candidates,
        vec![orphan.clone()],
        "候选集必须只包含零引用的那一条"
    );
    assert_eq!(store.attachment_refs(&live).unwrap(), 1, "另一条仍被引用");

    // 引用在这一瞬回来了：把 `live` 也塞进点名清单，它一条都不许被改。
    let ghost = notera_core::ContentHash::of("库里没有这一行".as_bytes())
        .as_str()
        .replace("sha256:", "");
    let changed = store
        .mark_attachments_quarantined(&[live.clone(), orphan.clone(), ghost.clone()])
        .expect("批量隔离");
    assert_eq!(
        changed, 1,
        "有引用的那条与库里没有的那条都不许算成已隔离：{changed}"
    );
    assert_eq!(
        store.attachment_for_state(&live),
        ("available".into(), "present".into()),
        "仍然被引用的行被批量语句顺手动到了 = 隔离判据根本没问引用"
    );
    let (local, deleted) = store
        .attachment_quarantine_state(&orphan)
        .expect("这一行该还在账上");
    assert_eq!(local, "missing", "隔离要把本机态写成本来没有");
    assert!(
        deleted.is_some(),
        "宽限期靠 `deleted_at` 起算，没落下去就等于没隔离"
    );
    assert_eq!(
        store.attachment_for_state(&orphan).1,
        "present",
        "远端态一字不动：隔离只关于本机，改成 absent 等于把服务器上有副本这件事判死"
    );

    // 附件待办要一起关掉：这一行已经不在两个队列的取活范围里，留着 pending 就是
    // `outbox_pending` 永远虚高（§18 要求这个数诚实）。
    let left: Vec<_> = store
        .outbox_take(notera_store::LOCAL_ACCOUNT_ID, 50)
        .unwrap()
        .into_iter()
        .filter(|o| o.kind == notera_core::EntityKind::Attachment && o.entity_key == orphan)
        .collect();
    assert!(left.is_empty(), "隔离之后还挂着附件待办：{left:?}");

    // 幂等：已隔离的行再点一次不该重复计入成功条数（host 那声 warn 的判据就是这个数）。
    assert_eq!(
        store
            .mark_attachments_quarantined(std::slice::from_ref(&orphan))
            .expect("重复隔离"),
        0
    );
}

/// 真删那一步的守卫：销毁清单由 SQL 判引用，`attachments` 行删不掉时**一个字节都不许动**。
///
/// `note_attachments.sha256` 上是 `ON DELETE RESTRICT`，所以就算判据哪天写错了，SQLite 也会
/// 把整笔事务顶回来；但那条 FK 只是兜底 —— 这一层先自己问过引用，才轮得到它。
#[test]
fn a_purge_refuses_rows_that_are_still_referenced_or_absent() {
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);
    let live = attach(&store, &folder, b"still-used", "blk0000b1");
    store
        .set_attachment_states(&live, Some("available"), Some("present"))
        .unwrap();

    let ghost = notera_core::ContentHash::of("没这一行".as_bytes())
        .as_str()
        .replace("sha256:", "");
    let purged = store
        .purge_attachment_rows(&[live.clone(), ghost.clone()])
        .expect("销毁清单");
    assert!(
        purged.is_empty(),
        "有引用的与库里没有的都不许销毁：{purged:?}"
    );
    assert_eq!(
        store.attachment_for_state(&live).0,
        "available",
        "被引用那一行的账被销毁掉了"
    );

    // 零引用 + 已隔离 + 过了宽限期 → 才进清单，也才真删得掉。
    let gone_note = store.create_note(&folder, doc_text("用完就删")).unwrap();
    let gone = store
        .attach_blob(&gone_note.id, b"orphan", "image/png", None, "blk0000b2")
        .unwrap()
        .sha256;
    store.purge_note(&gone_note.id).unwrap();
    store
        .set_attachment_states(&gone, Some("available"), Some("present"))
        .unwrap();
    assert_eq!(
        store
            .mark_attachments_quarantined(std::slice::from_ref(&gone))
            .unwrap(),
        1
    );
    let ready = store
        .gc_ready_to_purge("9999-01-01T00:00:00.000Z", 50)
        .unwrap();
    assert_eq!(
        ready,
        vec![gone.clone()],
        "过了宽限期的零引用行要进销毁清单"
    );
    assert!(
        store
            .gc_ready_to_purge("0000-01-01T00:00:00.000Z", 50)
            .unwrap()
            .is_empty(),
        "宽限期内的行也进了清单 = 那个 cutoff 参数根本没参与判定"
    );
    assert_eq!(
        store
            .purge_attachment_rows(std::slice::from_ref(&gone))
            .unwrap(),
        vec![gone.clone()],
    );
    assert_eq!(
        store.attachment_for_state(&gone),
        ("absent".into(), "absent".into()),
        "销毁后账上要彻底没有这一行"
    );
}

/// 一支 `failed` 的上传待办，在**后来这一份真的传上去了**之后必须被结掉。
///
/// 为什么这条与 FT-ATT-35 不重复：那一条管"GC 认领了一行"这种**这行不再有任何活可干**的情形；
/// 这一条管笔记仍被引用、活也确实干完了的情形。两边的共同点是 `outbox_pending` 的口径含
/// `failed`（`Store::stats` 与 §18 那两处 SQL），而**没有任何后台消费者会再去碰一支 failed 的
/// 附件待办** —— 文本引擎只按 `local_views` 规划、附件轮只按 `attachments` 的状态挑活，
/// `outbox_take` 在生产里没有调用方。所以"留着它总会自己掉下去"是不成立的。
///
/// 方向同样要守住另一半：**还没传上去的 failed 不许被结掉** —— 那是用户真该看到的一条
/// "这一项还没同步上去"（§27 的收手形状：404 / 内容不符时后台不再重试，界面上靠
/// 「重新上传本机这份」接手）。把它悄悄写成的，就不是诚实的计数了。
#[test]
fn a_failed_upload_is_closed_once_a_later_round_proves_it_uploaded() {
    let fx = Fix::new();
    let store = fx.open();
    let folder = default_folder(&store);
    let sha = attach(&store, &folder, b"retry-bytes", "blk0000d1");

    // 第一轮上传失败：走公开接口（取出待办 → 回填失败），不手改表
    store
        .outbox_take(notera_store::LOCAL_ACCOUNT_ID, 50)
        .expect("取待办");
    store.finish_attachment_ops(&sha, false).expect("标成失败");
    assert_eq!(
        store
            .outbox_len(
                notera_store::LOCAL_ACCOUNT_ID,
                &[notera_store::OpState::Failed]
            )
            .unwrap(),
        1,
        "前置：这一支确实是 failed"
    );

    // ① 还没传上去：这条要留着（用户看得见"这一项没同步上去"是对的）
    assert_eq!(
        store.settle_satisfied_attachment_ops().unwrap(),
        0,
        "远端还没有这份就结掉 failed = 把用户真实的失败藏起来"
    );

    // ② 后来这一份到了服务器上（另一轮成功，或对面设备传上去后清单确认了它）
    store
        .set_attachment_states(&sha, Some("available"), Some("present"))
        .unwrap();
    assert_eq!(
        store.settle_satisfied_attachment_ops().unwrap(),
        1,
        "已经传上去了的那支 failed 必须被结掉：outbox_pending 含 failed，留着就是永久虚高"
    );
    assert_eq!(
        store
            .outbox_len(
                notera_store::LOCAL_ACCOUNT_ID,
                &[notera_store::OpState::Failed]
            )
            .unwrap(),
        0,
        "结掉之后不能再挂着：这一项已经同步完了"
    );
}
