<script setup lang="ts">
/** 编辑器工具条：全部动作以事件抛给 RichEditor（选区只有它知道）。 */
import { computed, nextTick, onBeforeUnmount, onMounted, ref, type Ref } from 'vue';
import { t } from '../i18n';
import { TOOLBAR_MARK_BUTTONS, FONT_SIZE_MENU, INK_COLOR_MENU, INK_COLORS } from '../editor/marks';
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
    activeFontSize?: string | null;
    activeColor?: string | null;
  }>(),
  { disabled: false, activeMarks: () => [], blockType: 'paragraph', headingLevel: 1, indent: 0, canIndent: true, activeFontSize: null, activeColor: null },
);

const emit = defineEmits<{
  (event: 'mark', kind: string, attrs?: Record<string, unknown>): void;
  (event: 'unmark', kind: string): void;
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
  closeMenu();
  emit('type', value);
}

function pickHeading(level: number): void {
  closeMenu();
  emit('heading', level);
}

/**
 * 「标准 / 默认色」不发一个 step='m' 的标记 —— 那会在文档里留下一个没有任何视觉效果的标记，
 * 同步侧还白算一次内容哈希。它们发的是"去掉这一位"，由 RichEditor 用不带 exclusive 的
 * toggle 实现（覆盖满整段 ⇒ 去掉，见 model.spec 里"整段已加粗时再切换等于取消"那条）。
 */
function pickSize(step: string): void {
  closeMenu();
  if (step === 'm') emit('unmark', 'fontSize');
  else emit('mark', 'fontSize', { step });
}

function pickColor(name: string): void {
  closeMenu();
  if (name === 'default') emit('unmark', 'color');
  else emit('mark', 'color', { name });
}

/** 菜单里"当前是哪一档"的判据：默认档 = 没有这个标记（不是有个 step='m' 的标记）。 */
function currentSize(step: string): boolean {
  return step === 'm' ? !props.activeFontSize : props.activeFontSize === step;
}

function currentColor(name: string): boolean {
  return name === 'default' ? !props.activeColor : props.activeColor === name;
}

const activeInk = computed<string>(() => (props.activeColor ? (INK_COLORS[props.activeColor] ?? 'transparent') : 'transparent'));

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
let barObserver: ResizeObserver | null = null;
const moreRight = ref(false);
const moreLeft = ref(false);
/**
 * 三个下拉（块型 / 文字大小 / 文字颜色）共用一份"锚定 + 翻转"逻辑，只有一个是开的。
 *
 * **为什么要在 JS 里算坐标**：工具条为了窄栏可滚是 `overflow-x: auto` 的容器，
 * 而 CSS 规定一个轴不是 visible 时另一个轴的 `visible` 会算成 `auto` —— 于是这条 53 px 高的横条
 * 把自己 `absolute` 定位的弹层**整个裁掉**了（缺口 G29：按钮进入展开态，屏幕上却没有任何菜单，
 * 点下去命中的是编辑区）。`position: fixed` 不经过这个裁剪盒，坐标由触发器的矩形算出来。
 */
type MenuName = 'type' | 'size' | 'color';

const showMenu = ref<MenuName | null>(null);
const wrapType = ref<HTMLElement | null>(null);
const wrapSize = ref<HTMLElement | null>(null);
const wrapColor = ref<HTMLElement | null>(null);
const popType = ref<HTMLElement | null>(null);
const popSize = ref<HTMLElement | null>(null);
const popColor = ref<HTMLElement | null>(null);
const menuStyle = ref<Record<string, string>>({});
const MENU_GAP = 4;

async function toggleMenu(name: MenuName): Promise<void> {
  showMenu.value = showMenu.value === name ? null : name;
  if (!showMenu.value) return;
  await nextTick(); // 高度要等弹层真的在 DOM 里才量得到
  // 每个菜单各用自己的 ref：三个弹层共用一个 ref 时，切档过程中"旧的置 null"可能发生在
  // "新的赋值"之后，量到的是 null ⇒ 弹层没坐标，又回到 G29 那个"展开了但看不见"的形状。
  const anchors: Record<MenuName, [Ref<HTMLElement | null>, Ref<HTMLElement | null>]> = {
    type: [wrapType, popType],
    size: [wrapSize, popSize],
    color: [wrapColor, popColor],
  };
  const [wrapRef, popRef] = anchors[name];
  const wrap = wrapRef.value;
  const pop = popRef.value;
  if (!wrap || !pop) return;
  const r = wrap.getBoundingClientRect();
  let top = r.bottom + MENU_GAP;
  if (top + pop.offsetHeight > window.innerHeight - MENU_GAP) top = r.top - MENU_GAP - pop.offsetHeight;
  // 横向也要钳位。弹层是 `fixed`，不经过工具条那个裁剪盒 ⇒ 触发器停在容器右缘时，菜单会直接画到
  // 屏幕外（缺口 G64：390 宽实测字号菜单 301..481、颜色菜单 306..486，出界 91/96 px，
  // 菜单项中心的 `elementFromPoint` 是 null —— 人看不到也点不着，而 Playwright 会自己滚进去，
  // 所以"能点到"这一条单靠自动化是量不出来的）。
  const left = Math.min(r.left, window.innerWidth - MENU_GAP - pop.offsetWidth);
  menuStyle.value = {
    top: `${Math.round(Math.max(MENU_GAP, top))}px`,
    left: `${Math.round(Math.max(MENU_GAP, left))}px`,
  };
}

function closeMenu(): void {
  showMenu.value = null;
}

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

    <div ref="wrapType" class="tb__menu-wrap">
      <button type="button" class="tb__btn tb__btn--wide" :disabled="props.disabled" :aria-expanded="showMenu === 'type' ? 'true' : 'false'" @mousedown.prevent @click="toggleMenu('type')">
        {{ typeLabel }}
      </button>
      <div v-if="showMenu === 'type'" ref="popType" class="tb__popover" :style="menuStyle" role="menu">
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

    <!-- 「文字大小」「文字颜色」：用户那句「副文本选项应该像 Apple 便签那样只有几项，
         可能就是文字大小、颜色啊」的两颗。菜单只有几个档，不是自由字号/取色器 ——
         档位与色名只有一处真相（editor/marks.ts），渲染侧读同一份表（editor/dom.ts）。 -->
    <div ref="wrapSize" class="tb__menu-wrap">
      <button
        type="button"
        class="tb__btn tb__btn--wide"
        data-testid="tb-size"
        :disabled="props.disabled"
        :title="t('tb.size')"
        :aria-label="t('tb.size')"
        :aria-expanded="showMenu === 'size' ? 'true' : 'false'"
        @mousedown.prevent
        @click="toggleMenu('size')"
      >
        A<span class="tb__size-hint">{{ props.activeFontSize ?? '' }}</span>
      </button>
      <div v-if="showMenu === 'size'" ref="popSize" class="tb__popover" :style="menuStyle" role="menu" data-testid="tb-size-menu">
        <button
          v-for="option in FONT_SIZE_MENU"
          :key="option.step"
          type="button"
          role="menuitemradio"
          class="tb__item"
          :data-testid="`tb-size-${option.step}`"
          :aria-checked="currentSize(option.step) ? 'true' : 'false'"
          @mousedown.prevent
          @click="pickSize(option.step)"
        >
          {{ t(option.label) }}
        </button>
      </div>
    </div>

    <div ref="wrapColor" class="tb__menu-wrap">
      <button
        type="button"
        class="tb__btn tb__btn--wide"
        data-testid="tb-color"
        :disabled="props.disabled"
        :title="t('tb.textColor')"
        :aria-label="t('tb.textColor')"
        :aria-expanded="showMenu === 'color' ? 'true' : 'false'"
        @mousedown.prevent
        @click="toggleMenu('color')"
      >
        A<span class="tb__ink" :style="{ background: activeInk }" aria-hidden="true" />
      </button>
      <div v-if="showMenu === 'color'" ref="popColor" class="tb__popover" :style="menuStyle" role="menu" data-testid="tb-color-menu">
        <button
          v-for="option in INK_COLOR_MENU"
          :key="option.name"
          type="button"
          role="menuitemradio"
          class="tb__item"
          :data-testid="`tb-color-${option.name}`"
          :aria-checked="currentColor(option.name) ? 'true' : 'false'"
          @mousedown.prevent
          @click="pickColor(option.name)"
        >
          <span class="tb__ink" :style="{ background: INK_COLORS[option.name] ?? 'transparent' }" aria-hidden="true" />
          {{ t(option.label) }}
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

/* 色点：颜色这件事没法用文字说清，但也不能**只**用颜色说（无障碍里"仅靠颜色传达"不合格），
   所以每颗后面都跟着一个中文色名。 */
.tb__ink {
  display: inline-block;
  inline-size: var(--space-3);
  block-size: var(--space-3);
  margin-inline-end: var(--space-2);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-1);
  vertical-align: middle;
}

.tb__size-hint {
  margin-inline-start: var(--space-1);
  font-size: var(--text-xs);
  color: var(--text-secondary);
}

.tb__link {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  width: min(340px, 60vw);
}
</style>
