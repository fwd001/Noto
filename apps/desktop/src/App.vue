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
import { callCommand, inTauri, onLinkChange, onUiEvent, probeLink } from './api/bridge';
import type { UiEvent } from './api/types';
import { loadCaps } from './platform/caps';
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
  shell.goto('workspace');
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
    if (key === 'p' && !typing) {
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
  if (event.key === 'Escape' && shell.drawerTarget !== null) {
    shell.closeDrawer();
  }
}

let stopEvents: (() => void) | null = null;
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
  void settings.loadAccount();
  void settings.loadStats();
  void conflicts.load();
  void sync.syncNow();
}

onMounted(() => {
  stopViewport = shell.observeViewport();
  stopSystemTheme = settings.trackSystemTheme();
  stopEvents = onUiEvent(handleEvent);
  window.addEventListener('keydown', onGlobalKeydown);
  window.addEventListener('beforeunload', () => void editor.flush());
  void boot();
});

onBeforeUnmount(() => {
  stopEvents?.();
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
      <SidebarPanel v-show="shell.view === 'workspace' || !shell.isCompact" />
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

    <nav class="mobile-bar" :aria-label="t('mobile.menu')">
      <button type="button" class="btn btn--quiet" data-testid="mobile-sidebar" @click="shell.openDrawer('sidebar')">{{ t('mobile.menu') }}</button>
      <button type="button" class="btn btn--quiet" data-testid="mobile-new" @click="newNote">{{ t('list.newNote') }}</button>
      <button type="button" class="btn btn--quiet" data-testid="mobile-sync" @click="sync.syncNow()">{{ t('sync.syncNow') }}</button>
      <button type="button" class="btn btn--quiet" data-testid="mobile-settings" @click="shell.goto('settings')">{{ t('settings.title') }}</button>
    </nav>

    <p v-if="!booted" class="boot-hint" role="status">{{ t('state.boot') }}</p>

    <ToastHost />
  </div>
</template>

<style scoped>
.boot-hint {
  position: fixed;
  left: var(--space-3);
  bottom: var(--space-3);
  z-index: 60;
  padding: var(--space-2) var(--space-3);
  border-radius: var(--radius-pill);
  background: var(--bg-raised);
  border: 1px solid var(--border-subtle);
  color: var(--text-secondary);
  font-size: var(--text-xs);
}
</style>
