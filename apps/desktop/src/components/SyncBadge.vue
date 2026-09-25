<script setup lang="ts">
/** 同步徽标：用户可见的全部同步语义，只有四态。 */
import { computed } from 'vue';
import { useSyncStore } from '../stores/sync';
import { t } from '../i18n';

const sync = useSyncStore();

const GLYPHS: Record<string, string> = {
  synced: '✓',
  syncing: '↻',
  offline: '○',
  failed: '!',
};

const glyph = computed(() => GLYPHS[sync.badge] ?? '!');
const detailText = computed(() => (sync.detail ? `${sync.label} · ${sync.detail}` : sync.label));
const title = computed(() => (sync.badge === 'failed' ? t('sync.localReady') : sync.label));
</script>

<template>
  <div class="syncline">
    <button
      type="button"
      class="badge"
      :data-badge="sync.badge"
      :title="title"
      :aria-busy="sync.badge === 'syncing' ? 'true' : 'false'"
      :disabled="sync.busy"
      data-testid="sync-badge"
      @click="sync.syncNow()"
    >
      <span class="badge__glyph" aria-hidden="true">{{ glyph }}</span>
      <span>{{ sync.label }}</span>
      <span v-if="sync.percent !== null" class="text-muted">{{ t('sync.progress', { done: sync.percent, total: 100 }) }}</span>
    </button>
    <button v-if="sync.showRetry" type="button" class="btn btn--quiet text-sm" data-testid="sync-retry" @click="sync.syncNow()">
      {{ t('sync.retry') }}
    </button>
    <span class="visually-hidden" role="status" aria-live="polite">{{ detailText }}</span>
  </div>
</template>

<style scoped>
.syncline {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  flex-wrap: wrap;
}
</style>
