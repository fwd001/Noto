<script setup lang="ts">
import { ref } from 'vue';
import { useNoteStore } from '../stores/notes';
import { t } from '../i18n';
import AppIcon from './ui/AppIcon.vue';

const notes = useNoteStore();
const inputEl = ref<HTMLInputElement | null>(null);

function onInput(event: Event): void {
  notes.requestSearch((event.target as HTMLInputElement).value);
}

function focus(): void {
  inputEl.value?.focus();
  inputEl.value?.select();
}

function clear(): void {
  notes.clearSearch();
  inputEl.value?.focus();
}

defineExpose({ focus, clear });
</script>

<template>
  <div class="search">
    <input
      id="note-search"
      ref="inputEl"
      class="input search__input"
      type="search"
      role="searchbox"
      autocomplete="off"
      spellcheck="false"
      :aria-label="t('list.searchPlaceholder')"
      :placeholder="t('list.searchPlaceholder')"
      :value="notes.query"
      data-testid="search-input"
      @input="onInput"
      @keydown.escape.prevent="clear()"
    />
    <button v-if="notes.query" type="button" class="btn btn--quiet btn--icon search__clear" :aria-label="t('state.dismiss')" data-testid="search-clear" @click="clear">
      <AppIcon :size="18" name="close" />
    </button>
  </div>
</template>

<style scoped>
.search {
  position: relative;
  display: flex;
  align-items: center;
  padding: var(--sp-2) var(--sp-3);
  border-bottom: 1px solid var(--line);
  flex: 0 0 auto;
}

.search__input {
  padding-right: var(--sp-12);
}

.search__input::-webkit-search-cancel-button {
  display: none;
}

.search__clear {
  position: absolute;
  right: var(--sp-3);
}
</style>
