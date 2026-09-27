//! DATA-MODEL §15 的验收：创建 → 导出 → （换一台机器）导入 → 内容哈希逐条一致，
//! 且删除的事实不会被"导入"洗掉。
use notera_core::EntityId;
use notera_host::commands::{ExportCmd, ImportCmd};
use notera_host::App;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

static SEQ: AtomicUsize = AtomicUsize::new(0);

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("notera-xchg-{tag}-{}-{n}", std::process::id()));
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

fn doc(text: &str) -> serde_json::Value {
    json!({ "v": 1, "content": [{ "id": "blk000001", "type": "paragraph", "content": [{ "text": text }] }] })
}

fn eid(s: &str) -> EntityId {
    EntityId::parse(s).expect("命令层给出的 id 一定是合法的")
}

/// 活笔记的 id 集合（`list_notes` 默认不含回收站）。
fn live_ids(app: &App) -> Vec<EntityId> {
    app.store()
        .list_notes(&notera_store::NoteQuery::all())
        .unwrap()
        .into_iter()
        .map(|r| r.id)
        .collect()
}

fn export(app: &App, to: &Path, trash: bool) -> serde_json::Value {
    app.export_data(ExportCmd {
        folder_ids: vec![],
        include_attachments: false,
        include_trash: trash,
        path: Some(to.to_string_lossy().to_string()),
    })
    .unwrap()
}

fn import(
    app: &App,
    from: &Path,
    mode: &str,
) -> Result<serde_json::Value, notera_host::commands::CmdError> {
    app.import_data(ImportCmd {
        path: Some(from.to_string_lossy().to_string()),
        mode: Some(mode.to_string()),
    })
}

#[test]
fn export_then_import_into_another_library_preserves_every_hash() {
    let src = Tmp::new("src");
    let dst = Tmp::new("dst");
    let a = App::boot(src.path()).unwrap();
    let folder = a.default_folder_id().unwrap();
    let one = a.create_note(&folder, doc("第一条正文")).unwrap();
    let two = a.create_note(&folder, doc("第二条正文")).unwrap();

    let bundle = src.path().join("exchange.zip");
    let report = export(&a, &bundle, true);
    assert_eq!(
        report["counts"]["notes"].as_u64(),
        Some(2),
        "两条笔记都该在包里：{report}"
    );
    assert!(bundle.is_file());

    let b = App::boot(dst.path()).unwrap();
    let out = import(&b, &bundle, "merge").unwrap();
    assert!(
        out["merged"].as_u64().unwrap_or(0) >= 2,
        "导入报告要说清并入了几条：{out}"
    );

    let ids = live_ids(&b);
    assert!(
        ids.contains(&eid(&one.id)) && ids.contains(&eid(&two.id)),
        "id 必须原样保留，不能变成新副本"
    );
    for (dto, want) in [(&one, "第一条正文"), (&two, "第二条正文")] {
        let got = b
            .store()
            .get_note(&eid(&dto.id))
            .unwrap()
            .expect("导入后应能读到");
        assert_eq!(
            got.content_hash, dto.content_hash,
            "内容哈希逐条一致是 §15 的硬验收"
        );
        assert_eq!(got.title, want);
    }
}

#[test]
fn a_deleted_note_stays_deleted_after_export_import_round_trip() {
    // "导出→清空→导入"复活是主需求点名禁止的一类事故。
    let src = Tmp::new("dsrc");
    let dst = Tmp::new("ddst");
    let a = App::boot(src.path()).unwrap();
    let folder = a.default_folder_id().unwrap();
    let kept = a.create_note(&folder, doc("留下")).unwrap();
    let gone = a.create_note(&folder, doc("删掉")).unwrap();
    a.store().delete_note(&eid(&gone.id)).unwrap();

    let bundle = src.path().join("d.zip");
    export(&a, &bundle, true);

    let b = App::boot(dst.path()).unwrap();
    import(&b, &bundle, "merge").unwrap();

    assert!(live_ids(&b).contains(&eid(&kept.id)), "没删的那条要回来");
    assert!(
        !live_ids(&b).contains(&eid(&gone.id)),
        "删掉的那条不该出现在活笔记列表里"
    );
    match b.store().get_note(&eid(&gone.id)).unwrap() {
        None => {}
        Some(n) => assert!(
            n.deleted_at.is_some(),
            "删除事实必须一起带回来，不能变成活笔记：rev={:?}",
            n.rev
        ),
    }
    assert_eq!(
        b.store().stats().unwrap().notes,
        1,
        "活笔记计数只能是 1，多出来就是复活了"
    );
}

#[test]
fn importing_twice_is_a_no_op_rather_than_a_duplicate() {
    let src = Tmp::new("tsrc");
    let dst = Tmp::new("tdst");
    let a = App::boot(src.path()).unwrap();
    let folder = a.default_folder_id().unwrap();
    let n = a.create_note(&folder, doc("只写一次")).unwrap();
    let bundle = src.path().join("t.zip");
    export(&a, &bundle, true);

    let b = App::boot(dst.path()).unwrap();
    import(&b, &bundle, "merge").unwrap();
    import(&b, &bundle, "merge").unwrap();
    assert_eq!(
        live_ids(&b).iter().filter(|id| **id == eid(&n.id)).count(),
        1,
        "重复导入不该多出副本"
    );
    assert_eq!(
        b.store()
            .get_note(&eid(&n.id))
            .unwrap()
            .unwrap()
            .content_hash,
        n.content_hash
    );
}

#[test]
fn into_empty_refuses_a_library_that_already_has_notes() {
    let src = Tmp::new("esrc");
    let dst = Tmp::new("edst");
    let a = App::boot(src.path()).unwrap();
    let folder = a.default_folder_id().unwrap();
    a.create_note(&folder, doc("要导出的")).unwrap();
    let bundle = src.path().join("e.zip");
    export(&a, &bundle, true);

    let b = App::boot(dst.path()).unwrap();
    let bfolder = b.default_folder_id().unwrap();
    b.create_note(&bfolder, doc("这边已经有东西了")).unwrap();
    let err = import(&b, &bundle, "intoEmpty").expect_err("intoEmpty 必须拒绝非空库");
    assert!(
        format!("{err:?}").contains("非空"),
        "错误要说明为什么拒绝：{err:?}"
    );
    assert_eq!(live_ids(&b).len(), 1, "被拒绝的导入一条都不该写进去");
}

#[test]
fn exporting_one_folder_carries_only_its_subtree_and_says_so() {
    // 曾经是 `folder_scope_unsupported` 响亮拒绝（没做子树闭包就不能假装做了）。
    // 现在闭包真的做了，这条就钉住"做了什么"：只带子树、报告说自己带、未知 id 仍然拒绝。
    let src = Tmp::new("psrc");
    let a = App::boot(src.path()).unwrap();
    let root = a.default_folder_id().unwrap();
    let keep = a.store().create_folder(Some(&root), "要带走").unwrap();
    let aside = a.store().create_folder(Some(&root), "不带").unwrap();
    let kept = eid(&a.create_note(&keep.id, doc("范围内")).unwrap().id);
    let ignored = eid(&a.create_note(&aside.id, doc("范围外")).unwrap().id);

    let out = src.path().join("子树.zip");
    let got = a
        .export_data(ExportCmd {
            folder_ids: vec![keep.id.to_string()],
            include_attachments: false,
            include_trash: false,
            path: Some(out.to_string_lossy().to_string()),
        })
        .unwrap();
    assert_eq!(
        got["scope"], "folders",
        "报告必须自己说清这是子树包：{got:?}"
    );
    assert_eq!(got["counts"]["notes"], 1, "只带范围内那一篇：{got:?}");

    let b = notera_importer::read_bundle(&out).unwrap();
    assert!(
        b.manifest.unwrap().partial,
        "子树包要带 partial 标记，导入端据此拒绝整库还原"
    );
    assert_eq!(
        b.notes.len(),
        1,
        "包里就只有那一篇：{:?}",
        b.notes.iter().map(|n| n["id"].clone()).collect::<Vec<_>>()
    );
    assert_eq!(
        b.notes[0]["id"],
        json!(kept.to_string()),
        "带回来的必须是范围内那条"
    );
    assert!(
        !b.notes
            .iter()
            .any(|n| n["id"] == json!(ignored.to_string())),
        "平级文件夹的笔记不许混进来：{ignored}"
    );

    let err = a
        .export_data(ExportCmd {
            folder_ids: vec![EntityId::new().to_string()],
            include_attachments: false,
            include_trash: false,
            path: None,
        })
        .expect_err("范围里有个不存在的文件夹，不能当成\"那就不用带\"");
    assert!(
        format!("{err:?}").contains("文件夹不存在"),
        "要给出可机读的拒绝理由：{err:?}"
    );
}
