<script setup lang="ts">
/** 编辑器工具条：动作一律以事件抛给 RichEditor（选区只有它知道）。 */
import { computed, nextTick, onBeforeUnmount, onMounted, ref } from 'vue';
import { t } from '../i18n';
import { TOOLBAR_ITEMS, type ToolbarItem } from '../editor/toolbarItems';
import { planToolbar } from '../editor/toolbarOverflow';
import type { TextBlockType } from '../editor/model';
import AppIcon from './ui/AppIcon.vue';
import ToolbarCell from './ToolbarItem.vue';

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
  {
    disabled: false,
    activeMarks: () => [],
    blockType: 'paragraph',
    headingLevel: 1,
    indent: 0,
    canIndent: true,
    activeFontSize: null,
    activeColor: null,
  },
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

/** 这一格与下一格之间的 gap，与 CSS `gap: var(--sp-1)` 同值。 */
const GAP = 4;
/** 「更多 ›」量不到时的保守宽度。 */
const MORE_FALLBACK = 64;

const bar = ref<HTMLElement | null>(null);
const moreBtn = ref<HTMLElement | null>(null);
const morePop = ref<HTMLElement | null>(null);
let barObserver: ResizeObserver | null = null;
const overflowKeys = ref<Set<string>>(new Set());
/**
 * "全部摆在条上"的那一帧。要先量一次：只有那时每颗的宽度才是它**在条上**的宽度
 * （进了面板的行是整行宽，量出来的数没有意义）。
 */
const measuring = ref(true);
const moreOpen = ref(false);
const openKey = ref<string | null>(null);
const moreStyle = ref<Record<string, string>>({});
/** 极端窄档：连"留一颗"都放不下时条上仍可滚，那句话由渐隐继续说。 */
const scrollEdge = ref({ left: false, right: false });

const visibleItems = computed<readonly ToolbarItem[]>(() =>
  measuring.value ? TOOLBAR_ITEMS : TOOLBAR_ITEMS.filter((item) => !overflowKeys.value.has(item.key)),
);
const overflowItems = computed<readonly ToolbarItem[]>(() =>
  measuring.value ? [] : TOOLBAR_ITEMS.filter((item) => overflowKeys.value.has(item.key)),
);
const overflowing = computed(() => !measuring.value && overflowItems.value.length > 0);

async function remeasure(): Promise<void> {
  measuring.value = true;
  await nextTick();
  const el = bar.value;
  if (!el) return;
  const widths = new Map<string, number>();
  for (const cell of Array.from(el.querySelectorAll<HTMLElement>('[data-tb-key]'))) {
    const key = cell.dataset.tbKey;
    if (key) widths.set(key, cell.offsetWidth);
  }
  // 一颗都没量到 = 这一帧还没挂上，别拿空表去算计划（那会把整条收进面板）。
  if (widths.size === 0) return;
  // jsdom 里没有排版引擎，所有 `offsetWidth` 都是 0 —— 拿它算出来的"放不下"是仪器的形状不是界面的形状。
  // 真浏览器里这条永不命中（内容总长实测 800+），溢出那一格由 `verify-layout` 的浏览器腿量。
  const content = [...widths.values()].reduce((sum, value) => sum + value, 0);
  if (content === 0 || el.clientWidth === 0) return;
  // `clientWidth` 含容器自己的左右内边距，而格子排在 padding 之内的内容盒里 ——
  // 拿 clientWidth 当可用宽度会多出那 16px，结果"收纳完了条上还是滚得动十几 px"。
  const pad = Number.parseFloat(getComputedStyle(el).paddingInlineStart || '0') +
    Number.parseFloat(getComputedStyle(el).paddingInlineEnd || '0');
  const measured = TOOLBAR_ITEMS.map((item) => ({ key: item.key, width: widths.get(item.key) ?? 44 }));
  const usable = el.clientWidth - pad;
  // 「更多 ›」那颗按钮只有"确实要收东西"时才存在，而它的宽度又要先从预算里扣掉 ——
  // 第一遍只能拿兜底值算。所以这里迭代几遍：每一遍都用上一遍**真实量到**的宽度重算，
  // 直到计划不再变化。写死一个数是另一种错（字变宽之后就漏出十几 px 的滚动）。
  let moreWidth = moreBtn.value?.offsetWidth ?? MORE_FALLBACK;
  let plan = planToolbar(measured, usable, moreWidth, GAP);
  for (let pass = 0; pass < 3; pass += 1) {
    overflowKeys.value = new Set(plan.overflow);
    measuring.value = false;
    await nextTick();
    const real = moreBtn.value?.offsetWidth;
    if (!plan.overflowing || real === undefined || Math.abs(real - moreWidth) < 1) break;
    moreWidth = real;
    const next = planToolbar(measured, usable, moreWidth, GAP);
    if (next.overflow.join() === plan.overflow.join()) break;
    plan = next;
  }
  overflowKeys.value = new Set(plan.overflow);
  measuring.value = false;
  await nextTick();
  measureScroll();
}

function measureScroll(): void {
  const el = bar.value;
  if (!el) return;
  scrollEdge.value = {
    left: el.scrollLeft > 1,
    right: el.scrollLeft + el.clientWidth < el.scrollWidth - 1,
  };
}

async function toggleMore(): Promise<void> {
  moreOpen.value = !moreOpen.value;
  openKey.value = null;
  if (!moreOpen.value) return;
  await nextTick();
  const anchor = moreBtn.value;
  const panel = morePop.value;
  if (!anchor || !panel) return;
  const r = anchor.getBoundingClientRect();
  let top = r.bottom + 4;
  if (top + panel.offsetHeight > window.innerHeight - 4) top = r.top - 4 - panel.offsetHeight;
  const left = Math.min(r.left, window.innerWidth - 4 - panel.offsetWidth);
  moreStyle.value = {
    top: `${Math.round(Math.max(4, top))}px`,
    left: `${Math.round(Math.max(4, left))}px`,
  };
}

function onToggleMenu(key: string): void {
  openKey.value = openKey.value === key ? null : key;
  // 「更多 ›」面板**不跟着收**：收了就等于"点开字号菜单这件事把入口弄没了"，
  // 再点一次触发器收起菜单时那颗已经不在 DOM 里了（用户的第二下会点到空处）。
  // 真正做完一个动作才收（见 passThrough 里的 andClose）。
}

function onCloseMenu(): void {
  openKey.value = null;
}

/**
 * 键盘聚焦到视野外那颗时把它带到眼前。
 * 实测（1240 那一档）：`focus()` 对某几颗并不会滚容器 —— 焦点框停在右缘外，
 * 也就是"焦点在界面上看不见"。既然这个容器横向可滚，"焦点永远看得见"就得自己保证。
 */
function revealFocused(event: FocusEvent): void {
  const target = event.target instanceof HTMLElement ? event.target : null;
  if (!target || !bar.value?.contains(target)) return;
  target.scrollIntoView({ block: 'nearest', inline: 'nearest' });
}

/** 「更多 ›」里的动作做完就收起面板：留着它，用户会以为还要再点一下才算数。 */
function andClose(fn: () => void): () => void {
  return () => {
    fn();
    if (moreOpen.value) moreOpen.value = false;
  };
}

const passThrough = {
  mark: (kind: string, attrs?: Record<string, unknown>) => {
    andClose(() => emit('mark', kind, attrs))();
  },
  unmark: (kind: string) => {
    andClose(() => emit('unmark', kind))();
  },
  type: (value: TextBlockType) => {
    andClose(() => emit('type', value))();
  },
  heading: (level: number) => {
    andClose(() => emit('heading', level))();
  },
  indent: (delta: number) => {
    andClose(() => emit('indent', delta))();
  },
  rule: () => {
    andClose(() => emit('rule'))();
  },
  attach: (role: 'inline' | 'file') => {
    andClose(() => emit('attach', role))();
  },
  link: (href: string) => {
    andClose(() => emit('link', href))();
  },
  undo: () => {
    andClose(() => emit('undo'))();
  },
  redo: () => {
    andClose(() => emit('redo'))();
  },
};

onMounted(() => {
  void remeasure();
  barObserver = new ResizeObserver(() => void remeasure());
  if (bar.value) barObserver.observe(bar.value);
});
onBeforeUnmount(() => {
  barObserver?.disconnect();
  barObserver = null;
});
</script>

<template>
  <!-- 渐隐那道 28px 画在**外层包装**上，不画在滚动容器里。
       画在容器里它是一个 flex 项：Chromium 把它算进 scrollWidth，于是"收纳完了"的条
       还是能滚 14px —— 那道提示自己制造了它要提示的东西。
       （不能用 `mask-image`：那会让容器成为 fixed 后代的包含块，G29 的弹层又被裁掉。） -->
  <div
    class="tb-wrap"
    :data-more-left="overflowing || scrollEdge.left ? 'true' : undefined"
    :data-more-right="overflowing || scrollEdge.right ? 'true' : undefined"
  >
  <div
    ref="bar"
    class="tb"
    role="toolbar"
    :aria-label="t('mobile.toolbar')"
    aria-orientation="horizontal"
    :data-more-left="overflowing || scrollEdge.left ? 'true' : undefined"
    :data-more-right="overflowing || scrollEdge.right ? 'true' : undefined"
    @scroll="measureScroll"
    @focusin="revealFocused"
  >
    <ToolbarCell
      v-for="item in visibleItems"
      :key="item.key"
      :item="item"
      :disabled="props.disabled"
      :active-marks="props.activeMarks"
      :block-type="props.blockType"
      :heading-level="props.headingLevel"
      :active-font-size="props.activeFontSize"
      :active-color="props.activeColor"
      :can-indent="props.canIndent"
      :open="openKey === item.key"
      @mark="passThrough.mark"
      @unmark="passThrough.unmark"
      @type="passThrough.type"
      @heading="passThrough.heading"
      @indent="passThrough.indent"
      @rule="passThrough.rule"
      @attach="passThrough.attach"
      @link="passThrough.link"
      @undo="passThrough.undo"
      @redo="passThrough.redo"
      @toggle-menu="onToggleMenu"
      @close-menu="onCloseMenu"
    />

    <!-- 放不下才有这颗。§3.4：右缘 28px 渐隐 + 最右的「更多 ›」把溢出项收进去。
         粘在右缘：条上还能滚的极端档里，它跟着滚就是"看不见的入口"，那句话要一直在场。 -->
    <div v-if="overflowing" ref="moreBtn" class="tb__more-wrap">
      <button
        type="button"
        class="tb__btn tb__btn--wide"
        data-testid="tb-more"
        :title="t('tb.more')"
        :aria-label="t('tb.more')"
        :aria-expanded="moreOpen ? 'true' : 'false'"
        @mousedown.prevent
        @click="toggleMore"
      >
        {{ t('tb.more') }}
        <AppIcon :size="18" name="chevron-right" />
      </button>
    </div>
  </div>
  </div>

  <!-- 面板必须离开那个裁剪盒：`overflow-x:auto` 的容器会把 absolute 后代整个裁掉（G29 的同一族）。 -->
  <Teleport to="body">
    <div
      v-if="overflowing && moreOpen"
      ref="morePop"
      class="tb__popover"
      :style="moreStyle"
      role="menu"
      :aria-label="t('tb.more')"
      data-testid="tb-more-menu"
    >
      <ToolbarCell
        v-for="item in overflowItems"
        :key="item.key"
        :item="item"
        in-panel
        :disabled="props.disabled"
        :active-marks="props.activeMarks"
        :block-type="props.blockType"
        :heading-level="props.headingLevel"
        :active-font-size="props.activeFontSize"
        :active-color="props.activeColor"
        :can-indent="props.canIndent"
        :open="openKey === item.key"
        @mark="passThrough.mark"
        @unmark="passThrough.unmark"
        @type="passThrough.type"
        @heading="passThrough.heading"
        @indent="passThrough.indent"
        @rule="passThrough.rule"
        @attach="passThrough.attach"
        @link="passThrough.link"
        @undo="passThrough.undo"
        @redo="passThrough.redo"
        @toggle-menu="onToggleMenu"
        @close-menu="onCloseMenu"
      />
    </div>
  </Teleport>
</template>

<style scoped>
.tb {
  display: flex;
  align-items: center;
  gap: var(--sp-1);
  padding: var(--sp-1) var(--sp-2);
  border-bottom: 1px solid var(--line);
  background: var(--canvas);
  overflow-x: auto;
  /* 窄栏里工具条横向可滚，但不给那条滚动条留位置：
     一个 17px 的常驻横条会把编辑区第一行整个顶下去，而工具条本来就能滚。 */
  scrollbar-width: none;
  -ms-overflow-style: none;
  scroll-padding: 0 32px 0 8px;
  flex: 0 0 auto;
}

/* 渐隐画在这层，不画在滚动容器里（见模板那条注释）。 */
.tb-wrap {
  position: relative;
  flex: 0 0 auto;
}

.tb::-webkit-scrollbar {
  display: none;
}

.tb-wrap[data-more-right='true']::after,
.tb-wrap[data-more-left='true']::before {
  content: '';
  position: absolute;
  top: 0;
  bottom: 1px; /* 不吃那条 border-bottom */
  width: 28px;
  pointer-events: none;
  z-index: 1;
}

.tb-wrap[data-more-right='true']::after {
  right: 0;
  background: linear-gradient(to right, transparent, var(--canvas));
}

.tb-wrap[data-more-left='true']::before {
  left: 0;
  background: linear-gradient(to left, transparent, var(--canvas));
}

.tb__btn {
  flex: 0 0 auto;
  min-width: var(--touch);
  min-height: var(--touch);
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: var(--sp-1);
  border-radius: var(--r-card);
  color: var(--body);
  font-weight: 700;
  cursor: pointer;
  border: 1px solid transparent;
}

.tb__btn--wide {
  padding: 0 var(--sp-2);
  font-size: var(--text-sm);
  font-weight: 550;
}

.tb__more-wrap {
  position: sticky;
  right: var(--sp-1);
  flex: 0 0 auto;
  background: var(--canvas);
}

.tb__popover {
  /* fixed + Teleport 到 body：见模板里那条注释。 */
  position: fixed;
  z-index: 40;
  display: flex;
  flex-direction: column;
  min-width: 200px;
  max-height: calc(var(--app-vh) - 8px);
  overflow-y: auto;
  padding: var(--sp-1);
  background: var(--canvas);
  border: 1px solid var(--line);
  border-radius: var(--r-card);
  box-shadow: var(--shadow-3);
}
</style>
