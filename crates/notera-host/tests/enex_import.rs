//! `.enex` 从**命令面**进来的一条端到端：`import_files` → 导入器 → 真 Store → 磁盘上的字节。
//!
//! 为什么要有这一条而不是只留 importer 里那条：`crates/notera-importer/tests/importer.rs`
//! 证明的是"导入器能干活"，而用户点的是**那个命令名**。两边都绿却没有这条边的历史事故
//! 已经出过两次（压实、Range 探测），所以这里从 `App::import_files` 进去，走真 SQLite，
//! 并且要求附件字节真的躺在数据目录里 —— 只登记不写盘，或者只写盘不建链接，都会红。

use notera_host::App;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

static SEQ: AtomicUsize = AtomicUsize::new(0);

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "notera-enex-cmd-{tag}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("临时目录可建");
        Tmp(dir)
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// 真 .enex 的形状：正文是 CDATA 包起来的 ENML，附件 base64 内嵌，编码在属性上。
fn enex_body(png_b64: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<en-export version=\"6.5.1\">\n\
<note><title>报销单</title><content><![CDATA[<en-note><div>甲</div>\
<en-media hash=\"AA11\" type=\"image/png\" /></en-note>]]></content>\
<created>20230102T030405Z</created><tag>财务</tag>\
<resource><data encoding=\"base64\">{png_b64}</data><mime>image/png</mime>\
<resource-attributes><file-name>shot.png</file-name></resource-attributes>\
<md5>AA11</md5></resource></note>\n\
<note><title>第二条</title><content><![CDATA[<en-note><div>只有正文</div></en-note>]]></content></note>\n\
</en-export>"
    )
}

/// base64 编码（不引新依赖：手算，输入只有几个字节，且有测试反证它自洽）。
fn b64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let mut buf = [0u8; 3];
        buf[..chunk.len()].copy_from_slice(chunk);
        let n = (buf[0] as u32) << 16 | (buf[1] as u32) << 8 | buf[2] as u32;
        out.push(T[(n >> 18 & 63) as usize] as char);
        out.push(T[(n >> 12 & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            T[(n >> 6 & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// 在数据目录（含子目录）里找有没有这个名字的文件 —— 附件是内容寻址的，
/// 落点目录结构归存储层管，测试不该把布局写死。
fn exists_anywhere(root: &Path, name: &str) -> bool {
    let Ok(entries) = std::fs::read_dir(root) else {
        return false;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            if exists_anywhere(&p, name) {
                return true;
            }
        } else if p
            .file_name()
            .map(|s| s.to_string_lossy() == name)
            .unwrap_or(false)
        {
            return true;
        }
    }
    false
}

#[test]
fn import_files_command_pulls_every_note_out_of_one_enex_and_keeps_the_attachment() {
    let tmp = Tmp::new("import");
    let png = vec![
        0x89u8, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 3, 1, 4, 1, 5,
    ];
    let sha = notera_crypto::sha256_hex(&png);
    let file = tmp.0.join("我的笔记.enex");
    std::fs::write(&file, enex_body(&b64(&png))).expect("写 .enex");

    let app = App::boot(&tmp.0).expect("boot");
    let report = app
        .import_files(&[file.to_string_lossy().to_string()])
        .expect("import_files");

    let created = report["created"].as_array().expect("created 是数组");
    assert_eq!(created.len(), 2, "一份 .enex 两条笔记：{report}");
    assert_eq!(
        created[0]["title"].as_str().unwrap_or_default(),
        "报销单",
        "notes.title 是派生列，Evernote 的标题必须真的落进去：{report}"
    );
    assert!(
        report["failed"]
            .as_array()
            .map(|f| f.is_empty())
            .unwrap_or(false),
        "一条都不该失败：{}",
        report["failed"]
    );

    // 附件：字节必须真的躺在数据目录里（只登记行不写盘 = 用户下次打开看见占位）
    assert!(
        exists_anywhere(&tmp.0, &sha),
        "附件字节没落盘（找 {sha} 于 {:?}）",
        tmp.0
    );

    // "没坏但该知道"的事一路带到命令回报里，不是只进日志
    let notices = report["notices"].as_array().expect("notices 是数组");
    assert!(
        notices
            .iter()
            .any(|n| n.as_str().unwrap_or_default().contains("tag")),
        "Evernote 的 <tag> 没落地必须报给用户：{notices:?}"
    );

    // 幂等：同一个文件再导一次，一条都不新建，全部判重
    let again = app
        .import_files(&[file.to_string_lossy().to_string()])
        .expect("replay");
    assert_eq!(
        again["created"].as_array().map(Vec::len),
        Some(0),
        "重放不该新建任何笔记：{again}"
    );
    assert_eq!(
        again["duplicates"].as_u64(),
        Some(2),
        "两条都该被判成重复：{again}"
    );

    // 空路径列表是参数错误，不是"成功导入 0 条"
    assert!(
        app.import_files(&[]).is_err(),
        "空列表必须拒绝，不能谎报成功"
    );
}
