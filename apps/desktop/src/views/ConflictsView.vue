<script setup lang="ts">
/** 分歧收件箱：并排两份内容 + 三个动作（保留两份 / 用这个替换 / 我来合并）。 */
import { computed, onMounted } from 'vue';
import EmptyState from '../components/EmptyState.vue';
import SkeletonRows from '../components/SkeletonRows.vue';
import { useConflictStore, type ConflictAction } from '../stores/conflicts';
import { useEditorStore } from '../stores/editor';
import { useNoteStore } from '../stores/notes';
import { useShellStore } from '../stores/shell';
import { t, messageFor } from '../i18n';
import { formatWhen } from '../util/format';
import type { ConflictCard } from '../api/types';

const conflicts = useConflictStore();
const editor = useEditorStore();
const notes = useNoteStore();
const shell = useShellStore();

const cards = computed(() => conflicts.cards);
const selected = computed<ConflictCard | null>(() => conflicts.selected);
const errorText = computed(() => (conflicts.errorKey ? messageFor(conflicts.errorKey) : ''));
const localText = computed(() => (selected.value ? conflicts.previewFor(selected.value, 'local') : ''));
const remoteText = computed(() => (selected.value ? conflicts.previewFor(selected.value, 'remote') : ''));

function titleOf(card: ConflictCard): string {
  return card.noteTitle ?? card.title ?? t('conflict.title');
}

function choose(card: ConflictCard): void {
  conflicts.selectedId = card.conflictId;
}

async function act(action: ConflictAction): Promise<void> {
  const card = selected.value;
  if (!card) return;
  const done = await conflicts.resolve(card, action);
  if (!done) return;
  if (action === 'manualMerge') {
    const target = card.copyNoteId ?? card.noteId;
    if (target) {
      await notes.setMode({ kind: 'all' }, target);
      await editor.open(target);
      shell.goto('workspace');
      if (shell.isCompact) shell.openEditor();
    }
  }
}

onMounted(() => {
  void conflicts.load();
});
</script>

<template>
  <section class="pane conflicts" aria-label="Notera" data-testid="conflicts-view">
    <div class="pane-header">
      <button type="button" class="btn btn--quiet btn--icon" :aria-label="t('mobile.back')" @click="shell.goto('workspace')">‹</button>
      <span class="pane-title">{{ t('conflict.title') }}</span>
      <span v-if="conflicts.count > 0" class="text-sm text-muted">{{ t('conflict.count', { count: conflicts.count }) }}</span>
    </div>

    <p v-if="errorText" class="banner banner--danger" role="alert">{{ errorText }}</p>

    <div class="pane-body conflicts__body">
      <SkeletonRows v-if="conflicts.loading && cards.length === 0" :rows="3" />

      <EmptyState v-else-if="cards.length === 0" glyph="✓" :title="t('conflict.empty')" :hint="t('conflict.emptyHint')" />

      <div v-else class="conflicts__grid">
        <ul class="conflicts__list" role="listbox" :aria-label="t('conflict.title')">
          <li v-for="card in cards" :key="card.conflictId">
            <button
              type="button"
              class="conflicts__item"
              :data-active="card.conflictId === selected?.conflictId ? 'true' : 'false'"
              role="option"
              :aria-selected="card.conflictId === selected?.conflictId ? 'true' : 'false'"
              :data-testid="`conflict-${card.conflictId}`"
              @click="choose(card)"
            >
              <span class="conflicts__item-title">{{ titleOf(card) }}</span>
              <span class="text-sm text-muted">{{ formatWhen(card.createdAt) }}</span>
            </button>
          </li>
        </ul>

        <div v-if="selected" class="conflicts__detail">
          <div class="conflicts__panes">
            <figure class="conflicts__pane">
              <figcaption>{{ t('conflict.mine') }}</figcaption>
              <pre class="conflicts__text">{{ localText }}</pre>
              <button type="button" class="btn" :data-testid="`conflict-use-${selected.conflictId}`" @click="act('replaceWithLocal')">
                {{ t('conflict.replaceWithThis') }}
              </button>
            </figure>
            <figure class="conflicts__pane">
              <figcaption>{{ t('conflict.theirs') }}</figcaption>
              <pre class="conflicts__text">{{ remoteText }}</pre>
              <button type="button" class="btn" @click="act('replaceWithRemote')">{{ t('conflict.replaceWithThis') }}</button>
            </figure>
          </div>

          <p class="field-hint">{{ t('conflict.mergeHint') }}</p>

          <div class="row">
            <button type="button" class="btn btn--primary" data-testid="conflict-keep-both" aria-describedby="keep-both-hint" @click="act('keepBoth')">
              {{ t('conflict.keepBoth') }}
            </button>
            <span id="keep-both-hint" class="text-sm text-muted">{{ t('conflict.emptyHint') }}</span>
            <button type="button" class="btn" data-testid="conflict-manual-merge" @click="act('manualMerge')">{{ t('conflict.manualMerge') }}</button>
            <button v-if="selected.noteId ?? selected.copyNoteId" type="button" class="btn btn--quiet" @click="act('manualMerge')">{{ t('conflict.openNote') }}</button>
          </div>
        </div>
      </div>
    </div>
  </section>
</template>

<style scoped>
.conflicts {
  flex: 1 1 auto;
  min-width: 0;
}

.conflicts__body {
  padding: var(--space-4);
}

.conflicts__grid {
  display: grid;
  grid-template-columns: minmax(200px, 260px) minmax(0, 1fr);
  gap: var(--space-4);
  align-items: start;
}

.conflicts__list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
}

.conflicts__item {
  width: 100%;
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: var(--space-1);
  min-height: var(--touch-min);
  padding: var(--space-2) var(--space-3);
  border-radius: var(--radius-2);
  border: 1px solid transparent;
  cursor: pointer;
  text-align: left;
}

.conflicts__item:hover {
  background: var(--bg-hover);
}

.conflicts__item[data-active='true'] {
  background: var(--accent-soft);
  border-color: var(--accent);
}

.conflicts__item-title {
  font-weight: 650;
}

.conflicts__panes {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(min(320px, 100%), 1fr));
  gap: var(--space-3);
}

.conflicts__pane {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
  padding: var(--space-3);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-3);
  background: var(--bg-raised);
  min-width: 0;
}

.conflicts__pane figcaption {
  font-weight: 700;
  font-size: var(--text-sm);
  color: var(--text-secondary);
}

.conflicts__text {
  margin: 0;
  min-height: 160px;
  max-height: 420px;
  overflow: auto;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
  font-family: var(--font-mono);
  font-size: var(--text-sm);
  background: var(--bg-sunken);
  border-radius: var(--radius-2);
  padding: var(--space-3);
}

@media (max-width: 820px) {
  .conflicts__grid {
    grid-template-columns: minmax(0, 1fr);
  }
}
</style>
