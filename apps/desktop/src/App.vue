<script setup lang="ts">
/**
 * 外壳：首帧只读本地数据（不做任何网络等待），后台再接账户/统计/同步。
 * 事件回流在这里分发到各 store；平台差异只看能力，不看 UA。
 */
import { computed, onBeforeUnmount, onMounted, ref } from 'vue';
import TitleBar from './components/TitleBar.vue';
import BannerHost from './components/BannerHost.vue';
import ToastHost from './components/ToastHost.vue';
import SidebarPanel from './components/SidebarPanel.vue';
import WorkspaceView from './views/WorkspaceView.vue';
import SettingsView from './views/SettingsView.vue';
import ConflictsView from './views/ConflictsView.vue';
import { callCommand, inTauri, onLinkChange, onMenuAction, onUiEvent, probeLink } from './api/bridge';
import type { UiEvent } from './api/types';
import { loadCaps } from './platform/caps';
import { dispatchMenu } from './platform/menu';
import { LINK_PROBE_INTERVAL_MS } from './util/timing';
import { useConflictStore } from './stores/conflicts';
import { useEditorStore } from './stores/editor';
import { useFolderStore } from './stores/folders';
import { useNoteStore } from './stores/notes';
import { useSettingsStore } from './stores/settings';
import { useShellStore } from './stores/shell';
import { useSyncStore } from './stores/sync';
import { useToastStore } from './stores/toasts';
import { t } from './i18n';
import AppIcon from './components/ui/AppIcon.vue';
import { SYNC_ICONS, type IconName } from './components/ui/icons';

const shell = useShellStore();
const sync = useSyncStore();
const notes = useNoteStore();
const folders = useFolderStore();
const editor = useEditorStore();
const settings = useSettingsStore();
const conflicts = useConflictStore();
const toasts = useToastStore();

const booted = ref(false);

const shellAttrs = computed(() => ({
  layout: shell.layout,
  compact: String(shell.isCompact),
  pane: shell.mobilePane,
  sidebar: shell.sidebarOpen ? 'open' : 'collapsed',
  drawer: shell.drawerTarget ?? 'none',
  theme: settings.resolvedTheme,
}));

/**
 * 底栏那颗同步 tab 读的是**状态**，不是动词（§3.5 的移动端后果 + §4.3 的"五种事实一句不许少"）：
 * 窄屏下侧栏是抽屉，`.syncbar` 平时根本不在屏幕上 ⇒ 底栏是手机上**唯一**能说清
 * "此刻到底有没有在同步"的地方。写死成"立即同步"就等于把这五种事实从手机上拿掉了。
 */
const dockSyncIcon = computed<IconName>(() => SYNC_ICONS[sync.shownBadge] ?? 'sync-failed');

function handleEvent(event: UiEvent): void {
  if (event.kind === 'sync') {
    sync.applySignal({
      badge: event.badge,
      ...(event.progress ? { progress: event.progress } : {}),
      ...(event.errorCode ? { errorCode: event.errorCode } : {}),
      ...(event.messageKey ? { messageKey: event.messageKey } : {}),
    });
    return;
  }
  if (event.kind === 'notes-changed') {
    void notes.invalidate(event.ids ?? []);
    if (!editor.dirty && editor.noteId && (event.ids ?? []).includes(editor.noteId)) void editor.reloadRemote();
    return;
  }
  if (event.kind === 'conflict') {
    conflicts.noteNewConflict(event.conflictId);
    toasts.push('error.conflict_needs_attention', 'warn');
    return;
  }
  if (event.kind === 'toast') {
    toasts.push(event.messageKey, event.level ?? 'info');
  }
}

function newNote(): void {
  const folderId = notes.mode.kind === 'folder' ? notes.mode.folderId : null;
  void notes.create(folderId);
  shell.openEditor();
}

function trashCurrent(): void {
  const id = notes.selectedId;
  if (id && !notes.inTrash) void notes.moveToTrash(id);
}

function pinCurrent(): void {
  const id = notes.selectedId;
  const row = notes.rowById(id);
  if (id && row) void notes.setPinned(id, row.pinned !== true);
}

function focusSearch(): void {
  // 搜索框在列表那一面：窄屏如果停在编辑器上，光 goto('workspace') 到不了输入框
  shell.openList();
  const input = document.querySelector<HTMLInputElement>('[data-testid="search-input"]');
  input?.focus();
  input?.select();
}

function cycleFocus(): void {
  const targets = ['[data-testid="sidebar"]', '[data-testid="search-input"]', '[data-testid="editor-doc"]'];
  const active = document.activeElement;
  let next = 0;
  for (let i = 0; i < targets.length; i += 1) {
    const element = document.querySelector(targets[i]);
    if (element && (element === active || element.contains(active))) next = (i + 1) % targets.length;
  }
  const element = document.querySelector<HTMLElement>(targets[next] ?? targets[0]);
  const focusable = element?.querySelector<HTMLElement>('button, input, [contenteditable="true"], [tabindex]') ?? element;
  focusable?.focus();
}

function onGlobalKeydown(event: KeyboardEvent): void {
  const target = event.target as HTMLElement | null;
  const typing =
    target !== null &&
    (target.isContentEditable === true || ['INPUT', 'TEXTAREA', 'SELECT'].includes(target.tagName));
  const mod = event.ctrlKey || event.metaKey;

  if (mod && !event.altKey) {
    const key = event.key.toLowerCase();
    if (key === 'n') {
      event.preventDefault();
      newNote();
      return;
    }
    if (key === 'k' || (key === 'f' && !event.shiftKey)) {
      event.preventDefault();
      focusSearch();
      return;
    }
    if (key === '\\') {
      event.preventDefault();
      shell.toggleSidebar();
      return;
    }
    if (key === 'p') {
      // 这一条**不带** `!typing` 守卫（0.0.54 改的）：设置页那张表写的是「Ctrl+P 固定」，
      // 而用户最常要固定的正是**手上正开着、焦点在正文里**的那一篇 —— 带着守卫时那一刻按下去
      // 什么都不发生（表里那句话在最 common 的状态下不成立），更坏的是这个键位没被接住，
      // 就交给 WebView2 的"打印"弹窗口。同处的 `Delete`/`Backspace` 必须留守卫：
      // 那两个在正文里是删字符，不打断打字才有道理。
      event.preventDefault();
      pinCurrent();
      return;
    }
    if (key === 'c' && event.shiftKey) {
      event.preventDefault();
      shell.goto('conflicts');
      void conflicts.load();
      return;
    }
    if (key === 'f' && event.shiftKey) {
      event.preventDefault();
      editor.requestAttach('file');
      return;
    }
    if (key === 'backspace' && !typing) {
      event.preventDefault();
      trashCurrent();
      return;
    }
  }

  if (event.key === 'F5') {
    event.preventDefault();
    void sync.syncNow();
    return;
  }
  if (event.key === 'Delete' && !typing) {
    event.preventDefault();
    trashCurrent();
    return;
  }
  if (event.key === 'F6') {
    event.preventDefault();
    cycleFocus();
    return;
  }
  if (event.altKey && event.key === 'ArrowLeft') {
    if (shell.back()) event.preventDefault();
    return;
  }
  if (event.key === 'Escape') {
    // 抽屉优先：抽屉开着时 Esc 只关抽屉，不顺手把整页带走（否则在抽屉里按一次Esc
    // 会连带退回主界面，那是"按一下却走了两格"）。
    if (shell.drawerTarget !== null) {
      shell.closeDrawer();
      return;
    }
    // 其余交给 `back()`：它在设置页/冲突页回到工作区，在窄屏编辑器回到列表，
    // 已经在工作区且无抽屉时返回 false（= 没有可退的，不抢占按键）。
    // 此前Esc 只认抽屉，于是"设置页按 Esc 没反应"—— 返回键`‹` 与 Alt+← 都在，
    // 唯独键盘用户最习惯的那一键是死的。
    if (shell.back()) event.preventDefault();
  }
}

let stopEvents: (() => void) | null = null;
let stopMenu: (() => void) | null = null;
let stopViewport: (() => void) | null = null;
let stopSystemTheme: (() => void) | null = null;
let stopLinkEvents: (() => void) | null = null;
let probeTimer: number | null = null;

async function boot(): Promise<void> {
  // 首帧路径：只取本地数据；任何网络等待都不允许出现在这里（PLATFORM §3）
  await Promise.all([folders.load(), notes.load()]);
  booted.value = true;
  settings.applyThemeToDocument();

  // 后台并行：能力、账户、统计、分歧、同步
  stopLinkEvents = onLinkChange((state) => sync.setLink(state));
  sync.setLink(await probeLink());
  probeTimer = window.setInterval(() => {
    if (inTauri()) return;
    void probeLink().then((state) => sync.setLink(state));
  }, LINK_PROBE_INTERVAL_MS);

  void loadCaps(callCommand).then((caps) => settings.setCaps(caps));
  // 偏好从库里读回来（§6 第 6 格）。放在后台那一组里是刻意的：首帧已经按本机那份画过了，
  // 这一发只是把"只存在 WebView profile 里"升级成"跟着本地库走"，不该排在首帧路径上。
  void settings.hydratePrefs();
  void settings.loadAccount();
  void settings.loadStats();
  void conflicts.load();

  // **只在真的配了、且用户开着同步时才自动同步。**
  //
  // 原来这里是**无条件** `sync.syncNow()`（用户反馈"我没设WebDAV，它却一直转"的直接原因）。
  // ⚠ 别把这条想成"核心回了 no_account 没人接" —— 实测 `sync_now` **从不回错**
  // （只做 `dirty_ticks += 1` 然后 HTTP 200 `null`），而没配账户时调度器压根没 spawn，
  // 所以没有任何事件来带走 `syncing`：那一转就是**永久**的。守位置的地方在门口，不在 catch 里。
  //
  // 改成以配置为准：**没配就不动**。这是"安装后应该是干净的"这条底线 ——
  // 用户装一个笔记应用，不该看到一个"正在把你的笔记传到某处"的指示。
  //
  // 顺序有讲究：`loadAccount()` 本身是异步的，所以要**等它落地**再判配置。
  // 若不 await 就去读，`syncActive` 还是 false，首轮同步被跳过 ⇒ 用户配好了却不同步。
  //
  // 判的是 `syncActive` 而**不是** `hasAccount`：后者只说"配过"，前者才说"用户要它跑"。
  // 拿 hasAccount 当门，用户关了「启用同步」之后启动照样自动同步一遍 ——
  // 那是"以我的设置为主"这条要求被当面绕过。
  void settings
    .loadAccount()
    .then(() => {
      if (settings.syncActive) void sync.syncNow();
      else sync.markNoAccount();
    });
}

onMounted(() => {
  stopViewport = shell.observeViewport();
  stopSystemTheme = settings.trackSystemTheme();
  stopEvents = onUiEvent(handleEvent);
  // 原生菜单与键盘快捷键走的是同一批动作：菜单只是同一入口的另一层外壳。
  stopMenu = onMenuAction((id) => {
    dispatchMenu(id, {
      newNote,
      focusSearch,
      syncNow: () => void sync.syncNow(),
      gotoConflicts: () => shell.goto('conflicts'),
      gotoTrash: () => void notes.setMode({ kind: 'trash' }),
      gotoSettings: () => shell.goto('settings'),
    });
  });
  window.addEventListener('keydown', onGlobalKeydown);
  window.addEventListener('beforeunload', () => void editor.flush());
  void boot();
});

onBeforeUnmount(() => {
  stopEvents?.();
  stopMenu?.();
  stopViewport?.();
  stopSystemTheme?.();
  stopLinkEvents?.();
  if (probeTimer !== null) window.clearInterval(probeTimer);
  window.removeEventListener('keydown', onGlobalKeydown);
});
</script>

<template>
  <div
    class="app-shell"
    :class="{ 'reduced-motion': shell.reducedMotion }"
    :data-layout="shellAttrs.layout" :data-compact="shellAttrs.compact" :data-pane="shellAttrs.pane" :data-sidebar="shellAttrs.sidebar" :data-drawer="shellAttrs.drawer" :data-theme="shellAttrs.theme">
    <a class="skip-link" href="#note-search" @click.prevent="focusSearch">{{ t('a11y.skipToSearch') }}</a>

    <TitleBar />
    <BannerHost />

    <div class="app-body">
      <!-- 侧栏在窄屏是抽屉：以前"给不给看"只看 `view === 'workspace'`，于是停在设置页点「菜单」
           会把遮罩打开、侧栏却还藏着 ⇒ 一块黑幕 + 没有第二条路回笔记列表（§6 的"无法返回"）。
           抽屉既然被点名要开，就让它真的开。 -->
      <SidebarPanel v-show="shell.view === 'workspace' || shell.drawerTarget === 'sidebar' || !shell.isCompact" />
      <button
        v-if="shell.drawerTarget !== null"
        type="button"
        class="scrim"
        :aria-label="t('state.dismiss')"
        data-testid="scrim"
        @click="shell.closeDrawer()"
      />

      <WorkspaceView v-if="shell.view === 'workspace'" />
      <SettingsView v-else-if="shell.view === 'settings'" />
      <ConflictsView v-else />
    </div>

    <!-- §3.5 移动端底部胶囊栏：浮在内容之上、不参与文档流；tab 56×52、主按钮 116×52 实心 --ink；
         图标 20、标签 10px；外层左右 12 / 底部 20（安全区内），内容区留 ≥84 的 padding-bottom。 -->
    <nav class="dock" :aria-label="t('mobile.menu')" data-testid="dock">
      <button type="button" class="dock__tab" data-testid="mobile-sidebar" @click="shell.openDrawer('sidebar')">
        <AppIcon name="menu" />
        <span class="dock__label">{{ t('mobile.menu') }}</span>
      </button>
      <button type="button" class="dock__tab dock__tab--primary" data-testid="mobile-new" @click="newNote">
        <AppIcon name="plus" />
        <span class="dock__label">{{ t('list.newNote') }}</span>
      </button>
      <button type="button" class="dock__tab" :data-badge="sync.badge" data-testid="mobile-sync" @click="sync.syncNow()">
        <AppIcon class="dock__glyph" :name="dockSyncIcon" :data-spin="sync.badge === 'syncing' ? 'true' : 'false'" />
        <span class="dock__label">{{ sync.label }}</span>
      </button>
      <button type="button" class="dock__tab" data-testid="mobile-settings" @click="shell.goto('settings')">
        <AppIcon name="settings" />
        <span class="dock__label">{{ t('settings.title') }}</span>
      </button>
    </nav>

    <p v-if="!booted" class="boot-hint" role="status">{{ t('state.boot') }}</p>

    <ToastHost />
  </div>
</template>

<style scoped>
.boot-hint {
  position: fixed;
  left: var(--sp-3);
  bottom: var(--sp-3);
  z-index: 60;
  padding: var(--sp-2) var(--sp-3);
  border-radius: var(--r-chip);
  background: var(--canvas);
  border: 1px solid var(--line);
  color: var(--body);
  font-size: var(--text-xs);
}
</style>
