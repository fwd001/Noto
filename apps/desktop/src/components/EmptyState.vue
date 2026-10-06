<script setup lang="ts">
withDefaults(
  defineProps<{
    title: string;
    hint?: string;
    glyph?: string;
    compact?: boolean;
  }>(),
  { hint: '', glyph: '≡', compact: false },
);
</script>

<template>
  <div class="empty-state" :class="{ 'empty-state--compact': compact }" data-testid="empty-state">
    <span class="empty-state__glyph" aria-hidden="true">{{ glyph }}</span>
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
