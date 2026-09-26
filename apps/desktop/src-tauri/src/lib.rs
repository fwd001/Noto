//! Tauri 壳：窗口生命周期 + **一条**命令转发到 `notera_host::commands::dispatch`。
//!
//! 这里不允许出现业务规则，也不允许出现存储调用（ARCHITECTURE-MAP §5）。
//! 壳的职责只有四件：定位数据目录、装配 `App`、把命令搬出主线程、把总线事件推给 WebView。

use notera_host::{commands, App, BusEvent};
use std::sync::Arc;
use tauri::{Emitter, Manager};

/// 事件名。前端 `api/bridge.ts` 订阅的是同一个字面量，改动必须同步。
const EVENT_NAME: &str = "notera://event";

struct Shell {
    app: Arc<App>,
}

/// 命令拒绝载荷：UI 只认 `code` / `messageKey` / `retryable` / `detail`。
fn reject(code: &str, retryable: bool, why: String) -> serde_json::Value {
    serde_json::json!({
        "code": code,
        "messageKey": format!("cmd.{code}"),
        "retryable": retryable,
        "detail": { "why": why },
    })
}

#[tauri::command]
async fn notera_command(
    shell: tauri::State<'_, Shell>,
    name: String,
    args: serde_json::Value,
) -> Result<serde_json::Value, serde_json::Value> {
    let app = Arc::clone(&shell.app);
    // SQLite 是同步的：绝不在主线程跑命令，否则一次列表扫描就能冻结窗口。
    let joined = tauri::async_runtime::spawn_blocking(move || commands::dispatch(&app, &name, args)).await;
    match joined {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(err)) => Err(serde_json::to_value(&err).unwrap_or_else(|_| reject("serialize", false, err.code))),
        Err(e) => Err(reject("handler_panic", true, e.to_string())),
    }
}

fn pump_events(app: &App, handle: &tauri::AppHandle) {
    let rx = app.subscribe();
    let sink = handle.clone();
    std::thread::spawn(move || {
        while let Ok(event) = rx.recv() {
            if sink.emit(EVENT_NAME, &event).is_err() {
                return;
            }
        }
    });
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_shell::init())
        // 不注册的话命令面是空的：前端每次 invoke 都石沉大海，UI 看起来"点了没反应"。
        .invoke_handler(tauri::generate_handler![notera_command])
        .setup(|handle| {
            let dir = handle
                .path()
                .app_data_dir()
                .map_err(|e| format!("无法确定数据目录：{e}"))?;
            let app = App::boot(&dir).map_err(|e| format!("核心启动失败：{e}"))?;
            let app = Arc::new(app);
            pump_events(&app, handle.app_handle());
            handle.app_handle().manage(Shell { app: Arc::clone(&app) });
            if let Some(w) = handle.get_webview_window("main") {
                let _ = w.show();
                let _ = w.set_focus();
            }
            // 窗口已经出首帧，**这时**才允许碰网络（PLATFORM.md §3）。
            // 没配服务器或凭据还没接入钥匙串 → 引擎不启动，本地照常写（I8）。
            match app.sync_remote() {
                Ok(Some(remote)) => {
                    let scheduler = app.as_ref().clone().start_sync(remote);
                    tauri::async_runtime::spawn(scheduler.run());
                }
                Ok(None) => {}
                Err(e) => app.emit(BusEvent::Toast { message_key: e.message_key, level: "warn".into() }),
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .unwrap_or_else(|e| {
            eprintln!("[notera] 壳构建失败：{e}");
            std::process::exit(1);
        })
        .run(|_handle, _event| {
            // 连接池随进程退出关闭，SQLite 在干净关闭时自行 checkpoint WAL —— 壳不插手。
        });
}
