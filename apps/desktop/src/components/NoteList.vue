<script setup lang="ts">
/** 列表栏：虚拟滚动的笔记行 + 搜索态 + 回收站动作。 */
import { computed, onBeforeUnmount, onMounted, ref } from 'vue';
import SearchField from './SearchField.vue';
import EmptyState from './EmptyState.vue';
import SkeletonRows from './SkeletonRows.vue';
import AppPopover from './ui/AppPopover.vue';
import AppIcon from './ui/AppIcon.vue';
import { TEMPLATE_CHOICES, type NoteTemplate } from '../editor/templates';
import { useNoteStore } from '../stores/notes';
import { useEditorStore } from '../stores/editor';
import { useFolderStore } from '../stores/folders';
import { useShellStore } from '../stores/shell';
import { useConflictStore } from '../stores/conflicts';
import { t, messageFor } from '../i18n';
import { formatWhen } from '../util/format';
import { plainText } from '../util/plainText';
import { MARK_SWATCHES, MARK_DOT_SHAPE as DOT_SHAPE, dotStyle } from '../util/markColors';
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
 *
 * 92 → **124**（2026-10-09，第 42 刀）：标题从"一行 + 省略号"改成"折两行"之后，
 * 探针实测一行的自然高度是 `长标题 124 / 短标题 85`（1440 与 390 两档读数一样 —— 列表栏在这两档都这么宽），
 * 固定行高要按**最坏那一行**给，否则内容会从上下两侧溢出、压到相邻行上（就是下面 `.row-item__title`
 * 那条注释记着的老毛病，只是这次是竖向）。正文字号那颗 `--editor-font-scale` 只作用在编辑器正文，
 * 列表这一列不跟着长，所以这个常量不需要跟着缩放走。
 */
const ROW_HEIGHT = 124;
const OVERSCAN = 4;

/** 正卡在未裁决冲突里的那些笔记（§5.1 第 4 步：用户没选之前，列表上就该看得出来）。 */
const conflicts = useConflictStore();

interface Entry {
  id: string;
  title: string;
  summary: string;
  snippetHtml: string | null;
  /**
   * 那一行**预览**的完整文字（搜索结果取片段去掉 `<mark>`，普通行取摘要）。
   * 预览这一格按 §5 允许截成一行，代价是它必须"不静默"：被裁的那一段原样放在 `title` 属性里，
   * 而 ㊻ 那条判据就是照这个属性放行的 —— 属性里没有尾段，判据照样点名。
   */
  previewText: string;
  /** §6「颜色」：这一篇的标记色（`null` = 没打标记）。列表行标题前那颗小点读它。 */
  color: string | null;
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
  const summary = row.summary ?? '';
  return {
    id: row.id,
    title: row.title || t('editor.untitled'),
    summary,
    snippetHtml: null,
    previewText: summary,
    color: row.color ?? null,
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
    previewText: plainText(hit.snippetHtml),
    // 搜索结果这一支不读颜色：`search` 的命中里没有 color 这一格（要显示本该回查列表那一份），
    // 而"标记色"是列表的记号，搜索屏上不画 —— 这里写 null 是为了形状完整，不是在偷懒。
    color: null,
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

/**
 * §3.3 分档那一段要不要画。
 *
 * 空结果时不画：那一格归 §3.3 固定的那句「没有找到相关内容」管，再来一排「精准 0 · 模糊 0」
 * 是同一件事说两遍。搜索进行中也不画：那时这几格还是**上一次**查询的结论，顶着说就是假话。
 */
const tiers = computed(() => {
  const read = notes.searchTiers;
  if (read === null || notes.searching) return null;
  return read.exact + read.fuzzy > 0 ? read : null;
});

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
function currentFolderId(): string | null {
  return notes.mode.kind === 'folder' ? notes.mode.folderId : null;
}

function createBlank(): void {
  void notes.create(currentFolderId());
}

/** 挑一支（或清掉，空串 = 清）：先收色板再发命令 —— 命令要等一会儿才回，
 *  面板留在屏幕上会显得没反应（文件夹那颗同一个理由）。 */
async function pickColor(id: string, color: string, close: () => void): Promise<void> {
  close();
  await notes.setColor(id, color);
}

function createFrom(tpl: NoteTemplate): void {
  void notes.create(currentFolderId(), tpl.build());
}
</script>

<template>
  <section class="pane pane--list" :aria-label="t('list.searchPlaceholder')" data-testid="note-list">
    <div class="pane-header">
      <span class="pane-title">{{ listTitle }}</span>
      <!-- 这里**不再**放侧栏把手。原本列表头这颗与标题栏那颗、主区左上那颗会同时出现，
           用户看到的就是「折起之后下面一层还有一个折起」；把手的唯一归属由
           `drawsTitleBar(caps)` 决定（见 WorkspaceView / SidebarPanel）。 -->
      <button type="button" class="btn btn--quiet" data-testid="new-note" :title="t('list.newNote')" @click="createBlank">
        {{ t('list.newNote') }}
      </button>
      <!-- 那颗 ▾ 才是"快捷新建模板"。默认路径（上面那颗）保持**空白一页**，
           因为口径是"默认的模板要非常简洁，一进去就是请输入标题和正文"。 -->
      <AppPopover icon="chevron-down" testid="new-note-templates" :label="t('list.newFromTemplate')">
        <template #default="{ close }">
          <button
            v-for="tpl in TEMPLATE_CHOICES"
            :key="tpl.id"
            type="button"
            class="btn btn--block"
            :data-testid="`template-${tpl.id}`"
            @click="close(); createFrom(tpl)"
          >
            {{ t(tpl.label) }}
          </button>
        </template>
      </AppPopover>
      <button type="button" class="btn btn--quiet" data-testid="daily-note" :title="t('list.dailyNote')" :aria-label="t('list.dailyNote')" @click="notes.openToday()">
        {{ t('list.dailyNote') }}
      </button>
    </div>

    <SearchField ref="searchField" />

    <p v-if="notes.searching" class="list-status" role="status" data-testid="search-status">{{ t('list.searching') }}</p>

    <!-- §3.3 分档读数（§6 第 5 条"已实现未开放"里的第 5 格）。
         刻意放在滚动区**外面**：`.rows` 的 padTop/padBottom/endIndex 全按 ROW_HEIGHT 算，
         往视口里插一块会被滚走的东西，虚拟滚动的位置公式就整体偏一格 —— 表现是"滚到某处开始空白行"。
         搜索进行中不画：那时下面这几格还是上一次查询的结论，顶着说就是假话。 -->
    <div v-if="tiers !== null" class="search-summary" data-testid="search-summary">
      <p class="search-summary__count" role="status" data-testid="search-found">
        {{ t('list.searchFound', { query: notes.query.trim(), count: notes.hits?.length ?? 0 }) }}
      </p>
      <p class="search-summary__hint" data-testid="search-tier-hint">{{ t('list.searchTierHint') }}</p>
      <p class="search-summary__tiers">
        <span class="tier-chip" data-testid="tier-exact">{{ t('list.tierExact', { count: tiers.exact }) }}</span>
        <span class="tier-chip" data-testid="tier-fuzzy">{{ t('list.tierFuzzy', { count: tiers.fuzzy }) }}</span>
      </p>
    </div>

    <div ref="viewport" class="pane-body list-viewport" @scroll.passive="onScroll">
      <SkeletonRows v-if="notes.loading && total === 0" />

      <EmptyState
        v-else-if="total === 0 && notes.hits === null"
        :icon="notes.errorKey ? 'alert' : 'list'"
        :title="errorText || t('list.empty')"
        :hint="errorText ? '' : t('list.emptyHint')"
        data-testid="list-empty"
      >
        <button v-if="!notes.inTrash" type="button" class="btn btn--primary" @click="notes.create(notes.mode.kind === 'folder' ? notes.mode.folderId : null)">
          {{ t('list.newNote') }}
        </button>
      </EmptyState>

      <EmptyState v-else-if="total === 0" icon="question" :title="t('list.noResults')" :hint="t('list.noResultsHint')" data-testid="search-empty" />

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
            <!-- 标题折两行：§5 那句"不许被静默裁掉"里，标题是**认身份**的那一格，不许硬截
                 （2026-10-09 用户拍的形状）。两行还读不完的极长标题，全文在这颗 `title` 里 ——
                 ㊻ 那条判据放行的就是这个属性本身，不是"看着像省略号就算数"。
                 标题前那颗小点是**标记色**（§6 第 11 格，笔记级颜色当「标签」用，2026-10-09 拍板）：
                 `aria-hidden` —— 颜色是给眼睛的辅助，认哪一篇靠标题本身，读屏念标题就够。 -->
            <p class="row-item__title" :title="entry.title">
              <span
                v-if="entry.color"
                class="row-item__dot"
                :data-testid="`note-dot-${entry.id}`"
                :style="{ ...DOT_SHAPE, ...dotStyle(entry.color) }"
                aria-hidden="true"
              />
              {{ entry.title }}
            </p>
            <!-- 预览这一格按形状只给一行（一屏要扫得了十几条），代价是它必须把全文带在身上：
                 搜索结果取的是去掉 `<mark>` 的那段**纯文字**，不是原始 HTML。 -->
            <p v-if="entry.snippetHtml" class="row-item__snippet" :title="entry.previewText" v-html="entry.snippetHtml" />
            <p v-else class="row-item__summary" :title="entry.previewText">{{ entry.summary }}</p>
          </div>
          <div class="row-item__side">
            <span class="row-item__meta">
              <AppIcon v-if="entry.hasAttachment" :size="16" name="attach-file" :label="t('list.hasAttachment')" data-testid="row-has-attachment" />
              <AppIcon v-if="conflicts.contended.has(entry.id)" class="row-item__contended" :size="16" name="warn" :label="t('list.contendedNote')" data-testid="row-contended" />
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
              <AppIcon :name="entry.pinned ? 'pin-on' : 'pin-off'" :size="18" :testid="`pin-glyph-${entry.id}`" />
            </button>
            <span class="row-item__actions">
              <!-- §6「颜色」：笔记的标记色（当「标签」用）。入口与文件夹那颗同一形状：
                   面板走 AppPopover（一打开就把下面每一排顶走的色板等于让手指落到别的行为上），
                   触发字形用"当前色的点"（§2.4 那份图标清单里没有"调色板"这一枚，
                   不为了一颗按钮发明没规格的图标）。回收站里那一档不摆（核心同样只在那一侧拒）。 -->
              <AppPopover
                v-if="!notes.inTrash"
                testid="note-color-toggle"
                align="end"
                :label="t('sidebar.colorTitle')"
              >
                <template #glyph>
                  <span class="row-item__dot" :style="{ ...DOT_SHAPE, ...dotStyle(entry.color) }" />
                </template>
                <template #default="{ close }">
                  <div class="row-item__swatches" data-testid="note-color-panel">
                    <span class="row-item__swatch-row">
                      <button
                        v-for="s in MARK_SWATCHES"
                        :key="s.hex"
                        type="button"
                        class="btn btn--quiet btn--icon row-item__swatch"
                        :data-testid="`note-swatch-${s.hex}`"
                        :aria-label="t(s.labelKey)"
                        :title="t(s.labelKey)"
                        :aria-pressed="entry.color === s.hex ? 'true' : 'false'"
                        @click="pickColor(entry.id, s.hex, close)"
                      >
                        <span class="row-item__dot" :style="{ ...DOT_SHAPE, ...dotStyle(s.hex) }" />
                      </button>
                    </span>
                    <button
                      type="button"
                      class="btn btn--quiet btn--block"
                      data-testid="note-color-none"
                      :aria-pressed="entry.color ? 'false' : 'true'"
                      @click="pickColor(entry.id, '', close)"
                    >{{ t('sidebar.colorNone') }}</button>
                  </div>
                </template>
              </AppPopover>
              <button
                v-if="!notes.inTrash"
                type="button"
                class="btn btn--quiet btn--icon"
                :aria-label="t('list.moveToTrash')"
                :title="t('list.moveToTrash')"
                @click.stop="notes.moveToTrash(entry.id)"
              >
                <AppIcon :size="18" name="trash" />
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

    <!-- 列表栏底部那一格（设计稿第 1 页列表栏最后一行）。也在滚动区外面：它说的是"这一栏总共几行"，
         跟着内容滚走就成了一条要划到才看得见的信息。搜索时这一格不出现（上面那格在说「找到 N 条」，
         两个数一起说的是两件不同的事）。 -->
    <p v-if="notes.listReadout !== null" class="list-foot" data-testid="list-count">
      {{ t(notes.listReadout.key, { count: notes.listReadout.count }) }}
    </p>

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
  gap: var(--sp-1);
  padding: var(--sp-2) var(--sp-3);
  border-bottom: 1px solid var(--line);
  cursor: pointer;
  box-sizing: border-box;
}

.row-item:hover {
  background: var(--hover);
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
  /* 标题：**折两行，两行读不完才截断**（§5「文本不许被静默裁掉」，2026-10-09 拍的形状）。
     以前这一格是 `nowrap + ellipsis + max-height:1.35em` —— 一行装不下就吃掉尾巴，而 ㊻ 看不见它：
     列表容器写的是 `overflow-y:auto`，按 CSS 规则这会把 `overflow-x` 的**计算值**也带成 auto，
     于是"声明可滚"被当成"读得到"（实测推它的 `scrollLeft` 一动不动，长标题溢出 175–301 px 而门禁全绿）。
     判据换成"真去滚它一下"之后，这一格必须自己把话讲完。
     用 `-webkit-line-clamp` 而不是 `max-height: 2.7em`：clamp 自己数行，§4.7 那颗 fontScale 变了
     不用回来改这个数字。极长标题的全文在 `title` 属性里 —— 判据认的就是那颗属性。 */
  line-height: 1.35;
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: normal;
  overflow-wrap: break-word;
  color: var(--ink);
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
  min-width: var(--touch);
  min-height: var(--touch);
  padding: 0;
  border: 0;
  border-radius: var(--r-card);
  background: transparent;
  color: var(--mute);
  font-size: var(--text-md);
  line-height: 1;
  cursor: pointer;
}

.row-item__pin:hover {
  color: var(--body);
}

.row-item__pin--on {
  color: var(--accent);
}

.row-item__summary,
.row-item__snippet {
  font-size: var(--text-sm);
  color: var(--body);
  /* 同上：单行省略 + 行高封顶，父级固定高度时才不会被顶穿。 */
  line-height: 1.4;
  max-height: 1.4em;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.row-item__snippet :deep(mark) {
  background: var(--mark-bg);
  color: var(--ink);
  border-radius: 2px;
}

.row-item__snippet :deep(b) {
  font-weight: 700;
}

.row-item__side {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: var(--sp-2);
}

.row-item__meta {
  display: flex;
  align-items: center;
  gap: var(--sp-2);
  /* 那颗点搬到 meta 之后，靠这一条把 meta 顶到行首，右侧留给置顶 + 悬浮操作。 */
  margin-inline-end: auto;
  font-size: var(--text-xs);
  color: var(--mute);
}

.row-item__actions {
  display: flex;
  align-items: center;
  gap: var(--sp-1);
  opacity: 0;
}

.row-item:hover .row-item__actions,
.row-item:focus-within .row-item__actions {
  opacity: 1;
}

/* 触屏没有 hover：这一族不常驻就是"手机上看不见、却能点着"（`opacity:0` 仍然吃点击，
   实测 `hasTouch` 那一档 `pointer-events` 还是 `auto`）—— 那是隐形陷阱，比看不见更糟。
   它们本来就在 flex 流里占着位子，所以露出来不改变布局。 */
@media (hover: none) {
  .row-item__actions {
    opacity: 1;
  }
}

.row-item__actions .btn {
  /* A11Y-04 的下限就是 44pt，这两颗此前是 32×32（台账里挂"待查"那条）。
     不拿"视觉小"当理由：命不中就是误触，§6 明令禁止的那一类。 */
  min-height: var(--touch);
  min-width: var(--touch);
}

.row-item__confirm {
  position: absolute;
  inset: var(--sp-1);
  display: flex;
  align-items: center;
  gap: var(--sp-2);
  padding: var(--sp-2) var(--sp-3);
  background: var(--canvas);
  border: 1px solid var(--danger);
  border-radius: var(--r-card);
  box-shadow: var(--shadow-2);
  font-size: var(--text-sm);
  cursor: default;
}

.list-status {
  padding: var(--sp-2) var(--sp-3);
  font-size: var(--text-sm);
  color: var(--mute);
  text-align: center;
  flex: 0 0 auto;
}

.list-foot {
  padding: var(--sp-2) var(--sp-4);
  border-top: 1px solid var(--line);
  font-size: var(--text-xs);
  color: var(--mute);
  flex: 0 0 auto;
}

.search-summary {
  display: flex;
  flex: 0 0 auto;
  flex-direction: column;
  gap: var(--sp-1);
  padding: var(--sp-2) var(--sp-4);
  border-bottom: 1px solid var(--line);
}

.search-summary__count {
  font-size: var(--text-sm);
  font-weight: 600;
  color: var(--ink);
}

.search-summary__hint {
  font-size: var(--text-xs);
  line-height: 17px;
  color: var(--mute);
}

.search-summary__tiers {
  display: flex;
  gap: var(--sp-2);
}

.tier-chip {
  padding: 2px var(--sp-2);
  border: 1px solid var(--line);
  border-radius: var(--r-chip);
  font-size: var(--text-xs);
  color: var(--body);
}

/* §6「颜色」：标题前那一颗标记色点（笔记级颜色当「标签」用，2026-10-09 拍板）。
   形状走共享的 `MARK_DOT_SHAPE`（与侧栏文件夹那颗**同一套**，两边长得一样才叫同一套标记），
   这里只管它在标题行里的站位：与首行字面中线对齐，右边留一口气。 */
.row-item__dot {
  margin-right: var(--sp-2);
  vertical-align: -1px;
}

.row-item__swatches {
  display: flex;
  flex-direction: column;
  gap: var(--sp-1);
}

.row-item__swatch-row {
  display: flex;
  flex-direction: row;
  flex-wrap: wrap;
  gap: var(--sp-1);
}

.row-item__swatch {
  padding: var(--sp-2);
}
</style>
