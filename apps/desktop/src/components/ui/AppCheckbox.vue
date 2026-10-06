<script setup lang="ts">
/**
 * 统一的复选框：**只把绘制接过来**，语义仍然交给原生 `<input type="checkbox">`。
 *
 * 为什么原生那一颗不行：`accent-color` 只能改颜色，改不了**形状与尺寸** —— Windows 画 Fluent
 * 那颗方圆角、macOS 画另一套、手机上还是系统控件，用户那句「Windows 与 Mac 的 UI 要一致」
 * 在这里就没有解。而且默认那一颗只有 13×13（本项目设置页导出那两格连 class 都没上），
 * 远低于 §6 的 44pt 触摸下限。
 *
 * 为什么不用 `role="checkbox"` 自己画一颗按钮：那样键盘（空格切换）、读屏（真 checkbox 角色
 * 与 label 关联）、表单参与都要自己再实现一遍 —— 无头库的价值在**行为**，而 checkbox 的行为
 * 浏览器已经给了，这里缺的只是外观。所以 `appearance: none` + token 画框，其余不动。
 */
const props = withDefaults(
  defineProps<{
    modelValue: boolean;
    /** 读屏名字；同时作为可见文案（原生那颗靠旁边的 `<label>` 蹭名字，这里显式给）。 */
    label: string;
    testid?: string;
    disabled?: boolean;
  }>(),
  { testid: 'app-checkbox', disabled: false },
);

const emit = defineEmits<{ 'update:modelValue': [value: boolean] }>();
</script>

<template>
  <label class="app-check" :data-testid="`${props.testid}-row`">
    <input
      type="checkbox"
      class="app-check__box"
      :checked="props.modelValue"
      :disabled="props.disabled"
      :aria-label="props.label"
      :data-testid="props.testid"
      @change="emit('update:modelValue', ($event.target as HTMLInputElement).checked)"
    />
    <span class="app-check__text">{{ props.label }}</span>
  </label>
</template>

<style scoped>
/* 整行都是命中区：`min-height` 用 --touch，所以手指点在文案上也能切换 */
.app-check {
  display: flex;
  align-items: center;
  gap: var(--sp-2);
  min-height: var(--touch);
  padding: var(--sp-1) 0;
  cursor: pointer;
}

.app-check__box {
  appearance: none;
  -webkit-appearance: none;
  flex: 0 0 auto;
  display: grid;
  place-items: center;
  width: 1.375rem;
  height: 1.375rem;
  margin: 0;
  border: 1px solid var(--line-strong);
  border-radius: var(--r-row);
  background: var(--surface);
  cursor: pointer;
  transition:
    background var(--motion-fast) var(--ease),
    border-color var(--motion-fast) var(--ease);
}

/* 对勾是画出来的（一条转 45° 的边），不用字形：字体里那颗 ✓ 在两端宽度不同，
   而且未选中时它得完全不存在 —— 用 scale(0) 收掉，保留过渡。 */
.app-check__box::after {
  content: '';
  width: 0.6875rem;
  height: 0.375rem;
  border: 2px solid var(--on-accent);
  border-top: 0;
  border-right: 0;
  transform: rotate(-45deg) scale(0);
  transform-origin: center;
  transition: transform var(--motion-fast) var(--ease);
}

.app-check__box:checked {
  border-color: var(--accent);
  background: var(--accent);
}

.app-check__box:checked::after {
  transform: rotate(-45deg) scale(1);
}

.app-check__box:focus-visible {
  outline: var(--focus-width) solid var(--accent);
  outline-offset: 2px;
}

.app-check__box:disabled {
  cursor: default;
  opacity: 0.5;
}

.app-check__text {
  color: var(--ink);
  font-size: var(--text-sm);
}
</style>
