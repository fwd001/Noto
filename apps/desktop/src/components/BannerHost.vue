<script setup lang="ts">
/** 状态横幅：未连接本地服务 / 离线 / 本地数据版本过新 / 分歧待取舍。 */
import { computed } from 'vue';
import { useEditorStore } from '../stores/editor';
import { useNoteStore } from '../stores/notes';
import { useShellStore } from '../stores/shell';
import { useSyncStore } from '../stores/sync';
import { t, messageFor } from '../i18n';
import { probeLink } from '../api/bridge';
import AppIcon from './ui/AppIcon.vue';

const sync = useSyncStore();
const editor = useEditorStore();
const notes = useNoteStore();
const shell = useShellStore();

const showLink = computed(() => sync.linkDown);
const showOffline = computed(() => !sync.linkDown && sync.badge === 'offline');
const showDb = computed(() => shell.libraryReadOnly);
const showSearchError = computed(() => notes.searchErrorKey !== null);
const searchErrorText = computed(() => messageFor(notes.searchErrorKey ?? 'error.fallback'));

async function reconnect(): Promise<void> {
  sync.setLink('connecting');
  sync.setLink(await probeLink());
  if (sync.link === 'ready') {
    await sync.syncNow();
    await notes.load();
  }
}
</script>

<template>
  <div class="banners" role="region" :aria-label="t('sync.detail')">
    <div v-if="showLink" class="banner banner--danger" data-testid="banner-link">
      <span>{{ t('state.linkDown') }}</span>
      <span class="text-muted">{{ t('state.linkDownHint') }}</span>
      <span class="banner__spacer" />
      <button type="button" class="btn btn--quiet" data-testid="banner-reconnect" @click="reconnect">{{ t('state.reconnect') }}</button>
    </div>

    <div v-else-if="showOffline" class="banner" data-testid="banner-offline">
      <AppIcon :size="18" name="sync-offline" />
      <span>{{ t('state.offlineBanner') }}</span>
    </div>

    <div v-if="showDb" class="banner banner--warn" data-testid="banner-db">
      <AppIcon :size="18" name="warn" />
      <span>{{ t('state.dbTooNew') }}</span>
    </div>

    <div v-if="editor.hasDraftConflict" class="banner banner--warn" data-testid="banner-stale">
      <strong>{{ t('editor.staleTitle') }}</strong>
      <span class="text-muted">{{ t('editor.staleBody') }}</span>
      <span class="banner__spacer" />
      <button type="button" class="btn" data-testid="stale-use-mine" @click="editor.useLocalDraft()">{{ t('editor.useMyDraft') }}</button>
      <button type="button" class="btn btn--quiet" data-testid="stale-discard" @click="editor.discardLocalDraft()">{{ t('editor.discardMyDraft') }}</button>
    </div>

    <div v-if="showSearchError" class="banner">
      <span>{{ searchErrorText }}</span>
      <span class="banner__spacer" />
      <button type="button" class="btn btn--quiet" @click="notes.clearSearch()">{{ t('state.dismiss') }}</button>
    </div>
  </div>
</template>

<style scoped>
.banners {
  display: flex;
  flex-direction: column;
  flex: 0 0 auto;
}
</style>
