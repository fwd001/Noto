<script setup lang="ts">
/** 文件夹树：新建 / 重命名 / 移动 / 删除。删除文件夹不级联删笔记（由本地核心保证）。 */
import { computed, nextTick, ref } from 'vue';
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
const movingId = ref<string | null>(null);
const confirmingDelete = ref<string | null>(null);
const inputEl = ref<HTMLInputElement | null>(null);

const visibleNodes = computed<readonly FolderNode[]>(() => (props.depth === 0 ? folders.nodes : (props.nodes ?? [])));
const padLeft = computed(() => `calc(var(--space-2) + ${props.depth} * var(--space-4))`);

const moveTargets = computed<FlatFolder[]>(() => folders.flat.filter((entry) => entry.node.id !== movingId.value && !isUnderMoving(entry)));

/**
 * 核心对 `systemKind` 非空的文件夹一律拒绝改名/移动/删除（`assert_folder_writable`）。
 * 界面无凭据地摆出这三颗，用户点下去得到的是一句 Constraint 错 —— 所以按钮的可见性
 * 跟着核心的判据走，而不是另立一套"看起来像默认本"的猜测。
 */
function isSystem(node: FolderNode): boolean {
  return typeof node.systemKind === 'string' && node.systemKind.length > 0;
}

function isUnderMoving(entry: FlatFolder): boolean {
  const moving = movingId.value;
  if (!moving) return false;
  let node: FlatFolder | undefined = entry;
  const seen = new Set<string>();
  while (node && !seen.has(node.node.id)) {
    if (node.node.id === moving) return true;
    seen.add(node.node.id);
    const parentId: string | null = node.node.parentId ?? null;
    node = parentId === null ? undefined : folders.byId.get(parentId);
  }
  return false;
}

async function beginRename(id: string, current: string): Promise<void> {
  editingId.value = id;
  confirmingDelete.value = null;
  movingId.value = null;
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
  movingId.value = null;
  await folders.move(id, target);
  await notes.load();
}

async function removeFolder(id: string): Promise<void> {
  confirmingDelete.value = null;
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
          <button v-if="!isSystem(node)" type="button" class="btn btn--quiet btn--icon" data-testid="folder-move" :aria-expanded="movingId === node.id ? 'true' : 'false'" :title="t('sidebar.moveTo')" :aria-label="t('sidebar.moveTo')" @click="movingId = movingId === node.id ? null : node.id">
            ⇄
          </button>
          <button v-if="!isSystem(node)" type="button" class="btn btn--quiet btn--icon" data-testid="folder-delete" :title="t('sidebar.deleteFolder')" :aria-label="t('sidebar.deleteFolder')" @click="confirmingDelete = confirmingDelete === node.id ? null : node.id">
            ⌫
          </button>
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

      <div v-if="movingId === node.id" class="tree__popover">
        <button type="button" class="btn btn--block" @click="moveTo(node.id, null)">{{ t('sidebar.root') }}</button>
        <button v-for="target in moveTargets" :key="target.node.id" type="button" class="btn btn--block" @click="moveTo(node.id, target.node.id)">
          {{ target.path.join(' / ') }}
        </button>
      </div>

      <p v-if="confirmingDelete === node.id" class="tree__confirm">
        <span>{{ t('sidebar.deleteFolderHint') }}</span>
        <button type="button" class="btn btn--danger" data-testid="folder-delete-confirm" @click="removeFolder(node.id)">{{ t('list.confirm') }}</button>
        <button type="button" class="btn btn--quiet" @click="confirmingDelete = null">{{ t('list.cancel') }}</button>
      </p>

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
  /* 静止态不占宽：以前靠 opacity: 0 藏起来却仍在流里占 176 px（4 × 44，A11Y-04 的下限），
     246 px 的一行只剩 18~50 px 给名字，连"默认"都被裁掉半个字。
     用 width: 0 而不是 display/visibility: hidden —— 后两种会把四颗按钮摘出 Tab 序列，
     键盘就再也够不着这些动作了；留着焦点可达，:focus-within 才掀得开它。 */
  display: flex;
  align-items: center;
  width: 0;
  overflow: hidden;
  opacity: 0;
}

.tree__row:hover .tree__tools,
.tree__row:focus-within .tree__tools {
  width: auto;
  opacity: 1;
}

.tree__input,
.tree__create .input {
  margin: var(--space-1) var(--space-3);
  width: calc(100% - var(--space-6));
}

.tree__popover {
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
  margin: var(--space-1) var(--space-3);
  padding: var(--space-2);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-2);
  background: var(--bg-raised);
  box-shadow: var(--shadow-2);
  max-height: 260px;
  overflow: auto;
}

.tree__confirm {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: var(--space-2);
  padding: var(--space-2) var(--space-3);
  font-size: var(--text-sm);
  color: var(--text-secondary);
  background: var(--bg-sunken);
}

.tree__empty {
  padding: var(--space-2) var(--space-3);
}
</style>
