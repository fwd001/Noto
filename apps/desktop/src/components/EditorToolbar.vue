<script setup lang="ts">
/** 编辑器工具条：全部动作以事件抛给 RichEditor（选区只有它知道）。 */
import { computed, ref } from 'vue';
import { t, type MessageKey } from '../i18n';
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
  (event: 'checklist'): void;
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
 * 块型 → 文案键。必须写成表：之前这里是 `t(\`editor.block${Type}\`)`，
 * 而 MessageKey 就是 string，拼错/漏键编译期一声不响，7 种块型里 5 种在工具条上
 * 直接显示成 "editor.blockCodeBlock"。未知块型宁可显示原始类型名，也不假装是正文。
 */
const TYPE_LABELS: Record<string, MessageKey> = {
  paragraph: 'editor.blockParagraph',
  heading: 'editor.blockHeading',
  bulletList: 'editor.blockListBullet',
  orderedList: 'editor.blockListOrdered',
  checklistItem: 'editor.blockChecklist',
  blockquote: 'editor.blockQuote',
  codeBlock: 'editor.blockCode',
  image: 'editor.blockImage',
  attachment: 'editor.blockAttachment',
  rule: 'editor.blockRule',
};

const typeLabel = computed(() => {
  if (props.blockType === 'heading') return t('editor.blockHeading', { level: props.headingLevel });
  return TYPE_LABELS[props.blockType] ? t(TYPE_LABELS[props.blockType]) : props.blockType;
});

const markButtons: Array<{ kind: string; glyph: string; label: string }> = [
  { kind: 'bold', glyph: 'B', label: 'tb.bold' },
  { kind: 'italic', glyph: 'I', label: 'tb.italic' },
  { kind: 'underline', glyph: 'U', label: 'tb.underline' },
  { kind: 'strike', glyph: 'S', label: 'tb.strike' },
  { kind: 'code', glyph: '</>', label: 'tb.code' },
  { kind: 'highlight', glyph: 'H', label: 'tb.highlight' },
];

/** 类型菜单只列可切换的文本块型；顺序即菜单顺序。 */
const TEXT_TYPE_ORDER: readonly TextBlockType[] = ['paragraph', 'blockquote', 'codeBlock', 'orderedList', 'bulletList', 'checklistItem'];
const typeOptions: Array<{ value: TextBlockType; label: MessageKey }> = TEXT_TYPE_ORDER.map((value) => ({
  value,
  label: TYPE_LABELS[value] ?? 'editor.blockParagraph',
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
</script>

<template>
  <div class="tb" role="toolbar" :aria-label="t('mobile.toolbar')" aria-orientation="horizontal">
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

    <div class="tb__menu-wrap">
      <button type="button" class="tb__btn tb__btn--wide" :disabled="props.disabled" :aria-expanded="showTypeMenu ? 'true' : 'false'" @mousedown.prevent @click="showTypeMenu = !showTypeMenu">
        {{ typeLabel }}
      </button>
      <div v-if="showTypeMenu" class="tb__popover" role="menu">
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

    <button type="button" class="tb__btn" :disabled="props.disabled" :title="t('tb.checklist')" :aria-label="t('tb.checklist')" @mousedown.prevent @click="emit('checklist')">☑</button>
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
  flex: 0 0 auto;
}

.tb__btn {
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
  position: absolute;
  top: calc(100% + var(--space-1));
  left: 0;
  z-index: 40;
  display: flex;
  flex-direction: column;
  min-width: 180px;
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
