//! 移动壳（Android）：与桌面壳**同一条**命令转发到 `notera_host::commands::dispatch`。
//!
//! 这个文件里不许出现业务规则，也不许出现存储调用（ARCHITECTURE-MAP §5）。
//! 它和 `apps/desktop/src-tauri/src/lib.rs` 的差别**只有平台表面**：
//! 移动端没有系统托盘、没有全局快捷键、没有原生菜单栏，所以这三样一概不注册；
//! 命令面、事件面（`notera://event`）、数据目录口径、`App::boot` 都完全一致 ——
//! 两台壳共用的是 `notera-host` 那一份实现，不是第二份状态机（§39 不许复制协议/状态机）。
//!
//! 为什么是**另一个 crate**：桌面 crate 的 `[lib] crate-type` 只要多带 `cdylib`，
//! Windows GNU 工具链就报 `ld.exe: error: export ordinal too large`（实测），桌面产物链不出来。
//! Android 需要 `staticlib` + `cdylib` 给 JNI 入口，所以移动壳自己建 crate。

use notera_host::{commands, App};
use std::sync::Arc;
use tauri::{Emitter, Manager};
use tauri_plugin_notification::NotificationExt;

/// 事件名。前端 `api/bridge.ts` 订阅的是同一个字面量，改动必须与桌面壳同步。
const EVENT_NAME: &str = "notera://event";

struct Shell {
    app: Arc<App>,
}

/// 命令拒绝载荷：UI 只认 `code` / `messageKey` / `retryable` / `detail`（与桌面壳同一套字面量）。
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
    // SQLite 是同步的：绝不在主线程跑命令，否则一次列表扫描就能冻住 WebView。
    let joined =
        tauri::async_runtime::spawn_blocking(move || commands::dispatch(&app, &name, args)).await;
    match joined {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(err)) => {
            Err(serde_json::to_value(&err).unwrap_or_else(|_| reject("serialize", false, err.code)))
        }
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
            // 与桌面壳同一个判定（`notice_for`）：只有"要人裁决"的那两类才响。
            // 移动系统要求通知权限，拿不到就吵一声 —— 界面里的冲突收件箱不受影响。
            if let Some(n) = notera_host::platform::notice_for(&event) {
                if let Err(e) = sink
                    .notification()
                    .builder()
                    .title(&n.title)
                    .body(&n.body)
                    .show()
                {
                    eprintln!(
                        "[notera-mobile] 系统通知没送出去：{e}（界面上的冲突收件箱不受影响）"
                    );
                }
            }
        }
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .invoke_handler(tauri::generate_handler![notera_command])
        .setup(|handle| {
            // 数据目录口径与桌面壳/`notera-cli` 一致：`NOTERA_DATA_DIR` 优先，否则用系统
            // 给本应用的目录（Android 上是应用私有目录，可写、卸载即清）。
            let dir = match std::env::var_os("NOTERA_DATA_DIR") {
                Some(v) if !v.is_empty() => std::path::PathBuf::from(v),
                _ => handle
                    .path()
                    .app_data_dir()
                    .map_err(|e| format!("无法确定数据目录：{e}"))?,
            };
            let app = App::boot(&dir).map_err(|e| format!("核心启动失败：{e}"))?;
            let app = Arc::new(app);
            pump_events(&app, handle.app_handle());
            handle.app_handle().manage(Shell {
                app: Arc::clone(&app),
            });
            // 能力声明：移动壳**不**上报 Tray / GlobalShortcuts —— 这两个面在 Android 上不存在，
            // 不上报就是 false（`PlatformCaps` 的默认值）。上一批桌面那份能力表如果被照抄过来，
            // 设置页就会摆出三个按了没反应的开关，所以这里刻意什么都不报。
            //
            // 后台同步：移动壳以前**从来没有起过引擎**（桌面那条 spawn 只写在桌面壳里），
            // 也就是配好 WebDAV 也永远不上传。生命周期收进宿主之后，这里一句就够（缺口 G50）。
            app.enable_background_sync();
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("Noto 移动壳启动失败");
}
