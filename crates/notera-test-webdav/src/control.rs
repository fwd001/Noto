//! 控制面：`/_control/*` 与 `/_fs/dump`。
//!
//! 这两族路径**永不受注入影响、也不进请求日志** —— `STATS` 的语义是"协议流量序列"，
//! 而且服务器挂掉之后必须还能被 `ON` / `RESET` 拉起来。

use serde_json::{json, Value};

use crate::handler::Reply;
use crate::http::Request;
use crate::inject::Injection;
use crate::server::{Backend, Shared};
use crate::state::Store;

/// 控制面动作：由连接任务在**响应之后**执行（这些操作需要 async）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    Stop,
    Start,
    Restart,
}

pub fn is_control(path: &str) -> bool {
    path.starts_with("/_control") || path.starts_with("/_fs")
}

pub fn handle(shared: &Shared, req: &Request) -> (Reply, Action) {
    let path = req.path.as_str();
    let ok = |extra: Value| Reply::json(200, &json!({ "ok": true, "extra": extra }).to_string());

    match path {
        "/_fs/dump" | "/_fs/dump/" => {
            let prefix = query(&req.query, "prefix");
            let c = shared.cell.lock().expect("cell");
            (
                Reply::json(
                    200,
                    &dump_json(&c.store, prefix.as_deref(), c.generation, &c.backend).to_string(),
                ),
                Action::None,
            )
        }
        "/_control/reset" | "/_control/reset/" => {
            let c = shared.cell.lock().expect("cell");
            let mut c = c;
            if let Err(e) = c.store.clear() {
                return (Reply::json(500, &json!({ "error": e.to_string() }).to_string()), Action::None);
            }
            c.served_data = 0;
            c.counters = Default::default();
            (ok(json!({ "reset": true })), Action::None)
        }
        "/_control/inject" | "/_control/inject/" | "/_control/inject-failure" => {
            let inj = if req.body.iter().all(|b| b.is_ascii_whitespace()) {
                Injection::default()
            } else {
                match serde_json::from_slice::<Injection>(&req.body) {
                    Ok(i) => i,
                    Err(e) => {
                        return (
                            Reply::json(
                                400,
                                &json!({ "error": format!("注入 JSON 非法: {e}") }).to_string(),
                            ),
                            Action::None,
                        )
                    }
                }
            };
            let mut c = shared.cell.lock().expect("cell");
            c.injection = inj;
            c.counters = Default::default();
            c.served_data = 0;
            (ok(json!({ "injected": true })), Action::None)
        }
        "/_control/clear-injection" => {
            let mut c = shared.cell.lock().expect("cell");
            c.injection = Injection::default();
            c.counters = Default::default();
            c.served_data = 0;
            (ok(json!({ "cleared": true })), Action::None)
        }
        "/_control/stop" => (ok(json!({ "running": false })), Action::Stop),
        "/_control/start" => (ok(json!({ "running": true })), Action::Start),
        "/_control/restart" => (ok(json!({ "restarting": true })), Action::Restart),
        "/_control/inspect" | "/_control/inspect/" => {
            let c = shared.cell.lock().expect("cell");
            let v = json!({
                "addr": c.addr.to_string(),
                "backend": format!("{:?}", c.backend),
                "generation": c.generation,
                "running": c.running,
                "injection": c.injection,
                "counters": c.counters,
                "served_data": c.served_data,
                "requests": c.log,
            });
            (Reply::json(200, &v.to_string()), Action::None)
        }
        "/_control/log" => {
            let c = shared.cell.lock().expect("cell");
            (Reply::json(200, &json!(c.log).to_string()), Action::None)
        }
        other => (
            Reply::json(
                404,
                &json!({ "error": format!("未知控制路径 {other}") }).to_string(),
            ),
            Action::None,
        ),
    }
}

fn query(q: &str, key: &str) -> Option<String> {
    q.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == key).then(|| {
            percent_encoding::percent_decode_str(v)
                .decode_utf8_lossy()
                .into_owned()
        })
    })
}

/// 服务端权威快照。**这是测试唯一允许的服务端状态断言手段**（TEST-PLAN §记法 DUMP）。
pub fn dump_json(store: &Store, prefix: Option<&str>, generation: u64, backend: &Backend) -> Value {
    let entries = store.dump(prefix);
    let files = entries.iter().filter(|e| !e.is_dir).count();
    json!({
        "backend": match backend {
            Backend::Mem => "mem",
            Backend::Fs(_) => "fs",
        },
        "root": match backend {
            Backend::Mem => Value::Null,
            Backend::Fs(p) => p.display().to_string().into(),
        },
        "generation": generation,
        "prefix": prefix.unwrap_or(""),
        "count": files,
        "entries": entries,
    })
}
