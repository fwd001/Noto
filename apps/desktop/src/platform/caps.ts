/**
 * 平台能力：一律按能力（PlatformCaps）渲染，不按机型分支。
 * 首选 host 提供的 caps；缺失时退化为本地探测结果（仅用于窗口外观，不参与任何业务判定）。
 */

export type BgTaskKind = 'none' | 'desktopTimer' | 'workManager' | 'bgAppRefresh';
export type WindowChrome = 'custom' | 'overlay' | 'system';

export interface PlatformCaps {
  tray: boolean;
  globalShortcuts: boolean;
  bgTask: BgTaskKind;
  shareSheet: boolean;
  keychain: 'credentialManager' | 'keychain' | 'keystore' | 'none';
  filePicker: 'native' | 'web';
  biometric: boolean;
  windowChrome: WindowChrome;
  safeArea: boolean;
  compactToolbar: boolean;
  transparency: boolean;
  dragAndDrop: boolean;
}

export const CAPS_COMMAND = 'platform_caps';

type OsKind = 'windows' | 'macos' | 'ios' | 'android' | 'other';

/** 仅在本模块内用于确定窗口外观与触摸尺寸，不做任何业务判定。 */
function inferOs(): OsKind {
  if (typeof navigator !== 'undefined') {
    const ua = navigator.userAgent ?? '';
    if (/iPhone|iPad|iPod/.test(ua) || (/Mac/.test(ua) && typeof document !== 'undefined' && 'ontouchstart' in document)) return 'ios';
    if (/Android/.test(ua)) return 'android';
    if (/Windows NT/.test(ua)) return 'windows';
    if (/Mac OS X/.test(ua)) return 'macos';
  }
  return 'other';
}

export function localCaps(): PlatformCaps {
  const os = inferOs();
  const mobile = os === 'ios' || os === 'android';
  // 兜底值也必须只说"壳里真的做了的事"：这里曾经报 tray/globalShortcuts/keychain 为可用，
  // 而桌面壳一行托盘或钥匙串代码都没有 —— 于是设置页摆出一个存了没人读的开关。
  const keychain: PlatformCaps['keychain'] = 'none';
  return {
    tray: false,
    globalShortcuts: false,
    bgTask: mobile ? (os === 'ios' ? 'bgAppRefresh' : 'workManager') : 'desktopTimer',
    shareSheet: os === 'ios' || os === 'android',
    keychain,
    filePicker: mobile ? 'web' : 'native',
    biometric: os === 'ios' || os === 'android' || os === 'macos',
    windowChrome: os === 'windows' ? 'custom' : os === 'macos' || os === 'ios' ? 'overlay' : 'system',
    safeArea: mobile,
    compactToolbar: mobile,
    // 透明效果（PLATFORM.md §观感 写的 Mica/Acrylic + "必须提供关闭开关"）**今天没有实现**：
    // 我们这边一个 call site 都没有（`window-vibrancy` 只是 tauri 的传递依赖），而 `no-transparency` 那个类
    // 也没有任何 CSS 消费它。以前这里报 true ⇒ 设置页摆出一颗"窗口透明效果"的勾，
    // 勾得动、存得下、就是没有任何视觉后果 —— 正是上面那句注释要消灭的形状（托盘/快捷键/钥匙串
    // 那三颗已经消灭过一次）。能力照实报 false，开关就不出现；等真做特效时连同 §观感 一起翻回来。
    transparency: false,
    dragAndDrop: !mobile,
  };
}

function pick<T>(value: unknown, fallback: T): T {
  return (typeof value === typeof fallback ? (value as T) : fallback) ?? fallback;
}

/** 归一化 host 返回的能力表：缺字段用本地推断补，绝不因为缺数据而不渲染功能。 */
export function normalizeCaps(raw: unknown): PlatformCaps {
  const base = localCaps();
  if (typeof raw !== 'object' || raw === null) return base;
  const value = raw as Record<string, unknown>;
  return {
    tray: pick(value.tray, base.tray),
    globalShortcuts: pick(value.globalShortcuts, base.globalShortcuts),
    bgTask: (value.bgTask === 'none' || value.bgTask === 'desktopTimer' || value.bgTask === 'workManager' || value.bgTask === 'bgAppRefresh'
      ? value.bgTask
      : base.bgTask) as BgTaskKind,
    shareSheet: pick(value.shareSheet, base.shareSheet),
    keychain: (['credentialManager', 'keychain', 'keystore', 'none'] as const).includes(value.keychain as PlatformCaps['keychain'])
      ? (value.keychain as PlatformCaps['keychain'])
      : base.keychain,
    filePicker: value.filePicker === 'web' ? 'web' : 'native',
    biometric: pick(value.biometric, base.biometric),
    windowChrome: (['custom', 'overlay', 'system'] as const).includes(value.windowChrome as WindowChrome)
      ? (value.windowChrome as WindowChrome)
      : base.windowChrome,
    safeArea: pick(value.safeArea, base.safeArea),
    compactToolbar: pick(value.compactToolbar, base.compactToolbar),
    transparency: pick(value.transparency, base.transparency),
    dragAndDrop: pick(value.dragAndDrop, base.dragAndDrop),
  };
}

/**
 * 点「关闭」是收进托盘还是真退出。
 *
 * 两个条件缺一不可，各自挡掉一种真出过的坏结果：
 *  - 只看开关：托盘没挂上（系统不让挂、构建缺图标）时窗口被隐藏，用户面对一个
 *    "关不掉也找不回"的进程 —— 全局快捷键也可能同时没注册上，没有任何入口能唤回它。
 *  - 只看托盘：违背那个开关默认关闭的语义（PLATFORM.md §7：笔记软件常驻对多数用户
 *    是噪音，P6 的默认取向就是"关"），等于替用户做了一个他没做的决定。
 */
export function shouldHideOnClose(caps: Pick<PlatformCaps, 'tray'>, prefs: { trayHint: boolean }): boolean {
  return prefs.trayHint === true && caps.tray === true;
}

export async function loadCaps(fetcher?: (name: string, args?: Record<string, unknown>) => Promise<unknown>): Promise<PlatformCaps> {
  if (!fetcher) return localCaps();
  try {
    return normalizeCaps(await fetcher(CAPS_COMMAND, {}));
  } catch {
    return localCaps();
  }
}

/** 快捷键说明表按能力过滤（托盘/全局快捷键在移动端不显示）。 */
export interface ShortcutEntry {
  id: string;
  keys: string[];
  macKeys: string[];
  labelKey: string;
  requires?: 'globalShortcuts' | 'tray';
}

export function shortcutsFor(caps: PlatformCaps): ShortcutEntry[] {
  return SHORTCUTS.filter((entry) => !entry.requires || caps[entry.requires] === true);
}

export const SHORTCUTS: ShortcutEntry[] = [
  { id: 'new-note', keys: ['Ctrl', 'N'], macKeys: ['⌘', 'N'], labelKey: 'list.newNote' },
  { id: 'search', keys: ['Ctrl', 'K'], macKeys: ['⌘', 'K'], labelKey: 'list.searchPlaceholder' },
  { id: 'collapse', keys: ['Ctrl', '\\'], macKeys: ['⌘', '\\'], labelKey: 'sidebar.collapse' },
  { id: 'pin', keys: ['Ctrl', 'P'], macKeys: ['⌘', 'P'], labelKey: 'list.pin' },
  { id: 'delete', keys: ['Del'], macKeys: ['⌘', '⌫'], labelKey: 'sidebar.deleteFolder' },
  { id: 'bold', keys: ['Ctrl', 'B'], macKeys: ['⌘', 'B'], labelKey: 'tb.bold' },
  { id: 'italic', keys: ['Ctrl', 'I'], macKeys: ['⌘', 'I'], labelKey: 'tb.italic' },
  { id: 'underline', keys: ['Ctrl', 'U'], macKeys: ['⌘', 'U'], labelKey: 'tb.underline' },
  { id: 'heading', keys: ['Ctrl', '1/2/3'], macKeys: ['⌘', '1/2/3'], labelKey: 'editor.blockHeading' },
  { id: 'checklist', keys: ['Ctrl', 'Enter'], macKeys: ['⌘', 'Enter'], labelKey: 'tb.checklist' },
  { id: 'attach', keys: ['Ctrl', 'Shift', 'F'], macKeys: ['⌘', '⇧', 'F'], labelKey: 'tb.attach' },
  { id: 'sync', keys: ['F5'], macKeys: ['⌘', 'R'], labelKey: 'sync.syncNow' },
  { id: 'conflicts', keys: ['Ctrl', 'Shift', 'C'], macKeys: ['⌘', '⇧', 'C'], labelKey: 'conflict.title' },
  { id: 'tray', keys: ['—'], macKeys: ['—'], labelKey: 'settings.trayHint', requires: 'tray' },
  // 下面两行**不是**应用内快捷键，是系统级的（窗口收在托盘里、甚至焦点在别的应用上也生效）。
  // id 与组合键必须和 Rust 侧 `notera_host::platform::global_shortcut_plan()` 一字不差 ——
  // 那条跨语言测试（the_settings_page_shows_exactly_the_registered_global_shortcuts）就是钉这个的：
  // 多一行就是界面上摆一个按了没反应的键，少一行就是注册了却没人知道。
  { id: 'global.quick-note', keys: ['Ctrl', 'Alt', 'N'], macKeys: ['⌘', '⌥', 'N'], labelKey: 'settings.gsQuickNote', requires: 'globalShortcuts' },
  { id: 'global.toggle-window', keys: ['Ctrl', 'Alt', 'I'], macKeys: ['⌘', '⌥', 'I'], labelKey: 'settings.gsToggleWindow', requires: 'globalShortcuts' },
];
