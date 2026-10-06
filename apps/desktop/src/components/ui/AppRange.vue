<script setup lang="ts">
import { computed } from 'vue';

/**
 * 统一的滑杆：轨道与滑块都由 token 画，取值语义仍交给原生 `<input type="range">`。
 *
 * 为什么不用默认那一颗：它**完全没有样式入口** —— 轨道粗细、滑块形状、焦点环都是系统画的，
 * Windows 与 macOS 上是两个不同的控件（用户第 ⑪ 条点名的就是这一类）。
 * 而本项目这颗是「编辑器字号」，值会直接改变正文大小，滑块太小时在手机上根本按不住，
 * 所以触摸端额外放大滑块（见 `pointer: coarse` 那一段）。
 *
 * 只换绘制：键盘（←→ / Home / End / PgUp / PgDn）、`aria` 的取值与范围、拖拽行为都是浏览器那份。
 */
const props = withDefaults(
  defineProps<{
    modelValue: number;
    min: number;
    max: number;
    step?: number;
    label: string;
    testid?: string;
  }>(),
  { step: 1, testid: 'app-range' },
);

const emit = defineEmits<{ 'update:modelValue': [value: number] }>();

/** 已走过的那一段用主色填（原生那颗在两端一个填一个不填，正是"不一致"的来源）。 */
const fillPercent = computed(() => {
  const span = props.max - props.min;
  if (span <= 0) return 0;
  return Math.min(100, Math.max(0, ((props.modelValue - props.min) / span) * 100));
});
</script>

<template>
  <label class="app-range" :data-testid="`${props.testid}-row`">
    <input
      type="range"
      class="app-range__input"
      :style="{ '--app-range-fill': `${fillPercent}%` }"
      :min="props.min"
      :max="props.max"
      :step="props.step"
      :value="props.modelValue"
      :aria-label="props.label"
      :data-testid="props.testid"
      @input="emit('update:modelValue', Number(($event.target as HTMLInputElement).value))"
    />
  </label>
</template>

<style scoped>
.app-range {
  display: flex;
  align-items: center;
  min-height: var(--touch);
}

.app-range__input {
  appearance: none;
  -webkit-appearance: none;
  width: 100%;
  height: 1.5rem;
  margin: 0;
  background: none;
  cursor: pointer;
}

.app-range__input::-webkit-slider-runnable-track {
  height: 0.375rem;
  border-radius: var(--r-chip);
  background: linear-gradient(
    to right,
    var(--accent) 0 var(--app-range-fill),
    var(--line) var(--app-range-fill) 100%
  );
}

.app-range__input::-webkit-slider-thumb {
  appearance: none;
  -webkit-appearance: none;
  width: 1.25rem;
  height: 1.25rem;
  margin-top: -0.4375rem;
  border: 1px solid var(--line-strong);
  border-radius: var(--r-chip);
  background: var(--canvas);
}

.app-range__input:focus-visible {
  outline: none;
}

.app-range__input:focus-visible::-webkit-slider-thumb {
  outline: var(--focus-width) solid var(--accent);
  outline-offset: 2px;
}

/* 触摸端：滑块是要按住的，1.25rem 按不准 —— 放大到 1.75rem，轨道同步加粗 */
@media (pointer: coarse) {
  .app-range__input {
    height: var(--touch);
  }

  .app-range__input::-webkit-slider-runnable-track {
    height: 0.5rem;
  }

  .app-range__input::-webkit-slider-thumb {
    width: 1.75rem;
    height: 1.75rem;
    margin-top: -0.625rem;
  }
}
</style>
