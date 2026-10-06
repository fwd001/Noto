<script setup lang="ts">
/**
 * 悬浮操作面板：一颗触发按钮 + 一层**不占版面**的面板（Headless UI Popover 管开合、
 * Esc、点击外部、焦点圈定；我们只画样子）。
 *
 * 为什么要有它：以前"确认删除"和"移动到哪个文件夹"是插在列表里的普通块，
 * 一打开就把下面所有行往下顶 —— 用户那句「各种确认删除的提示应该是悬浮窗的层级，
 * 而不是底下占了一个」。占位还有一个更实际的坏处：面板把目标行顶走，
 * 手指/鼠标原位那点下去就点到别的东西了。
 */
import { Popover, PopoverButton, PopoverPanel } from '@headlessui/vue';

const props = withDefaults(
  defineProps<{
    /** 触发按钮的可读名字（读屏与 title 都用它）。 */
    label: string;
    icon: string;
    testid: string;
    align?: 'start' | 'end';
    disabled?: boolean;
  }>(),
  { align: 'end', disabled: false },
);
</script>

<template>
  <div class="app-popover">
    <Popover v-slot="{ open, close }">
      <PopoverButton
        class="btn btn--quiet btn--icon"
        :data-testid="props.testid"
        :aria-label="props.label"
        :title="props.label"
        :aria-expanded="open ? 'true' : 'false'"
        :disabled="props.disabled"
      >
        {{ props.icon }}
      </PopoverButton>

      <!-- @click.stop：面板长在列表行里面，不挡住冒泡的话点面板会顺手把那一行打开。 -->
      <PopoverPanel
        class="app-popover__panel"
        :class="props.align === 'start' ? 'app-popover__panel--start' : 'app-popover__panel--end'"
        data-testid="app-popover-panel"
        @click.stop
      >
        <slot :close="close" />
      </PopoverPanel>
    </Popover>
  </div>
</template>

<style scoped>
.app-popover {
  position: relative;
  display: inline-flex;
  flex: 0 0 auto;
}

.app-popover__panel {
  z-index: 90;
  position: absolute;
  top: calc(100% + var(--sp-1));
  display: flex;
  flex-direction: column;
  gap: var(--sp-1);
  min-width: 13rem;
  max-width: min(20rem, 80vw);
  max-height: 16rem;
  padding: var(--sp-2);
  overflow: auto;
  border: 1px solid var(--line-strong);
  border-radius: var(--r-card);
  background: var(--canvas);
  box-shadow: var(--shadow-2);
  color: var(--ink);
  font-size: var(--text-sm);
  text-align: start;
}

.app-popover__panel--end {
  right: 0;
}

.app-popover__panel--start {
  left: 0;
}
</style>
