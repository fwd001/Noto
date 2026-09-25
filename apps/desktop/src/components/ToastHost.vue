<script setup lang="ts">
import { useToastStore } from '../stores/toasts';
import { t } from '../i18n';

const toasts = useToastStore();
</script>

<template>
  <div class="toast-host" data-testid="toast-host">
    <div v-for="toast in toasts.items" :key="toast.id" class="toast" :class="`toast--${toast.level}`" role="status">
      <span>{{ toast.text }}</span>
      <span class="banner__spacer" />
      <button type="button" class="btn btn--quiet btn--icon" :aria-label="t('state.dismiss')" @click="toasts.dismiss(toast.id)">×</button>
    </div>
  </div>
</template>
