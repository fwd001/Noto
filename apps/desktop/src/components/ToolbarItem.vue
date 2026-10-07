<script setup lang="ts">
/**
 * 工具条上的**一个格子**。§3.4 的溢出会让同一个格子出现在条上或「更多 ›」面板里，
 * 两处必须是这一个组件，不许把 markup 抄两遍（抄两遍迟早一份有 aria-label、一份没有）。
 *
 * 弹层坐标在这里算：触发器在哪个位置（条上 / 面板里）只有这个组件自己知道，
 * 而它必须继续是 `position: fixed` —— 工具条是 `overflow-x:auto` 的裁剪盒，
 * `absolute` 的弹层会被整个裁掉（缺口 G29），横向也要钳位（缺口 G64）。
 */
import { computed, nextTick, ref, type PropType } from 'vue';
import { t } from '../i18n';
import AppIcon from './ui/AppIcon.vue';
import { FONT_SIZE_MENU, INK_COLORS, INK_COLOR_MENU, TOOLBAR_MARK_BUTTONS } from '../editor/marks';
import { TEXT_TYPE_ORDER, type ToolbarItem } from '../editor/toolbarItems';
import { blockTypeLabel } from '../editor/labels';
import type { TextBlockType } from '../editor/model';

const props = defineProps({
  item: { type: Object as PropType<ToolbarItem>, required: true },
  disabled: { type: Boolean, default: false },
  /** 在「更多 ›」面板里：形状是整行菜单项，不是 44 见方的小方块。 */
  inPanel: { type: Boolean, default: false },
  /** 当前选区上的标记（决定 pressed 态）。 */
  activeMarks: { type: Array as PropType<readonly string[]>, default: () => [] },
  blockType: { type: String, default: 'paragraph' },
  headingLevel: { type: Number, default: 1 },
  activeFontSize: { type: String as PropType<string | null>, default: null },
  activeColor: { type: String as PropType<string | null>, default: null },
  canIndent: { type: Boolean, default: true },
  /** 三个下拉只许一个开着，由父级记"现在开的是哪一格"。 */
  open: { type: Boolean, default: false },
});

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
  (event: 'toggle-menu', key: string): void;
  (event: 'close-menu'): void;
}>();

const wrap = ref<HTMLElement | null>(null);
const pop = ref<HTMLElement | null>(null);
const menuStyle = ref<Record<string, string>>({});
const MENU_GAP = 4;

/** 「链接」那一格自带一小段输入：以前这颗按钮的表单画在整条工具条的最末尾，离触发器很远。 */
const linkOpen = ref(false);
const linkValue = ref('');

function applyLink(): void {
  const href = linkValue.value.trim();
  if (href.length > 0) emit('link', href);
  linkValue.value = '';
  linkOpen.value = false;
}

const pressed = computed(() =>
  props.item.kind === 'mark' && props.item.payload !== undefined && props.activeMarks.includes(props.item.payload),
);

const typeLabel = computed(() => blockTypeLabel(props.blockType, props.headingLevel));
const activeInk = computed<string>(() =>
  props.activeColor ? (INK_COLORS[props.activeColor] ?? 'transparent') : 'transparent',
);

/** glyph 只有一个真相（`editor/marks.ts`）：这里查表，不在此处再抄一份 B/I/U/S。 */
const glyph = computed(
  () => TOOLBAR_MARK_BUTTONS.find((entry) => entry.kind === props.item.payload)?.glyph ?? '',
);

/** 触发器上那一小段可见内容：字号档与色点是把状态画在按钮上，不能只藏在菜单里。 */
const trailing = computed(() => (props.item.key === 'size' ? (props.activeFontSize ?? '') : ''));

async function onTrigger(): Promise<void> {
  if (props.item.kind === 'mark' && props.item.payload) emit('mark', props.item.payload);
  else if (props.item.kind === 'icon' && props.item.key === 'rule') emit('rule');
  else if (props.item.kind === 'icon' && props.item.key === 'undo') emit('undo');
  else if (props.item.kind === 'icon' && props.item.key === 'redo') emit('redo');
  else if (props.item.kind === 'icon') emit('indent', props.item.key === 'indent' ? 1 : -1);
  else if (props.item.kind === 'text') emit('attach', props.item.payload === 'inline' ? 'inline' : 'file');
  else if (props.item.kind === 'link') linkOpen.value = !linkOpen.value;
  else if (props.item.kind === 'menu') {
    emit('toggle-menu', props.item.key);
    await nextTick(); // 高度要等弹层真的在 DOM 里才量得到
    const anchor = wrap.value;
    const panel = pop.value;
    if (!anchor || !panel) return;
    const rect = anchor.getBoundingClientRect();
    let top = rect.bottom + MENU_GAP;
    if (top + panel.offsetHeight > window.innerHeight - MENU_GAP) top = rect.top - MENU_GAP - panel.offsetHeight;
    const left = Math.min(rect.left, window.innerWidth - MENU_GAP - panel.offsetWidth);
    menuStyle.value = {
      top: `${Math.round(Math.max(MENU_GAP, top))}px`,
      left: `${Math.round(Math.max(MENU_GAP, left))}px`,
    };
  }
}

function pickType(value: TextBlockType): void {
  emit('close-menu');
  emit('type', value);
}

function pickHeading(level: number): void {
  emit('close-menu');
  emit('heading', level);
}

/**
 * 「标准 / 默认色」不发一个 step='m' 的标记 —— 那会在文档里留下一个没有视觉效果的标记，
 * 同步侧还白算一次哈希。它们发的是"去掉这一位"（与 RichEditor 的 toggle 语义一对）。
 */
function pickSize(step: string): void {
  emit('close-menu');
  if (step === 'm') emit('unmark', 'fontSize');
  else emit('mark', 'fontSize', { step });
}

function pickColor(name: string): void {
  emit('close-menu');
  if (name === 'default') emit('unmark', 'color');
  else emit('mark', 'color', { name });
}

function currentSize(step: string): boolean {
  return step === 'm' ? !props.activeFontSize : props.activeFontSize === step;
}

function currentColor(name: string): boolean {
  return name === 'default' ? !props.activeColor : props.activeColor === name;
}

/** 这一格自己 disabled 与否：缩进两颗要看当前块型（父级算好传进来）。 */
const isDisabled = computed(() => props.disabled || (props.item.needsIndent === true && !props.canIndent));
</script>

<template>
  <div ref="wrap" class="tb__cell" :class="{ 'tb__cell--menu': item.kind === 'menu' }" :data-tb-key="item.key">
    <button
      type="button"
      :class="[
        inPanel ? 'tb__row' : 'tb__btn',
        { 'tb__btn--mono': item.key === 'code', 'tb__btn--wide': item.kind === 'menu' || item.kind === 'text' },
      ]"
      :title="t(item.label)"
      :aria-label="t(item.label)"
      :data-testid="item.testid"
      :aria-pressed="item.kind === 'mark' ? (pressed ? 'true' : 'false') : undefined"
      :aria-expanded="item.kind === 'menu' ? (open ? 'true' : 'false') : undefined"
      :disabled="isDisabled"
      @mousedown.prevent
      @click="onTrigger"
    >
      <AppIcon v-if="item.icon" :size="18" :name="item.icon" />
      <template v-else-if="item.kind === 'mark'">
        <span aria-hidden="true">{{ glyph }}</span>
        <template v-if="inPanel">{{ t(item.label) }}</template>
      </template>
      <template v-else-if="item.kind === 'menu' && item.key === 'type'">
        {{ inPanel ? `${t('tb.blockType')}：${typeLabel}` : typeLabel }}
      </template>
      <template v-else-if="item.key === 'size'">
        A<span v-if="trailing" class="tb__size-hint">{{ trailing }}</span>
        <template v-if="inPanel"> {{ t('tb.size') }}</template>
      </template>
      <template v-else-if="item.key === 'color'">
        A<span class="tb__ink" :style="{ background: activeInk }" aria-hidden="true" />
        <template v-if="inPanel"> {{ t('tb.textColor') }}</template>
      </template>
      <template v-else>{{ t(item.label) }}</template>
    </button>

    <form v-if="item.kind === 'link' && linkOpen" class="tb__link" @submit.prevent="applyLink">
      <input
        v-model="linkValue"
        class="input"
        type="url"
        :placeholder="t('tb.linkPrompt')"
        :aria-label="t('tb.linkPrompt')"
        data-testid="tb-link-input"
      />
      <button type="submit" class="btn btn--primary">{{ t('tb.linkApply') }}</button>
    </form>

    <div
      v-if="item.kind === 'menu' && open"
      ref="pop"
      class="tb__popover"
      :style="menuStyle"
      role="menu"
      :aria-label="t(item.label)"
      :data-testid="item.key === 'size' ? 'tb-size-menu' : item.key === 'color' ? 'tb-color-menu' : 'tb-type-menu'"
    >
      <template v-if="item.key === 'type'">
        <button
          v-for="value in TEXT_TYPE_ORDER"
          :key="value"
          type="button"
          role="menuitem"
          class="tb__item"
          @mousedown.prevent
          @click="pickType(value)"
        >
          {{ t(blockTypeLabel(value)) }}
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
      </template>
      <template v-else-if="item.key === 'size'">
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
      </template>
      <template v-else>
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
      </template>
    </div>
  </div>
</template>

<style scoped>
/* 这一格在 flex 里不许被压扁：窄栏的溢出是"收进更多"，不是"把按钮挤变形"。 */
.tb__cell {
  display: inline-flex;
  flex: 0 0 auto;
}

.tb__btn {
  flex: 0 0 auto;
  min-width: var(--touch);
  min-height: var(--touch);
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border-radius: var(--r-card);
  color: var(--body);
  font-weight: 700;
  cursor: pointer;
  border: 1px solid transparent;
}

.tb__btn:hover:not(:disabled) {
  background: var(--hover);
  color: var(--ink);
}

.tb__btn[aria-pressed='true'] {
  background: var(--accent-soft);
  color: var(--ink);
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
  padding: 0 var(--sp-2);
  font-size: var(--text-sm);
  font-weight: 550;
}

/* 「更多 ›」面板里的那一行：与条上那颗是同一个组件，只是形状换成整行。 */
.tb__row {
  display: flex;
  align-items: center;
  gap: var(--sp-2);
  width: 100%;
  min-height: var(--touch);
  padding: 0 var(--sp-3);
  border: 0;
  border-radius: var(--r-row);
  background: none;
  color: var(--ink);
  font-size: var(--text-sm);
  text-align: left;
  cursor: pointer;
}

.tb__row:hover:not(:disabled) {
  background: var(--hover);
}

.tb__row:disabled {
  opacity: 0.45;
  cursor: default;
}

.tb__popover {
  position: fixed;
  z-index: 40;
  display: flex;
  flex-direction: column;
  min-width: 180px;
  max-height: calc(var(--app-vh) - 8px);
  overflow-y: auto;
  padding: var(--sp-1);
  background: var(--canvas);
  border: 1px solid var(--line);
  border-radius: var(--r-card);
  box-shadow: var(--shadow-3);
}

.tb__item {
  min-height: var(--touch);
  text-align: left;
  padding: 0 var(--sp-3);
  border: 0;
  border-radius: var(--r-row);
  background: none;
  color: var(--ink);
  cursor: pointer;
}

.tb__item:hover {
  background: var(--hover);
}

.tb__item--sub {
  font-size: var(--text-sm);
  color: var(--body);
  padding-left: var(--sp-6);
}

.tb__ink {
  display: inline-block;
  inline-size: var(--sp-3);
  block-size: var(--sp-3);
  margin-inline-end: var(--sp-2);
  border: 1px solid var(--line);
  border-radius: var(--r-row);
  vertical-align: middle;
}

.tb__size-hint {
  margin-inline-start: var(--sp-1);
  font-size: var(--text-xs);
  color: var(--body);
}

.tb__link {
  display: flex;
  align-items: center;
  gap: var(--sp-2);
  width: min(340px, 60vw);
}
</style>
