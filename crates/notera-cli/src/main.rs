//! notera-cli —— 诊断与测试驱动。不是产品功能，但它是"可验证"的前提。
//!
//! 关键约束：CLI **复用 notera-host 的同一份装配**，绝不重实现业务逻辑。
//! 否则测出来的绿灯与产品行为无关（这是这类工具最容易犯的错）。

use clap::{Parser, Subcommand};
use notera_host::{commands, App};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "notera", version, about = "Notera 诊断与测试驱动")]
struct Cli {
    /// 应用数据目录（默认 %LOCALAPPDATA%/notera 或 ./.notera-dev）
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// 起本地 dev 桥（127.0.0.1），供浏览器里的同一份前端连真实核心
    Serve {
        #[arg(long, default_value_t = notera_host::devserver::DEFAULT_PORT)]
        port: u16,
    },
    /// 跑一轮同步并打印计划/统计/不变式检查
    SyncOnce {
        #[arg(long)]
        json: bool,
    },
    /// 打印本地库统计与不变式自检结果
    Verify {
        #[arg(long)]
        json: bool,
    },
    /// 列出未处理冲突
    Conflicts,
    /// 探测 WebDAV 服务器能力（cap_mask）
    DavProbe,
    /// 代理与出口差分探测（PROXY.md §9 证据链③）
    NetProbe {
        #[arg(long)]
        url: Option<String>,
    },
    /// 导出/导入冒烟
    Export {
        #[arg(long)]
        out: PathBuf,
    },
    /// 内部：跑一次全功能自检（供 CI 冒烟）
    SelfTest,
}

/// 退出码语义（CI-CD.md 规定）：0=PASS，1=ASSERT_FAIL，2=BLOCKED。
/// 2 绝不能被解释成通过。
const EXIT_OK: i32 = 0;
const EXIT_FAIL: i32 = 1;
const EXIT_BLOCKED: i32 = 2;

fn main() {
    let cli = Cli::parse();
    let dir = cli.data_dir.unwrap_or_else(default_data_dir);
    std::process::exit(run(dir, cli.cmd));
}

fn default_data_dir() -> PathBuf {
    std::env::var("NOTERA_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(".notera-dev")
        })
}

fn boot(dir: &std::path::Path) -> Result<App, i32> {
    match App::boot(dir) {
        Ok(a) => Ok(a),
        Err(e) => {
            eprintln!("启动失败: {e}");
            Err(EXIT_BLOCKED)
        }
    }
}

fn run(dir: PathBuf, cmd: Cmd) -> i32 {
    match cmd {
        Cmd::Serve { port } => {
            let app = match boot(&dir) {
                Ok(a) => a,
                Err(c) => return c,
            };
            match notera_host::devserver::start(app.clone(), port) {
                Ok(_srv) => {
                    println!("dev 桥已启动: http://127.0.0.1:{port}  (数据目录 {})", dir.display());
                    println!("健康检查: curl http://127.0.0.1:{port}/health");
                    // 常驻：由调用方（CI / 开发脚本）终止
                    loop {
                        std::thread::sleep(std::time::Duration::from_secs(3600));
                    }
                }
                Err(e) => {
                    eprintln!("端口 {port} 无法绑定: {e}");
                    EXIT_BLOCKED
                }
            }
        }

        Cmd::Verify { json } => {
            let app = match boot(&dir) {
                Ok(a) => a,
                Err(c) => return c,
            };
            let violations = app.store().verify();
            let search = app.store().verify_search();
            let stats = app.store().stats().ok();
            if json {
                let out = serde_json::json!({
                    "invariants": violations.iter().map(|v| serde_json::json!({"id": v.id, "detail": v.detail})).collect::<Vec<_>>(),
                    "search": search.iter().map(|v| serde_json::json!({"id": v.id, "detail": v.detail})).collect::<Vec<_>>(),
                    "stats": stats,
                });
                println!("{out}");
            } else {
                println!("数据目录: {}", dir.display());
                if let Some(s) = stats {
                    println!("统计: {s:?}");
                }
                for v in violations.iter().chain(search.iter()) {
                    println!("不变式 {} 被违反: {}", v.id, v.detail);
                }
                println!("{}", if violations.is_empty() && search.is_empty() { "不变式检查通过" } else { "存在不变式违反" });
            }
            if violations.is_empty() && search.is_empty() {
                EXIT_OK
            } else {
                EXIT_FAIL
            }
        }

        Cmd::Conflicts => {
            let app = match boot(&dir) {
                Ok(a) => a,
                Err(c) => return c,
            };
            match app.open_conflicts() {
                Ok(v) if v.is_empty() => {
                    println("无未处理冲突");
                    EXIT_OK
                }
                Ok(v) => {
                    for c in &v {
                        println!("#{} 「{}」 本地 rev {} / 远端 rev {}", c.id, c.note_title, c.local_rev, c.remote_rev);
                    }
                    EXIT_FAIL
                }
                Err(e) => {
                    eprintln!("查询失败: {}", e.code);
                    EXIT_FAIL
                }
            }
        }

        Cmd::SyncOnce { json } => {
            // 需要已配置账户与可达服务器；没有就报 BLOCKED，不假装成功。
            let app = match boot(&dir) {
                Ok(a) => a,
                Err(c) => return c,
            };
            if app.current_account().ok().flatten().is_none() {
                eprintln!("未配置 WebDAV 账户：无法执行 sync-once（BLOCKED，不是 PASS）");
                return EXIT_BLOCKED;
            }
            eprintln!("sync-once 需要 RemotePort 适配器；当前构建未接线（见 notera-host::start_sync）。");
            EXIT_BLOCKED
        }

        Cmd::DavProbe | Cmd::NetProbe { .. } => {
            eprintln!("需要 notera-webdav/net 接线完成（Phase 3）。当前标记 BLOCKED，不计通过。");
            EXIT_BLOCKED
        }

        Cmd::Export { out } => {
            let app = match boot(&dir) {
                Ok(a) => a,
                Err(c) => return c,
            };
            eprintln!("导出待 notera-importer 接线（Phase 7）。已启动核心：{:?}", app.stats().is_ok());
            let _ = out;
            EXIT_BLOCKED
        }

        Cmd::SelfTest => {
            // 端到端最小闭环：建库 → 建夹 → 建笔记 → 改 → 搜 → 删 → 恢复 → 自检
            let app = match boot(&dir) {
                Ok(a) => a,
                Err(c) => return c,
            };
            let doc = |t: &str| serde_json::json!({"v":1,"content":[{"id":"b1","type":"paragraph","content":[{"text":t}]}]});
            let folders = match app.list_folders() {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("列文件夹失败: {}", e.code);
                    return EXIT_FAIL;
                }
            };
            let Some(root) = folders.first() else {
                eprintln!("默认文件夹缺失");
                return EXIT_FAIL;
            };
            let root_id = match notera_core::EntityId::parse(&root.id) {
                Ok(v) => v,
                Err(_) => return EXIT_FAIL,
            };
            let created = match app.create_note(&root_id, doc("自检：跨设备同步不应复活已删笔记")) {
                Ok(n) => n,
                Err(e) => {
                    eprintln!("建笔记失败: {}", e.code);
                    return EXIT_FAIL;
                }
            };
            let nid = match notera_core::EntityId::parse(&created.id) {
                Ok(v) => v,
                Err(_) => return EXIT_FAIL,
            };
            let edited = app.edit_note(&nid, doc("自检：两字中文检索必须能命中这条笔记"), notera_core::Rev(created.rev));
            if edited.is_err() {
                eprintln!("编辑失败（expectedRev 应为 {}）: {:?}", created.rev, edited.err().map(|e| e.code));
                return EXIT_FAIL;
            }
            let hits = app.search(commands::SearchCmd { text: "两字".into(), limit: 20 });
            match hits {
                Ok(h) if !h.iter().any(|x| x.note_id == created.id) => {
                    eprintln!("两字中文检索未命中刚写入的笔记（回归！见 DATA-MODEL §7.2）");
                    return EXIT_FAIL;
                }
                Err(e) => {
                    eprintln!("搜索失败: {}", e.code);
                    return EXIT_FAIL;
                }
                _ => {}
            }
            if let Err(e) = app.store().delete_note(&nid) {
                eprintln!("删除失败: {e}");
                return EXIT_FAIL;
            }
            if app.store().get_note(&nid).map(|n| n.is_some()).unwrap_or(true) {
                eprintln!("删除后仍可读到笔记");
                return EXIT_FAIL;
            }
            if let Err(e) = app.store().restore_note(&nid) {
                eprintln!("恢复失败: {e}");
                return EXIT_FAIL;
            }
            let v = app.store().verify();
            let s = app.store().verify_search();
            println!(
                "自检通过：建/改/搜/删/恢复 全链路可用；不变式违反 {} 项、搜索索引违反 {} 项",
                v.len(),
                s.len()
            );
            if v.is_empty() && s.is_empty() {
                EXIT_OK
            } else {
                for x in v.iter().chain(s.iter()) {
                    println!("  {} {}", x.id, x.detail);
                }
                EXIT_FAIL
            }
        }
    }
}

fn println(msg: impl std::fmt::Display) {
    println!("{msg}");
}
