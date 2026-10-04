<script setup lang="ts">
/** 文件夹树：新建 / 重命名 / 移动 / 删除。删除文件夹不级联删笔记（由本地核心保证）。 */
import { computed, nextTick, ref } from 'vue';
import AppPopover from './ui/AppPopover.vue';
import { useFolderStore, type FlatFolder } from '../stores/folders';
import { useNoteStore } from '../stores/notes';
import { useShellStore } from '../stores/shell';
import { t } from '../i18n';
import type { FolderNode } from '../api/types';

defineOptions({ name: 'FolderTree' });

const props = withDefaults(defineProps<{ nodes?: readonly FolderNode[]; depth?: number }>(), { nodes: undefined, depth: 0 });

const folders = useFolderStore();
const notes = useNoteStore();
const shell = useShellStore();

const editingId = ref<string | null>(null);
const creatingUnder = ref<string | null>(null);
const nameDraft = ref('');
const inputEl = ref<HTMLInputElement | null>(null);

const visibleNodes = computed<readonly FolderNode[]>(() => (props.depth === 0 ? folders.nodes : (props.nodes ?? [])));
const padLeft = computed(() => `calc(var(--space-2) + ${props.depth} * var(--space-4))`);

/**
 * 「移动到」那一份候选：开合交给 AppPopover，所以这里按"给哪一行算"来出，
 * 不再存一个全局 movingId（以前那个 ref 同时管开合与过滤，两件事缠在一起）。
 */
function moveTargetsFor(nodeId: string): FlatFolder[] {
  return folders.flat.filter((entry) => entry.node.id !== nodeId && !isUnder(nodeId, entry));
}

/** `entry` 是不是 `nodeId` 那棵的子层 —— 父亲不能挪到儿子底下（核心也会拒）。 */
function isUnder(nodeId: string, entry: FlatFolder): boolean {
  let node: FlatFolder | undefined = entry;
  const seen = new Set<string>();
  while (node && !seen.has(node.node.id)) {
    if (node.node.id === nodeId) return true;
    seen.add(node.node.id);
    const parentId: string | null = node.node.parentId ?? null;
    node = parentId === null ? undefined : folders.byId.get(parentId);
  }
  return false;
}

/**
 * 核心对 `systemKind` 非空的文件夹一律拒绝改名/移动/删除（`assert_folder_writable`）。
 * 界面无凭据地摆出这三颗，用户点下去得到的是一句 Constraint 错 —— 所以按钮的可见性
 * 跟着核心的判据走，而不是另立一套"看起来像默认本"的猜测。
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

async function beginCreate(parentId: string | null): Promise<void> {
  creatingUnder.value = parentId ?? '__root__';
  nameDraft.value = '';
  await nextTick();
  inputEl.value?.focus();
}

async function commitCreate(): Promise<void> {
  const name = nameDraft.value.trim();
  const raw = creatingUnder.value;
  creatingUnder.value = null;
  nameDraft.value = '';
  if (name.length === 0 || raw === null) return;
  await folders.create(raw === '__root__' ? null : raw, name);
  await notes.load();
}

async function commitRename(id: string): Promise<void> {
  if (editingId.value !== id) return;
  const name = nameDraft.value.trim();
  editingId.value = null;
  if (name.length === 0) return;
  await folders.rename(id, name);
}

async function moveTo(id: string, target: string | null): Promise<void> {
  await folders.move(id, target);
  await notes.load();
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
</script>

<template>
  <ul class="tree" role="group">
    <li v-for="node in visibleNodes" :key="node.id" class="tree__item">
      <div class="tree__row" :data-active="isActive(node.id) ? 'true' : 'false'" :style="{ paddingLeft: padLeft }">
        <button type="button" class="tree__name" :data-testid="`folder-${node.id}`" @click="openFolder(node.id)">
          <span class="tree__label" :title="node.name">{{ node.name }}</span>
          <!-- **空文件夹不显示计数**。
               原来只要 `noteCount` 是数字就渲染，于是"默认名 0"那一行
               在侧栏里孤零零地挂着一个 0（用户截图里能看到）。
               计数的作用是"告诉你哪个文件夹有东西"，0 传达不了这件事，
               反而在视觉上多一个噪点—— 而且它紧跟在"文件夹"小标题下面，
               让人误以为"文件夹"分区里就只剩这一个 0。
               Notion / Obsidian 都是这么做的：有内容才给数字。 -->
          <span v-if="typeof node.noteCount === 'number' && node.noteCount > 0" class="tree__count">{{ node.noteCount }}</span>
        </button>

        <div class="tree__tools">
          <button v-if="!isSystem(node)" type="button" class="btn btn--quiet btn--icon" data-testid="folder-rename" :title="t('sidebar.rename')" :aria-label="t('sidebar.rename')" @click="beginRename(node.id, node.name)">
            ✎
          </button>
          <button type="button" class="btn btn--quiet btn--icon" :data-testid="`folder-new-sub-${node.id}`" :title="t('sidebar.newSubfolder')" :aria-label="t('sidebar.newSubfolder')" @click="beginCreate(node.id)">
            ＋
          </button>
          <AppPopover
            v-if="!isSystem(node)"
            icon="⇄"
            testid="folder-move"
            :label="t('sidebar.moveTo')"
          >
            <template #default="{ close }">
              <button type="button" class="btn btn--block" @click="close(); moveTo(node.id, null)">{{ t('sidebar.root') }}</button>
              <button v-for="target in moveTargetsFor(node.id)" :key="target.node.id" type="button" class="btn btn--block" @click="close(); moveTo(node.id, target.node.id)">
                {{ target.path.join(' / ') }}
              </button>
            </template>
          </AppPopover>
          <AppPopover
            v-if="!isSystem(node)"
            icon="⌫"
            testid="folder-delete"
            :label="t('sidebar.deleteFolder')"
          >
            <template #default="{ close }">
              <p class="tree__confirm-text">{{ t('sidebar.deleteFolderHint') }}</p>
              <span class="tree__confirm-row">
                <button type="button" class="btn btn--danger" data-testid="folder-delete-confirm" @click="close(); removeFolder(node.id)">{{ t('list.confirm') }}</button>
                <button type="button" class="btn btn--quiet" @click="close()">{{ t('list.cancel') }}</button>
              </span>
            </template>
          </AppPopover>
        </div>
      </div>

      <input
        v-if="editingId === node.id"
        ref="inputEl"
        v-model="nameDraft"
        class="input tree__input"
        type="text"
        :aria-label="t('sidebar.rename')"
        @keydown.enter.prevent="commitRename(node.id)"
        @keydown.escape.prevent="editingId = null"
        @blur="commitRename(node.id)"
      />

      <div v-if="creatingUnder === node.id" class="tree__create">
        <input
          v-model="nameDraft"
          class="input"
          type="text"
          data-testid="folder-create-input"
          :aria-label="t('sidebar.newSubfolder')"
          :placeholder="t('sidebar.newSubfolder')"
          @keydown.enter.prevent="commitCreate"
          @keydown.escape.prevent="creatingUnder = null"
          @blur="commitCreate"
        />
      </div>

      <FolderTree v-if="node.children && node.children.length > 0" :nodes="node.children" :depth="props.depth + 1" />
    </li>

    <li v-if="props.depth === 0 && folders.nodes.length === 0" class="tree__empty text-sm text-muted">{{ t('sidebar.folders') }}：0</li>

    <li v-if="creatingUnder === '__root__'" class="tree__create">
      <input
        v-model="nameDraft"
        class="input"
        type="text"
        :aria-label="t('sidebar.newFolder')"
        :placeholder="t('sidebar.newFolder')"
        @keydown.enter.prevent="commitCreate"
        @keydown.escape.prevent="creatingUnder = null"
        @blur="commitCreate"
      />
    </li>
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
  /* 工具层要浮在这一行之上（见 .tree__tools），所以这一行得是它的定位父级。 */
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
   *     两行往上跳，正是用户那句"而不是底下占了一个"的同一种病，只是换了个方向。
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

.tree__input,
.tree__create .input {
  margin: var(--space-1) var(--space-3);
  width: calc(100% - var(--space-6));
}

.tree__empty {
  padding: var(--space-2) var(--space-3);
}
</style>
