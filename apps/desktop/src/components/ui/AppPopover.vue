<script setup lang="ts">
/**
 * 悬浮操作面板：一颗触发按钮 + 一层**不占版面**的面板（Headless UI Popover 管开合、
 * Esc、点击外部、焦点圈定；我们只画样子）。
 *
 * 为什么要有它：以前"确认删除"和"移动到哪个文件夹"是插在列表里的普通块，
 * 一打开就把下面所有行往下顶 —— 用户那句「各种确认删除的提示应该是悬浮窗的层级，
 * 而不是底下占了一个」。占位还有一个更实际的坏处：面板把目标行顶走，
 * 手指/鼠标原位那点下去就点到别的东西了。
 *
 * 位置是**开的时候量出来的**（缺口 G101）：CSS 只会一味朝下开、按 `right: 0` 贴右，
 * 于是 390 触摸档上编辑器角那颗「版本」把面板顶到视口外（实测 `x=-11 / bottom=1137`，
 * 里面那两颗动作看不见也点不到）。翻转与夹取放在这一层，四个调用点一起好 ——
 * 各自在组件里补一次就是第四套真相。
 */
import { Popover, PopoverButton, PopoverPanel } from '@headlessui/vue';
import { ref } from 'vue';
import AppIcon from './AppIcon.vue';
import type { IconName } from './icons';

const props = withDefaults(
  defineProps<{
    /** 触发按钮的可读名字（读屏与 title 都用它）。 */
    label: string;
    icon: IconName;
    testid: string;
    align?: 'start' | 'end';
    disabled?: boolean;
  }>(),
  { align: 'end', disabled: false },
);

const wrap = ref<HTMLElement | null>(null);
const placed = ref<Record<string, string>>({});
let observer: ResizeObserver | null = null;
let watched: HTMLElement | null = null;

/** 离屏幕边至少留这么多 —— 贴边会让描边与阴影看着像被切掉。 */
const EDGE = 8;

/**
 * 面板挂载 = 打开（Headless UI 默认关掉就卸载），所以这个回调就是"开的时候量一次"。
 *
 * 元素**不从回调参数拿**：`ref` 挂在组件上给回来的是实例代理（`$el` 又可能是 Fragment 的锚点文本），
 * 第一版就这么把代理喂给 `ResizeObserver.observe` —— 整棵应用被一条
 * `parameter 1 is not of type 'Element'` 打死（列表根本没渲染出来）。改成从自己的容器里查那一个节点，
 * 并且查不到就什么都不做（宁可少一次翻转，也不许把界面弄没了）。
 *
 * 先读**自然尺寸**再写样式：样式里留着上一次的 `left/top` 会把这一轮的量读歪
 * （同一处偏移连续开两次会一路往屏幕外跑）。
 */
function mountPanel(): void {
  const el = wrap.value?.querySelector('.app-popover__panel');
  const panel = el instanceof HTMLElement ? el : null;
  if (observer && watched === panel) return;
  observer?.disconnect();
  observer = null;
  watched = null;
  if (!panel) {
    placed.value = {};
    return;
  }
  place(panel);
  watched = panel;
  // 面板会**长大**（版本那一格点开某一版之后多出正文与那颗覆盖按钮），
  // 只在挂载时量一次 = 长大之后再越出屏幕，而且是在用户已经看见内容之后。
  if (typeof ResizeObserver !== 'undefined') {
    observer = new ResizeObserver(() => place(panel));
    observer.observe(panel);
  }
}

function place(panel: HTMLElement): void {
  const trigger = wrap.value?.querySelector('button') ?? null;
  if (!trigger) return;
  panel.style.left = '';
  panel.style.top = '';
  panel.style.right = '';
  const t = trigger.getBoundingClientRect();
  const r = panel.getBoundingClientRect();
  const vw = document.documentElement.clientWidth;
  const vh = document.documentElement.clientHeight;
  const gap = 4;

  let left = props.align === 'start' ? t.left : t.right - r.width;
  left = Math.max(EDGE, Math.min(left, vw - r.width - EDGE));
  // 下面放不下就朝上开；上面也放不下时贴着能放的那一侧（面板自己有 max-height + 滚动）。
  const fitsBelow = t.bottom + gap + r.height <= vh - EDGE;
  const top = fitsBelow ? t.bottom + gap : Math.max(EDGE, t.top - gap - r.height);
  placed.value = {
    left: `${Math.round(left - t.left)}px`,
    top: `${Math.round(top - t.top)}px`,
    right: 'auto',
  };
}
</script>

<template>
  <div ref="wrap" class="app-popover">
    <Popover v-slot="{ open, close }">
      <PopoverButton
        class="btn btn--quiet btn--icon"
        :data-testid="props.testid"
        :aria-label="props.label"
        :title="props.label"
        :aria-expanded="open ? 'true' : 'false'"
        :disabled="props.disabled"
      >
        <AppIcon :size="18" :name="props.icon" />
      </PopoverButton>

      <!-- @click.stop：面板长在列表行里面，不挡住冒泡的话点面板会顺手把那一行打开。 -->
      <PopoverPanel
        :ref="mountPanel"
        class="app-popover__panel"
        :class="props.align === 'start' ? 'app-popover__panel--start' : 'app-popover__panel--end'"
        :style="placed"
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
  max-width: min(20rem, calc(100vw - 16px));
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
