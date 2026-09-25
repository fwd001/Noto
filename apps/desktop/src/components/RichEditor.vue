<script setup lang="ts">
/**
 * 富文本编辑器：contenteditable + 块模型双向映射。
 * 不依赖任何第三方编辑器：块 id 由我们生成并保持，未知块只读保留，版本过高只读。
 */
import { computed, nextTick, onMounted, ref } from 'vue';
import EditorToolbar from './EditorToolbar.vue';
import { vEditable, markAsParsed } from '../editor/editableDirective';
import { applySelection, parseEditable, safeHref, selectionIn } from '../editor/dom';
import {
  applyMark,
  backspace,
  changeType,
  cycleChecklist,
  insertRule,
  insertText,
  mergeWithPrevious,
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
  return data && /^data:image\//i.test(data) ? data : null;
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
}

async function onKeydown(index: number, event: KeyboardEvent): Promise<void> {
  if (readOnly.value) return;
  const block = blocks.value[index];
  if (!block) return;
  await capture(index);
  const mod = event.ctrlKey || event.metaKey;
  const key = event.key;
  const text = inlineText(block.content);

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

function onAttach(role: 'inline' | 'file'): void {
  void store.attachFile(role);
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

    <div ref="docEl" class="editor-scroll" data-testid="editor-doc">
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
          :style="{ marginLeft: `calc(${indentOf(block)} * var(--space-4))` }"
        >
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

          <div
            v-if="block.shape === 'text'"
            class="nb-content"
            v-editable="{ content: block.content }"
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
          />

          <figure v-else-if="block.shape === 'image'" class="nb-media">
            <img v-if="imageSrc(block)" class="nb-image" :src="imageSrc(block) ?? undefined" :alt="stringAttr(block, 'alt') ?? ''" />
            <figcaption v-else class="nb-chip nb-chip--missing">
              <span class="nb-chip__glyph" aria-hidden="true">▦</span>
              <span>{{ t('editor.imageMissing') }}</span>
              <span v-if="attachmentName(block)" class="nb-chip__meta">{{ attachmentName(block) }}</span>
              <button type="button" class="btn btn--quiet" @click="sync.syncNow()">{{ t('editor.attachmentDownload') }}</button>
            </figcaption>
          </figure>

          <div v-else-if="block.shape === 'attachment'" class="nb-media">
            <span class="nb-chip" :class="{ 'nb-chip--missing': attachmentMissing(block) }">
              <span class="nb-chip__glyph" aria-hidden="true">▤</span>
              <span>{{ attachmentName(block) || t('editor.blockAttachment') }}</span>
              <span v-if="formatSize(block.attrs.size)" class="nb-chip__meta">{{ formatSize(block.attrs.size) }}</span>
              <template v-if="attachmentMissing(block)">
                <span class="nb-chip__meta">{{ t('editor.attachmentMissing') }}</span>
                <button type="button" class="btn btn--quiet" @click="sync.syncNow()">{{ t('editor.attachmentDownload') }}</button>
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
  </div>
</template>

<style scoped>
.editor {
  display: flex;
  flex-direction: column;
  min-height: 0;
  flex: 1;
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
</style>
