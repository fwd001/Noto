<script setup lang="ts">
/** 侧栏：文件夹树 + 新建/入口 + 同步徽标 + 导航。 */
import { computed, ref } from 'vue';
import FolderTree from './FolderTree.vue';
import AppDialog from './ui/AppDialog.vue';
import SyncBadge from './SyncBadge.vue';
import { useFolderStore } from '../stores/folders';
import { useNoteStore } from '../stores/notes';
import { useSettingsStore } from '../stores/settings';
import { useShellStore } from '../stores/shell';
import { useConflictStore } from '../stores/conflicts';
import { drawsTitleBar } from '../platform/caps';
import { t, messageFor } from '../i18n';
import { formatBytes, formatNumber } from '../util/format';
import AppIcon from './ui/AppIcon.vue';

const folders = useFolderStore();
const notes = useNoteStore();
const settings = useSettingsStore();
const shell = useShellStore();
const conflicts = useConflictStore();

const newFolderOpen = ref(false);
const newName = ref('');

const trashCount = computed(() => settings.stats?.notesInTrash ?? 0);
const allCount = computed(() => settings.stats?.notes ?? 0);
const errorText = computed(() => messageFor(folders.errorKey ?? 'error.fallback'));

/** §3.2：底部那一行要同时给"设置入口"和"本地库读数"—— 让人知道东西在哪、有多大。 */
const libraryReadout = computed(() => {
  const s = settings.stats;
  if (!s || typeof s.notes !== 'number' || typeof s.folders !== 'number') return null;
  return t('sidebar.libraryReadout', {
    notes: formatNumber(s.notes),
    folders: formatNumber(s.folders),
    size: formatBytes(s.dbBytes),
  });
});

function openNewFolder(): void {
  newName.value = '';
  newFolderOpen.value = true;
}

/**
 * 新建一律落在**根**：以前这里取"当前打开的那个文件夹"当父级，
 * 于是从子文件夹里点＋会长出孙层 —— 而产品口径已经收成"只有一层文件夹用于归档"。
 */
async function commitNewFolder(): Promise<void> {
  const name = newName.value.trim();
  newFolderOpen.value = false;
  newName.value = '';
  if (name.length === 0) return;
  const created = await folders.create(null, name);
  if (created) await notes.setMode({ kind: 'folder', folderId: created.id });
}

function selectAll(): void {
  void notes.setMode({ kind: 'all' });
  // 库内导航必须能把人从设置/冲突页、以及窄屏的编辑器那一面带回列表：
  // 以前只改 mode，停在设置页时点「全部笔记」看着像死的。
  shell.openList();
}

function selectTrash(): void {
  void notes.setMode({ kind: 'trash' });
  shell.openList();
}
</script>

<template>
  <aside class="pane pane--sidebar" :aria-label="t('sidebar.folders')" data-testid="sidebar">
    <div class="pane-header">
      <span class="pane-title">{{ t('app.name') }}</span>
      <!-- 收/开用**同一颗**按钮：三栏时它是"收起"，两栏/单栏时这块面板是抽屉，
           它就是抽屉里那个"关掉"。`toggleSidebar()` 现在按布局落到对的位上。
           有自绘标题栏时不画 —— 那颗把手归标题栏，画两颗就是"折起下面还有一层折起"。 -->
      <button v-if="!drawsTitleBar(settings.caps)" type="button" class="btn btn--quiet btn--icon" :aria-label="t(shell.sidebarShown ? 'sidebar.collapse' : 'sidebar.expand')" data-testid="sidebar-collapse" @click="shell.toggleSidebar()">
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

    <!--同步状态。
         这一块原先只有上下两条分隔线、**没有小节标题**，于是在界面上
         像一个"从别处掉下来的悬空胶囊"（用户反馈"长度对不齐、层级乱"）。
         按 Notion / Obsidian 的通行做法，每个分区都带一个同样式的小标题，
         于是三块（导航 / 同步 / 文件夹）在视觉上是**同级**的，
         眼睛不需要靠"有没有边框"去猜层级。 -->
    <div class="side-sync">
      <div class="section-head section-head--sync">
        <span class="section-title">{{ t('sidebar.syncSection') }}</span>
      </div>
      <SyncBadge />
    </div>

    <div class="pane-body">
      <div class="section-head">
        <span class="section-title">{{ t('sidebar.folders') }}</span>
        <button type="button" class="btn btn--quiet btn--icon" :aria-label="t('sidebar.newFolder')" data-testid="new-folder" @click="openNewFolder"><AppIcon :size="18" name="plus" /></button>
      </div>

      <AppDialog
        :open="newFolderOpen"
        :title="t('sidebar.newFolder')"
        testid="new-folder-dialog"
        :confirm-disabled="newName.trim().length === 0"
        @close="newFolderOpen = false"
        @confirm="commitNewFolder"
      >
        <input
          v-model="newName"
          class="input"
          type="text"
          data-testid="new-folder-input"
          :aria-label="t('sidebar.newFolder')"
          :placeholder="t('sidebar.newFolder')"
          @keydown.enter.prevent="commitNewFolder"
        />
      </AppDialog>

      <FolderTree />
      <p v-if="folders.errorKey" class="side-error" role="alert">{{ errorText }}</p>
    </div>

    <div class="side-foot safe-bottom">
      <button type="button" class="nav-btn" :data-active="shell.view === 'settings' ? 'true' : 'false'" data-testid="nav-settings" @click="shell.goto('settings')">
        {{ t('settings.title') }}
      </button>
      <!-- §3.2：同一行给本地库读数（东西在哪、有多大）。没问到就整行不出现，不画"— · —"那种空壳。 -->
      <span v-if="libraryReadout" class="side-foot__readout" data-testid="library-readout">{{ libraryReadout }}</span>
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
  padding: var(--sp-3) var(--sp-3) var(--sp-1);
  min-height: var(--touch);
}

.section-title {
  font-size: var(--text-xs);
  letter-spacing: 0.08em;
  text-transform: uppercase;
  color: var(--mute);
  font-weight: 700;
}

.side-nav,
.side-sync,
.side-foot {
  padding: var(--sp-2) var(--sp-3);
  display: flex;
  flex-direction: column;
  gap: var(--sp-1);
  flex: 0 0 auto;
}

.side-sync {
  border-top: 1px solid var(--line);
}

/* 分区小标题（"同步"）。
   `.section-head` 原本给"文件夹"那块的用法是 `justify-content: space-between`
   + `min-height: var(--touch)`，因为它右边要放一个"＋"按钮。
   同步这块右边没东西，若沿用那套就会**左对齐但占满一整行高度**，
   看起来又是一块空地。所以单列一个修饰符：只留上半padding、去掉 min-height，
   让它紧贴上面的分隔线、下面是内容 —— 与"文件夹"那条标题的视觉重量对齐。 */
.section-head--sync {
  justify-content: flex-start;
  min-height: 0;
  padding: var(--sp-3) var(--sp-3) var(--sp-1);
}

.side-foot {
  border-top: 1px solid var(--line);
}

/* 读数那一行：弱化文字也要 ≥4.5:1（§5），所以用 --body 而不是 --mute。 */
.side-foot__readout {
  padding: 0 var(--sp-3);
  font-size: var(--text-xs);
  line-height: 1.5;
  color: var(--body);
}

.nav-btn {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--sp-2);
  min-height: var(--touch);
  padding: 0 var(--sp-3);
  border-radius: var(--r-card);
  color: var(--ink);
  text-align: left;
  cursor: pointer;
  font-size: var(--text-sm);
  font-weight: 600;
}

.nav-btn:hover {
  background: var(--hover);
}

.nav-btn[data-active='true'] {
  background: var(--accent-soft);
}

.nav-btn__count {
  font-size: var(--text-xs);
  color: var(--mute);
}

.nav-btn__count--warn {
  color: var(--danger);
}

.new-folder {
  padding: var(--sp-2) var(--sp-3);
  display: flex;
  flex-direction: column;
  gap: var(--sp-2);
}

.side-error {
  padding: var(--sp-2) var(--sp-3);
  color: var(--danger);
  font-size: var(--text-sm);
}
</style>
