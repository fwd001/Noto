<script setup lang="ts">
/** 列表栏：虚拟滚动的笔记行 + 搜索态 + 回收站动作。 */
import { computed, onBeforeUnmount, onMounted, ref } from 'vue';
import SearchField from './SearchField.vue';
import EmptyState from './EmptyState.vue';
import SkeletonRows from './SkeletonRows.vue';
import { useNoteStore } from '../stores/notes';
import { useEditorStore } from '../stores/editor';
import { useFolderStore } from '../stores/folders';
import { useShellStore } from '../stores/shell';
import { useConflictStore } from '../stores/conflicts';
import { t, messageFor } from '../i18n';
import { formatWhen } from '../util/format';
import type { NoteListRow, SearchHit } from '../api/types';

/**
 * 列表行的**唯一**高度来源。
 *
 * 原来是 76px写死，而一行里实际要装：标题（text-base）+ 摘要（text-sm）+ 时间（text-xs）
 * 再加上 padding 与 gap —— 实测需要 92px 才装得下。装不下又没溢出保护时，
 * 时间那一行会**压到下一行的标题上**（用户截图里"阿斯蒂芬"正好盖住下一行标题）。
 *
 * 关键约束：虚拟滚动的 `startIndex / endIndex / padTop / padBottom` 全都按这个常量算，
 * 所以它必须同时是"渲染高度"和"计算高度"。改这里等于改整个滚动模型。
 */
const ROW_HEIGHT = 92;
const OVERSCAN = 4;

/** 正卡在未裁决冲突里的那些笔记（§5.1 第 4 步：用户没选之前，列表上就该看得出来）。 */
const conflicts = useConflictStore();

interface Entry {
  id: string;
  title: string;
  summary: string;
  snippetHtml: string | null;
  updatedAt: string;
  pinned: boolean;
  hasAttachment: boolean;
  folderName: string;
  deletedAt: string | null;
}

const emit = defineEmits<{ (event: 'open'): void }>();

const notes = useNoteStore();
const editor = useEditorStore();
const folders = useFolderStore();
const shell = useShellStore();

const viewport = ref<HTMLElement | null>(null);
const scrollTop = ref(0);
const viewportHeight = ref(600);
const confirmingPurge = ref<string | null>(null);
const searchField = ref<InstanceType<typeof SearchField> | null>(null);

const entries = computed<Entry[]>(() => {
  if (notes.hits !== null) return notes.hits.map(fromHit);
  return notes.rows.map(fromRow);
});

function fromRow(row: NoteListRow): Entry {
  return {
    id: row.id,
    title: row.title || t('editor.untitled'),
    summary: row.summary ?? '',
    snippetHtml: null,
    updatedAt: row.updatedAt,
    pinned: row.pinned === true,
    hasAttachment: row.hasAttachment === true,
    folderName: row.folderName ?? (row.folderId ? folders.nameOf(row.folderId) : ''),
    deletedAt: row.deletedAt ?? null,
  };
}

function fromHit(hit: SearchHit): Entry {
  return {
    id: hit.noteId,
    title: notes.titles[hit.noteId] || t('editor.untitled'),
    summary: '',
    snippetHtml: hit.snippetHtml,
    updatedAt: '',
    pinned: false,
    hasAttachment: false,
    folderName: '',
    deletedAt: null,
  };
}

const total = computed(() => entries.value.length);
const startIndex = computed(() => Math.max(0, Math.floor(scrollTop.value / ROW_HEIGHT) - OVERSCAN));
const endIndex = computed(() => Math.min(total.value, Math.ceil((scrollTop.value + viewportHeight.value) / ROW_HEIGHT) + OVERSCAN));
const visible = computed(() => entries.value.slice(startIndex.value, endIndex.value));
const padTop = computed(() => startIndex.value * ROW_HEIGHT);
const padBottom = computed(() => Math.max(0, (total.value - endIndex.value) * ROW_HEIGHT));

const listTitle = computed(() => {
  if (notes.mode.kind === 'trash') return t('list.trashMode');
  if (notes.mode.kind === 'folder' && notes.mode.folderId) return folders.nameOf(notes.mode.folderId);
  return t('sidebar.allNotes');
});

const errorText = computed(() => (notes.errorKey ? messageFor(notes.errorKey) : ''));

function onScroll(event: Event): void {
  const el = event.target as HTMLElement;
  scrollTop.value = el.scrollTop;
  viewportHeight.value = el.clientHeight || viewportHeight.value;
  if (el.scrollTop + el.clientHeight >= el.scrollHeight - ROW_HEIGHT * 2) void notes.loadMore();
}

async function open(entry: Entry): Promise<void> {
  confirmingPurge.value = null;
  notes.selectedId = entry.id;
  await editor.open(entry.id);
  emit('open');
  if (shell.isCompact) shell.openEditor();
}

function focusSearch(): void {
  searchField.value?.focus();
}

defineExpose({ focusSearch });

onMounted(() => {
  const el = viewport.value;
  if (el) viewportHeight.value = el.clientHeight || ROW_HEIGHT * 8;
});

onBeforeUnmount(() => {
  void editor.flush();
});
</script>

<template>
  <section class="pane pane--list" :aria-label="t('list.searchPlaceholder')" data-testid="note-list">
    <div class="pane-header">
      <span class="pane-title">{{ listTitle }}</span>
      <!-- 这里**不再**放侧栏把手。原本列表头这颗与标题栏那颗、主区左上那颗会同时出现，
           用户看到的就是「折起之后下面一层还有一个折起」；把手的唯一归属由
           `drawsTitleBar(caps)` 决定（见 WorkspaceView / SidebarPanel）。 -->
      <button type="button" class="btn btn--quiet" data-testid="new-note" :title="t('list.newNote')" @click="notes.create(notes.mode.kind === 'folder' ? notes.mode.folderId : null)">
        {{ t('list.newNote') }}
      </button>
      <button type="button" class="btn btn--quiet" data-testid="daily-note" :title="t('list.dailyNote')" :aria-label="t('list.dailyNote')" @click="notes.openToday()">
        {{ t('list.dailyNote') }}
      </button>
    </div>

    <SearchField ref="searchField" />

    <p v-if="notes.searching" class="list-status" role="status" data-testid="search-status">{{ t('list.searching') }}</p>

    <div ref="viewport" class="pane-body list-viewport" @scroll.passive="onScroll">
      <SkeletonRows v-if="notes.loading && total === 0" />

      <EmptyState
        v-else-if="total === 0 && notes.hits === null"
        :glyph="notes.errorKey ? '!' : '≡'"
        :title="errorText || t('list.empty')"
        :hint="errorText ? '' : t('list.emptyHint')"
        data-testid="list-empty"
      >
        <button v-if="!notes.inTrash" type="button" class="btn btn--primary" @click="notes.create(notes.mode.kind === 'folder' ? notes.mode.folderId : null)">
          {{ t('list.newNote') }}
        </button>
      </EmptyState>

      <EmptyState v-else-if="total === 0" :glyph=" '?'" :title="t('list.noResults')" :hint="t('list.noResultsHint')" data-testid="search-empty" />

      <div v-else class="rows" :style="{ paddingTop: `${padTop}px`, paddingBottom: `${padBottom}px` }" role="list">
        <article
          v-for="entry in visible"
          :key="entry.id"
          class="row-item"
          :class="{ 'row-item--selected': entry.id === notes.selectedId }"
          :style="{ height: `${ROW_HEIGHT}px` }"
          role="listitem"
          tabindex="0"
          :aria-current="entry.id === notes.selectedId ? 'true' : undefined"
          :data-testid="`note-row-${entry.id}`"
          @click="open(entry)"
          @keydown.enter.prevent="open(entry)"
          @keydown.space.prevent="open(entry)"
        >
          <div class="row-item__main">
            <p class="row-item__title">
              {{ entry.title }}
            </p>
            <p v-if="entry.snippetHtml" class="row-item__snippet" v-html="entry.snippetHtml" />
            <p v-else class="row-item__summary">{{ entry.summary }}</p>
          </div>
          <div class="row-item__side">
            <span class="row-item__meta">
              <span v-if="entry.hasAttachment" :title="t('list.hasAttachment')" :aria-label="t('list.hasAttachment')">▤</span>
              <span v-if="conflicts.contended.has(entry.id)" class="row-item__contended" data-testid="row-contended" :title="t('list.contendedNote')" :aria-label="t('list.contendedNote')">⚠</span>
              <span>{{ formatWhen(entry.updatedAt) }}</span>
            </span>
            <button
              v-if="!notes.inTrash"
              type="button"
              class="row-item__pin"
              :class="{ 'row-item__pin--on': entry.pinned }"
              data-testid="note-pin-toggle"
              :aria-pressed="entry.pinned ? 'true' : 'false'"
              :aria-label="entry.pinned ? t('list.unpin') : t('list.pin')"
              :title="entry.pinned ? t('list.unpin') : t('list.pin')"
              @click.stop="notes.setPinned(entry.id, !entry.pinned)"
            >
              {{ entry.pinned ? '●' : '○' }}
            </button>
            <span class="row-item__actions">
              <button
                v-if="!notes.inTrash"
                type="button"
                class="btn btn--quiet btn--icon"
                :aria-label="t('list.moveToTrash')"
                :title="t('list.moveToTrash')"
                @click.stop="notes.moveToTrash(entry.id)"
              >
                ⌫
              </button>
              <button v-if="notes.inTrash" type="button" class="btn btn--quiet" :aria-label="t('list.restore')" data-testid="restore-note" @click.stop="notes.restore(entry.id)">
                {{ t('list.restore') }}
              </button>
              <button
                v-if="notes.inTrash"
                type="button"
                class="btn btn--quiet btn--danger"
                :aria-label="t('list.purge')"
                data-testid="purge-note"
                @click.stop="confirmingPurge = confirmingPurge === entry.id ? null : entry.id"
              >
                {{ t('list.purge') }}
              </button>
            </span>
          </div>

          <p v-if="confirmingPurge === entry.id" class="row-item__confirm" @click.stop>
            <span>{{ t('list.purgeConfirmShort') }}</span>
            <button type="button" class="btn btn--danger" data-testid="purge-confirm" @click.stop="notes.purge(entry.id)">{{ t('list.confirm') }}</button>
            <button type="button" class="btn btn--quiet" @click.stop="confirmingPurge = null">{{ t('list.cancel') }}</button>
          </p>
        </article>
      </div>
    </div>

    <p v-if="notes.hasMore && total > 0" class="list-status">
      <button type="button" class="btn btn--quiet" data-testid="load-more" @click="notes.loadMore()">{{ t('list.loadMore') }}</button>
    </p>
  </section>
</template>

<style scoped>
.list-viewport {
  position: relative;
}

.rows {
  display: flex;
  flex-direction: column;
}

.row-item {
  position: relative;
  display: flex;
  flex-direction: column;
  justify-content: center;
  gap: var(--space-1);
  padding: var(--space-2) var(--space-3);
  border-bottom: 1px solid var(--border-subtle);
  cursor: pointer;
  box-sizing: border-box;
}

.row-item:hover {
  background: var(--bg-hover);
}

.row-item--selected {
  background: var(--accent-soft);
  box-shadow: inset 3px 0 0 var(--accent);
}

.row-item__main {
  min-width: 0;
}

.row-item__title {
  font-weight: 650;
  font-size: var(--text-base);
  /* 标题/摘要各自单行省略。它们本来就有 ellipsis，但**没有行高上限**：
     父级`.row-item` 是固定高度 + `justify-content: center`，
     所以内容一旦超出版心就��从上下两侧溢出，压到相邻行上 ——
     ellipsis 救不了溢出，只救"单行太长"。 */
  line-height: 1.35;
  max-height: 1.35em;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--text-primary);
}

/* 置顶那颗点：**常显**，且状态用图形本身表达（空心 ↔ 实心 + 主色）。
   此前它藏在 `.row-item__actions` 里（父级 opacity:0，只有悬停才出现），
   点完唯一的反馈是标题前多一个 ✓ —— 用户读不到"这颗点改了什么"。
   父级 opacity 是压不住的（子元素无法把自己从 opacity:0 里救回来），
   所以这颗必须搬到 `__actions` 外面，而不是在里面加一条覆盖。 */
.row-item__pin {
  flex: 0 0 auto;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-width: var(--touch-min);
  min-height: var(--touch-min);
  padding: 0;
  border: 0;
  border-radius: var(--radius-2);
  background: transparent;
  color: var(--text-muted);
  font-size: var(--text-md);
  line-height: 1;
  cursor: pointer;
}

.row-item__pin:hover {
  color: var(--text-secondary);
}

.row-item__pin--on {
  color: var(--accent);
}

.row-item__summary,
.row-item__snippet {
  font-size: var(--text-sm);
  color: var(--text-secondary);
  /* 同上：单行省略 + 行高封顶，父级固定高度时才不会被顶穿。 */
  line-height: 1.4;
  max-height: 1.4em;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.row-item__snippet :deep(mark) {
  background: var(--bg-highlight);
  color: var(--text-primary);
  border-radius: 2px;
}

.row-item__snippet :deep(b) {
  font-weight: 700;
}

.row-item__side {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: var(--space-2);
}

.row-item__meta {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  /* 那颗点搬到 meta 之后，靠这一条把 meta 顶到行首，右侧留给置顶 + 悬浮操作。 */
  margin-inline-end: auto;
  font-size: var(--text-xs);
  color: var(--text-muted);
}

.row-item__actions {
  display: flex;
  align-items: center;
  gap: var(--space-1);
  opacity: 0;
}

.row-item:hover .row-item__actions,
.row-item:focus-within .row-item__actions {
  opacity: 1;
}

.row-item__actions .btn {
  /* A11Y-04 的下限就是 44pt，这两颗此前是 32×32（台账里挂"待查"那条）。
     不拿"视觉小"当理由：命不中就是误触，§6 明令禁止的那一类。 */
  min-height: var(--touch-min);
  min-width: var(--touch-min);
}

.row-item__confirm {
  position: absolute;
  inset: var(--space-1);
  display: flex;
  align-items: center;
  gap: var(--space-2);
  padding: var(--space-2) var(--space-3);
  background: var(--bg-raised);
  border: 1px solid var(--danger);
  border-radius: var(--radius-2);
  box-shadow: var(--shadow-2);
  font-size: var(--text-sm);
  cursor: default;
}

.list-status {
  padding: var(--space-2) var(--space-3);
  font-size: var(--text-sm);
  color: var(--text-muted);
  text-align: center;
  flex: 0 0 auto;
}
</style>
