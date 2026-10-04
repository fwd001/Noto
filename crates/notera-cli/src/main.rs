//! notera-cli —— 诊断与测试驱动。不是产品功能，但它是"可验证"的前提。
//!
//! 关键约束：CLI **复用 notera-host 的同一份装配**，绝不重实现业务逻辑。
//! 否则测出来的绿灯与产品行为无关（这是这类工具最容易犯的错）。

use clap::{Parser, Subcommand};
use notera_host::{commands, App};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "notera", version, about = "Noto 诊断与测试驱动")]
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
    /// 探测 WebDAV 服务器能力（cap_mask）并写回账户
    DavProbe {
        #[arg(long)]
        json: bool,
    },
    /// 代理与出口差分探测（PROXY.md §9 证据链③；纯判定，不发请求）
    NetProbe {
        #[arg(long)]
        url: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// 导出为自描述 ZIP（整库，或按文件夹的子树），写完立刻回读校验
    Export {
        #[arg(long)]
        out: PathBuf,
        /// 只导这些文件夹（自动带上祖先链，否则导回去是外键失败）。留空 = 整库。
        /// 注意：子树包缺库内其它内容与"无法归属到文件夹"的永久删除公告，不能当整库备份用。
        #[arg(long, use_value_delimiter = true)]
        folders: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// 备份当前库到指定目录（`VACUUM INTO` 的一致性单文件快照）
    Backup {
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

/// 诊断命令要的只是"跑完一件事再退出"，所以是单线程、用完即弃的运行时；
/// 产品侧的多线程运行时由壳/`devserver` 自己持有。
fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio 运行时")
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
                    // dev 桥以前只转发命令面，**没有任何调度器** ⇒ 浏览器里点"立即同步"
                    // 永远等不到事件回流（G50 的那一颗圈在 dev 下反而更好复现）。
                    // 打开托管之后，lane 测到的才是产品那条真链路。
                    app.enable_background_sync();
                    println!(
                        "dev 桥已启动: http://127.0.0.1:{port}  (数据目录 {})",
                        dir.display()
                    );
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
                println!(
                    "{}",
                    if violations.is_empty() && search.is_empty() {
                        "不变式检查通过"
                    } else {
                        "存在不变式违反"
                    }
                );
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
                        println!(
                            "#{} 「{}」 本地 rev {} / 远端 rev {}",
                            c.id, c.note_title, c.local_rev, c.remote_rev
                        );
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
            // 走的是 `App::sync_once` —— 调度器每 25 秒跑的就是同一条路径，
            // 所以这里绿了才可以说产品那一轮也绿（本文件开头的第一约束）。
            let stats = match rt().block_on(app.sync_once()) {
                Ok(s) => s,
                Err(e) => {
                    let why = e
                        .detail
                        .as_ref()
                        .map(|d| format!(" {}", d))
                        .unwrap_or_default();
                    eprintln!("sync-once 未能执行：{}{why}（BLOCKED，不是 PASS）", e.code);
                    return EXIT_BLOCKED;
                }
            };
            if json {
                println!(
                    "{}",
                    serde_json::to_string(&stats).unwrap_or_else(|_| "{}".into())
                );
            } else {
                println!(
                    "一轮完成: {:?} 请求 {} 次 ↑{}B ↓{}B 推 {} 拉 {} 冲突 {} CAS 重试 {}",
                    stats.outcome,
                    stats.requests,
                    stats.bytes_up,
                    stats.bytes_down,
                    stats.pushed,
                    stats.pulled,
                    stats.conflicts,
                    stats.cas_retries
                );
            }
            match stats.outcome {
                // NoOp 是健康结果（空轮 1 请求 0 字节），不是"什么都没做所以失败"
                notera_sync::RoundOutcome::NoOp | notera_sync::RoundOutcome::Converged => EXIT_OK,
                notera_sync::RoundOutcome::Partial => EXIT_FAIL,
                notera_sync::RoundOutcome::Failed => {
                    eprintln!("本轮失败：见 sync_status / verify 日志");
                    EXIT_FAIL
                }
            }
        }

        Cmd::DavProbe { json } => {
            let app = match boot(&dir) {
                Ok(a) => a,
                Err(c) => return c,
            };
            // 强制探测（不看每日一次）：这是诊断命令，用户就是要现在的真实结论。
            match rt().block_on(app.probe_and_store_caps()) {
                Ok(v) => {
                    if json {
                        println!(
                            "{}",
                            serde_json::to_string(&v).unwrap_or_else(|_| "{}".into())
                        );
                    } else {
                        println!("{}", v["describe"].as_str().unwrap_or(""));
                        println!("写入策略: {}", v["writeStrategy"].as_str().unwrap_or("?"));
                    }
                    EXIT_OK
                }
                Err(e) => {
                    // 探不到就是探不到：报 BLOCKED，绝不回退成"这台服务器什么都不支持"
                    // 那会把支持条件写的服务器降级成 S3 盲写，方向正好是丢数据的那边。
                    eprintln!("能力探测未完成：{}（BLOCKED，不是 PASS）", e.code);
                    EXIT_BLOCKED
                }
            }
        }

        Cmd::NetProbe { url, json } => {
            let app = match boot(&dir) {
                Ok(a) => a,
                Err(c) => return c,
            };
            // PROXY.md §9 证据链③：出口判定，一个包都不发，所以离线也能跑。
            let target = match url {
                Some(u) => u,
                None => match app.current_account() {
                    Ok(Some(a)) => a.base_url,
                    _ => {
                        eprintln!("既没给 --url 也没配置账户，无目标可判定（BLOCKED）");
                        return EXIT_BLOCKED;
                    }
                },
            };
            match app.route_for(&target) {
                Ok(v) => {
                    if json {
                        println!(
                            "{}",
                            serde_json::to_string(&v).unwrap_or_else(|_| "{}".into())
                        );
                    } else {
                        println!("配置来源: {}", v["configFrom"].as_str().unwrap_or("?"));
                        println!("{}", v["oneLine"].as_str().unwrap_or(""));
                    }
                    EXIT_OK
                }
                Err(e) => {
                    eprintln!("出口判定失败：{}（BLOCKED，不是 PASS）", e.code);
                    EXIT_BLOCKED
                }
            }
        }

        Cmd::Export { out, folders, json } => {
            let app = match boot(&dir) {
                Ok(a) => a,
                Err(c) => return c,
            };
            let cmd = notera_host::commands::ExportCmd {
                folder_ids: folders.clone(),
                include_attachments: true,
                include_trash: true,
                path: Some(out.to_string_lossy().to_string()),
            };
            match app.export_data(cmd) {
                Ok(v) => {
                    if json {
                        println!(
                            "{}",
                            serde_json::to_string(&v).unwrap_or_else(|_| "{}".into())
                        );
                    } else {
                        println!("已导出: {}", v["path"].as_str().unwrap_or(""));
                        println!("范围: {} · 计数: {}", v["scope"], v["counts"]);
                    }
                    // 导出的东西必须真的是个能打开的包：写完立刻回读校验，
                    // 否则"文件存在"就成了"数据可恢复"的假证据。
                    match notera_importer::read_bundle(&out) {
                        Ok(b) => {
                            println!(
                                "回读校验通过: {} 条笔记 / {} 个附件",
                                b.notes.len(),
                                b.attachments.len()
                            );
                            // 数量也要对得上：这里曾经只打印不判定，于是"包能打开但一个附件
                            // 都没有"的导出照样算通过 —— 用户手里是一份缺全部附件的"完整备份"。
                            // 按文件夹导时基准必须是**那一棵子树**的附件：直接拿勾选的那几个
                            // id 去数会漏掉子层里的附件（基准偏低 → 缺附件也判通过），
                            // 而拿全库去数又会把一次正常的部分导出误判成失败。
                            let want = if folders.is_empty() {
                                app.store().local_attachment_shas()
                            } else {
                                let ids: Vec<notera_core::EntityId> = folders
                                    .iter()
                                    .filter_map(|s| notera_core::EntityId::parse(s).ok())
                                    .collect();
                                match app.store().folder_subtree(&ids) {
                                    Ok(set) => app.store().attachment_shas_in_folders(
                                        &set.into_iter().collect::<Vec<_>>(),
                                    ),
                                    Err(e) => {
                                        eprintln!("导出范围算不出来：{e}（ASSERT_FAIL）");
                                        return EXIT_FAIL;
                                    }
                                }
                            }
                            .map(|v| v.len())
                            .unwrap_or(usize::MAX);
                            if b.attachments.len() < want {
                                eprintln!("导出的包里少了附件：库里有 {want} 个，包里只有 {} 个（ASSERT_FAIL）", b.attachments.len());
                                return EXIT_FAIL;
                            }
                            EXIT_OK
                        }
                        Err(e) => {
                            eprintln!("导出文件回读失败：{e}（ASSERT_FAIL）");
                            EXIT_FAIL
                        }
                    }
                }
                Err(e) => {
                    eprintln!("导出失败：{}", e.code);
                    EXIT_FAIL
                }
            }
        }

        Cmd::Backup { out } => {
            let app = match boot(&dir) {
                Ok(a) => a,
                Err(c) => return c,
            };
            match app.backup_db(Some(&out)) {
                Ok(info) => {
                    println!(
                        "备份: {} ({} 字节, SHA-256 {}, 建于 {})",
                        info.path.display(),
                        info.bytes,
                        info.sha256,
                        info.created_at
                    );
                    EXIT_OK
                }
                Err(e) => {
                    eprintln!("备份失败：{}", e.code);
                    EXIT_FAIL
                }
            }
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
            let created = match app.create_note(&root_id, doc("自检：跨设备同步不应复活已删笔记"))
            {
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
            let edited = app.edit_note(
                &nid,
                doc("自检：两字中文检索必须能命中这条笔记"),
                notera_core::Rev(created.rev),
            );
            if edited.is_err() {
                eprintln!(
                    "编辑失败（expectedRev 应为 {}）: {:?}",
                    created.rev,
                    edited.err().map(|e| e.code)
                );
                return EXIT_FAIL;
            }
            let hits = app.search(commands::SearchCmd {
                text: "两字".into(),
                limit: 20,
            });
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
            if app
                .store()
                .get_note(&nid)
                .map(|n| n.is_some())
                .unwrap_or(true)
            {
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
