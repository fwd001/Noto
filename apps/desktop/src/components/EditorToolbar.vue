<script setup lang="ts">
/** 编辑器工具条：全部动作以事件抛给 RichEditor（选区只有它知道）。 */
import { computed, nextTick, onBeforeUnmount, onMounted, ref } from 'vue';
import { t } from '../i18n';
import { TOOLBAR_MARK_BUTTONS } from '../editor/marks';
import { blockTypeLabel } from '../editor/labels';
import type { TextBlockType } from '../editor/model';

const props = withDefaults(
  defineProps<{
    disabled?: boolean;
    activeMarks?: readonly string[];
    blockType?: string;
    headingLevel?: number;
    indent?: number;
    canIndent?: boolean;
  }>(),
  { disabled: false, activeMarks: () => [], blockType: 'paragraph', headingLevel: 1, indent: 0, canIndent: true },
);

const emit = defineEmits<{
  (event: 'mark', kind: string, attrs?: Record<string, unknown>): void;
  (event: 'type', type: TextBlockType): void;
  (event: 'heading', level: number): void;
  (event: 'indent', delta: number): void;
  (event: 'rule'): void;
  (event: 'attach', role: 'inline' | 'file'): void;
  (event: 'link', href: string): void;
  (event: 'undo'): void;
  (event: 'redo'): void;
}>();

const linkOpen = ref(false);
const linkValue = ref('');
const showTypeMenu = ref(false);

/**
 * 类型菜单的落点。**为什么要在 JS 里算坐标**：工具条为了窄栏可滚是 `overflow-x: auto` 的容器，
 * 而 CSS 规定一个轴不是 visible 时另一个轴的 `visible` 会算成 `auto` —— 于是这条 53 px 高的横条
 * 把自己 `absolute` 定位的弹层**整个裁掉**了（缺口 G29：按钮进入展开态，屏幕上却没有任何菜单，
 * 点下去命中的是编辑区）。`position: fixed` 不经过这个裁剪盒，坐标由触发器的矩形算出来。
 */
const typeWrap = ref<HTMLElement | null>(null);
const menuEl = ref<HTMLElement | null>(null);
const menuStyle = ref<Record<string, string>>({});
const MENU_GAP = 4;

async function toggleTypeMenu(): Promise<void> {
  showTypeMenu.value = !showTypeMenu.value;
  if (!showTypeMenu.value) return;
  await nextTick(); // 高度要等弹层真的在 DOM 里才量得到
  const wrap = typeWrap.value;
  const pop = menuEl.value;
  if (!wrap || !pop) return;
  const r = wrap.getBoundingClientRect();
  let top = r.bottom + MENU_GAP;
  if (top + pop.offsetHeight > window.innerHeight - MENU_GAP) top = r.top - MENU_GAP - pop.offsetHeight;
  menuStyle.value = { top: `${Math.round(Math.max(MENU_GAP, top))}px`, left: `${Math.round(r.left)}px` };
}

/**
 * 块型 → 文案键的表在 `editor/labels.ts`（全项目一份）。这里只留调用点：
 * 读屏念的名字（编辑区每个文本块的 `aria-label`）与工具条显示的类型名必须是同一份，
 * 两处各写一份迟早漂移。
 */
const typeLabel = computed(() => blockTypeLabel(props.blockType, props.headingLevel));

/** 与浮动选区条同一份定义：glyph、文案键、顺序只有一处真相。 */
const markButtons = TOOLBAR_MARK_BUTTONS;

/** 类型菜单只列可切换的文本块型；顺序即菜单顺序。 */
const TEXT_TYPE_ORDER: readonly TextBlockType[] = ['paragraph', 'blockquote', 'codeBlock', 'orderedList', 'bulletList', 'checklistItem'];
const typeOptions: Array<{ value: TextBlockType; label: string }> = TEXT_TYPE_ORDER.map((value) => ({
  value,
  label: blockTypeLabel(value),
}));

function onMark(kind: string): void {
  emit('mark', kind);
}

function pickType(value: TextBlockType): void {
  showTypeMenu.value = false;
  emit('type', value);
}

function pickHeading(level: number): void {
  showTypeMenu.value = false;
  emit('heading', level);
}

function applyLink(): void {
  const href = linkValue.value.trim();
  if (href.length > 0) emit('link', href);
  linkValue.value = '';
  linkOpen.value = false;
}

/**
 * 工具条放不下时是横向可滚的（窄窗口 / 三栏都开的默认 1240 就是这一档：内容 818、可视 638）。
 * 那条滚动条是刻意不留位置的（17 px 常驻会把编辑区第一行整个顶下去），可"看不见的按钮"会被
 * 当成"没有这个按钮"—— 缺口 G30：插入附件 / 图片 / 撤销 / 重做 四颗初始在视野外，界面上没有任何
 * 一句话说明这边还有东西。所以量出"右边还有没有"，交给 CSS 那道渐隐去说这句话。
 */
const bar = ref<HTMLElement | null>(null);
const moreRight = ref(false);
const moreLeft = ref(false);
let barObserver: ResizeObserver | null = null;

function measureEdges(): void {
  const el = bar.value;
  if (!el) return;
  moreRight.value = el.scrollLeft + el.clientWidth < el.scrollWidth - 1;
  moreLeft.value = el.scrollLeft > 1;
}

/**
 * 键盘聚焦到视野外那颗时，把它带到眼前。
 * 实测（1240 那一档）：`focus()` 对「插入附件」并不会滚容器 —— 焦点框停在右缘外 16 px，
 * 也就是"焦点在界面上看不见"；对再往右那颗却会一口气滚到底。既然这个容器是横向可滚的，
 * "焦点永远看得见"就得自己保证，不能指望浏览器的默认对齐。
 */
function revealFocused(event: FocusEvent): void {
  const target = event.target instanceof HTMLElement ? event.target : null;
  if (!target || !bar.value?.contains(target)) return;
  target.scrollIntoView({ block: 'nearest', inline: 'nearest' });
}

onMounted(() => {
  measureEdges();
  barObserver = new ResizeObserver(measureEdges);
  if (bar.value) barObserver.observe(bar.value);
});
onBeforeUnmount(() => {
  barObserver?.disconnect();
  barObserver = null;
});
</script>

<template>
  <div
    ref="bar"
    class="tb"
    role="toolbar"
    :aria-label="t('mobile.toolbar')"
    aria-orientation="horizontal"
    :data-more-left="moreLeft ? 'true' : undefined"
    :data-more-right="moreRight ? 'true' : undefined"
    @scroll="measureEdges"
    @focusin="revealFocused"
  >
    <button
      v-for="entry in markButtons"
      :key="entry.kind"
      type="button"
      class="tb__btn"
      :class="{ 'tb__btn--mono': entry.kind === 'code' }"
      :title="t(entry.label)"
      :aria-label="t(entry.label)"
      :aria-pressed="props.activeMarks.includes(entry.kind) ? 'true' : 'false'"
      :disabled="props.disabled"
      @mousedown.prevent
      @click="onMark(entry.kind)"
    >
      {{ entry.glyph }}
    </button>

    <button
      type="button"
      class="tb__btn tb__btn--wide"
      :disabled="props.disabled"
      :aria-expanded="linkOpen ? 'true' : 'false'"
      :title="t('tb.link')"
      @mousedown.prevent
      @click="linkOpen = !linkOpen"
    >
      {{ t('tb.link') }}
    </button>

    <div ref="typeWrap" class="tb__menu-wrap">
      <button type="button" class="tb__btn tb__btn--wide" :disabled="props.disabled" :aria-expanded="showTypeMenu ? 'true' : 'false'" @mousedown.prevent @click="toggleTypeMenu">
        {{ typeLabel }}
      </button>
      <div v-if="showTypeMenu" ref="menuEl" class="tb__popover" :style="menuStyle" role="menu">
        <button
          v-for="option in typeOptions"
          :key="option.value"
          type="button"
          role="menuitem"
          class="tb__item"
          @mousedown.prevent
          @click="pickType(option.value)"
        >
          {{ t(option.label) }}
        </button>
        <button
          v-for="level in [1, 2, 3]"
          :key="`h${level}`"
          type="button"
          role="menuitem"
          class="tb__item tb__item--sub"
          @mousedown.prevent
          @click="pickHeading(level)"
        >
          H{{ level }}
        </button>
      </div>
    </div>

    <button type="button" class="tb__btn" :disabled="props.disabled || !props.canIndent" :title="t('tb.outdent')" :aria-label="t('tb.outdent')" @mousedown.prevent @click="emit('indent', -1)">⇤</button>
    <button type="button" class="tb__btn" :disabled="props.disabled || !props.canIndent" :title="t('tb.indent')" :aria-label="t('tb.indent')" @mousedown.prevent @click="emit('indent', 1)">⇥</button>
    <button type="button" class="tb__btn" :disabled="props.disabled" :title="t('tb.rule')" :aria-label="t('tb.rule')" @mousedown.prevent @click="emit('rule')">—</button>
    <button type="button" class="tb__btn tb__btn--wide" :disabled="props.disabled" :title="t('tb.attach')" @mousedown.prevent @click="emit('attach', 'file')">{{ t('tb.attach') }}</button>
    <button type="button" class="tb__btn tb__btn--wide" :disabled="props.disabled" :title="t('editor.blockImage')" @mousedown.prevent @click="emit('attach', 'inline')">{{ t('editor.blockImage') }}</button>
    <span class="tb__spacer" />
    <button type="button" class="tb__btn" :disabled="props.disabled" :title="t('tb.undo')" :aria-label="t('tb.undo')" @mousedown.prevent @click="emit('undo')">↶</button>
    <button type="button" class="tb__btn" :disabled="props.disabled" :title="t('tb.redo')" :aria-label="t('tb.redo')" @mousedown.prevent @click="emit('redo')">↷</button>

    <form v-if="linkOpen" class="tb__link" @submit.prevent="applyLink">
      <input v-model="linkValue" class="input" type="url" :placeholder="t('tb.linkPrompt')" :aria-label="t('tb.linkPrompt')" />
      <button type="submit" class="btn btn--primary">{{ t('tb.linkApply') }}</button>
    </form>
  </div>
</template>

<style scoped>
.tb {
  display: flex;
  align-items: center;
  gap: var(--space-1);
  padding: var(--space-1) var(--space-2);
  border-bottom: 1px solid var(--border-subtle);
  background: var(--bg-pane);
  overflow-x: auto;
  /* 窄栏里工具条横向可滚，但不给那条滚动条留位置：
     一个 17px 的常驻横条会把编辑区第一行整个顶下去，而工具条本来就能滚。 */
  scrollbar-width: none;
  -ms-overflow-style: none;
  /* 对齐时留出渐隐那 28px，别让聚焦那颗压在提示底下 */
  scroll-padding: 0 32px 0 8px;
  flex: 0 0 auto;
}

.tb::-webkit-scrollbar {
  display: none;
}

/* 放不下时用一道 28px 的渐隐说"这边还有东西"（G30）。
   两个坑都踩过：① 常驻滚动条会把编辑区第一行顶下去；② 给容器加 `mask-image` 会让它成为
   `position: fixed` 后代的包含块 —— 于是刚修好的块型菜单（G29）又被裁掉。
   所以提示画在**滚动条道里的一个 sticky 假元素**上：不动布局、不影响任何祖先的层叠。 */
.tb[data-more-right='true']::after,
.tb[data-more-left='true']::before {
  content: '';
  position: sticky;
  flex: 0 0 28px;
  align-self: stretch;
  pointer-events: none;
}

.tb[data-more-right='true']::after {
  right: 0;
  margin-right: -28px; /* 别把 28px 加进可滚内容的长度里 */
  background: linear-gradient(to right, transparent, var(--bg-pane));
}

.tb[data-more-left='true']::before {
  left: 0;
  margin-left: -28px;
  background: linear-gradient(to left, transparent, var(--bg-pane));
}

.tb__btn {
  flex: 0 0 auto;
  min-width: var(--touch-min);
  min-height: var(--touch-min);
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border-radius: var(--radius-2);
  color: var(--text-secondary);
  font-weight: 700;
  cursor: pointer;
  border: 1px solid transparent;
}

.tb__btn:hover:not(:disabled) {
  background: var(--bg-hover);
  color: var(--text-primary);
}

.tb__btn[aria-pressed='true'] {
  background: var(--accent-soft);
  color: var(--text-primary);
  border-color: var(--accent);
}

.tb__btn:disabled {
  opacity: 0.45;
  cursor: default;
}

.tb__btn--mono {
  font-family: var(--font-mono);
  font-size: var(--text-xs);
}

.tb__btn--wide {
  padding: 0 var(--space-2);
  font-size: var(--text-sm);
  font-weight: 550;
}

.tb__spacer {
  flex: 1;
  min-width: var(--space-2);
}

.tb__menu-wrap {
  position: relative;
}

.tb__popover {
  /* fixed 而不是 absolute：工具条是 `overflow-x: auto` 的容器，absolute 的弹层会被它裁掉（G29）。
     top/left 由组件按触发器矩形算（见 toggleTypeMenu）。 */
  position: fixed;
  z-index: 40;
  display: flex;
  flex-direction: column;
  min-width: 180px;
  max-height: calc(var(--app-vh) - 8px);
  overflow-y: auto;
  padding: var(--space-1);
  background: var(--bg-raised);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-2);
  box-shadow: var(--shadow-3);
}

.tb__item {
  min-height: var(--touch-min);
  text-align: left;
  padding: 0 var(--space-3);
  border-radius: var(--radius-1);
  color: var(--text-primary);
  cursor: pointer;
}

.tb__item:hover {
  background: var(--bg-hover);
}

.tb__item--sub {
  font-size: var(--text-sm);
  color: var(--text-secondary);
  padding-left: var(--space-5);
}

.tb__link {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  width: min(340px, 60vw);
}
</style>
