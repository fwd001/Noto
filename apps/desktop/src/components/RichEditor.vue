<script setup lang="ts">
/**
 * 富文本编辑器：contenteditable + 块模型双向映射。
 * 不依赖任何第三方编辑器：块 id 由我们生成并保持，未知块只读保留，版本过高只读。
 */
import { computed, nextTick, onMounted, ref, watch } from 'vue';
import EditorToolbar from './EditorToolbar.vue';
import { vEditable, markAsParsed } from '../editor/editableDirective';
import { applySelection, measureEditable, parseEditable, safeHref, selectionBoxIn, selectionIn } from '../editor/dom';
import { placeBar, type Box } from '../editor/selectionBar';
import { MARK_BUTTONS } from '../editor/marks';
import { applySlash, filterSlash, markdownShortcut, slashQuery, type SlashItem } from '../editor/quickInsert';
import { destinationFor, gapAt } from '../editor/interaction';
import {
  applyMark,
  backspace,
  changeType,
  cycleChecklist,
  insertBelow,
  insertRule,
  insertText,
  mergeWithPrevious,
  moveBlock,
  orderedNumbers,
  removeBlockAt,
  setChecked,
  shiftIndent,
  splitAt,
  type BlockEdit,
} from '../editor/commands';
import {
  blockText,
  docCharCount,
  hasMarkInRange,
  headingLevel,
  indentOf,
  inlineText,
  isChecked,
  stringAttr,
  type EditorBlock,
  type TextBlockType,
} from '../editor/model';
import { useEditorStore } from '../stores/editor';
import { useSyncStore } from '../stores/sync';
import { t } from '../i18n';

const MARK_KINDS = ['bold', 'italic', 'underline', 'strike', 'code', 'highlight', 'link'] as const;

const store = useEditorStore();
const sync = useSyncStore();

const docEl = ref<HTMLElement | null>(null);
const blocks = computed(() => store.blocks);
const readOnly = computed(() => store.writeBlocked);
const numbers = computed(() => orderedNumbers(blocks.value));
const activeIndex = ref(0);
const range = ref({ start: 0, end: 0 });
const activeMarks = ref<string[]>([]);
const charCount = computed(() => docCharCount(blocks.value));

/** "/" 面板：查询串为 null 表示当前不是命令输入。选中项用键盘维护。 */
const slashQ = ref<string | null>(null);
const slashSel = ref(0);
const slashList = computed<SlashItem[]>(() => (slashQ.value === null ? [] : filterSlash(slashQ.value)));

async function chooseSlash(item: SlashItem | undefined): Promise<void> {
  slashQ.value = null;
  if (!item) return;
  await run(applySlash(blocks.value, activeIndex.value, item));
}

/**
 * 块把手：拖拽重排走 pointer 事件而不是 HTML5 DnD —— 后者在触摸端基本不触发，
 * 手机上就没了重排。落点用"块中线"判定，绘制一条插入指示线。
 */
const dragFrom = ref<number | null>(null);
const dropGap = ref(0);

function blockBoxes(): { top: number; bottom: number }[] {
  const root = docEl.value;
  if (!root) return [];
  return Array.from(root.querySelectorAll<HTMLElement>('.nb-block')).map((element) => {
    const rect = element.getBoundingClientRect();
    return { top: rect.top, bottom: rect.bottom };
  });
}

/** 拖到可视区上下边缘时滚动，长笔记里不必把块拖出屏幕就断掉。 */
function autoScroll(clientY: number): void {
  const root = docEl.value;
  if (!root) return;
  const rect = root.getBoundingClientRect();
  const edge = 40;
  if (clientY < rect.top + edge) root.scrollTop -= Math.ceil((rect.top + edge - clientY) / 4);
  else if (clientY > rect.bottom - edge) root.scrollTop += Math.ceil((clientY - (rect.bottom - edge)) / 4);
}

async function commitDrop(): Promise<void> {
  const from = dragFrom.value;
  dragFrom.value = null;
  if (from === null) return;
  const to = destinationFor(dropGap.value, from);
  if (to === from) return;
  await run(moveBlock(blocks.value, from, to));
}

function onGripPointerDown(index: number, event: PointerEvent): void {
  if (readOnly.value) return;
  event.preventDefault();
  const grip = event.currentTarget as HTMLElement | null;
  grip?.setPointerCapture?.(event.pointerId);
  dragFrom.value = index;
  dropGap.value = index;
}

function onGripPointerMove(event: PointerEvent): void {
  if (dragFrom.value === null) return;
  dropGap.value = gapAt(blockBoxes(), event.clientY);
  autoScroll(event.clientY);
}

function onGripPointerUp(event: PointerEvent): void {
  const grip = event.currentTarget as HTMLElement | null;
  if (grip?.hasPointerCapture?.(event.pointerId)) grip.releasePointerCapture(event.pointerId);
  void commitDrop();
}

/** 键盘重排：把手聚焦后用上下方向键移动。拖拽不是唯一入口，也是无障碍兜底。 */
async function onGripKeydown(index: number, event: KeyboardEvent): Promise<void> {
  if (readOnly.value) return;
  if (event.key !== 'ArrowUp' && event.key !== 'ArrowDown') return;
  event.preventDefault();
  const to = index + (event.key === 'ArrowUp' ? -1 : 1);
  if (to < 0 || to >= blocks.value.length) return;
  await run(moveBlock(blocks.value, index, to));
}

function onInsertBelow(index: number): void {
  void run(insertBelow(blocks.value, index));
}

/** 指示线画在哪：落在自己原来的缝就不画，免得看着像在动。 */
function dropMarker(index: number): 'before' | 'after' | undefined {
  const from = dragFrom.value;
  if (from === null) return undefined;
  const total = blocks.value.length;
  if (destinationFor(dropGap.value, from) === from) return undefined;
  if (dropGap.value < total && dropGap.value === index) return 'before';
  return dropGap.value === total && index === total - 1 ? 'after' : undefined;
}

const currentBlock = computed<EditorBlock | null>(() => blocks.value[activeIndex.value] ?? blocks.value[0] ?? null);
const currentIndent = computed(() => (currentBlock.value ? indentOf(currentBlock.value) : 0));
const currentType = computed(() => currentBlock.value?.type ?? 'paragraph');
const currentHeading = computed(() => (currentBlock.value ? headingLevel(currentBlock.value) : 1));

function elOf(id: string): HTMLElement | null {
  if (!docEl.value) return null;
  return docEl.value.querySelector<HTMLElement>(`[data-block-id="${id}"] .nb-content`);
}

function indexOf(id: string): number {
  return blocks.value.findIndex((block) => block.id === id);
}

function imageSrc(block: EditorBlock): string | null {
  const direct = safeHref(stringAttr(block, 'src'));
  if (direct) return direct;
  const data = stringAttr(block, 'dataUrl');
  if (data && /^data:image\//i.test(data)) return data;
  // 正文里只有内容键 sha256，字节在盘上 —— 显示要另取一次（取回来只活在内存的
  // URL 表里，写进块属性就等于写进 doc、随同步把附件在正文里再存一份）。
  return store.attachmentUrl(stringAttr(block, 'sha256'));
}

/** 缺附件时的那颗按钮：既要让同步去把 blob 拉回来，也要在拉回后重新取一次显示 URL。 */
function retryAttachment(block: EditorBlock): void {
  const sha = stringAttr(block, 'sha256') ?? stringAttr(block, 'ref');
  if (sha) void store.ensureAttachmentUrl(sha);
  void sync.syncNow();
}

function attachmentName(block: EditorBlock): string {
  return stringAttr(block, 'name') ?? stringAttr(block, 'fileName') ?? stringAttr(block, 'sha256')?.slice(0, 12) ?? '';
}

function attachmentMissing(block: EditorBlock): boolean {
  if (store.attachmentState(stringAttr(block, 'ref')) === 'missing') return true;
  return stringAttr(block, 'sha256') === undefined && stringAttr(block, 'ref') === undefined;
}

function formatSize(value: unknown): string {
  if (typeof value !== 'number' || !Number.isFinite(value)) return '';
  if (value < 1024) return `${value} B`;
  if (value < 1024 * 1024) return `${Math.round(value / 1024)} KB`;
  return `${(value / (1024 * 1024)).toFixed(1)} MB`;
}

async function capture(index: number): Promise<void> {
  const block = blocks.value[index];
  if (!block) return;
  activeIndex.value = index;
  const el = elOf(block.id);
  if (!el) return;
  const selection = selectionIn(el);
  if (selection) range.value = selection;
  const marks: string[] = [];
  for (const kind of MARK_KINDS) {
    if (hasMarkInRange(block.content, range.value.start, range.value.end, kind)) marks.push(kind);
  }
  activeMarks.value = marks;
  await updateSelBar(el);
}

/**
 * 浮动选区条：选中文字就浮到选区上方（AppFlowy 的做法），而不是逼人去够顶部工具条。
 * 位置按编辑区自己的矩形算，所以它永远盖不住顶部工具条，也永远不出可视区。
 */
const BAR_ITEMS = MARK_BUTTONS;
const selBox = ref<Box | null>(null);
const barEl = ref<HTMLElement | null>(null);
const barStyle = ref<Record<string, string>>({});

async function updateSelBar(el: HTMLElement): Promise<void> {
  const box = readOnly.value || slashQ.value !== null ? null : selectionBoxIn(el);
  selBox.value = box;
  if (!box) return;
  await nextTick();
  const view = docEl.value?.getBoundingClientRect();
  if (!view) return;
  const place = placeBar(
    box,
    { width: barEl.value?.offsetWidth || 260, height: barEl.value?.offsetHeight || 40 },
    { top: view.top, bottom: view.bottom, left: view.left, right: view.right },
  );
  barStyle.value = { top: `${place.top}px`, left: `${place.left}px` };
}

function hideSelBar(): void {
  selBox.value = null;
}


async function focusBlock(id: string, caret: number): Promise<void> {
  const el = elOf(id);
  if (!el) return;
  el.focus({ preventScroll: false });
  applySelection(el, caret, caret);
  const index = indexOf(id);
  if (index >= 0) {
    activeIndex.value = index;
    range.value = { start: caret, end: caret };
  }
  activeMarks.value = [];
}

async function run(edit: BlockEdit): Promise<void> {
  hideSelBar();
  store.commitStructural(edit.blocks, edit.focusId, edit.caret);
  await nextTick();
  if (edit.focusId) await focusBlock(edit.focusId, edit.caret);
}

function setHeading(index: number, level: number): BlockEdit {
  const edit = changeType(blocks.value, index, 'heading');
  const block = edit.blocks[index];
  if (block) edit.blocks[index] = { ...block, attrs: { ...block.attrs, level } };
  return edit;
}

function toggleMarkKind(kind: string): void {
  const index = activeIndex.value;
  const block = blocks.value[index];
  if (!block || block.shape !== 'text') return;
  if (kind === 'link') {
    run(applyMark(blocks.value, index, range.value, { kind: 'link' }, ['link']));
    return;
  }
  run(applyMark(blocks.value, index, range.value, { kind }));
}

async function onInput(index: number, event: Event): Promise<void> {
  if (readOnly.value) return;
  const block = blocks.value[index];
  const element = event.currentTarget as HTMLElement | null;
  if (!block || !element) return;
  const content = parseEditable(element);
  markAsParsed(element, content);
  store.updateBlock({ ...block, content });
  await capture(index);
  // 缩写优先：命中就换块型并吃掉前缀，此时不该再把它当命令查询
  const edit = markdownShortcut(store.blocks, index);
  if (edit) {
    slashQ.value = null;
    await run(edit);
    return;
  }
  slashQ.value = slashQuery(inlineText(content));
  slashSel.value = 0;
}

async function onKeydown(index: number, event: KeyboardEvent): Promise<void> {
  if (readOnly.value) return;
  const block = blocks.value[index];
  if (!block) return;
  await capture(index);
  const mod = event.ctrlKey || event.metaKey;
  const key = event.key;
  const text = inlineText(block.content);

  if (slashQ.value !== null) {
    if (key === 'Escape') {
      event.preventDefault();
      slashQ.value = null;
      return;
    }
    if (slashList.value.length > 0) {
      if (key === 'ArrowDown' || key === 'ArrowUp') {
        event.preventDefault();
        const n = slashList.value.length;
        slashSel.value = (slashSel.value + (key === 'ArrowDown' ? 1 : n - 1)) % n;
        return;
      }
      if (key === 'Enter' || key === 'Tab') {
        event.preventDefault();
        await chooseSlash(slashList.value[slashSel.value]);
        return;
      }
    }
  }

  if (mod && !event.altKey) {
    const lower = key.toLowerCase();
    if (lower === 'b' || lower === 'i' || lower === 'u') {
      event.preventDefault();
      toggleMarkKind(lower === 'b' ? 'bold' : lower === 'i' ? 'italic' : 'underline');
      return;
    }
    if (key === 'Enter') {
      event.preventDefault();
      await run(cycleChecklist(blocks.value, index));
      return;
    }
    if (lower === '1' || lower === '2' || lower === '3') {
      event.preventDefault();
      await run(setHeading(index, Number(lower)));
      return;
    }
  }

  // Alt+↑/↓：不离开正文就能把当前块上移/下移一行，光标停在原列
  if (event.altKey && !mod && (key === 'ArrowUp' || key === 'ArrowDown')) {
    event.preventDefault();
    const to = index + (key === 'ArrowUp' ? -1 : 1);
    if (to < 0 || to >= blocks.value.length) return;
    const edit = moveBlock(blocks.value, index, to);
    edit.caret = range.value.start;
    await run(edit);
    return;
  }

  if (key === 'Enter' && !event.shiftKey && block.type !== 'codeBlock') {
    event.preventDefault();
    await run(splitAt(blocks.value, index, range.value.end));
    return;
  }

  if (key === 'Tab') {
    event.preventDefault();
    await run(shiftIndent(blocks.value, index, event.shiftKey ? -1 : 1));
    return;
  }

  if (key === 'Backspace' && range.value.start === 0 && range.value.end === 0) {
    if (index === 0 && blocks.value.length === 1) {
      event.preventDefault();
      return;
    }
    event.preventDefault();
    const merged = mergeWithPrevious(blocks.value, index);
    await run(merged ?? { blocks: [...blocks.value], focusId: block.id, caret: 0 });
    return;
  }

  if (key === 'ArrowUp' && range.value.start === 0 && range.value.end === 0 && index > 0) {
    const previous = blocks.value[index - 1];
    if (previous && previous.shape === 'text') {
      event.preventDefault();
      await focusBlock(previous.id, blockText(previous).length);
    }
    return;
  }

  if (key === 'ArrowDown' && range.value.start >= text.length && index < blocks.value.length - 1) {
    const next = blocks.value[index + 1];
    if (next && next.shape === 'text') {
      event.preventDefault();
      await focusBlock(next.id, 0);
    }
  }
}

async function onPaste(index: number, event: ClipboardEvent): Promise<void> {
  if (readOnly.value) return;
  event.preventDefault();
  const text = event.clipboardData?.getData('text/plain') ?? '';
  if (text.length === 0) return;
  await capture(index);
  await run(insertText(blocks.value, index, range.value, text.replace(/\r\n?/g, '\n')));
}

async function onBlurBlock(index: number): Promise<void> {
  const block = blocks.value[index];
  if (!block) return;
  const element = elOf(block.id);
  if (element) {
    const content = parseEditable(element);
    markAsParsed(element, content);
    if (JSON.stringify(content) !== JSON.stringify(block.content)) store.updateBlock({ ...block, content });
  }
  await store.flush();
  // flush 是异步的：这期间用户可能已经点到别的块并选中了文字，
  // 无条件收起会把刚浮出来的工具条抹掉。只有焦点确实离开编辑区才收。
  if (!docEl.value?.contains(document.activeElement)) hideSelBar();
}

function onCheckbox(block: EditorBlock, index: number): void {
  activeIndex.value = index;
  void run(setChecked(blocks.value, index, !isChecked(block)));
}

function onType(type: TextBlockType): void {
  void run(changeType(blocks.value, activeIndex.value, type));
}

function onHeading(level: number): void {
  void run(setHeading(activeIndex.value, level));
}

function onRule(): void {
  void run(insertRule(blocks.value, activeIndex.value));
}

function onIndent(delta: number): void {
  void run(shiftIndent(blocks.value, activeIndex.value, delta));
}

function onDeleteBlock(): void {
  void run(removeBlockAt(blocks.value, activeIndex.value));
}

function onBackspaceInBlock(index: number): void {
  void run(backspace(blocks.value, index, range.value));
}

/**
 * "插入图片 / 附件"走一个隐藏的 `<input type=file>`：在 Tauri 的 WebView 里点的就是
 * 操作系统原生的选择器，而在浏览器 dev 桥里是同一条代码路径 —— 于是这一步能被端到端
 * 真的点一遍（原生对话框插件那条路做不到，只能标 BLOCKED）。
 */
const attachInput = ref<HTMLInputElement | null>(null);
const attachRole = ref<'inline' | 'file'>('inline');

function onAttach(role: 'inline' | 'file'): void {
  attachRole.value = role;
  if (attachInput.value) {
    attachInput.value.accept = role === 'inline' ? 'image/*' : '';
    attachInput.value.value = '';
    attachInput.value.click();
  }
}

async function onAttachPicked(event: Event): Promise<void> {
  const input = event.target as HTMLInputElement | null;
  const file = input?.files?.[0] ?? null;
  await store.attachFile(attachRole.value, file);
  if (input) input.value = '';
}

// 全局快捷键（Shift+F）递过来的意图，走的还是上面这条唯一的路径。
watch(
  () => store.attachRequest,
  (role) => {
    if (!role) return;
    store.clearAttachRequest();
    onAttach(role);
  },
);

/**
 * 点在最后一行**下面的空白**：光标送到最后一个文本块的末尾。
 *
 * 不接这一句，这次点击会把焦点丢给 `body`（块在文档上半部，空白处不属于任何块），
 * 于是"点一下空白、马上开始打字"的头几个字直接消失。Apple Notes / AppFlowy 在这
 * 一处的行为都是"照旧能写"，所以这里补的是原生感，不是装饰。
 * 焦点与光标仍走 `focusBlock` 那一条唯一的路径，不在这里另写一套选区逻辑。
 */
function onBlankClick(ev: MouseEvent): void {
  if (readOnly.value) return;
  const target = ev.target as HTMLElement | null;
  // 落在块里的点击归块自己管（contenteditable + `@click="capture"`），这里不插手
  if (!target || target.closest('.nb-block')) return;
  const last = [...blocks.value].reverse().find((block) => block.shape === 'text');
  const element = last ? elOf(last.id) : null;
  if (!last || !element) return;
  void focusBlock(last.id, measureEditable(element).text.length);
}

function onNative(kind: 'undo' | 'redo'): void {
  const block = currentBlock.value;
  const element = block ? elOf(block.id) : null;
  element?.focus();
  const executable = document as Document & { execCommand?: (name: string) => boolean };
  try {
    executable.execCommand?.(kind);
  } catch {
    // WebView 未实现该命令时什么都不做：内容仍与模型一致
  }
  if (element && block) {
    const content = parseEditable(element);
    markAsParsed(element, content);
    if (JSON.stringify(content) !== JSON.stringify(block.content)) store.updateBlock({ ...block, content });
  }
}

function consumeHint(): void {
  const hint = store.consumeFocusHint();
  if (hint?.id) void focusBlock(hint.id, hint.caret);
}

onMounted(() => {
  consumeHint();
  const first = blocks.value[0];
  if (!readOnly.value && first && first.shape === 'text') void focusBlock(first.id, inlineText(first.content).length);
});

defineExpose({ onBackspaceInBlock, focusBlock, capture });
</script>

<template>
  <div class="editor">
    <!-- 附件的唯一取文件入口。`aria-hidden` + 不占位：它不是给用户看的控件，
         但必须真的在 DOM 里 —— 端到端就是往它塞文件来验这条边的。 -->
    <input
      ref="attachInput"
      type="file"
      class="editor-file-input"
      data-testid="attach-input"
      tabindex="-1"
      aria-hidden="true"
      @change="onAttachPicked"
    />
    <EditorToolbar
      :disabled="readOnly"
      :active-marks="activeMarks"
      :block-type="currentType"
      :heading-level="currentHeading"
      :indent="currentIndent"
      :can-indent="currentBlock?.shape === 'text'"
      @mark="toggleMarkKind"
      @type="onType"
      @heading="onHeading"
      @checklist="run(cycleChecklist(blocks, activeIndex))"
      @indent="onIndent"
      @rule="onRule"
      @attach="onAttach"
      @link="(href: string) => run(applyMark(blocks, activeIndex, range, { kind: 'link', attrs: { href } }, ['link']))"
      @undo="onNative('undo')"
      @redo="onNative('redo')"
    />

    <div ref="docEl" class="editor-scroll" data-testid="editor-doc" @click="onBlankClick">
      <div class="editor-doc" :data-readonly="readOnly ? 'true' : 'false'">
        <div
          v-for="(block, index) in blocks"
          :key="block.id"
          class="nb-block"
          :data-type="block.type"
          :data-block-id="block.id"
          :data-level="block.type === 'heading' ? headingLevel(block) : undefined"
          :data-checked="block.type === 'checklistItem' ? (isChecked(block) ? 'true' : 'false') : undefined"
          :data-readonly="readOnly ? 'true' : 'false'"
          :data-conflict="store.hasDraftConflict && index === 0 ? 'true' : 'false'"
          :data-drop="dropMarker(index)"
          :class="{ 'nb-block--drag': dragFrom === index }"
          :style="{ marginLeft: `calc(${indentOf(block)} * var(--space-4))` }"
        >
          <div
            v-if="!readOnly"
            class="nb-handles"
            :class="{ 'nb-handles--on': dragFrom === index || activeIndex === index }"
          >
            <button
              type="button"
              class="nb-handle nb-handle--add"
              data-testid="insert-below"
              tabindex="-1"
              :aria-label="t('editor.insertBelow')"
              :title="t('editor.insertBelow')"
              @mousedown.prevent
              @click="onInsertBelow(index)"
            >
              +
            </button>
            <span
              class="nb-handle nb-grip"
              role="button"
              tabindex="0"
              data-testid="drag-handle"
              :aria-label="t('editor.dragHandle')"
              :title="t('editor.dragHint')"
              @pointerdown="onGripPointerDown(index, $event)"
              @pointermove="onGripPointerMove"
              @pointerup="onGripPointerUp"
              @pointercancel="onGripPointerUp"
              @keydown="onGripKeydown(index, $event)"
            >⠿</span>
          </div>

          <span v-if="block.type === 'orderedList'" class="nb-gutter" aria-hidden="true">{{ numbers[index] }}.</span>
          <span v-else-if="block.type === 'bulletList'" class="nb-gutter" aria-hidden="true">•</span>

          <button
            v-if="block.type === 'checklistItem'"
            type="button"
            class="nb-check"
            role="checkbox"
            :aria-checked="isChecked(block) ? 'true' : 'false'"
            :aria-label="t('tb.checklist')"
            @mousedown.prevent
            @click="onCheckbox(block, index)"
          >
            <span class="nb-check__box" aria-hidden="true">{{ isChecked(block) ? '✓' : '' }}</span>
          </button>

          <ul
            v-if="slashQ !== null && index === activeIndex && slashList.length > 0"
            class="slash-menu"
            role="listbox"
            :aria-label="t('slash.menu')"
            data-testid="slash-menu"
          >
            <li v-for="(item, i) in slashList" :key="`${item.type}-${i}`">
              <button
                type="button"
                role="option"
                class="slash-item"
                :class="{ 'slash-item--on': i === slashSel }"
                :aria-selected="i === slashSel ? 'true' : 'false'"
                :data-testid="`slash-${item.type}-${i}`"
                @mousedown.prevent
                @click="chooseSlash(item)"
              >
                <span class="slash-item__name">{{ t(item.labelKey) }}</span>
                <span class="slash-item__hint">{{ t(item.hintKey) }}</span>
              </button>
            </li>
          </ul>

          <div
            v-if="block.shape === 'text'"
            class="nb-content"
            v-editable="{ content: block.content, type: block.type }"
            :contenteditable="readOnly ? 'false' : 'true'"
            role="textbox"
            :aria-multiline="true"
            :aria-readonly="readOnly ? 'true' : 'false'"
            :data-placeholder="t('editor.placeholder')"
            @input="onInput(index, $event)"
            @keydown="onKeydown(index, $event)"
            @paste="onPaste(index, $event)"
            @blur="onBlurBlock(index)"
            @click="capture(index)"
            @keyup="capture(index)"
            @mouseup="capture(index)"
          />

          <figure v-else-if="block.shape === 'image'" class="nb-media">
            <img v-if="imageSrc(block)" class="nb-image" :src="imageSrc(block) ?? undefined" :alt="stringAttr(block, 'alt') ?? ''" />
            <figcaption v-else class="nb-chip nb-chip--missing">
              <span class="nb-chip__glyph" aria-hidden="true">▦</span>
              <span>{{ t('editor.imageMissing') }}</span>
              <span v-if="attachmentName(block)" class="nb-chip__meta">{{ attachmentName(block) }}</span>
              <button type="button" class="btn btn--quiet" @click="retryAttachment(block)">{{ t('editor.attachmentDownload') }}</button>
            </figcaption>
          </figure>

          <div v-else-if="block.shape === 'attachment'" class="nb-media">
            <span class="nb-chip" :class="{ 'nb-chip--missing': attachmentMissing(block) }">
              <span class="nb-chip__glyph" aria-hidden="true">▤</span>
              <span>{{ attachmentName(block) || t('editor.blockAttachment') }}</span>
              <span v-if="formatSize(block.attrs.size)" class="nb-chip__meta">{{ formatSize(block.attrs.size) }}</span>
              <template v-if="attachmentMissing(block)">
                <span class="nb-chip__meta">{{ t('editor.attachmentMissing') }}</span>
                <button type="button" class="btn btn--quiet" @click="retryAttachment(block)">{{ t('editor.attachmentDownload') }}</button>
              </template>
            </span>
          </div>

          <div v-else-if="block.shape === 'rule'" class="nb-rule" role="separator" :aria-label="t('editor.blockRule')" />

          <div v-else class="nb-unknown" :aria-label="t('editor.unknownBlock')">
            <div class="nb-unknown__head">
              <span>{{ t('editor.unknownBlock') }}</span>
              <span class="nb-unknown__type">{{ block.type }}</span>
            </div>
            <pre class="nb-unknown__json">{{ JSON.stringify(block.raw, null, 2) }}</pre>
          </div>

          <span v-if="block.shape === 'text' && block.type === 'codeBlock' && stringAttr(block, 'lang')" class="nb-flag">
            {{ stringAttr(block, 'lang') }}
          </span>
        </div>
      </div>

      <p v-if="readOnly && blocks.length > 0" class="editor-note" role="status">
        <template v-if="store.readOnlyReason === 'versionTooNew'">{{ t('editor.versionTooNew') }}</template>
        <template v-else>{{ t('state.inTrash') }}</template>
      </p>
    </div>

    <div class="editor-corner">
      <span class="text-sm text-muted">{{ charCount }}</span>
      <span class="text-sm" :data-save-state="store.saveState">{{ store.saveLabel }}</span>
      <button v-if="!readOnly && currentBlock" type="button" class="btn btn--quiet text-sm" :title="t('editor.deleteBlock')" @click="onDeleteBlock">
        {{ t('editor.deleteBlock') }}
      </button>
    </div>
    <div
      v-if="selBox"
      ref="barEl"
      class="sel-bar"
      :style="barStyle"
      role="toolbar"
      :aria-label="t('editor.selectionBar')"
      data-testid="selection-bar"
    >
      <button
        v-for="item in BAR_ITEMS"
        :key="item.kind"
        type="button"
        class="sel-bar__btn"
        :class="{ 'sel-bar__btn--on': activeMarks.includes(item.kind) }"
        :aria-pressed="activeMarks.includes(item.kind) ? 'true' : 'false'"
        :aria-label="t(item.label)"
        :data-testid="`sel-${item.kind}`"
        @mousedown.prevent
        @click="toggleMarkKind(item.kind)"
      >
        {{ item.glyph }}
      </button>
    </div>
  </div>
</template>

<style scoped>
.editor {
  display: flex;
  flex-direction: column;
  min-height: 0;
  flex: 1;
}

/* 隐藏的取文件入口：不占位、不吃焦点，但留在可测的 DOM 里。 */
.editor-file-input {
  position: absolute;
  width: 1px;
  height: 1px;
  opacity: 0;
  pointer-events: none;
}

.editor-note {
  max-width: var(--editor-measure);
  margin: var(--space-4) auto 0;
  padding: var(--space-3);
  border: 1px solid var(--border-strong);
  border-radius: var(--radius-2);
  background: var(--bg-sunken);
  color: var(--text-secondary);
  font-size: var(--text-sm);
}

.editor-corner {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: var(--space-3);
  padding: var(--space-2) var(--space-4);
  border-top: 1px solid var(--border-subtle);
  background: var(--bg-pane);
  min-height: var(--touch-min);
}

.editor-corner [data-save-state='saving'],
.editor-corner [data-save-state='pending'] {
  color: var(--text-muted);
}

.editor-corner [data-save-state='error'] {
  color: var(--danger);
}

.editor-corner [data-save-state='saved'] {
  color: var(--success);
}

/* 浮动选区条：fixed 定位（坐标按视口算），层级压在正文与侧栏之上 */
.sel-bar {
  position: fixed;
  z-index: 30;
  display: flex;
  align-items: center;
  gap: var(--space-1);
  padding: var(--space-1);
  border: 1px solid var(--border-strong);
  border-radius: var(--radius-2);
  background: var(--bg-raised);
  box-shadow: var(--shadow-2);
}

.sel-bar__btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-width: 32px;
  height: 32px;
  padding: 0 var(--space-2);
  border: 0;
  border-radius: var(--radius-1);
  background: none;
  color: var(--text-primary);
  font: inherit;
  font-size: var(--text-sm);
  line-height: 1;
  cursor: pointer;
}

.sel-bar__btn:hover {
  background: var(--bg-hover);
}

.sel-bar__btn--on {
  background: var(--accent-soft);
  color: var(--accent);
}

.sel-bar__btn:focus-visible {
  outline: var(--focus-width) solid var(--border-focus);
  outline-offset: 1px;
}

@media (pointer: coarse) {
  .sel-bar__btn {
    min-width: var(--touch-min);
    height: var(--touch-min);
  }
}
</style>
