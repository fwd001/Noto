<script setup lang="ts">
import { t } from '../i18n';

withDefaults(defineProps<{ rows?: number }>(), { rows: 5 });
</script>

<template>
  <div class="skeleton" role="status" :aria-label="t('state.loading')" data-testid="skeleton">
    <div v-for="n in rows" :key="n" class="skeleton__row" />
  </div>
</template>
