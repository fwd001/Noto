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
 * 位置是**开的时候量一次**（缺口 G101）：CSS 只会一味朝下开、按 `right: 0` 贴右，
 * 于是 390 触摸档上编辑器角那颗「版本」把面板顶到视口外（实测 `x=-11 / bottom=1137`，
 * 里面那两颗动作看不见也点不到；1440×900 也漏，`y=891 / bottom=1147`）。
 * 翻转与夹取放在这一层，几个调用点一起好 —— 各自在组件里补一次就是第四套真相。
 */
import { Popover, PopoverButton, PopoverPanel } from '@headlessui/vue';
import { nextTick, ref } from 'vue';
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

/** 离屏幕边至少留这么多 —— 贴边会让描边与阴影看着像被切掉。 */
const EDGE = 8;

/**
 * 面板挂载 = 打开（Headless UI 默认关掉就卸载），所以这个回调就是"开的时候量一次"。
 *
 * 三条都是踩出来的，改回去之前先读完：
 *  · **元素不从回调参数拿** —— `ref` 挂在组件上给回来的是实例代理（`$el` 还可能是 Fragment 的
 *    锚点文本），当 DOM 节点用会把整棵应用弄白屏；
 *  · **要等一拍**（`nextTick`）—— 回调触发时父组件自己的模板 ref 还没赋值（Vue 先挂子节点、
 *    后回填父 ref），当场量读到的是 null；
 *  · **样式直接写到节点上，不走响应式 `:style`** —— 上一版把量出来的值塞进 `ref` 再绑到 `:style`，
 *    面板挂载后因此多一次重渲染；整跑里那条"Esc 关得掉那层确认"的腿之后就被一个没关掉的面板拦死
 *    （`verify-layout.mjs:450` hover 超时，两次同样红；回退之后 554 项全绿）。
 *    **那是现象、不是已证的根因**；但直接写样式同时是更小的实现 —— 少一次重渲染，就没有那次
 *    重渲染能惹的事，也不用先欠着一个猜。
 */
function mountPanel(raw: unknown): void {
  if (raw === null) return;
  void nextTick(() => {
    const host = wrap.value;
    const panel = host?.querySelector('.app-popover__panel');
    const trigger = host?.querySelector('button');
    if (!(panel instanceof HTMLElement && trigger instanceof HTMLElement)) return;
    // 先清掉上一轮的 left/top 再量自然尺寸：留着会把这一轮读歪（连开两次会一路往屏幕外跑）。
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
    panel.style.left = `${Math.round(left - t.left)}px`;
    panel.style.top = `${Math.round(top - t.top)}px`;
    panel.style.right = 'auto';
  });
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
