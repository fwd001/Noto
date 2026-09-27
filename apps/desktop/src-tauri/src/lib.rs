//! Tauri 壳：窗口生命周期 + **一条**命令转发到 `notera_host::commands::dispatch`。
//!
//! 这里不允许出现业务规则，也不允许出现存储调用（ARCHITECTURE-MAP §5）。
//! 壳的职责只有四件：定位数据目录、装配 `App`、把命令搬出主线程、把总线事件推给 WebView。

use notera_host::{commands, App, BusEvent, NativeCap};
use std::sync::Arc;
use tauri::{Emitter, Manager};
use tauri_plugin_notification::NotificationExt;

use notera_host::platform::{
    global_shortcut_plan, menu_plan, notice_for, tray_plan, ShortcutRoute,
};

/// 事件名。前端 `api/bridge.ts` 订阅的是同一个字面量，改动必须同步。
const EVENT_NAME: &str = "notera://event";
/// 原生菜单被点击 → 前端按 `id` 路由到界面动作（新建、搜索、去冲突收件箱…）。
/// 菜单**不**自己碰业务：它和键盘快捷键是同一个入口的两种外壳。
const MENU_EVENT_NAME: &str = "notera://menu";

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
            // 窗口不在前台时，"要人裁决"的那两类事实只能靠系统通知递到手边。
            // 判定是纯函数（`notice_for`），所以"哪些事件会响"这件事本身有测试。
            if let Some(n) = notice_for(&event) {
                if let Err(e) = sink
                    .notification()
                    .builder()
                    .title(&n.title)
                    .body(&n.body)
                    .show()
                {
                    eprintln!("[notera] 系统通知没送出去：{e}（界面上的冲突收件箱不受影响）");
                }
            }
        }
    });
}

/// 按平台把菜单计划摆成原生菜单。摆不上去要**吵**，不能静默退回"没有菜单"——
/// 那正是"能力声明说有你却看不到"的那种假。
fn attach_menu(app: &tauri::AppHandle) -> tauri::Result<()> {
    use tauri::menu::{MenuBuilder, MenuItemBuilder, SubmenuBuilder};
    let plan = menu_plan(cfg!(target_os = "macos"));
    let mut builder = MenuBuilder::new(app);
    for (title, items) in &plan.groups {
        let mut sub = SubmenuBuilder::new(app, *title);
        for item in items {
            let mut b = MenuItemBuilder::new(item.label).id(item.id);
            if let Some(a) = item.accel {
                // Tauri 认 `Command` 而不是计划表里的 `Cmd`；解析失败就宁可少一个
                // 快捷键（菜单项还在），也不要把整条菜单构建搞崩。
                b = b.accelerator(a.replace("Cmd", "Command").as_str());
            }
            sub = sub.item(&b.build(app)?);
        }
        builder = builder.item(&sub.build()?);
    }
    app.set_menu(builder.build()?)?;
    Ok(())
}

/// 托盘的 id：`on_tray_icon_event` 与菜单事件都按它归口，改名要连窗口事件一起看。
const TRAY_ID: &str = "notera-tray";

/// 显示 / 隐藏主窗口。托盘左键、托盘菜单、全局快捷键三条路都汇到这一个函数 ——
/// "怎么算显示"只有一份实现（§39 不许复制状态机）。
fn toggle_main_window(app: &tauri::AppHandle) {
    let Some(w) = app.get_webview_window("main") else {
        eprintln!("[notera] 找不到 main 窗口，显示/隐藏没发生");
        return;
    };
    if w.is_visible().unwrap_or(false) && !w.is_focused().unwrap_or(false) {
        let _ = w.set_focus();
        return;
    }
    if w.is_visible().unwrap_or(false) {
        let _ = w.hide();
    } else {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// 挂托盘图标 + 托盘菜单。**返回是否真的挂上了** —— 能力声明由这个返回值写，
/// 不由"这个平台原则上能做到"猜（那是此前两次假声明的形状）。
fn attach_tray(app: &tauri::AppHandle) -> Result<(), String> {
    use tauri::menu::{MenuBuilder, MenuItemBuilder};
    use tauri::tray::TrayIconBuilder;
    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or("构建里没有窗口图标")?;
    let mut builder = MenuBuilder::new(app);
    for item in tray_plan() {
        let item = MenuItemBuilder::new(item.label)
            .id(item.id)
            .build(app)
            .map_err(|e| e.to_string())?;
        builder = builder.item(&item);
    }
    let menu = builder.build().map_err(|e| e.to_string())?;
    TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        // 左键留给"显示/隐藏"，菜单走右键 —— 和 Apple Notes 所在的那类菜单栏应用一致。
        // 左键也弹菜单的话，一次点击两个意图，用户分不清自己刚刚是收起还是打开了菜单。
        .show_menu_on_left_click(false)
        .icon(icon)
        .tooltip("Notera")
        .build(app)
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// 注册系统级快捷键。返回**全部**注册成功才叫成功 —— 注册了一半就报 true，
/// 设置页会摆出一个按了没反应的组合键，那正是被 §40 明令禁止的那种"看着有"。
fn register_global_shortcuts(app: &tauri::AppHandle) -> bool {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
    let gs = app.global_shortcut();
    let mut all_ok = true;
    for spec in global_shortcut_plan(cfg!(target_os = "macos")) {
        let sink = app.clone();
        let route = spec.route.clone();
        let accel = spec.accel;
        let result = gs.on_shortcut(accel, move |_app, _shortcut, event| {
            // 按住不放会连续来 Pressed/Released；只在按下时动一次，
            // 否则"新建笔记"会被连开好几条。
            if event.state != ShortcutState::Pressed {
                return;
            }
            match &route {
                ShortcutRoute::Menu(menu_id) => {
                    let _ = sink.emit(MENU_EVENT_NAME, (*menu_id).to_string());
                }
                ShortcutRoute::ShowOrHideWindow => toggle_main_window(&sink),
            }
        });
        if let Err(e) = result {
            eprintln!("[notera] 全局快捷键 {accel} 没注册上：{e}");
            all_ok = false;
        }
    }
    all_ok
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_shell::init())
        // 快捷键只从 Rust 侧注册，前端不碰这个插件的 IPC —— 所以不需要给它开
        // capability 权限（能力面越小，越不存在"前端能替用户注册全局键"这件事）。
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        // 不注册的话命令面是空的：前端每次 invoke 都石沉大海，UI 看起来"点了没反应"。
        .invoke_handler(tauri::generate_handler![notera_command])
        .on_menu_event(|app, event| {
            let id = event.id().as_ref().to_string();
            // 只有托盘独有的两项由壳自己办（窗口显隐、退出）。其余一律转发给前端，
            // 与键盘快捷键、系统级快捷键共用同一批动作 —— 托盘不许有第二条实现。
            match id.as_str() {
                "tray.toggle" => toggle_main_window(app),
                "tray.quit" => {
                    // 托盘上那个"退出"必须是真退出：先把图标收掉再 exit，
                    // 否则万一 exit 被挡住，用户看到的是"图标还在、点什么都没反应"。
                    if let Some(t) = app.tray_by_id(TRAY_ID) {
                        let _ = t.set_visible(false);
                    }
                    app.exit(0);
                }
                _ => {
                    let _ = app.emit(MENU_EVENT_NAME, id);
                }
            }
        })
        .on_tray_icon_event(|tray, event| {
            // 左键按下 = 显示/隐藏；右键（以及按住不放的那些）交给系统菜单。
            if let tauri::tray::TrayIconEvent::Click {
                button: tauri::tray::MouseButton::Left,
                button_state: tauri::tray::MouseButtonState::Up,
                ..
            } = event
            {
                toggle_main_window(tray.app_handle());
            }
        })
        .setup(|handle| {
            let dir = handle
                .path()
                .app_data_dir()
                .map_err(|e| format!("无法确定数据目录：{e}"))?;
            let app = App::boot(&dir).map_err(|e| format!("核心启动失败：{e}"))?;
            let app = Arc::new(app);
            pump_events(&app, handle.app_handle());
            handle.app_handle().manage(Shell {
                app: Arc::clone(&app),
            });
            // 原生菜单在窗口显示**之前**挂好：第一帧就该看到本应用的菜单，
            // 而不是 Tauri 那份只有 Cut/Copy/Quit 的默认菜单。
            // 移动端没有原生菜单栏，`set_menu` 在那里是**会失败**的 —— 不加这道
            // 守卫，同一个 setup 会把 Android/iOS 构建直接顶死在启动上。
            if cfg!(any(
                target_os = "windows",
                target_os = "macos",
                target_os = "linux"
            )) {
                attach_menu(handle.app_handle()).map_err(|e| format!("原生菜单挂载失败：{e}"))?;
                // 托盘与系统级快捷键：这两件**可以失败**（系统不让挂、键位被别的应用占了），
                // 所以它们不像菜单那样 `?` 上去把启动顶死，而是把结果如实写回能力声明 ——
                // 设置页随之自己回到"这台设备不支持"，同时吵一声让用户知道刚刚没成。
                let tray_on = match attach_tray(handle.app_handle()) {
                    Ok(()) => true,
                    Err(e) => {
                        eprintln!("[notera] 托盘没挂上：{e}（设置页那两项会显示为不支持）");
                        false
                    }
                };
                let shortcuts_on = register_global_shortcuts(handle.app_handle());
                app.report_native_cap(NativeCap::Tray, tray_on);
                app.report_native_cap(NativeCap::GlobalShortcuts, shortcuts_on);
                if !tray_on || !shortcuts_on {
                    app.emit(BusEvent::Toast {
                        message_key: "platform.caps_degraded".into(),
                        level: "warn".into(),
                    });
                }
            }
            if let Some(w) = handle.get_webview_window("main") {
                let _ = w.show();
                let _ = w.set_focus();
            }
            // 窗口已经出首帧，**这时**才允许碰网络（PLATFORM.md §3）。
            // 没配服务器或凭据还没接入钥匙串 → 引擎不启动，本地照常写（I8）。
            match app.sync_remote() {
                Ok(Some(_)) => {
                    // 协商在启动调度器**之前**：两个库指向同一目录、或服务器上的
                    // 协议区间不相交时，必须一次都不写，而不是先同步了再解释。
                    let host = app.as_ref().clone();
                    tauri::async_runtime::spawn(async move {
                        // §5「首次连接与每日一次」：探测必须在装适配器**之前**完成，
                        // 否则这次会话仍按保守默认写，探到的能力要等下次启动才生效。
                        let remote = match host.remote_for_sync().await {
                            Ok(Some(r)) => r,
                            // 配置在启动期间被改掉（拔了账户）：静默退回"只用本地"。
                            Ok(None) => return,
                            Err(e) => {
                                host.emit(BusEvent::Toast {
                                    message_key: e.message_key,
                                    level: "warn".into(),
                                });
                                return;
                            }
                        };
                        match host.negotiate(&remote).await {
                            Ok(()) => {
                                // 附件走自己的循环（§13）：与文本轮次互不等待、互不阻塞，
                                // 一个 20 MB 的图片不该让文字同步停下来。
                                let att = host.clone();
                                let att_remote = std::sync::Arc::clone(&remote);
                                tauri::async_runtime::spawn(async move {
                                    att.run_attachments(
                                        att_remote,
                                        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(
                                            false,
                                        )),
                                    )
                                    .await;
                                });
                                host.start_sync(remote).run().await
                            }
                            Err(key) => {
                                host.emit(BusEvent::Toast {
                                    message_key: key.to_string(),
                                    level: "warn".into(),
                                });
                            }
                        }
                    });
                }
                Ok(None) => {}
                Err(e) => app.emit(BusEvent::Toast {
                    message_key: e.message_key,
                    level: "warn".into(),
                }),
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
