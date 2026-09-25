<script setup lang="ts">
/**
 * 标题栏：
 *  - Windows：自绘标题栏（拖拽区 + 最小/最大/关闭），保留系统贴靠（data-tauri-drag-region）
 *  - macOS：隐藏标题栏，这里只留交通灯位与拖拽区
 *  - 其它：由壳层自带，前端不画
 */
import { computed } from 'vue';
import { inTauri } from '../api/bridge';
import { useSettingsStore } from '../stores/settings';
import { useShellStore } from '../stores/shell';
import { t } from '../i18n';

const settings = useSettingsStore();
const shell = useShellStore();

const chrome = computed(() => settings.caps.windowChrome);
const visible = computed(() => chrome.value !== 'system');
const overlay = computed(() => chrome.value === 'overlay');

async function windowAction(action: 'minimize' | 'maximize' | 'close'): Promise<void> {
  if (!inTauri()) return;
  try {
    const api = await import('@tauri-apps/api/window');
    const win = api.getCurrentWindow();
    if (action === 'minimize') await win.minimize();
    else if (action === 'maximize') await win.toggleMaximize();
    else await win.close();
  } catch {
    // 壳层未就绪时什么都不做，绝不让界面报错
  }
}
</script>

<template>
  <header v-if="visible" class="titlebar" :class="{ 'titlebar--overlay': overlay }" data-testid="titlebar">
    <div class="titlebar__drag" data-tauri-drag-region>
      <button type="button" class="btn btn--quiet btn--icon" :aria-label="t(shell.sidebarOpen ? 'sidebar.collapse' : 'sidebar.expand')" @click="shell.toggleSidebar()">
        ☰
      </button>
      <span class="titlebar__brand" data-tauri-drag-region>{{ t('app.name') }}</span>
      <span v-if="!overlay" class="text-sm text-muted" data-tauri-drag-region>{{ t('app.tagline') }}</span>
    </div>
    <div v-if="!overlay" class="titlebar__actions">
      <button type="button" class="titlebar__button" :aria-label="t('win.minimize')" :title="t('win.minimize')" @click="windowAction('minimize')">
        ─
      </button>
      <button type="button" class="titlebar__button" :aria-label="t('win.maximize')" :title="t('win.maximize')" @click="windowAction('maximize')">
        ▢
      </button>
      <button type="button" class="titlebar__button titlebar__button--close" :aria-label="t('win.close')" :title="t('win.close')" @click="windowAction('close')">
        ×
      </button>
    </div>
  </header>
</template>
