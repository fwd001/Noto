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
import { formatModified } from '../util/format';
import { useSettingsStore } from '../stores/settings';
import { useShellStore } from '../stores/shell';
import { useToastStore } from '../stores/toasts';
import type { Inline } from '../api/types';
import { t, type MessageKey } from '../i18n';
import { attachmentNotice } from '../editor/attachmentNotice';
import { roleForFile } from '../editor/attachmentWire';
import AppIcon from './ui/AppIcon.vue';
import { SAVE_ICONS, type IconName } from './ui/icons';

const MARK_KINDS = ['bold', 'italic', 'underline', 'strike', 'code', 'highlight', 'link', 'fontSize', 'color'] as const;

/**
 * 选区里这一档标记的属性值（"当前是哪一个字号 / 哪一种颜色"要给菜单里的
 * `aria-checked` 用）。混合选区取先遇到的那个值 —— 这里要的是"该勾哪一项"，
 * 不是"这批字符是不是完全同构"。
 */
function attrInRange(content: readonly Inline[], start: number, end: number, kind: string, key: string): string | null {
  if (end <= start) return null;
  let cursor = 0;
  for (const inline of content) {
    const from = cursor;
    const to = cursor + inline.text.length;
    cursor = to;
    if (to <= start || from >= end) continue;
    const mark = (inline.marks ?? []).find((m) => m.kind === kind);
    const value = mark?.attrs?.[key];
    if (typeof value === 'string') return value;
  }
  return null;
}

const store = useEditorStore();

/** 保存那一格的图形：与文案同读 `store.saveKind`（表在 `ui/icons.ts`，四格共用一张纸这个载体）。 */
const saveIcon = computed<IconName>(() => SAVE_ICONS[store.saveKind ?? ''] ?? 'save-saved');
const shell = useShellStore();
const settings = useSettingsStore();
const toasts = useToastStore();

const docEl = ref<HTMLElement | null>(null);
const blocks = computed(() => store.blocks);
const readOnly = computed(() => store.writeBlocked);
const numbers = computed(() => orderedNumbers(blocks.value));
const activeIndex = ref(0);
const range = ref({ start: 0, end: 0 });
const activeMarks = ref<string[]>([]);
const activeFontSize = ref<string | null>(null);
const activeColor = ref<string | null>(null);
const charCount = computed(() => docCharCount(blocks.value));
/** 设计稿那句 `改于 14:22`：读核心的 `updatedAt`。取不到就整条不画，不许编一个"刚刚"。 */
const modifiedAt = computed(() => formatModified(store.noteUpdatedAt));

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

/**
 * 缺附件/坏附件占位上的两个用户动作（2026-09-28 的决定：终态那一格必须给用户一个能点的东西）。
 *
 * 两颗都递意图，不做判断 —— 前端看不见账上那对状态（本机有没有、远端被不被否定），
 * 而"什么时候该重试、什么时候该覆盖上传"是核心的判据。在这里猜就等于在前端长第二台
 * 状态机（§39），而判据漂掉之后表现是"产品没问题、按钮点了没反应"。
 * 该按钮不适用于当前情况时，核心回的是**具名错误**，界面上有明确的 toast 说明为什么 ——
 * 安静地不显示按钮同样是让用户猜，两条比不过"点一下然后听懂为什么不行"。
 */
function retryAttachment(block: EditorBlock): void {
  void store.retryAttachmentFetch(stringAttr(block, 'sha256') ?? stringAttr(block, 'ref'));
}

function reuploadAttachment(block: EditorBlock): void {
  void store.reuploadAttachment(stringAttr(block, 'sha256') ?? stringAttr(block, 'ref'));
}

function attachmentName(block: EditorBlock): string {
  return stringAttr(block, 'name') ?? stringAttr(block, 'fileName') ?? stringAttr(block, 'sha256')?.slice(0, 12) ?? '';
}

/**
 * 这颗附件芯片该说的那句话（`null` = 没什么要说）。
 *
 * 判据来自核心的那本账（`attachment_states`），**不是**"这次会话上传过没有"：
 * 后者让从别的设备同步来、本机还没字节的附件画起来跟完好的一样 —— 既不说明缺，
 * 也不给那两颗已存在的动作（G74）。
 * 两个例外都要说：块里连内容键都没有（写坏了的引用），以及账说本机没有可用字节。
 */
function attachmentNoticeKey(block: EditorBlock): MessageKey | null {
  const sha = stringAttr(block, 'sha256') ?? stringAttr(block, 'ref');
  if (!sha) return 'editor.attachmentNotOnDevice';
  const pair = store.attachmentLedger(sha);
  if (pair) return attachmentNotice(pair);
  // 账还没回（这篇刚打开、那次问账还在飞，或是本次会话刚传上的那颗）：
  // 保留原有的"本次会话里它报过 missing"这一格判据 —— 加了新判据不该把已有信号弄丢。
  if (store.attachmentState(stringAttr(block, 'ref')) === 'missing') return 'editor.attachmentNotOnDevice';
  return null;
}

/** 模板里只做一次取值；空串表示这颗芯片没什么要说（`v-if` 已经把那一格挡住了）。 */
function attachmentNoticeText(block: EditorBlock): string {
  const key = attachmentNoticeKey(block);
  return key ? t(key) : '';
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
  activeFontSize.value = attrInRange(block.content, range.value.start, range.value.end, 'fontSize', 'step');
  activeColor.value = attrInRange(block.content, range.value.start, range.value.end, 'color', 'name');
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
  activeFontSize.value = null;
  activeColor.value = null;
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

/**
 * 带属性的样式档（字号 / 颜色）：先剥掉同类的旧值再放新的，所以"从大改成特大"是**替换**，
 * 而不是叠两层（叠两层的渲染结果取决于顺序，同步到另一台设备上顺序一变就变样）。
 */
function applyStyledMark(kind: string, attrs: Record<string, unknown> | undefined): void {
  const index = activeIndex.value;
  const block = blocks.value[index];
  if (!block || block.shape !== 'text') return;
  run(applyMark(blocks.value, index, range.value, { kind, ...(attrs ? { attrs } : {}) }, [kind]));
}

/**
 * 去掉一档样式（菜单里"标准 / 默认色"那颗）。走的是不带 exclusive 的 toggle：
 * 整段都已有该样式时它就是把这一位摘掉，与"整段已加粗时再切换等于取消"同一条路。
 */
function removeStyledMark(kind: string): void {
  const index = activeIndex.value;
  const block = blocks.value[index];
  if (!block || block.shape !== 'text') return;
  run(applyMark(blocks.value, index, range.value, { kind }));
}

/** 工具条那三颗普通样式是开关，两颗带档位的（字号 / 颜色）是替换 —— 走两条路，别硬塞进一个 handler。 */
function onMark(kind: string, attrs?: Record<string, unknown>): void {
  if (kind === 'fontSize' || kind === 'color') applyStyledMark(kind, attrs);
  else toggleMarkKind(kind);
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

/**
 * 拖文件进窗口（§6 那一格：能力位 `dragAndDrop` 早就开了，缺的只是这一个处理器）。
 *
 * 分流只有一条规则：图片进正文、其余进附件行 —— 与工具条那两颗按钮同一套语义；
 * 落地走的是**同一个** `store.attachFile`（32 MiB 上限、空文件、rev 接管、失败各自的回执都在那里，
 * 不在这里再实现一遍）。只有一件事必须在这儿做：**不能写的时候要说得出为什么**，
 * 而不是"松手了，什么都没发生"（`attachFile` 在只读时是静默 return 的）。
 */
const dropping = ref(false);

/** 只读那一格的说法与正文上方那条横幅同一份键表，不分两处各写一句。 */
const DROP_BLOCKED_KEYS: Record<string, MessageKey> = {
  libraryReadOnly: 'editor.libraryReadOnly',
  versionTooNew: 'editor.versionTooNew',
  inTrash: 'state.inTrash',
};

function draggedFiles(event: DragEvent): File[] {
  if (!Array.from(event.dataTransfer?.types ?? []).includes('Files')) return [];
  return Array.from(event.dataTransfer?.files ?? []);
}

function onDragOver(event: DragEvent): void {
  if (!settings.caps.dragAndDrop || draggedFiles(event).length === 0) return;
  // 不拦这一下，浏览器会自己去"打开"这个文件 —— 整个界面被换掉，用户以为程序崩了
  event.preventDefault();
  if (event.dataTransfer) event.dataTransfer.dropEffect = store.writeBlocked ? 'none' : 'copy';
  dropping.value = true;
}

function onDragLeave(event: DragEvent): void {
  // 从容器移到它的子元素上同样会触发 dragleave：指针还在这一栏里就不许把提示收掉
  const next = event.relatedTarget;
  if (next instanceof Node && event.currentTarget instanceof Node && event.currentTarget.contains(next)) return;
  dropping.value = false;
}

async function onDrop(event: DragEvent): Promise<void> {
  dropping.value = false;
  const files = draggedFiles(event);
  if (files.length === 0) return;
  // 只要真的是文件，就先拦下浏览器的默认行为（它会自己去打开这个文件），
  // 再谈这一档能不能写 —— 顺序反了就等于"拒绝的同时把整个界面换掉"。
  event.preventDefault();
  if (!settings.caps.dragAndDrop) return;
  if (store.writeBlocked) {
    toasts.push(DROP_BLOCKED_KEYS[store.readOnlyReason ?? ''] ?? 'editor.notWritable', 'warn');
    return;
  }
  for (const file of files) await store.attachFile(roleForFile(file), file);
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
 * 软键盘弹起（或收起）之后，把**正在编辑那一行**带回可视区。
 *
 * `shell` 已经让版心跟着可视视口缩了（iOS 的键盘不改 `innerHeight`，只改 `visualViewport`），
 * 但缩完之后**没人管光标**：编辑区那一栏从底下被截掉 300 px，焦点行如果本来就在下面那一段，
 * 它就留在键盘底下 —— 字还在打，屏幕上看不见（390×844 实测：焦点行底 734 vs 版心 544，
 * 而 `scrollTop` 一动没动）。这里只补"滚回来看得见的地方"这一句，焦点与选区都不碰。
 */
watch(
  () => shell.keyboardInset,
  async () => {
    await nextTick();
    const el = document.activeElement;
    const root = docEl.value;
    if (!(el instanceof HTMLElement) || !root?.contains(el)) return;
    const box = root.getBoundingClientRect();
    const r = el.getBoundingClientRect();
    if (r.top >= box.top && r.bottom <= box.bottom) return; // 本来就看得见，不许乱跳
    el.scrollIntoView({ block: 'center', behavior: 'auto' });
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
  <div class="editor" @dragover="onDragOver" @dragleave="onDragLeave" @drop="onDrop">
    <!-- 只在指针真的带着文件进来时出现（§4.9：浮层不参与布局；能力不存在时这颗压根不画）。
         `pointer-events:none` 是必须的 —— 它一旦吃住指针，drop 就落不到容器上，
         提示会变成"看得见但松不了手"的那一层。 -->
    <div v-if="dropping" class="editor-drop" data-testid="editor-drop" role="status">
      <AppIcon :size="20" name="attach-file" />
      <span>{{ t('editor.dropHere') }}</span>
    </div>
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
      :active-font-size="activeFontSize"
      :active-color="activeColor"
      @mark="onMark"
      @unmark="removeStyledMark"
      @type="onType"
      @heading="onHeading"
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
          :style="{ marginLeft: `calc(${indentOf(block)} * var(--sp-4))` }"
        >
          <div
            v-if="!readOnly"
            class="nb-handles"
            :class="{ 'nb-handles--on': dragFrom === index || activeIndex === index }"
          >
            <!-- 行首那颗 `+`（在这一行下面插一块）已经拿掉：回车就是新起一行，
                 工具条上还有"插入附件/图片/分隔线"，那颗 `+` 是第三种做同一件事的入口，
                 而且它让每一行左边看起来像有一列加减号控件 —— 用户原话：
                 「这块我觉得有没有必要还有一个加号减号这种感觉」。拖动排序那颗 ⠿ 保留。 -->
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
            ><AppIcon :size="18" name="drag-handle" /></span>
          </div>

          <span v-if="block.type === 'orderedList'" class="nb-gutter" aria-hidden="true">{{ numbers[index] }}.</span>
          <span v-else-if="block.type === 'bulletList'" class="nb-gutter nb-gutter--dot" aria-hidden="true" />

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
            <span class="nb-check__box" aria-hidden="true"><AppIcon v-if="isChecked(block)" :size="16" name="check" /></span>
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
            :data-placeholder="index === 0 ? t('editor.placeholder') : ''"
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
              <AppIcon class="nb-chip__glyph" :size="18" name="attach-image" />
              <span>{{ t('editor.imageMissing') }}</span>
              <span v-if="attachmentName(block)" class="nb-chip__meta">{{ attachmentName(block) }}</span>
              <button type="button" class="btn btn--quiet" data-testid="attachment-retry" @click="retryAttachment(block)">
                {{ t('editor.attachmentRetry') }}
              </button>
              <button type="button" class="btn btn--quiet" data-testid="attachment-reupload" @click="reuploadAttachment(block)">
                {{ t('editor.attachmentReupload') }}
              </button>
            </figcaption>
          </figure>

          <div v-else-if="block.shape === 'attachment'" class="nb-media">
            <span class="nb-chip" :class="{ 'nb-chip--missing': attachmentNoticeKey(block) !== null }">
              <AppIcon class="nb-chip__glyph" :size="18" name="attach-file" />
              <span>{{ attachmentName(block) || t('editor.blockAttachment') }}</span>
              <span v-if="formatSize(block.attrs.size)" class="nb-chip__meta">{{ formatSize(block.attrs.size) }}</span>
              <template v-if="attachmentNoticeKey(block)">
                <span class="nb-chip__meta" data-testid="attachment-notice">{{ attachmentNoticeText(block) }}</span>
                <button type="button" class="btn btn--quiet" data-testid="attachment-retry" @click="retryAttachment(block)">
                  {{ t('editor.attachmentRetry') }}
                </button>
                <button type="button" class="btn btn--quiet" data-testid="attachment-reupload" @click="reuploadAttachment(block)">
                  {{ t('editor.attachmentReupload') }}
                </button>
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
        <template v-else-if="store.readOnlyReason === 'libraryReadOnly'">{{ t('editor.libraryReadOnly') }}</template>
        <template v-else>{{ t('state.inTrash') }}</template>
      </p>
    </div>

    <div class="editor-corner">
      <span class="text-sm text-muted">{{ charCount }}</span>
      <span
        v-if="store.saveKind"
        class="editor-corner__save"
        :data-save-state="store.saveKind"
        data-testid="save-state"
      >
        <AppIcon
          class="editor-corner__glyph"
          :name="saveIcon"
          :size="16"
          :data-spin="store.saveKind === 'saving' ? 'true' : 'false'"
        />
        <span class="text-sm">{{ store.saveLabel }}</span>
      </span>
      <span v-if="modifiedAt" class="text-sm text-muted" data-testid="editor-modified">{{ t('editor.modifiedAt', { time: modifiedAt }) }}</span>
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
        <AppIcon :size="18" :name="item.icon" />
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
  /* 拖放提示是浮在这一栏上的，所以它得是定位父级 */
  position: relative;
}

.editor-drop {
  position: absolute;
  inset: var(--sp-2);
  z-index: 20;
  display: flex;
  align-items: center;
  justify-content: center;
  gap: var(--sp-2);
  border: 2px dashed var(--accent);
  border-radius: var(--r-card);
  background: var(--accent-soft);
  color: var(--ink);
  font-size: var(--text-sm);
  font-weight: 600;
  pointer-events: none;
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
  max-width: var(--measure);
  margin: var(--sp-4) auto 0;
  padding: var(--sp-3);
  border: 1px solid var(--line-strong);
  border-radius: var(--r-card);
  background: var(--sunken);
  color: var(--body);
  font-size: var(--text-sm);
}

.editor-corner {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: var(--sp-3);
  padding: var(--sp-2) var(--sp-4);
  border-top: 1px solid var(--line);
  background: var(--canvas);
  min-height: var(--touch);
}

.editor-corner [data-save-state='saving'] {
  color: var(--mute);
}

/* 「还有改动没存」是一句事实，不是警告也不是错误 —— 用 --body，别抢 error 那格的注意力。 */
.editor-corner [data-save-state='dirty'] {
  color: var(--body);
}

.editor-corner [data-save-state='error'] {
  color: var(--danger);
}

.editor-corner [data-save-state='saved'] {
  color: var(--ok);
}

/* §4.1 那一格：图形与文字读同一个派生值（`saveKind`），所以它俩不会各说各话。 */
.editor-corner__save {
  display: inline-flex;
  align-items: center;
  gap: var(--sp-2);
}

/* §1.6 / §2.3 同一条规矩：四格里**只有"正在保存"会动**。 */
.editor-corner__glyph[data-spin='true'] {
  animation: sync-spin 1.4s linear infinite;
  transform-origin: 50% 50%;
}

/* 浮动选区条：fixed 定位（坐标按视口算），层级压在正文与侧栏之上 */
.sel-bar {
  position: fixed;
  z-index: 30;
  display: flex;
  align-items: center;
  gap: var(--sp-1);
  padding: var(--sp-1);
  border: 1px solid var(--line-strong);
  border-radius: var(--r-card);
  background: var(--canvas);
  box-shadow: var(--shadow-2);
}

.sel-bar__btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-width: 32px;
  height: 32px;
  padding: 0 var(--sp-2);
  border: 0;
  border-radius: var(--r-row);
  background: none;
  color: var(--ink);
  font: inherit;
  font-size: var(--text-sm);
  line-height: 1;
  cursor: pointer;
}

.sel-bar__btn:hover {
  background: var(--hover);
}

.sel-bar__btn--on {
  background: var(--accent-soft);
  color: var(--accent);
}

.sel-bar__btn:focus-visible {
  outline: var(--focus-width) solid var(--accent);
  outline-offset: 1px;
}

@media (pointer: coarse) {
  .sel-bar__btn {
    min-width: var(--touch);
    height: var(--touch);
  }
}
</style>
