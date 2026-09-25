<script setup lang="ts">
/** 工作区：列表 + 编辑器（编辑器再窄也占满剩余宽度）。 */
import { computed } from 'vue';
import NoteList from '../components/NoteList.vue';
import RichEditor from '../components/RichEditor.vue';
import EmptyState from '../components/EmptyState.vue';
import SkeletonRows from '../components/SkeletonRows.vue';
import { useEditorStore } from '../stores/editor';
import { useFolderStore } from '../stores/folders';
import { useNoteStore } from '../stores/notes';
import { useShellStore } from '../stores/shell';
import { t } from '../i18n';

const editor = useEditorStore();
const folders = useFolderStore();
const notes = useNoteStore();
const shell = useShellStore();

const currentRow = computed(() => notes.rowById(notes.selectedId));
const title = computed(() => currentRow.value?.title || t('editor.untitled'));
const folderId = computed(() => currentRow.value?.folderId ?? '');
const pinned = computed(() => currentRow.value?.pinned === true);
const canEdit = computed(() => notes.selectedId !== null && !editor.loading);

function onFolderChange(event: Event): void {
  const id = notes.selectedId;
  if (!id) return;
  void notes.moveTo(id, (event.target as HTMLSelectElement).value || null);
}
</script>

<template>
  <div class="workspace">
    <NoteList v-if="shell.listVisible" @open="shell.openEditor()" />

    <section v-if="shell.editorVisible" class="pane pane--editor" aria-label="Notera" data-testid="editor-pane">
      <div class="pane-header editor-head">
        <button v-if="shell.isCompact" type="button" class="btn btn--quiet btn--icon" :aria-label="t('mobile.back')" data-testid="back-to-list" @click="shell.backToList()">
          ‹
        </button>
        <span class="pane-title" :title="title">{{ title }}</span>

        <select
          class="select editor-head__folder"
          :aria-label="t('sidebar.moveTo')"
          :disabled="!canEdit"
          :value="folderId"
          data-testid="move-folder"
          @change="onFolderChange"
        >
          <option value="">{{ t('sidebar.root') }}</option>
          <option v-for="entry in folders.flat" :key="entry.node.id" :value="entry.node.id">{{ entry.path.join(' / ') }}</option>
        </select>

        <button
          type="button"
          class="btn btn--quiet"
          :disabled="!canEdit"
          :aria-pressed="pinned ? 'true' : 'false'"
          data-testid="toggle-pin"
          @click="notes.selectedId && notes.setPinned(notes.selectedId, !pinned)"
        >
          {{ pinned ? t('list.unpin') : t('list.pin') }}
        </button>
        <button
          type="button"
          class="btn btn--quiet btn--danger"
          :disabled="!canEdit || notes.inTrash"
          data-testid="trash-note"
          @click="notes.selectedId && notes.moveToTrash(notes.selectedId)"
        >
          {{ t('list.trashMode') }}
        </button>
      </div>

      <SkeletonRows v-if="editor.loading" :rows="4" />

      <RichEditor v-else-if="editor.blocks.length > 0" :key="editor.noteId ?? 'empty'" />

      <EmptyState v-else-if="notes.selectedId === null" glyph="✎" :title="t('list.empty')" :hint="t('state.boot')">
        <button type="button" class="btn btn--primary" data-testid="first-new-note" @click="notes.create(notes.mode.kind === 'folder' ? notes.mode.folderId : null)">
          {{ t('list.newNote') }}
        </button>
      </EmptyState>

      <div v-else class="editor-blank" role="status">
        <p class="text-sm text-muted">{{ editor.saveErrorKey ? t('error.fallback') : t('state.loading') }}</p>
        <button type="button" class="btn" @click="notes.selectedId && editor.open(notes.selectedId)">{{ t('sync.retry') }}</button>
      </div>
    </section>
  </div>
</template>

<style scoped>
.workspace {
  display: flex;
  flex: 1;
  min-width: 0;
  min-height: 0;
  gap: var(--space-1);
}

.editor-head {
  gap: var(--space-2);
  flex-wrap: nowrap;
}

.editor-head__folder {
  width: auto;
  max-width: 180px;
  min-height: var(--touch-min);
  font-size: var(--text-sm);
}

.editor-blank {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: var(--space-3);
  padding: var(--space-6);
}
</style>
