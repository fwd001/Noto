/** 轻提示：文案一律来自本地文案表（messageKey），不显示原始错误码。 */
import { defineStore } from 'pinia';
import { ref } from 'vue';
import { messageFor, t } from '../i18n';
import { realTimers, type Timers } from '../util/timing';

export type ToastLevel = 'info' | 'warn' | 'error';

export interface Toast {
  id: number;
  text: string;
  level: ToastLevel;
}

const TTL_MS = 4500;
let counter = 0;

export const useToastStore = defineStore('toast', () => {
  const items = ref<Toast[]>([]);
  let timers: Timers = realTimers;

  function dismiss(id: number): void {
    items.value = items.value.filter((item) => item.id !== id);
  }

  function pushText(text: string, level: ToastLevel = 'info'): Toast {
    counter += 1;
    const toast: Toast = { id: counter, text, level };
    items.value = [...items.value, toast];
    timers.set(() => dismiss(toast.id), TTL_MS);
    return toast;
  }

  function push(messageKey: string, level: ToastLevel = 'info'): Toast {
    return pushText(messageFor(messageKey), level);
  }

  function setTimers(next: Timers): void {
    timers = next;
  }

  return { items, push, pushText, dismiss, setTimers, t };
});
