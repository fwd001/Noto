<script setup lang="ts">
/** 空态/错误态那一格。§2：图形一律 SVG —— 这里的 glyph 位原来是 ≡ ! ? ✓ ✎ 五个字形。 */
import AppIcon from './ui/AppIcon.vue';
import type { IconName } from './ui/icons';

withDefaults(
  defineProps<{
    title: string;
    hint?: string;
    icon?: IconName;
    compact?: boolean;
  }>(),
  { hint: '', icon: 'list', compact: false },
);
</script>

<template>
  <div class="empty-state" :class="{ 'empty-state--compact': compact }" data-testid="empty-state">
    <AppIcon class="empty-state__glyph" :name="icon" :size="28" />
    <p class="empty-state__title">{{ title }}</p>
    <p v-if="hint" class="text-sm text-muted">{{ hint }}</p>
    <slot />
  </div>
</template>

<style scoped>
.empty-state--compact {
  min-height: 140px;
  padding: var(--sp-6) var(--sp-4);
}
</style>
