<script setup lang="ts">
/**
 * 统一的下拉选择：外观由本项目的 token 决定，行为交给 Headless UI。
 *
 * 为什么不用原生 `<select>`：它的展开面板是**操作系统**画的 —— 同一份代码在
 * Windows 是 Fluent 那一套、在 macOS 是原生那一圈、手机上又是一层原生覆盖，
 * 用户那句「Windows 与 Mac 的 UI 要一致」在这里就没有解。
 * 交给无头库之后，键盘（↑↓ / Home / End / Esc / 首字母跳转）由库负责，
 * 我们只写样式，不再自己实现一遍下拉的行为。
 */
import { computed } from 'vue';
import { Listbox, ListboxButton, ListboxOption, ListboxOptions } from '@headlessui/vue';
import AppIcon from './AppIcon.vue';

export interface SelectOption {
  value: string;
  label: string;
}

const props = withDefaults(
  defineProps<{
    modelValue: string;
    options: readonly SelectOption[];
    /** 读屏与 `labelledby` 用的名字（原生 select 以前靠旁边的 <label> 蹭名字）。 */
    label: string;
    testid?: string;
    disabled?: boolean;
  }>(),
  { testid: 'app-select', disabled: false },
);

const emit = defineEmits<{ 'update:modelValue': [value: string] }>();

const currentLabel = computed(
  () => props.options.find((o) => o.value === props.modelValue)?.label ?? props.options[0]?.label ?? '',
);
</script>

<template>
  <Listbox
    :model-value="props.modelValue"
    :disabled="props.disabled"
    @update:model-value="emit('update:modelValue', String($event))"
  >
    <ListboxButton class="app-select" :data-testid="props.testid" :aria-label="props.label">
      <span class="app-select__value">{{ currentLabel }}</span>
      <AppIcon class="app-select__chevron" :size="16" name="chevron-down" />
    </ListboxButton>

    <ListboxOptions class="app-select__panel" data-testid="app-select-panel">
      <ListboxOption
        v-for="opt in props.options"
        :key="opt.value"
        :value="opt.value"
        class="app-select__option"
        :data-value="opt.value"
      >
        {{ opt.label }}
      </ListboxOption>
    </ListboxOptions>
  </Listbox>
</template>

<style scoped>
.app-select {
  display: inline-flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--sp-2);
  min-height: var(--touch);
  min-width: 8.5rem;
  padding: 0 var(--sp-3);
  border: 1px solid var(--line-strong);
  border-radius: var(--r-card);
  background: var(--canvas);
  color: var(--ink);
  font: inherit;
  font-size: var(--text-sm);
  text-align: start;
  cursor: pointer;
}

.app-select:hover:not(:disabled) {
  border-color: var(--line-strong);
  background: var(--hover);
}

.app-select:disabled {
  opacity: 0.5;
  cursor: default;
}

.app-select__value {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.app-select__chevron {
  flex: 0 0 auto;
  color: var(--mute);
}

/* 面板画在我们这边：这一格就是"两端一致"的全部内容。
   z-index 90 与 base.css 里浮层那一批同档（overlay/drawer 是 100，必须盖住我们）。 */
.app-select__panel {
  z-index: 90;
  max-height: 18rem;
  margin: var(--sp-1) 0;
  padding: var(--sp-1);
  overflow: auto;
  border: 1px solid var(--line-strong);
  border-radius: var(--r-card);
  background: var(--canvas, var(--canvas));
  box-shadow: var(--shadow-2);
  outline: none;
}

.app-select__option {
  display: flex;
  align-items: center;
  min-height: var(--touch);
  padding: 0 var(--sp-3);
  border-radius: var(--r-row);
  color: var(--ink);
  font-size: var(--text-sm);
  cursor: pointer;
}

.app-select__option[data-active='true'] {
  background: var(--hover);
}

.app-select__option[data-selected='true'] {
  color: var(--accent);
  font-weight: 600;
}
</style>
