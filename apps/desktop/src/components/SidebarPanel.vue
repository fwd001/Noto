<script setup lang="ts">
/** 侧栏：文件夹树 + 新建/入口 + 同步徽标 + 导航。 */
import { computed, nextTick, ref } from 'vue';
import FolderTree from './FolderTree.vue';
import SyncBadge from './SyncBadge.vue';
import { useFolderStore } from '../stores/folders';
import { useNoteStore } from '../stores/notes';
import { useSettingsStore } from '../stores/settings';
import { useShellStore } from '../stores/shell';
import { useConflictStore } from '../stores/conflicts';
import { t, messageFor } from '../i18n';

const folders = useFolderStore();
const notes = useNoteStore();
const settings = useSettingsStore();
const shell = useShellStore();
const conflicts = useConflictStore();

const showNewFolder = ref(false);
const newName = ref('');
const nameInput = ref<HTMLInputElement | null>(null);

const parentLabel = computed(() => (notes.mode.kind === 'folder' && notes.mode.folderId ? folders.nameOf(notes.mode.folderId) : t('sidebar.root')));
const trashCount = computed(() => settings.stats?.notesInTrash ?? 0);
const allCount = computed(() => settings.stats?.notes ?? 0);
const errorText = computed(() => messageFor(folders.errorKey ?? 'error.fallback'));

async function openNewFolder(): Promise<void> {
  showNewFolder.value = true;
  newName.value = '';
  await nextTick();
  nameInput.value?.focus();
}

async function commitNewFolder(): Promise<void> {
  const name = newName.value.trim();
  showNewFolder.value = false;
  newName.value = '';
  if (name.length === 0) return;
  const parentId = notes.mode.kind === 'folder' ? notes.mode.folderId : null;
  const created = await folders.create(parentId, name);
  if (created) await notes.setMode({ kind: 'folder', folderId: created.id });
}

function selectAll(): void {
  void notes.setMode({ kind: 'all' });
  // 库内导航必须能把人从设置/冲突页带回列表：以前只改 mode，
  // 停在设置页时点「全部笔记」看着像死的。
  shell.goto('workspace');
  shell.closeDrawer();
}

function selectTrash(): void {
  void notes.setMode({ kind: 'trash' });
  shell.goto('workspace');
  shell.closeDrawer();
}
</script>

<template>
  <aside class="pane pane--sidebar" :aria-label="t('sidebar.folders')" data-testid="sidebar">
    <div class="pane-header">
      <span class="pane-title">{{ t('app.name') }}</span>
      <button v-if="!shell.isCompact" type="button" class="btn btn--quiet btn--icon" :aria-label="t('sidebar.collapse')" data-testid="sidebar-collapse" @click="shell.toggleSidebar()">
        ⟨
      </button>
    </div>

    <div class="side-nav">
      <button type="button" class="nav-btn" :data-active="shell.view === 'workspace' && notes.mode.kind === 'all' ? 'true' : 'false'" data-testid="nav-all" @click="selectAll">
        <span>{{ t('sidebar.allNotes') }}</span>
        <span v-if="allCount" class="nav-btn__count">{{ allCount }}</span>
      </button>
      <button type="button" class="nav-btn" :data-active="shell.view === 'workspace' && notes.mode.kind === 'trash' ? 'true' : 'false'" data-testid="nav-trash" @click="selectTrash">
        <span>{{ t('sidebar.trash') }}</span>
        <span v-if="trashCount" class="nav-btn__count">{{ trashCount }}</span>
      </button>
      <button type="button" class="nav-btn" :data-active="shell.view === 'conflicts' ? 'true' : 'false'" data-testid="nav-conflicts" @click="shell.goto('conflicts')">
        <span>{{ t('conflict.title') }}</span>
        <span v-if="conflicts.count > 0" class="nav-btn__count nav-btn__count--warn">{{ t('conflict.count', { count: conflicts.count }) }}</span>
      </button>
    </div>

    <div class="side-sync">
      <SyncBadge />
    </div>

    <div class="pane-body">
      <div class="section-head">
        <span class="section-title">{{ t('sidebar.folders') }}</span>
        <button type="button" class="btn btn--quiet btn--icon" :aria-label="t('sidebar.newFolder')" data-testid="new-folder" @click="openNewFolder">＋</button>
      </div>

      <form v-if="showNewFolder" class="new-folder" @submit.prevent="commitNewFolder">
        <input ref="nameInput" v-model="newName" class="input" type="text" :aria-label="t('sidebar.newFolder')" :placeholder="t('sidebar.newFolder')" @keydown.escape.prevent="showNewFolder = false" />
        <p class="field-hint">{{ t('sidebar.newSubfolder') }}：{{ parentLabel }}</p>
        <div class="row">
          <button type="submit" class="btn btn--primary">{{ t('list.confirm') }}</button>
          <button type="button" class="btn btn--quiet" @click="showNewFolder = false">{{ t('list.cancel') }}</button>
        </div>
      </form>

      <FolderTree :depth="0" />
      <p v-if="folders.errorKey" class="side-error" role="alert">{{ errorText }}</p>
    </div>

    <div class="side-foot safe-bottom">
      <button type="button" class="nav-btn" :data-active="shell.view === 'settings' ? 'true' : 'false'" data-testid="nav-settings" @click="shell.goto('settings')">
        {{ t('settings.title') }}
      </button>
    </div>
  </aside>
</template>

<style scoped>
.section-head,
.section-title {
  display: flex;
  align-items: center;
}

.section-head {
  justify-content: space-between;
  padding: var(--space-3) var(--space-3) var(--space-1);
  min-height: var(--touch-min);
}

.section-title {
  font-size: var(--text-xs);
  letter-spacing: 0.08em;
  text-transform: uppercase;
  color: var(--text-muted);
  font-weight: 700;
}

.side-nav,
.side-sync,
.side-foot {
  padding: var(--space-2) var(--space-3);
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
  flex: 0 0 auto;
}

.side-sync {
  border-top: 1px solid var(--border-subtle);
  border-bottom: 1px solid var(--border-subtle);
}

.side-foot {
  border-top: 1px solid var(--border-subtle);
}

.nav-btn {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-2);
  min-height: var(--touch-min);
  padding: 0 var(--space-3);
  border-radius: var(--radius-2);
  color: var(--text-primary);
  text-align: left;
  cursor: pointer;
  font-size: var(--text-sm);
  font-weight: 600;
}

.nav-btn:hover {
  background: var(--bg-hover);
}

.nav-btn[data-active='true'] {
  background: var(--accent-soft);
}

.nav-btn__count {
  font-size: var(--text-xs);
  color: var(--text-muted);
}

.nav-btn__count--warn {
  color: var(--danger);
}

.new-folder {
  padding: var(--space-2) var(--space-3);
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
}

.side-error {
  padding: var(--space-2) var(--space-3);
  color: var(--danger);
  font-size: var(--text-sm);
}
</style>
