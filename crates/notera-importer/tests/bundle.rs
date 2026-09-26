//! 导出包的读写契约（DATA-MODEL §15）。
//!
//! 沿用 `importer.rs` 的做法：不引 `tempfile`（那会动 Cargo.lock），
//! 用 `std::env::temp_dir()` + 唯一目录 + Drop 清理。
use notera_core::EntityId;
use notera_importer::{read_bundle, write_bundle, Bundle, Manifest, BUNDLE_FORMAT};
use serde_json::json;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

static SEQ: AtomicUsize = AtomicUsize::new(0);

struct Tmp(PathBuf);

impl Tmp {
    fn new() -> Self {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("notera-bundle-{}-{n}", std::process::id()));
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

fn manifest() -> Manifest {
    Manifest {
        format: BUNDLE_FORMAT,
        protocol: 1,
        exported_at: "2026-09-26T00:00:00.000Z".into(),
        app_version: "0.1.0".into(),
        root_id: None,
        counts: BTreeMap::new(),
        partial: false,
    }
}

fn envelope(kind: &str, text: &str) -> serde_json::Value {
    json!({
        "protocol": 1,
        "kind": kind,
        "id": EntityId::new().as_str(),
        "rev": 1,
        "hash": format!("sha256:{text}"),
        "purged": false,
        "enc": { "alg": "none" },
        "payload": { "v": 1, "content": [{ "id": "blk000001", "type": "paragraph", "content": [{ "text": text }] }] }
    })
}

#[test]
fn bundle_round_trips_every_entry_kind() {
    let tmp = Tmp::new();
    let path = tmp.path().join("out.zip");
    let body = b"attachment bytes".to_vec();
    let sha = notera_crypto::sha256_hex(&body);
    let note = envelope("note", "回来以后还在");
    let folder = envelope("folder", "默认本");
    let wanted = Bundle {
        manifest: Some(manifest()),
        folders: vec![folder],
        notes: vec![note.clone()],
        tombstones: vec![],
        attachments: vec![(sha.clone(), body.clone())],
    };
    write_bundle(&path, &wanted).unwrap();

    let got = read_bundle(&path).unwrap();
    assert_eq!(got.folders.len(), 1);
    assert_eq!(got.notes, vec![note], "笔记信封必须逐字回来");
    assert_eq!(got.attachments, vec![(sha, body)]);
    assert_eq!(got.manifest.as_ref().unwrap().format, BUNDLE_FORMAT);
}

#[test]
fn a_bundle_written_before_the_partial_flag_still_reads_back_as_a_full_library() {
    // 上一版导出的包里没有 `partial` 这个键。读不回来就等于"我们自己的升级让用户备份失效"，
    // 那是数据丢失，不是兼容性问题 —— 所以这一条必须钉住 serde(default) 那行。
    let tmp = Tmp::new();
    let path = tmp.path().join("old.zip");
    let body = serde_json::to_vec(&json!({
        "format": BUNDLE_FORMAT, "protocol": 1, "exported_at": "2026-09-01T00:00:00.000Z",
        "app_version": "0.1.0", "root_id": null, "counts": {}
    }))
    .unwrap();
    let file = std::fs::File::create(&path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
    zip.start_file("manifest.json", opts).unwrap();
    std::io::Write::write_all(&mut zip, &body).unwrap();
    zip.start_file("folders.json", opts).unwrap();
    std::io::Write::write_all(&mut zip, b"[]").unwrap();
    zip.start_file("tombstones.json", opts).unwrap();
    std::io::Write::write_all(&mut zip, b"[]").unwrap();
    zip.finish().unwrap();

    let got = read_bundle(&path).expect("老包必须还能读");
    assert!(!got.manifest.unwrap().partial, "缺 partial = 整库，不是子树");
}

#[test]
fn a_real_zip_tool_can_locate_and_read_each_note() {
    // "自描述 ZIP" 的全部意义：别人不必用我们的程序也能把数据取回去。
    // 所以这里绕开 read_bundle，只用 zip 读取器按名字取一个条目。
    let tmp = Tmp::new();
    let path = tmp.path().join("out.zip");
    let id = EntityId::new();
    let mut note = envelope("note", "可移植");
    note["id"] = json!(id.as_str());
    write_bundle(
        &path,
        &Bundle {
            manifest: Some(manifest()),
            folders: vec![envelope("folder", "默认本")],
            notes: vec![note],
            ..Default::default()
        },
    )
    .unwrap();

    let mut zip = zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
    let want = format!("notes/{}.json", id.as_str());
    let names = (0..zip.len()).map(|i| zip.by_index(i).unwrap().name().to_string()).collect::<Vec<_>>();
    assert!(names.contains(&want), "每条笔记一个以 id 命名的文件，实际条目 {names:?}");
    assert!(names.contains(&"manifest.json".to_string()));

    let mut entry = zip.by_name(&want).unwrap();
    let mut body = String::new();
    std::io::Read::read_to_string(&mut entry, &mut body).unwrap();
    assert!(body.contains("可移植"), "取回来的必须是能看懂的 JSON 正文");
    assert!(body.contains("\"protocol\": 1"), "信封要带协议号，接收方才知道怎么解释");
}

#[test]
fn tampered_attachment_content_is_rejected_wholesale() {
    // 附件按内容寻址：文件名说 sha=X 而字节算出来不是 X，整包都不该被信。
    let tmp = Tmp::new();
    let path = tmp.path().join("out.zip");
    let fake_sha = notera_crypto::sha256_hex(b"something else");
    write_bundle(
        &path,
        &Bundle {
            manifest: Some(manifest()),
            attachments: vec![(fake_sha, b"actual bytes".to_vec())],
            ..Default::default()
        },
    )
    .unwrap();
    let err = read_bundle(&path).expect_err("内容对不上文件名必须报错");
    assert!(format!("{err}").contains("与文件名不符"), "错误要指出来龙去脉：{err}");
}

#[test]
fn unknown_format_or_protocol_is_refused_before_any_write() {
    let tmp = Tmp::new();
    let path = tmp.path().join("future.zip");
    let mut m = manifest();
    m.format = BUNDLE_FORMAT + 7;
    write_bundle(&path, &Bundle { manifest: Some(m), ..Default::default() }).unwrap();
    let err = read_bundle(&path).expect_err("不认识的格式号不能硬读");
    assert!(format!("{err}").contains("格式号"), "{err}");

    let path2 = tmp.path().join("proto.zip");
    let mut m2 = manifest();
    m2.protocol = 99;
    write_bundle(&path2, &Bundle { manifest: Some(m2), ..Default::default() }).unwrap();
    let err2 = read_bundle(&path2).expect_err("高于本程序的协议号必须拒绝");
    assert!(format!("{err2}").contains("协议号"), "{err2}");
}

#[test]
fn missing_manifest_is_not_treated_as_an_empty_bundle() {
    let tmp = Tmp::new();
    let path = tmp.path().join("bare.zip");
    write_bundle(&path, &Bundle::default()).unwrap(); // 默认会补一个 manifest
    assert!(read_bundle(&path).is_ok(), "空包也该能读，只是什么都不带");

    let no_manifest = tmp.path().join("silent.zip");
    let file = std::fs::File::create(&no_manifest).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
    zip.start_file("folders.json", opts).unwrap();
    std::io::Write::write_all(&mut zip, b"[]").unwrap();
    zip.finish().unwrap();
    let err = read_bundle(&no_manifest).expect_err("没有 manifest 就无法判断这是什么包");
    assert!(format!("{err}").contains("manifest"), "{err}");
}
