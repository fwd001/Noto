<script setup lang="ts">
/** 设置页：账户 / 代理 / 证书 / 外观 / 数据 / 快捷键（按能力显示）。 */
import { computed, onMounted, ref } from 'vue';
import SyncBadge from '../components/SyncBadge.vue';
import { currentTransport } from '../api/bridge';
import type { ProxyMode, TlsPolicyKind } from '../api/types';
import { useSettingsStore } from '../stores/settings';
import { useSyncStore } from '../stores/sync';
import { useNoteStore } from '../stores/notes';
import { useShellStore } from '../stores/shell';
import { shortcutsFor, type PlatformCaps } from '../platform/caps';
import { t, messageFor } from '../i18n';
import { formatBytes, formatNumber, formatWhen } from '../util/format';
import { FONT_SCALE_MAX, FONT_SCALE_MIN, type ThemeMode } from '../stores/settings';

const settings = useSettingsStore();
const sync = useSyncStore();
const notes = useNoteStore();
const shell = useShellStore();

const bypassText = ref('');
const savedAt = ref<number | null>(null);
const dataPath = ref('');
const importMode = ref<'intoEmpty' | 'merge'>('merge');

const tlsOptions: Array<{ value: TlsPolicyKind; label: string }> = [
  { value: 'strict', label: 'settings.tlsStrict' },
  { value: 'pin', label: 'settings.tlsPin' },
  { value: 'caBundle', label: 'settings.tlsCaBundle' },
  { value: 'insecureLocal', label: 'settings.tlsInsecure' },
];

const proxyOptions: Array<{ value: ProxyMode; label: string }> = [
  { value: 'direct', label: 'settings.proxyDirect' },
  { value: 'system', label: 'settings.proxySystem' },
  { value: 'http', label: 'settings.proxyHttp' },
  { value: 'https', label: 'settings.proxyHttps' },
  { value: 'socks5', label: 'settings.proxySocks5' },
];

const themeOptions: Array<{ value: ThemeMode; label: string }> = [
  { value: 'system', label: 'settings.themeSystem' },
  { value: 'light', label: 'settings.themeLight' },
  { value: 'dark', label: 'settings.themeDark' },
];

const proxyNeedsHost = computed(() => ['http', 'https', 'socks5'].includes(settings.draft.proxy.mode ?? 'direct'));
const needsProxyAuth = computed(() => proxyNeedsHost.value);
const accountError = computed(() => (settings.accountErrorKey ? messageFor(settings.accountErrorKey) : ''));
const preferMacKeys = computed(() => settings.caps.windowChrome === 'overlay');
const shortcuts = computed(() => shortcutsFor(settings.caps as PlatformCaps));
const transportLabel = computed(() => (currentTransport() === 'tauri' ? t('settings.pathTauri') : t('settings.pathHttp')));
const report = computed(() => settings.describeReport(settings.lastReport));

function keysFor(entry: { keys: string[]; macKeys: string[] }): string {
  return (preferMacKeys.value ? entry.macKeys : entry.keys).join(preferMacKeys.value ? '' : '+');
}

function onPortInput(event: Event): void {
  const raw = (event.target as HTMLInputElement).value.trim();
  const parsed = Number.parseInt(raw, 10);
  settings.draft.proxy.port = Number.isFinite(parsed) && parsed > 0 ? parsed : undefined;
}

onMounted(async () => {
  bypassText.value = (settings.draft.proxy.bypass ?? []).join('\n');
  await settings.loadAccount();
  await settings.loadStats();
  bypassText.value = (settings.draft.proxy.bypass ?? []).join('\n');
});

async function save(): Promise<void> {
  settings.draft.proxy.bypass = bypassText.value
    .split('\n')
    .map((line) => line.trim())
    .filter((line) => line.length > 0);
  const ok = await settings.saveAccount();
  if (ok) {
    savedAt.value = Date.now();
    settings.draft.password = '';
    await sync.syncNow();
  }
}

async function syncNow(): Promise<void> {
  await sync.syncNow();
}

async function doExport(): Promise<void> {
  await settings.exportData({
    folderIds: [],
    includeAttachments: true,
    includeTrash: true,
    ...(dataPath.value.trim() ? { path: dataPath.value.trim() } : {}),
  });
  await settings.loadStats();
}

async function doImport(): Promise<void> {
  await settings.importData({
    mode: importMode.value,
    ...(dataPath.value.trim() ? { path: dataPath.value.trim() } : {}),
  });
  await settings.loadStats();
  await notes.load();
}

async function doBackup(): Promise<void> {
  await settings.exportData({ folderIds: [], includeAttachments: true, includeTrash: true, ...(dataPath.value.trim() ? { path: dataPath.value.trim() } : {}) });
}

async function doRestore(): Promise<void> {
  await settings.importData({ mode: 'intoEmpty', ...(dataPath.value.trim() ? { path: dataPath.value.trim() } : {}) });
  await notes.load();
}

function keyHint(): string {
  return shortcuts.value.map((entry) => keysFor(entry)).join(' · ');
}
</script>

<template>
  <section class="pane settings" aria-label="Notera">
    <div class="pane-header">
      <button type="button" class="btn btn--quiet btn--icon" :aria-label="t('mobile.back')" data-testid="settings-back" @click="shell.goto('workspace')">‹</button>
      <span class="pane-title">{{ t('settings.title') }}</span>
      <SyncBadge />
    </div>

    <div class="pane-body settings__body">
      <div class="settings__grid">
        <form class="card" @submit.prevent="save">
          <h2 class="card__title">{{ t('settings.account') }}</h2>

          <label class="field">
            <span>{{ t('settings.accountBaseUrl') }}</span>
            <input v-model="settings.draft.baseUrl" class="input" type="url" autocomplete="off" placeholder="https://dav.example.com/dav" data-testid="account-baseUrl" />
          </label>

          <div v-if="settings.usesPlainHttp" class="banner banner--danger" role="alert">
            <span>{{ t('sync.insecureWarn') }}</span>
          </div>

          <label class="field">
            <span>{{ t('settings.accountRootPrefix') }}</span>
            <input v-model="settings.draft.rootPrefix" class="input" type="text" spellcheck="false" data-testid="account-rootPrefix" />
          </label>

          <label class="field">
            <span>{{ t('settings.accountUser') }}</span>
            <input v-model="settings.draft.username" class="input" type="text" autocomplete="off" data-testid="account-username" />
          </label>

          <label class="field">
            <span>{{ t('settings.accountPassword') }}</span>
            <input
              v-model="settings.draft.password"
              class="input"
              type="password"
              autocomplete="new-password"
              :placeholder="settings.passwordIsSet ? t('settings.accountPasswordSet') : ''"
              data-testid="account-password"
            />
            <span v-if="settings.passwordIsSet" class="field-hint">{{ t('settings.accountPasswordSet') }}</span>
          </label>

          <label class="field">
            <span>{{ t('settings.tls') }}</span>
            <select v-model="settings.draft.tlsPolicy.kind" class="select" data-testid="account-tls">
              <option v-for="option in tlsOptions" :key="option.value" :value="option.value">{{ t(option.label) }}</option>
            </select>
            <span v-if="settings.draft.tlsPolicy.kind === 'insecureLocal'" class="field-hint field-hint--warn">{{ t('sync.insecureWarn') }}</span>
          </label>

          <label class="field">
            <span>{{ t('settings.proxyMode') }}</span>
            <select v-model="settings.draft.proxy.mode" class="select" data-testid="proxy-mode">
              <option v-for="option in proxyOptions" :key="option.value" :value="option.value">{{ t(option.label) }}</option>
            </select>
          </label>

          <template v-if="proxyNeedsHost">
            <label class="field">
              <span>{{ t('settings.proxyHost') }}</span>
              <input v-model="settings.draft.proxy.host" class="input" type="text" autocomplete="off" data-testid="proxy-host" />
            </label>
            <label class="field">
              <span>{{ t('settings.proxyPort') }}</span>
              <input
                class="input"
                type="number"
                min="1"
                max="65535"
                :value="settings.draft.proxy.port ?? ''"
                data-testid="proxy-port"
                @input="onPortInput"
              />
            </label>
            <label v-if="needsProxyAuth" class="field">
              <span>{{ t('settings.proxyUser') }}</span>
              <input v-model="settings.draft.proxy.username" class="input" type="text" autocomplete="off" />
            </label>
            <label v-if="needsProxyAuth" class="field">
              <span>{{ t('settings.proxyPassword') }}</span>
              <input v-model="settings.draft.proxy.password" class="input" type="password" autocomplete="new-password" />
            </label>
          </template>

          <label class="field">
            <span>{{ t('settings.proxyBypass') }}</span>
            <textarea v-model="bypassText" class="textarea" rows="3" spellcheck="false" />
          </label>

          <label class="row">
            <input id="account-enabled" v-model="settings.draft.enabled" type="checkbox" class="checkbox" />
            <span>{{ t('settings.enabled') }}</span>
          </label>

          <div class="row">
            <button type="submit" class="btn btn--primary" :disabled="settings.accountSaving" data-testid="account-save">{{ t('settings.save') }}</button>
            <span v-if="savedAt" class="field-hint">{{ t('settings.saved') }} · {{ formatWhen(new Date(savedAt).toISOString()) }}</span>
          </div>
          <p v-if="accountError" class="field-hint field-hint--warn" role="alert">{{ accountError }}</p>
        </form>

        <div class="card">
          <h2 class="card__title">{{ t('sync.detail') }}</h2>
          <p class="text-sm">
            <SyncBadge />
          </p>
          <p class="text-sm text-muted">
            {{ sync.state.finishedAt ? t('sync.lastRound', { when: formatWhen(new Date(sync.state.finishedAt).toISOString()) }) : t('sync.never') }}
          </p>
          <button type="button" class="btn" data-testid="sync-now" @click="syncNow()">{{ t('sync.syncNow') }}</button>

          <dl v-if="settings.stats" class="stats">
            <div>
              <dt>{{ t('settings.notesCount', { count: formatNumber(settings.stats.notes), folders: formatNumber(settings.stats.folders) }) }}</dt>
              <dd>{{ t('settings.trashCount', { count: formatNumber(settings.stats.notesInTrash) }) }}</dd>
              <dd>{{ t('settings.attachments', { count: formatNumber(settings.stats.attachments) }) }}</dd>
              <dd>{{ t('settings.storage', { size: formatBytes(settings.stats.dbBytes) }) }}</dd>
              <dd>{{ t('settings.inflight', { count: formatNumber(settings.stats.inflightOps) }) }}</dd>
            </div>
          </dl>
        </div>

        <div class="card">
          <h2 class="card__title">{{ t('settings.theme') }}</h2>
          <div class="row" role="radiogroup" :aria-label="t('settings.theme')">
            <button
              v-for="option in themeOptions"
              :key="option.value"
              type="button"
              class="btn"
              :aria-pressed="settings.prefs.theme === option.value ? 'true' : 'false'"
              :data-testid="`theme-${option.value}`"
              @click="settings.setTheme(option.value)"
            >
              {{ t(option.label) }}
            </button>
          </div>

          <label class="field">
            <span>{{ t('settings.fontScale') }} · {{ settings.prefs.fontScale.toFixed(2) }}</span>
            <input
              type="range"
              :min="FONT_SCALE_MIN"
              :max="FONT_SCALE_MAX"
              step="0.05"
              :value="settings.prefs.fontScale"
              data-testid="font-scale"
              @input="settings.setFontScale(Number(($event.target as HTMLInputElement).value))"
            />
          </label>

          <label v-if="settings.caps.transparency" class="row">
            <input v-model="settings.prefs.transparency" type="checkbox" class="checkbox" />
            <span>{{ t('settings.transparency') }}</span>
          </label>

          <label v-if="settings.caps.tray" class="row">
            <input v-model="settings.prefs.trayHint" type="checkbox" class="checkbox" data-testid="tray-toggle" />
            <span>{{ t('settings.trayHint') }}</span>
          </label>
          <p v-else class="field-hint">{{ t('settings.trayUnavailable') }}</p>

          <p class="field-hint">{{ t('settings.path') }}：{{ transportLabel }}</p>
        </div>

        <div class="card">
          <h2 class="card__title">{{ t('settings.data') }}</h2>
          <label class="field">
            <span>{{ t('settings.export') }} · path</span>
            <input v-model="dataPath" class="input" type="text" spellcheck="false" placeholder="留空由本地核心决定位置" />
          </label>
          <div class="row">
            <button type="button" class="btn" :disabled="settings.dataBusy" data-testid="export-data" @click="doExport">{{ t('settings.export') }}</button>
            <button type="button" class="btn" :disabled="settings.dataBusy" @click="doImport">{{ t('settings.import') }}</button>
            <button type="button" class="btn" :disabled="settings.dataBusy" @click="doBackup">{{ t('settings.backup') }}</button>
            <button type="button" class="btn btn--danger" :disabled="settings.dataBusy" @click="doRestore">{{ t('settings.restore') }}</button>
          </div>
          <label class="row">
            <span class="text-sm">{{ t('settings.importModeEmpty') }}</span>
            <select v-model="importMode" class="select">
              <option value="merge">{{ t('settings.importModeMerge') }}</option>
              <option value="intoEmpty">{{ t('settings.importModeEmpty') }}</option>
            </select>
          </label>
          <p v-if="report" class="field-hint">{{ t('settings.report', { text: report }) }}</p>
        </div>

        <div class="card">
          <h2 class="card__title">{{ t('settings.shortcuts') }}</h2>
          <table class="keys">
            <tbody>
              <tr v-for="entry in shortcuts" :key="entry.id">
                <td>{{ t(entry.labelKey, { level: 3 }) }}</td>
                <td class="keys__combo">{{ keysFor(entry) }}</td>
              </tr>
            </tbody>
          </table>
          <p class="field-hint">{{ keyHint() }}</p>
        </div>
      </div>
    </div>
  </section>
</template>

<style scoped>
.settings {
  flex: 1 1 auto;
  min-width: 0;
}

.settings__body {
  padding: var(--space-4);
}

.settings__grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(min(360px, 100%), 1fr));
  gap: var(--space-4);
  align-items: start;
}

.card {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
  padding: var(--space-4);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-3);
  background: var(--bg-raised);
  box-shadow: var(--shadow-1);
}

.card__title {
  font-size: var(--text-md);
  font-weight: 700;
  margin-bottom: var(--space-2);
}

.stats {
  margin: var(--space-2) 0 0;
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
  font-size: var(--text-sm);
  color: var(--text-secondary);
}

.stats dt {
  font-weight: 650;
  color: var(--text-primary);
}

.stats dd {
  margin: 0;
}

.keys {
  width: 100%;
  border-collapse: collapse;
  font-size: var(--text-sm);
}

.keys td {
  padding: var(--space-1) 0;
  border-bottom: 1px solid var(--border-subtle);
}

.keys__combo {
  text-align: right;
  font-family: var(--font-mono);
  color: var(--text-secondary);
}

.checkbox {
  width: 22px;
  height: 22px;
  accent-color: var(--accent);
}
</style>
