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
      <!-- §4.9：Toast 要带一颗「知道了」。以前这里是一颗 `×`（U+00D7，图标门禁的枚举表没列到它）——
           既是拿符号顶图形，也少了"这条提示我读过了"那句话。文案键 `state.dismiss` 本来就是"知道了"。 -->
      <button type="button" class="btn btn--quiet toast__ack" data-testid="toast-ack" @click="toasts.dismiss(toast.id)">{{ t('state.dismiss') }}</button>
    </div>
  </div>
</template>
