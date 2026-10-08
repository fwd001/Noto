<script setup lang="ts">
/**
 * §6「版本历史浏览」那一格（缺口 G100）。
 *
 * 挂在编辑器文档头的「改于 …」旁边 —— 那行字本来就在说"这一版是什么时候的"，
 * 再往右一步就是"还有哪几版"，不必为它新造一个入口。
 *
 * 三件事是刻意的：
 *  · 列表一行只说**谁、什么时候、是不是现在**；正文要点开那一行才读（见 stores/versions 的口径）。
 *  · 覆盖之前要**再点一次确认**，并且明说"现在写的内容不会丢"——它变成历史里的另一行。
 *  · 只读的那几种场合（库过新、在回收站）**根本不渲染那颗覆盖按钮**，不画灰着的控件（§2.5）。
 */
import { computed, ref, watch } from 'vue';
import AppPopover from './ui/AppPopover.vue';
import { useEditorStore } from '../stores/editor';
import { useVersionsStore } from '../stores/versions';
import { t } from '../i18n';
import { formatWhen } from '../util/format';

const props = defineProps<{ noteId: string }>();

const editor = useEditorStore();
const versions = useVersionsStore();

const previewing = ref<number | null>(null);
const arming = ref(false);
const done = ref<string | null>(null);
const failedWrite = ref(false);

const canWrite = computed(() => !editor.writeBlocked && !editor.inTrash);

watch(
  () => props.noteId,
  (id) => {
    previewing.value = null;
    arming.value = false;
    done.value = null;
    failedWrite.value = false;
    versions.closePreview();
    if (id) void versions.load(id);
  },
  { immediate: true },
);

function originKey(origin: string): string {
  const known = ['local', 'remote', 'merged', 'conflictCopy', 'restored'];
  const camel = origin.includes('_')
    ? origin.replace(/_([a-z])/g, (_, c: string) => String(c).toUpperCase())
    : origin;
  return known.includes(camel) ? `editor.origin${camel[0].toUpperCase()}${camel.slice(1)}` : 'editor.originLocal';
}

function rowText(rev: number, origin: string, when: string): string {
  return t('editor.versionRow', { rev, who: t(originKey(origin)), when: formatWhen(when) || when });
}

async function pick(rev: number): Promise<void> {
  if (previewing.value === rev) {
    previewing.value = null;
    versions.closePreview();
    return;
  }
  previewing.value = rev;
  arming.value = false;
  done.value = null;
  failedWrite.value = false;
  await versions.openPreview(props.noteId, rev);
}

function rowIsCurrent(rev: number): boolean {
  return versions.rows.find((r) => r.rev === rev)?.sameAsNow === true;
}

async function confirmRestore(rev: number): Promise<void> {
  const to = await versions.restore(props.noteId, rev);
  arming.value = false;
  if (to === null) {
    failedWrite.value = true;
    return;
  }
  done.value = t('editor.versionRestoreDone', { from: rev, to });
  // 屏幕上那一版由 `notes-changed` 那次回读换过来（编辑器自己订阅了它）—— 这里不再补一次
  // `editor.open()`：变异台量过一次"把这行删掉，判据照样全绿"，说明它是第二条来路，
  // 留着只会让人以为重读靠的是它（两处各读一次，下一次改事件回读的人看不见这里）。
  await versions.load(props.noteId);
  previewing.value = null;
  versions.closePreview();
}
</script>

<template>
  <AppPopover
    :label="t('editor.versions')"
    icon="chevron-down"
    testid="version-history"
    align="end"
  >
    <template #default="{ close }">
      <div class="versions" data-testid="version-panel">
        <p class="versions__title">{{ t('editor.versionsTitle') }}</p>

        <p v-if="versions.failed" class="text-sm text-muted" data-testid="version-failed">
          {{ t('editor.versionFailed') }}
        </p>
        <template v-else>
          <p v-if="versions.rows.length === 0" class="text-sm text-muted" data-testid="version-empty">
            {{ t('editor.versionEmpty') }}
          </p>
          <ul v-else class="versions__list" data-testid="version-list">
            <li v-for="r in versions.rows" :key="r.rev" class="versions__row">
              <button
                type="button"
                class="versions__row-btn"
                :data-testid="`version-row-${r.rev}`"
                :aria-pressed="previewing === r.rev ? 'true' : 'false'"
                @click="pick(r.rev)"
              >
                {{ rowText(r.rev, r.origin, r.createdAt) }}
              </button>
              <span v-if="r.sameAsNow" class="versions__now" data-testid="version-now-tag">
                {{ t('editor.versionNow') }}
              </span>
            </li>
          </ul>
          <p v-if="versions.truncated" class="text-sm text-muted" data-testid="version-truncated">
            {{ t('editor.versionTruncated') }}
          </p>
        </template>

        <p v-if="versions.previewFailed" class="text-sm text-muted" data-testid="version-preview-failed">
          {{ t('editor.versionPreviewFailed') }}
        </p>
        <div v-if="versions.preview" class="versions__preview" data-testid="version-preview">
          <p class="versions__preview-title">{{ t('editor.versionPreview') }}</p>
          <p v-for="(line, i) in versions.previewLines" :key="i" class="versions__line">{{ line }}</p>
          <p v-if="versions.previewLines.length === 0" class="text-sm text-muted">
            {{ t('editor.versionBlank') }}
          </p>
          <div v-if="canWrite && previewing !== null && !rowIsCurrent(previewing)" class="versions__actions">
            <button
              v-if="!arming"
              type="button"
              class="btn btn--quiet"
              data-testid="version-restore"
              @click="arming = true"
            >
              {{ t('editor.versionRestore') }}
            </button>
            <div v-else class="versions__confirm" data-testid="version-restore-confirm">
              <p class="text-sm">{{ t('editor.versionRestoreArmed', { rev: versions.currentRev + 1 }) }}</p>
              <span class="versions__confirm-row">
                <button type="button" class="btn btn--danger" data-testid="version-restore-yes" @click="confirmRestore(previewing as number)">
                  {{ t('list.confirm') }}
                </button>
                <button type="button" class="btn btn--quiet" data-testid="version-restore-no" @click="arming = false">
                  {{ t('list.cancel') }}
                </button>
              </span>
            </div>
          </div>
        </div>

        <p v-if="failedWrite" class="text-sm text-muted" data-testid="version-restore-failed">
          {{ t('editor.versionRestoreFailed') }}
        </p>
        <p v-if="done" class="text-sm text-muted" data-testid="version-restore-done">{{ done }}</p>

        <span class="versions__foot">
          <button type="button" class="btn btn--quiet" data-testid="version-close" @click="close()">
            {{ t('editor.versionClose') }}
          </button>
        </span>
      </div>
    </template>
  </AppPopover>
</template>

<style scoped>
.versions {
  display: flex;
  flex-direction: column;
  gap: var(--sp-2);
  min-width: 260px;
  max-width: 380px;
}
.versions__title,
.versions__preview-title {
  font-size: var(--text-xs);
  color: var(--mute);
}
.versions__list {
  display: flex;
  flex-direction: column;
  gap: var(--sp-1);
  margin: 0;
  padding: 0;
  list-style: none;
  max-height: 260px;
  overflow-y: auto;
}
.versions__row {
  display: flex;
  align-items: center;
  gap: var(--sp-2);
}
.versions__row-btn {
  flex: 1 1 auto;
  min-height: var(--touch);
  padding: 0 var(--sp-2);
  border: 0;
  border-radius: var(--r-row);
  background: transparent;
  color: var(--ink);
  font-size: var(--text-sm);
  text-align: left;
}
.versions__row-btn:hover {
  background: var(--sunken);
}
.versions__now {
  flex: 0 0 auto;
  padding: 0 var(--sp-2);
  border-radius: var(--r-chip);
  background: var(--sunken);
  color: var(--body);
  font-size: var(--text-xs);
}
.versions__preview {
  display: flex;
  flex-direction: column;
  gap: var(--sp-1);
  padding: var(--sp-2);
  border: 1px solid var(--line);
  border-radius: var(--r-card);
  background: var(--sunken);
  max-height: 220px;
  overflow-y: auto;
}
.versions__line {
  margin: 0;
  color: var(--body);
  font-size: var(--text-sm);
  line-height: 1.5;
}
.versions__actions {
  margin-top: var(--sp-2);
}
.versions__confirm {
  display: flex;
  flex-direction: column;
  gap: var(--sp-2);
}
.versions__confirm-row {
  display: flex;
  gap: var(--sp-2);
}
.versions__foot {
  display: flex;
  justify-content: flex-end;
}
</style>
