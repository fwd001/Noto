<script setup lang="ts">
/**
 * 文件夹列表：**一层，平铺**。
 *
 * 产品口径（用户原话）：「我们应该没有子文件夹这种逻辑，就是只有一层的文件夹可以归档」。
 * 所以这里不再有缩进、不再有"在这下面新建"、也不再有"移动到某个父级" ——
 * 那三样都是层级 UI，留着就等于口径没落地。
 *
 * 为什么不直接忽略 `parentId`：库里可能已经有历史嵌套（本机开发数据就有），
 * 只渲染 depth 0 会让那些子层**连同里面的笔记一起消失**。所以取的是
 * "把整片森林摊平成一排"这一种：一个都不少，只是不再表达层级。
 * 摊平后名字用路径（`父 / 子`）显示，历史子层还认得出来。
 *
 * 删除不级联删笔记（由本地核心保证：笔记移入默认本，只给文件夹本身写一条墓碑）。
 */
import { computed, nextTick, ref } from 'vue';
import type { ComponentPublicInstance } from 'vue';
import AppPopover from './ui/AppPopover.vue';
import { useFolderStore } from '../stores/folders';
import { useNoteStore } from '../stores/notes';
import { useShellStore } from '../stores/shell';
import { t } from '../i18n';
import type { FolderNode } from '../api/types';

const folders = useFolderStore();
const notes = useNoteStore();
const shell = useShellStore();

const editingId = ref<string | null>(null);
const nameDraft = ref('');
const inputEl = ref<HTMLInputElement | null>(null);

const rows = computed(() =>
  [...folders.flat]
    .map((entry) => ({ node: entry.node, label: entry.path.length > 1 ? entry.path.join(' / ') : entry.node.name }))
    .sort((a, b) => a.label.localeCompare(b.label, 'zh-CN')),
);

/**
 * 核心对 `systemKind` 非空的文件夹一律拒绝改名/移动/删除（`assert_folder_writable`）。
 * 按钮的可见性跟着核心的判据走，而不是另立一套"看起来像默认本"的猜测
 * —— "默认"只是用户可改的显示名。
 */
function isSystem(node: FolderNode): boolean {
  return typeof node.systemKind === 'string' && node.systemKind.length > 0;
}

async function beginRename(id: string, current: string): Promise<void> {
  editingId.value = id;
  nameDraft.value = current;
  await nextTick();
  inputEl.value?.focus();
  inputEl.value?.select();
}

async function commitRename(id: string): Promise<void> {
  if (editingId.value !== id) return;
  const name = nameDraft.value.trim();
  editingId.value = null;
  if (name.length === 0) return;
  await folders.rename(id, name);
}

async function removeFolder(id: string): Promise<void> {
  await folders.remove(id);
  if (notes.mode.kind === 'folder' && notes.mode.folderId === id) await notes.setMode({ kind: 'all' });
  else await notes.load();
}

function openFolder(id: string | null): void {
  void notes.setMode(id === null ? { kind: 'all' } : { kind: 'folder', folderId: id });
  // 点文件夹也是"回列表"的动作：停在设置页时它以前只改 mode，界面却一动不动；
  // 窄屏停在编辑器那一面时同理（落点收在 shell.openList 一处）。
  shell.openList();
}

function isActive(id: string | null): boolean {
  if (shell.view !== 'workspace') return false;
  if (notes.mode.kind !== 'folder') return false;
  return notes.mode.folderId === id;
}

function rowRef(el: Element | ComponentPublicInstance | null): void {
  if (el instanceof HTMLInputElement) inputEl.value = el;
}
</script>

<template>
  <ul class="tree" role="list" data-testid="folder-list">
    <li v-for="row in rows" :key="row.node.id" class="tree__item">
      <div class="tree__row" :data-active="isActive(row.node.id) ? 'true' : 'false'" :data-depth="String(row.node.parentId ? 1 : 0)" data-testid="folder-row">
        <button type="button" class="tree__name" :data-testid="`folder-${row.node.id}`" @click="openFolder(row.node.id)">
          <span class="tree__label" :title="row.label">{{ row.label }}</span>
          <!-- 空文件夹不显示计数：0 传达不了"这里有没有东西"，只多一个噪点。 -->
          <span v-if="typeof row.node.noteCount === 'number' && row.node.noteCount > 0" class="tree__count">{{ row.node.noteCount }}</span>
        </button>

        <div class="tree__tools">
          <button v-if="!isSystem(row.node)" type="button" class="btn btn--quiet btn--icon" data-testid="folder-rename" :title="t('sidebar.rename')" :aria-label="t('sidebar.rename')" @click="beginRename(row.node.id, row.node.name)">
            ✎
          </button>
          <AppPopover
            v-if="!isSystem(row.node)"
            icon="⌫"
            testid="folder-delete"
            :label="t('sidebar.deleteFolder')"
          >
            <template #default="{ close }">
              <p class="tree__confirm-text">{{ t('sidebar.deleteFolderHint') }}</p>
              <span class="tree__confirm-row">
                <button type="button" class="btn btn--danger" data-testid="folder-delete-confirm" @click="close(); removeFolder(row.node.id)">{{ t('list.confirm') }}</button>
                <button type="button" class="btn btn--quiet" @click="close()">{{ t('list.cancel') }}</button>
              </span>
            </template>
          </AppPopover>
        </div>
      </div>

      <input
        v-if="editingId === row.node.id"
        :ref="rowRef"
        v-model="nameDraft"
        class="input tree__input"
        type="text"
        data-testid="folder-rename-input"
        :aria-label="t('sidebar.rename')"
        @keydown.enter.prevent="commitRename(row.node.id)"
        @keydown.escape.prevent="editingId = null"
        @blur="commitRename(row.node.id)"
      />
    </li>

    <li v-if="rows.length === 0" class="tree__empty text-sm text-muted">{{ t('sidebar.folders') }}：0</li>
  </ul>
</template>

<style scoped>
.tree {
  list-style: none;
  margin: 0;
  padding: 0;
}

.tree__row {
  display: flex;
  align-items: center;
  gap: var(--space-1);
  min-height: var(--touch-min);
  /* 工具层浮在这一行之上（见 .tree__tools），所以这一行得是它的定位父级。
     注意这里**没有** padding-left：层级不再用缩进表达。 */
  position: relative;
  padding-right: var(--space-2);
}

.tree__row:hover {
  background: var(--bg-hover);
}

.tree__row[data-active='true'] {
  background: var(--accent-soft);
}

.tree__name {
  flex: 1;
  display: flex;
  align-items: center;
  gap: var(--space-2);
  min-height: var(--touch-min);
  text-align: left;
  padding: 0 var(--space-2);
  border-radius: var(--radius-1);
  color: var(--text-primary);
  cursor: pointer;
  min-width: 0;
}

.tree__label {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.tree__count {
  font-size: var(--text-xs);
  color: var(--text-muted);
}

.tree__tools {
  /**
   * 工具层**浮在行上**，不参与这一行的排版。三个理由，缺一不可：
   *  ① 静止态不能再"占 0 宽"或"占 176 宽"：占 0 宽就得配 `overflow: hidden`，
   *     那会把悬浮面板一起剪掉（面板量到 inView=false 就是这么来的）；
   *     占 176 宽则把名字挤成两行 —— 真读数：打开确认层时侧栏 scrollHeight 从 536 掉到 492、
   *     两行往上跳，正是"提示不该在底下占一格"的同一种病，只是换了个方向。
   *  ② 面板要能逃出这一栏的裁剪，所以这里不能有任何 overflow 裁剪。
   *  ③ 静止态要不可点（`pointer-events: none`），否则透明按钮会盖住名字那一片的点击。
   */
  position: absolute;
  top: 0;
  right: var(--space-2);
  bottom: 0;
  display: flex;
  align-items: center;
  gap: var(--space-1);
  opacity: 0;
  pointer-events: none;
}

.tree__row:hover .tree__tools,
.tree__row:focus-within .tree__tools,
/* 面板开着的时候不许收回去：指针移到面板上就已经离开这一行了，
   没有这一条的话"确认删除"永远点不到 —— 而 aria-expanded 是 Headless UI 自己维护的，
   不另立一套状态。 */
.tree__tools:has([aria-expanded='true']) {
  opacity: 1;
  pointer-events: auto;
}

.tree__confirm-text {
  margin: 0;
  color: var(--text-secondary);
}

.tree__confirm-row {
  display: flex;
  gap: var(--space-1);
}

.tree__input {
  margin: var(--space-1) var(--space-3);
  width: calc(100% - var(--space-6));
}

.tree__empty {
  padding: var(--space-2) var(--space-3);
}
</style>
