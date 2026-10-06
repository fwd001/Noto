<script setup lang="ts">
/**
 * 一枚 SVG 图标（设计稿 §2 / §2.5）。规格与形状清单在 `./icons.ts`，那里是唯一来源。
 *
 * 尺寸：20 是基准画布；§2.5 说的 28/44/52 是**触摸容器**的尺寸，不是把图标放大。
 * 可及性：旁边没有文字时必须传 `label`，否则整枚 `aria-hidden`（§5 要求每个可交互控件有可读名字，
 * 那是控件的事，不是装饰图的事 —— 装饰图进可及性树反而会被念两遍）。
 */
import { computed } from 'vue';
import { ICONS, type IconName } from './icons';

const props = withDefaults(defineProps<{
  name: IconName;
  size?: number;
  testid?: string;
  label?: string;
}>(), { size: 20 });

const spec = computed(() => ICONS[props.name] ?? { d: [] });
/** 描边随尺寸等比缩放，保持 §2.1 那个 1.75 : 20 的比例（同屏不许混粗细）。 */
const stroke = computed(() => Number((props.size / 20 * 1.75).toFixed(2)));
</script>

<template>
  <svg
    class="icon"
    :width="props.size"
    :height="props.size"
    viewBox="0 0 20 20"
    :aria-label="props.label"
    :aria-hidden="props.label ? undefined : 'true'"
    :data-testid="props.testid"
    :data-icon="props.name"
    fill="none"
  >
    <g
      stroke="currentColor"
      :stroke-width="stroke"
      stroke-linecap="round"
      stroke-linejoin="round"
      fill="none"
    >
      <path v-for="(d, i) in spec.d" :key="`p${i}`" :d="d" />
    </g>
    <g fill="currentColor" stroke="none">
      <path v-for="(d, i) in spec.solid ?? []" :key="`s${i}`" :d="d" />
      <circle v-for="(c, i) in spec.dots ?? []" :key="`c${i}`" :cx="c[0]" :cy="c[1]" :r="stroke" />
    </g>
  </svg>
</template>

<style scoped>
.icon {
  display: inline-block;
  flex: 0 0 auto;
  vertical-align: middle;
  color: inherit;
}
</style>
