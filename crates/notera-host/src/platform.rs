//! "平台原生物"的**计划表**：应用菜单项与系统通知的判定。：应用菜单项与系统通知的**判定**。
//!
//! 刻意写成不依赖 `AppHandle` 的纯函数，理由和 `host::PlatformCaps` 一样 ——
//! 只有把"该有什么"和"怎么摆上去"分开，能力声明才可被机器验一遍。
//! 判据来自这条边曾出过的事故：`for_current_target()` 对 macOS 报 `global_shortcuts: true`，而壳里一个
//! 快捷键注册都没有，于是设置页摆出一组按了没反应的组合键。

use crate::{Badge, BusEvent};

/// 一个菜单项：`id` 是给前端的路由键，`label` 是原生菜单上显示的字，
/// `accel` 是 accelerator（`Cmd+…` / `Ctrl+…`，由调用方按平台给）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuSpec {
    pub id: &'static str,
    pub label: &'static str,
    pub accel: Option<&'static str>,
}

/// 分组的原生菜单（每组渲染成一个子菜单）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuPlan {
    pub groups: Vec<(&'static str, Vec<MenuSpec>)>,
}

/// macOS 用 `Cmd`，其余桌面用 `Ctrl`。这是**平台差异**，不是机型分支（ADR-0011）。
pub fn menu_plan(mac: bool) -> MenuPlan {
    let (note_new, search, settings, sync, conflicts) = if mac {
        ("Cmd+N", "Cmd+F", "Cmd+,", "Cmd+S", "Cmd+K")
    } else {
        ("Ctrl+N", "Ctrl+F", "Ctrl+,", "Ctrl+S", "Ctrl+K")
    };
    let groups: Vec<(&'static str, Vec<MenuSpec>)> = vec![
        (
            "笔记",
            vec![
                MenuSpec {
                    id: "note.new",
                    label: "新建笔记",
                    accel: Some(note_new),
                },
                MenuSpec {
                    id: "note.search",
                    label: "搜索笔记",
                    accel: Some(search),
                },
            ],
        ),
        (
            "同步",
            vec![
                MenuSpec {
                    id: "sync.now",
                    label: "立即同步",
                    accel: Some(sync),
                },
                MenuSpec {
                    id: "view.conflicts",
                    label: "冲突收件箱",
                    accel: Some(conflicts),
                },
            ],
        ),
        (
            "前往",
            vec![
                MenuSpec {
                    id: "view.trash",
                    label: "最近删除",
                    accel: None,
                },
                MenuSpec {
                    id: "view.settings",
                    label: "设置",
                    accel: Some(settings),
                },
            ],
        ),
    ];
    MenuPlan { groups }
}

impl MenuPlan {
    #[cfg(test)]
    pub fn ids(&self) -> Vec<&'static str> {
        self.groups
            .iter()
            .flat_map(|(_, items)| items.iter().map(|i| i.id))
            .collect()
    }
}

/// 一条系统通知的正文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub title: String,
    pub body: String,
}

/// **哪些**总线事件值得打扰用户一次。
///
/// 判据是"不处理就会丢东西或用户会等"：冲突要人去裁决、同步失败要人去修配置。
/// 其余（进度、Toast、笔记变化、恢复联网后的成功）一律不弹 ——
/// 每 25 秒一轮的后台任务如果每次都弹系统通知，用户会直接去关掉本应用的通知权限。
pub fn notice_for(ev: &BusEvent) -> Option<Notice> {
    match ev {
        BusEvent::Conflict { note_title, .. } => Some(Notice {
            title: "有一条笔记需要处理".into(),
            body: format!("「{note_title}」在另一台设备上也改了，两版都留着，去挑一版"),
        }),
        BusEvent::Sync {
            badge, error_code, ..
        } => {
            // 只在"错误"这一下弹一次；徽标为 syncing/synced/offline 都不该响
            match badge {
                Badge::Failed if error_code.as_deref().is_some() => Some(Notice {
                    title: "同步没有成功".into(),
                    body: "内容都还在本机，点开应用可以看原因".into(),
                }),
                _ => None,
            }
        }
        _ => None,
    }
}

/// 一条**系统级**快捷键（应用不在前台、甚至窗口收在托盘里时也生效）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalShortcut {
    pub id: &'static str,
    /// `tauri-plugin-global-shortcut` 认的 accelerator 字面量。
    pub accel: &'static str,
    pub route: ShortcutRoute,
}

/// 快捷键按下去之后走哪条路。刻意只有两条：
/// `Menu` 复用原生菜单那**同一批**动作（不另写一套行为），窗口显隐是壳自己的事。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShortcutRoute {
    Menu(&'static str),
    ShowOrHideWindow,
}

/// 系统级快捷键的**精选集**，不是 `menu_plan` 的副本。
///
/// 为什么不从菜单计划派生：菜单上的 accel 是应用内的 `Ctrl+S` / `Ctrl+F`，把它们注册成
/// **全局**就是劫持 —— 用户在别的编辑器里按保存会去动我们的库，那是最难查的一种串味。
/// 所以这里只用"必然不与人打架"的三键组合（修饰键 + Alt），并且只放两条真正值得从
/// 系统层发起的动作。加一条就要在设置页显示一条，两边由
/// `the_settings_page_shows_exactly_the_registered_global_shortcuts` 钉住。
pub fn global_shortcut_plan(mac: bool) -> Vec<GlobalShortcut> {
    let (quick_note, toggle) = if mac {
        ("Command+Option+N", "Command+Option+I")
    } else {
        ("CommandOrControl+Alt+N", "CommandOrControl+Alt+I")
    };
    vec![
        GlobalShortcut {
            id: "global.quick-note",
            accel: quick_note,
            route: ShortcutRoute::Menu("note.new"),
        },
        GlobalShortcut {
            id: "global.toggle-window",
            accel: toggle,
            route: ShortcutRoute::ShowOrHideWindow,
        },
    ]
}

/// 托盘菜单上的一项。托盘**不是**应用菜单的镜像，是一份更短的日常集。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayItem {
    pub id: &'static str,
    pub label: &'static str,
}

/// 托盘菜单计划。除两条托盘独有项之外，其余 id 必须是 `menu_plan` 里已有的 ——
/// 托盘点了"新建笔记"必须和菜单点了走同一条前端路由，不能各写一套。
pub fn tray_plan() -> Vec<TrayItem> {
    vec![
        TrayItem {
            id: "tray.toggle",
            label: "显示 / 隐藏窗口",
        },
        TrayItem {
            id: "note.new",
            label: "新建笔记",
        },
        TrayItem {
            id: "sync.now",
            label: "立即同步",
        },
        TrayItem {
            id: "tray.quit",
            label: "退出",
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Badge;

    #[test]
    fn tray_items_other_than_the_trays_own_reuse_the_menu_routes() {
        let menu = menu_plan(false).ids();
        for item in tray_plan() {
            if item.id.starts_with("tray.") {
                assert!(
                    ["tray.toggle", "tray.quit"].contains(&item.id),
                    "托盘独有项要新增就得连壳一起改：{}",
                    item.id
                );
                continue;
            }
            assert!(
                menu.contains(&item.id),
                "托盘项 {} 在应用菜单里没有对应项 —— 它会变成第二条独立实现（§39 禁止复制状态机）",
                item.id
            );
        }
    }

    #[test]
    fn global_shortcuts_never_reuse_a_bare_app_menu_accelerator() {
        // 菜单 accel 是应用内的（Ctrl+S、Ctrl+F）；它们一旦被全局注册，就是在别的
        // 应用里抢键。这条断言防的是"图省事，把 menu_plan 直接喂给快捷键插件"。
        let menu_accels: Vec<&str> = menu_plan(false)
            .groups
            .iter()
            .flat_map(|(_, v)| v.iter())
            .filter_map(|i| i.accel)
            .collect();
        for gs in global_shortcut_plan(false) {
            assert!(
                !menu_accels.contains(&gs.accel),
                "全局快捷键 {} 用的正是应用菜单的 {}，会劫持别的应用的按键",
                gs.id,
                gs.accel
            );
            // 只允许"两个修饰键 + 键"这种几乎不可能撞车的组合
            let plus = gs.accel.matches('+').count();
            assert!(
                plus >= 2,
                "全局快捷键 {} 的修饰键不够（{}）",
                gs.id,
                gs.accel
            );
        }
        for gs in global_shortcut_plan(true) {
            assert!(gs.accel.starts_with("Command+Option"), "mac {}", gs.accel);
        }
    }

    #[test]
    fn global_shortcut_ids_are_prefixed_so_the_two_event_sources_cannot_collide() {
        // 壳里托盘、全局快捷键、原生菜单三条路都往同一个事件名发；id 前缀是
        // 区分"谁触发的"的唯一线索，混用会让日志与前端判断一起失效。
        for gs in global_shortcut_plan(false) {
            assert!(gs.id.starts_with("global."), "{}", gs.id);
        }
    }

    /// 跨语言契约（同 `the_frontend_routes_exactly_the_ids_the_shell_declares` 的路子）：
    /// 设置页那张快捷键表里"需要全局快捷键能力"的行，必须与真正注册的集合一模一样，
    /// 而且**界面上写出来的组合键要和实际注册的那个字面一致**。
    /// 少一行 = 用户不知道有这功能；多一行 = 界面摆着一个按了没反应的组合键 ——
    /// 正是此前 `global_shortcuts: true` 那次假声明留下的坑。
    #[test]
    fn the_settings_page_shows_exactly_the_registered_global_shortcuts() {
        let spec = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../apps/desktop/src/platform/caps.ts"
        );
        let src = std::fs::read_to_string(spec).unwrap_or_else(|e| panic!("读不到 {spec}：{e}"));
        let rows = src
            .lines()
            .filter(|l| l.contains("requires: 'globalShortcuts'"))
            .count();
        let plan = global_shortcut_plan(false);
        assert_eq!(
            rows,
            plan.len(),
            "设置页里带全局快捷键标记的行数是 {rows}，真正注册的是 {}",
            plan.len()
        );
        for gs in &plan {
            let marker = format!("id: '{}'", gs.id);
            let row = src
                .lines()
                .find(|l| l.contains(&marker) && l.contains("requires: 'globalShortcuts'"))
                .unwrap_or_else(|| panic!("设置页里没有 {} 这一行 —— 注册了却看不见", gs.id));
            let key = gs.accel.split('+').next_back().unwrap();
            let win = quoted_list(row, "keys:");
            let mac = quoted_list(row, "macKeys:");
            // 三个都要：键面、修饰键、以及"这条确实标了需要全局快捷键能力"
            assert!(
                win.last().map(|s| s == key).unwrap_or(false),
                "{} 在 Windows 上显示的按键不是 {}（{row}）",
                gs.id,
                key
            );
            assert!(
                mac.last().map(|s| s == key).unwrap_or(false),
                "{} 在 mac 上显示的按键不是 {}（{row}）",
                gs.id,
                key
            );
            for (list, want, on) in [(&win, "Ctrl", "win"), (&mac, "⌘", "mac")] {
                assert!(
                    list.iter().any(|s| s == want),
                    "{} 在 {on} 上没显示修饰键 {want}（{row}）",
                    gs.id
                );
            }
            assert!(
                win.iter().any(|s| s == "Alt") && mac.iter().any(|s| s == "⌥"),
                "{} 必须显示 Alt/⌥ —— 少一个修饰键就是别的应用的键（{row}）",
                gs.id
            );
        }
    }

    /// 取 `keys: ['Ctrl', 'Alt', 'N']` 这种字段里的字符串数组。
    fn quoted_list(line: &str, field: &str) -> Vec<String> {
        let Some(at) = line.find(field) else {
            return vec![];
        };
        let after = &line[at + field.len()..];
        let Some(open) = after.find('[') else {
            return vec![];
        };
        let close = after[open..].find(']').unwrap_or(after.len() - open);
        after[open + 1..open + close]
            .split(',')
            .map(|s| s.trim().trim_matches('\'').trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }

    #[test]
    fn mac_and_windows_get_the_same_items_with_their_own_accelerators() {
        let m = menu_plan(true);
        let w = menu_plan(false);
        assert_eq!(m.ids(), w.ids(), "两个平台的菜单项必须一致，只差修饰键");
        let accel = |plan: &MenuPlan, id: &str| {
            plan.groups
                .iter()
                .flat_map(|(_, v)| v.iter())
                .find(|i| i.id == id)
                .unwrap()
                .accel
        };
        assert_eq!(accel(&m, "note.new"), Some("Cmd+N"));
        assert_eq!(accel(&w, "note.new"), Some("Ctrl+N"));
        for (id, suffix) in [
            ("note.new", "N"),
            ("note.search", "F"),
            ("sync.now", "S"),
            ("view.settings", ","),
        ] {
            assert!(
                accel(&m, id).unwrap().starts_with("Cmd")
                    && accel(&m, id).unwrap().ends_with(suffix),
                "mac {id}"
            );
            assert!(
                accel(&w, id).unwrap().starts_with("Ctrl")
                    && accel(&w, id).unwrap().ends_with(suffix),
                "win {id}"
            );
        }
    }

    #[test]
    fn ids_are_unique_because_the_frontend_routes_on_them() {
        let ids = menu_plan(false).ids();
        let mut sorted = ids.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            ids.len(),
            "重复的 id 会让两个菜单项抢同一条前端路由"
        );
    }

    #[test]
    fn only_conflicts_and_failed_syncs_escalate_to_a_system_notification() {
        assert!(notice_for(&BusEvent::Conflict {
            conflict_id: 1,
            note_title: "甲".into()
        })
        .is_some());
        assert!(notice_for(&BusEvent::Sync {
            badge: Badge::Failed,
            progress: None,
            error_code: Some("net.timeout".into())
        })
        .is_some());
        // 这三类必须安静：进度每轮都有、Toast 已经在界面里、NotesChanged 是自家写库的回声
        assert!(notice_for(&BusEvent::Sync {
            badge: Badge::Syncing,
            progress: None,
            error_code: None
        })
        .is_none());
        assert!(notice_for(&BusEvent::Sync {
            badge: Badge::Synced,
            progress: None,
            error_code: None
        })
        .is_none());
        assert!(notice_for(&BusEvent::NotesChanged {
            ids: vec!["x".into()]
        })
        .is_none());
        assert!(notice_for(&BusEvent::Toast {
            message_key: "x".into(),
            level: "warn".into()
        })
        .is_none());
    }

    #[test]
    fn an_error_badge_without_a_code_does_not_nag() {
        // 没带原因的错误态弹出去只会得到一句"失败了"，用户什么也做不了
        assert!(notice_for(&BusEvent::Sync {
            badge: Badge::Failed,
            progress: None,
            error_code: None
        })
        .is_none());
    }

    #[test]
    fn the_conflict_notification_names_the_note_so_it_is_actionable() {
        let n = notice_for(&BusEvent::Conflict {
            conflict_id: 7,
            note_title: "报销单".into(),
        })
        .unwrap();
        assert!(
            n.body.contains("报销单"),
            "通知里必须点出是哪一条：{}",
            n.body
        );
    }

    /// 跨语言契约：壳声明的菜单 id 与前端路由表必须**一模一样**。
    ///
    /// 为什么在 Rust 侧读 TS 而不是反过来：vitest 跑在 vite 的模块图里，读仓库外的
    /// 文件要被 `server.fs.allow` 放行（试过，配置项本身成了新的漂移源）；而这里
    /// `std::fs` 就够了。两边各自看都是好的，中间那条边却最容易断 ——
    /// 壳加一项"最近删除"忘了挂号，用户点了什么也不会发生。
    #[test]
    fn the_frontend_routes_exactly_the_ids_the_shell_declares() {
        let spec = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../apps/desktop/src/platform/menu.spec.ts"
        );
        let src = std::fs::read_to_string(spec)
            .unwrap_or_else(|e| panic!("读不到前端路由表 {spec}：{e}"));
        let routed: std::collections::BTreeSet<String> = src
            .lines()
            .filter_map(|line| {
                // 形如：      'note.new': 'newNote',   （冒号后有空格，所以不能按空白切 token）
                let (before, after) = line.split_once("': '")?;
                let id = before.split('\'').next_back()?;
                let dotted = id.bytes().all(|b| b.is_ascii_lowercase() || b == b'.');
                (id.contains('.') && dotted && after.starts_with(|c: char| c.is_alphabetic()))
                    .then_some(id.to_string())
            })
            .collect();
        let declared: std::collections::BTreeSet<String> = menu_plan(false)
            .ids()
            .into_iter()
            .map(str::to_string)
            .collect();
        assert!(!declared.is_empty());
        assert_eq!(
            declared, routed,
            "壳的菜单 id 与前端路由表不一致（左边=点了没反应，右边=永远不触发）"
        );
    }
}
