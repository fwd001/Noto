<script setup lang="ts">
/**
 * 统一的模态对话框（Headless UI Dialog 管焦点陷阱、Esc、遮罩；我们只画样子）。
 *
 * 为什么要有它：以前"新建文件夹"是在列表底下**长出一行输入框**，
 * 用户的说法是「所有的操作都是弹窗输入，或者在原始的那一行里输出，
 * 而不是底下突然补充一个新的」。这条走"弹窗"那一支。
 *
 * 提交语义说清楚：`confirmDisabled` 由调用方给（比如名字为空），
 * 组件自己**不猜**什么叫"可以提交" —— 空名字该禁用还是该报错是产品决定，不在这里定。
 */
import { Dialog, DialogOverlay, DialogPanel, DialogTitle } from '@headlessui/vue';
import { t } from '../../i18n';

const props = withDefaults(
  defineProps<{
    open: boolean;
    title: string;
    confirmLabel?: string;
    cancelLabel?: string;
    confirmDisabled?: boolean;
    testid: string;
  }>(),
  { confirmLabel: undefined, cancelLabel: undefined, confirmDisabled: false },
);

const emit = defineEmits<{ close: []; confirm: [] }>();
</script>

<template>
  <Dialog :open="props.open" @close="emit('close')">
    <DialogOverlay class="app-dialog__scrim" />
    <div class="app-dialog__center">
      <DialogPanel
        class="app-dialog"
        :data-testid="props.testid"
        :initial-focus="{ tagName: 'INPUT' }"
      >
        <DialogTitle class="app-dialog__title">{{ props.title }}</DialogTitle>
        <slot />
        <div class="app-dialog__actions">
          <button type="button" class="btn btn--quiet" data-testid="app-dialog-cancel" @click="emit('close')">
            {{ props.cancelLabel ?? t('list.cancel') }}
          </button>
          <button
            type="button"
            class="btn btn--primary"
            data-testid="app-dialog-confirm"
            :disabled="props.confirmDisabled"
            @click="emit('confirm')"
          >
            {{ props.confirmLabel ?? t('list.confirm') }}
          </button>
        </div>
      </DialogPanel>
    </div>
  </Dialog>
</template>

<style scoped>
.app-dialog__scrim {
  position: fixed;
  inset: 0;
  background: var(--bg-overlay);
}

.app-dialog__center {
  position: fixed;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: var(--space-4);
  pointer-events: none;
}

.app-dialog {
  pointer-events: auto;
  display: flex;
  flex-direction: column;
  gap: var(--space-3);
  width: min(26rem, 100%);
  padding: var(--space-4);
  border: 1px solid var(--border-strong);
  border-radius: var(--radius-3);
  background: var(--bg-raised);
  box-shadow: var(--shadow-2);
}

.app-dialog__title {
  margin: 0;
  font-size: var(--text-md);
  font-weight: 650;
  color: var(--text-primary);
}

.app-dialog__actions {
  display: flex;
  justify-content: flex-end;
  gap: var(--space-2);
}
</style>
