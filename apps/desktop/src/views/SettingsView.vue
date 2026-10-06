<script setup lang="ts">
/** 设置页：账户 / 代理 / 证书 / 外观 / 数据 / 快捷键（按能力显示）。 */
import { computed, onMounted, ref } from 'vue';
import SyncBadge from '../components/SyncBadge.vue';
import AppSelect from '../components/ui/AppSelect.vue';
import AppCheckbox from '../components/ui/AppCheckbox.vue';
import AppRange from '../components/ui/AppRange.vue';
import { currentTransport } from '../api/bridge';
import type { ProxyMode, TlsPolicyKind } from '../api/types';
import { useSettingsStore } from '../stores/settings';
import { useSyncStore } from '../stores/sync';
import { useNoteStore } from '../stores/notes';
import { useConflictStore } from '../stores/conflicts';
import { useEditorStore } from '../stores/editor';
import { useFolderStore } from '../stores/folders';
import { useShellStore } from '../stores/shell';
import { shortcutsFor, type PlatformCaps } from '../platform/caps';
import { t, messageFor } from '../i18n';
import { formatBytes, formatNumber, formatWhen } from '../util/format';
import { FONT_SCALE_MAX, FONT_SCALE_MIN, type ImportFilesReport, type ThemeMode } from '../stores/settings';
import { capChips, capsState } from '../sync/serverCaps';

const settings = useSettingsStore();
const sync = useSyncStore();
const notes = useNoteStore();
const folders = useFolderStore();
const shell = useShellStore();

const bypassText = ref('');
/** `pin` 档的指纹输入（一行一条）。核心不回传 PEM 本体，所以那一格另说（见 caPemIsSet 提示）。 */
const pinText = ref('');
const savedAt = ref<number | null>(null);
const dataPath = ref('');
const outPath = ref('');
const restoreHint = ref('');
const importMode = ref<'intoEmpty' | 'merge'>('merge');
// 「清除一切」的确认闸门：默认 false（确认区不展开），点过确认后立刻复位。
const eraseArmed = ref(false);
const eraseReport = ref('');
// 按文件夹导出：勾了才发 folderIds，不勾就是整库。默认整库 —— 备份的语义不该被误点改窄。
const exportScoped = ref(false);
const pickedFolders = ref<string[]>([]);

function toggleScoped(on: boolean): void {
  exportScoped.value = on;
  // 每次打开都重拉："非空"不等于"最新" —— 同步刚从另一台设备带过来的文件夹，
  // 或这次会话里新建的，都可能在旧快照里没有，于是选择器少一项、用户导不全。
  if (on) void folders.load();
}

function togglePicked(id: string): void {
  pickedFolders.value = pickedFolders.value.includes(id)
    ? pickedFolders.value.filter((picked) => picked !== id)
    : [...pickedFolders.value, id];
}

// §5 的判定：核心算好策略发下来，这里只负责把它讲成人话（不许前端自己反推）。
const capsStateOf = computed(() => capsState(settings.account));
const capChipsOf = computed(() => capChips(settings.account?.capMask));
const capsProbedWhen = computed(() => {
  const at = settings.account?.capsProbedAt;
  return at ? formatWhen(at) : '';
});

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

// 交给 AppSelect 的那三份：文案在这里翻成当前语言，控件本身不认识 i18n。
const tlsChoices = computed(() => tlsOptions.map((o) => ({ value: o.value, label: t(o.label) })));
const proxyChoices = computed(() => proxyOptions.map((o) => ({ value: o.value, label: t(o.label) })));
const importChoices = computed(() => [
  { value: 'merge', label: t('settings.importModeMerge') },
  { value: 'intoEmpty', label: t('settings.importModeEmpty') },
]);

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
  pinText.value = (settings.draft.tlsPolicy.fingerprints ?? []).join('\n');
  await settings.loadAccount();
  await settings.loadStats();
  bypassText.value = (settings.draft.proxy.bypass ?? []).join('\n');
  // 回填必须在 loadAccount 之后：那一步会用核心的 DTO 重建 draft，
  // 早先设进去的值会被换掉（这条 lane 的"改一次设置就得重填"就是这个坑）。
  pinText.value = (settings.draft.tlsPolicy.fingerprints ?? []).join('\n');
});

async function save(): Promise<void> {
  settings.draft.proxy.bypass = bypassText.value
    .split('\n')
    .map((line) => line.trim())
    .filter((line) => line.length > 0);
  // 指纹按"一行一条"收。全空 = 这一格没动 ⇒ 核心保留已存的那批（与口令同一套语义），
  // 所以清空只能先把策略切回 strict —— 这是有意的，不许"点一下就静默失去指纹保护"。
  settings.draft.tlsPolicy.fingerprints = pinText.value
    .split('\n')
    .map((line) => line.trim().replace(/[:\s-]+/g, ''))
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

/** 删账户的确认位：默认不展开，点过之后立刻复位（与「清除一切」同一形）。 */
const removeArmed = ref(false);

async function doRemoveAccount(): Promise<void> {
  const ok = await settings.removeAccount();
  removeArmed.value = false;
  if (ok) await settings.loadStats();
}

async function doExport(): Promise<void> {
  await settings.exportData({
    folderIds: exportScoped.value ? pickedFolders.value : [],
    includeAttachments: true,
    includeTrash: true,
    ...(outPath.value.trim() ? { path: outPath.value.trim() } : {}),
  });
  await settings.loadStats();
}

/** 导入散文件（Evernote 的 `.enex`、Markdown、纯文本）。与上面那个"整库还原"是两条路。 */
const filesHint = ref('');
const filesReport = ref<ImportFilesReport | null>(null);

async function doImportFiles(): Promise<void> {
  const path = dataPath.value.trim();
  if (!path) {
    filesHint.value = t('settings.importFilesNeedsPath');
    return;
  }
  filesHint.value = '';
  filesReport.value = await settings.importFiles([path]);
  await settings.loadStats();
  await notes.load();
}

async function doImport(): Promise<void> {  await settings.importData({
    mode: importMode.value,
    ...(dataPath.value.trim() ? { path: dataPath.value.trim() } : {}),
  });
  await settings.loadStats();
  await notes.load();
}

async function doBackup(): Promise<void> {
  const info = await settings.backupDb(outPath.value.trim() ? outPath.value.trim() : undefined);
  // 备份成功后把产物填进"输入路径"：下一步点恢复默认就是刚这一份，不必手抄长路径。
  // 绝不回填到"输出路径"——那会让下一次导出正好盖掉这份备份。
  if (info) dataPath.value = info.path;
}

async function doRestore(): Promise<void> {
  const path = dataPath.value.trim();
  if (!path) {
    restoreHint.value = t('settings.restoreNeedsPath');
    return;
  }
  restoreHint.value = '';
  await settings.restoreDb(path);
}

/**
 * 清除一切数据。
 *
 * 两道闸门在**界面**上：先点`eraseArmed` 才展开确认区，再点确认才真发命令。
 * 第三道在命令面：`erase_all_data` 要求 `confirmed: true`（不可撤销的命令不该只靠界面保证）。
 * 做完后**不自动刷新界面** —— 库里已经空了，刷新会让当前这些页面全部变成"空列表"，
 * 与其让用户看着界面突然清空，不如让他自己重启应用（回执里已说明要重启）。
 */
async function doErase(): Promise<void> {
  const out = await settings.eraseAllData();
  if (!out) return;
  eraseArmed.value = false;
  const kb = Math.max(1, Math.round(out.freedBytes / 1024));
  eraseReport.value = t('settings.eraseDoneDetail', { tables: out.tables, kb });

  // 库已经空了，但**界面还显示着清库之前的样子** —— 用户反馈"清除了，列表上还是 40 笔记 / 3 最近删除"。
  // 根因是侧栏那两颗数字读`settings.stats`，而它只在 App.vue 启动时取一次
  // （以及笔记增删时由 notes.refreshCounts 刷）；清库这条路径**不经过 notes store**，
  // 所以那份 stats 一直是"清库前"的快照。
  //
  // ⇒ 清完主动把三处数据源都重载：统计、笔记列表、冲突计数。
  // 不重载的话，用户会以为"清除没生效"，然后去重复点击 —— 那才是真正的危险。
  //
  // 还要**把编辑器关掉**：它开着的那一篇已经被永久删除了，
  // 而编辑器手里还留着它的 blocks（`editor.open(null)` 会清空）——
  // 否则用户看着一篇"已经不存在的笔记"，还挂着一句关于"最近删除"的假提示。
  // 先关编辑器再 load：反过来会让 load 把 selectedId 迁到新列表的第一行，
  // 编辑器却还开着上一篇。
  await useEditorStore().open(null);
  await Promise.all([
    settings.loadStats(),
    useNoteStore().load(),
    useConflictStore().load(),
  ]);
}

function keyHint(): string {
  return shortcuts.value.map((entry) => keysFor(entry)).join(' · ');
}
/** 分节跳转：id 与卡片一一对应，顺序 = 页面上的顺序。 */
const sections = [
  { id: 'sec-account', label: 'settings.account' },
  { id: 'sec-sync', label: 'sync.detail' },
  { id: 'sec-appearance', label: 'settings.theme' },
  { id: 'sec-data', label: 'settings.data' },
  { id: 'sec-danger', label: 'settings.dangerZone' },
  { id: 'sec-keys', label: 'settings.shortcuts' },
];

function jumpTo(id: string): void {
  const reduce = typeof window !== 'undefined' && window.matchMedia?.('(prefers-reduced-motion: reduce)').matches === true;
  document.getElementById(id)?.scrollIntoView({ behavior: reduce ? 'auto' : 'smooth', block: 'start' });
}
</script>

<template>
  <section class="pane settings" :aria-label="t('settings.title')">
    <div class="pane-header">
      <button type="button" class="btn btn--quiet btn--icon" :aria-label="t('mobile.back')" data-testid="settings-back" @click="shell.goto('workspace')">‹</button>
      <span class="pane-title">{{ t('settings.title') }}</span>
      <SyncBadge />
    </div>

    <div class="pane-body settings__body">
      <!-- 单列内容 + 左侧分节跳转（Obsidian / Notion 的设置页都是这一形）：
           一栏到底解决了"2 列 3 列对不齐"，但也把页面拉长了，
           所以给一条粘性的分节导航，而不是让人一路滚到底。 -->
      <nav class="settings__rail" :aria-label="t('settings.title')">
        <button
          v-for="sec in sections"
          :key="sec.id"
          type="button"
          class="settings__rail-item"
          data-testid="settings-rail"
          @click="jumpTo(sec.id)"
        >
          {{ t(sec.label) }}
        </button>
      </nav>
      <div class="settings__grid">
        <form id="sec-account" class="card" @submit.prevent="save">
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
            <!-- 缺口 G38 选了 B：没有系统凭据库的平台上口令只留在这次进程的内存里。
                 这句话必须在敲口令**之前**说出来（`caps.keychain`），保存之后也要跟着说
                 （`credentialLive` / `credentialPersistent` 那两位）—— 三种状态各有各的话，
                 合成一句就必然在其中一种状态下说谎。 -->
            <p v-if="settings.credentialStoreUnavailable" class="field-hint field-hint--warn" data-testid="credential-store-none">
              {{ t('settings.credentialStoreNone') }}
            </p>
            <p v-if="settings.credentialVolatile" class="field-hint field-hint--warn" data-testid="credential-volatile">
              {{ t('settings.credentialVolatile') }}
            </p>
            <p v-if="settings.credentialSavedButGone" class="field-hint field-hint--warn" data-testid="credential-gone">
              {{ t('settings.credentialGone') }}
            </p>
          </label>

          <label class="field">
            <span>{{ t('settings.tls') }}</span>
            <AppSelect
              :model-value="settings.draft.tlsPolicy.kind ?? 'strict'"
              :options="tlsChoices"
              :label="t('settings.tls')"
              testid="account-tls"
              @update:model-value="settings.draft.tlsPolicy.kind = $event as typeof settings.draft.tlsPolicy.kind"
            />
            <span v-if="settings.draft.tlsPolicy.kind === 'insecureLocal'" class="field-hint field-hint--warn">{{ t('sync.insecureWarn') }}</span>
          </label>

          <!-- 选了 `ca_bundle` / `pin` 才出来的两格。此前这两档在下拉里能选到，却没有任何输入口
               —— 于是"内网自签主路径"（PROXY.md §6）在界面上根本走不到，选它只会在保存时被
               核心的配置校验顶回来。留空 = 不改已存的那份，与口令同一套语义，所以必须有 caPemIsSet
               那位提示，否则重开表单看着像被吞了。 -->
          <label v-if="settings.draft.tlsPolicy.kind === 'caBundle'" class="field">
            <span>{{ t('settings.tlsCaPem') }}</span>
            <textarea
              v-model="settings.draft.tlsPolicy.caBundlePem"
              class="textarea"
              rows="4"
              spellcheck="false"
              autocomplete="off"
              :placeholder="settings.caPemIsSet ? t('settings.tlsCaPemSet') : t('settings.tlsCaPemPlaceholder')"
              data-testid="account-ca-pem"
            />
            <span class="field-hint">{{ t(settings.caPemIsSet ? 'settings.tlsCaPemHintKept' : 'settings.tlsCaPemHint') }}</span>
          </label>

          <label v-if="settings.draft.tlsPolicy.kind === 'pin'" class="field">
            <span>{{ t('settings.tlsPinFingerprints') }}</span>
            <textarea
              v-model="pinText"
              class="textarea"
              rows="3"
              spellcheck="false"
              autocomplete="off"
              :placeholder="t('settings.tlsPinPlaceholder')"
              data-testid="account-pin"
            />
            <span class="field-hint">{{ t('settings.tlsPinHint') }}</span>
          </label>

          <label class="field">
            <span>{{ t('settings.proxyMode') }}</span>
            <AppSelect
              :model-value="settings.draft.proxy.mode ?? 'direct'"
              :options="proxyChoices"
              :label="t('settings.proxyMode')"
              testid="proxy-mode"
              @update:model-value="settings.draft.proxy.mode = $event as typeof settings.draft.proxy.mode"
            />
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
              <input
                v-model="settings.draft.proxy.username"
                class="input"
                type="text"
                autocomplete="off"
                :placeholder="settings.account?.proxyHasUsername ? t('settings.proxyUserSet') : ''"
                data-testid="proxy-user"
              />
            </label>
            <label v-if="needsProxyAuth" class="field">
              <span>{{ t('settings.proxyPassword') }}</span>
              <input
                v-model="settings.draft.proxy.password"
                class="input"
                type="password"
                autocomplete="new-password"
                data-testid="proxy-pass"
              />
            </label>
          </template>

          <label class="field">
            <span>{{ t('settings.proxyBypass') }}</span>
            <textarea v-model="bypassText" class="textarea" rows="3" spellcheck="false" />
          </label>

          <div class="row">
            <AppCheckbox v-model="settings.draft.enabled" :label="t('settings.enabled')" testid="account-enabled" />
          </div>

          <div class="row">
            <button type="submit" class="btn btn--primary" :disabled="settings.accountSaving" data-testid="account-save">{{ t('settings.save') }}</button>
            <span v-if="savedAt" class="field-hint">{{ t('settings.saved') }} · {{ formatWhen(new Date(savedAt).toISOString()) }}</span>
          </div>
          <p v-if="accountError" class="field-hint field-hint--warn" role="alert">{{ accountError }}</p>
        </form>

        <div id="sec-sync" class="card">
          <h2 class="card__title">{{ t('sync.detail') }}</h2>
          <p class="text-sm">
            <SyncBadge />
          </p>
          <p class="text-sm text-muted">
            {{ sync.state.finishedAt ? t('sync.lastRound', { when: formatWhen(new Date(sync.state.finishedAt).toISOString()) }) : t('sync.never') }}
          </p>
          <button type="button" class="btn" data-testid="sync-now" @click="syncNow()">{{ t('sync.syncNow') }}</button>

          <!-- 删账户与删库是两件事：核心一直有 `remove_account`，界面上却没有入口，
               于是"不想再同步了"只能连笔记一起清掉。两道确认与「清除一切」同一形。 -->
          <template v-if="settings.hasAccount">
            <button v-if="!removeArmed" type="button" class="btn btn--quiet" data-testid="remove-account" @click="removeArmed = true">
              {{ t('settings.removeAccount') }}
            </button>
            <div v-else class="row" role="group" :aria-label="t('settings.removeAccount')">
              <span class="text-sm">{{ t('settings.removeAccountConfirm') }}</span>
              <button type="button" class="btn btn--primary" data-testid="remove-account-yes" @click="doRemoveAccount">
                {{ t('settings.removeAccountYes') }}
              </button>
              <button type="button" class="btn btn--quiet" @click="removeArmed = false">{{ t('list.cancel') }}</button>
            </div>
            <p class="field-hint">{{ t('settings.removeAccountHint') }}</p>
          </template>

          <div v-if="settings.hasAccount" class="caps" data-testid="server-caps">
            <h3 class="caps__title">{{ t('settings.serverCaps') }}</h3>
            <p
              v-if="capsStateOf === 'unprotected'"
              class="caps__note caps__note--warn"
              data-testid="server-caps-verdict"
            >
              {{ t('sync.capsUnprotected') }}
            </p>
            <p v-else-if="capsStateOf === 'protected'" class="caps__note" data-testid="server-caps-verdict">
              {{ t('sync.capsProtected', { strategy: settings.account?.writeStrategy ?? '' }) }}
            </p>
            <p v-else class="caps__note caps__note--muted" data-testid="server-caps-verdict">
              {{ t('sync.capsUnknown') }}
            </p>
            <ul v-if="capChipsOf.length" class="caps__chips" aria-label="capabilities">
              <li
                v-for="chip in capChipsOf"
                :key="chip.name"
                class="caps__chip"
                :class="chip.on ? 'caps__chip--on' : 'caps__chip--off'"
                :data-testid="`cap-${chip.name}`"
                :aria-label="chip.labelKey"
              >
                <span aria-hidden="true">{{ chip.on ? '✓' : '✕' }}</span>
                {{ t(chip.labelKey) }}
              </li>
            </ul>
            <p v-if="capsProbedWhen" class="caps__when">{{ t('sync.capsProbedAt', { when: capsProbedWhen }) }}</p>
          </div>

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

        <div id="sec-appearance" class="card">
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

          <div class="field">
            <span>{{ t('settings.fontScale') }} · {{ settings.prefs.fontScale.toFixed(2) }}</span>
            <AppRange
              :model-value="settings.prefs.fontScale"
              :min="FONT_SCALE_MIN"
              :max="FONT_SCALE_MAX"
              :step="0.05"
              :label="t('settings.fontScale')"
              testid="font-scale"
              @update:model-value="settings.setFontScale(Number($event))"
            />
          </div>

          <div v-if="settings.caps.transparency" class="row" data-testid="pref-transparency">
            <AppCheckbox v-model="settings.prefs.transparency" :label="t('settings.transparency')" testid="transparency-toggle" />
          </div>

          <div v-if="settings.caps.tray" class="row">
            <AppCheckbox v-model="settings.prefs.trayHint" :label="t('settings.trayHint')" testid="tray-toggle" />
          </div>
          <p v-else class="field-hint">{{ t('settings.trayUnavailable') }}</p>

          <p class="field-hint">{{ t('settings.path') }}：{{ transportLabel }}</p>
        </div>

        <div id="sec-data" class="card">
          <h2 class="card__title">{{ t('settings.data') }}</h2>
          <label class="field">
            <span>{{ t('settings.exportPathLabel') }}</span>
            <input v-model="outPath" class="input" type="text" spellcheck="false" data-testid="export-path" :placeholder="t('settings.exportPathHint')" />
          </label>
          <div class="field">
            <AppCheckbox
              :model-value="exportScoped"
              :label="t('settings.exportScoped')"
              testid="export-scoped"
              @update:model-value="toggleScoped($event)"
            />
            <div v-if="exportScoped" class="folder-pick" data-testid="export-folder-list">
              <div v-for="f in folders.flat" :key="f.node.id" class="pick" :style="{ paddingLeft: `${0.5 + f.depth * 0.75}rem` }">
                <AppCheckbox
                  :model-value="pickedFolders.includes(f.node.id)"
                  :label="f.node.name"
                  :testid="`export-folder-${f.node.id}`"
                  @update:model-value="togglePicked(f.node.id)"
                />
              </div>
              <p class="field-hint">{{ t('settings.exportScopedHint') }}</p>
            </div>
          </div>
          <label class="field">
            <span>{{ t('settings.inputPathLabel') }}</span>
            <input v-model="dataPath" class="input" type="text" spellcheck="false" data-testid="data-path" :placeholder="t('settings.inputPathHint')" />
          </label>
          <div class="row">
            <button type="button" class="btn" :disabled="settings.dataBusy" data-testid="export-data" @click="doExport">{{ t('settings.export') }}</button>
            <button type="button" class="btn" :disabled="settings.dataBusy" data-testid="import-data" @click="doImport">{{ t('settings.import') }}</button>
            <button type="button" class="btn btn--quiet" :disabled="settings.dataBusy" data-testid="import-files" @click="doImportFiles">{{ t('settings.importFiles') }}</button>
            <p v-if="filesHint" class="field-hint" data-testid="import-files-hint">{{ filesHint }}</p>
            <p v-else class="field-hint">{{ t('settings.importFilesHint') }}</p>
            <div v-if="filesReport" class="field-hint" data-testid="import-files-report">
              <p>{{ t('settings.importFilesResult') }}：{{ filesReport.created.length }} / {{ filesReport.duplicates }} / {{ filesReport.failed.length }}</p>
              <!-- 没坏但用户该知道的事：Evernote 里没落地的字段、按字面保留的表格结构……
                   §39 不许静默降级，所以这些一路从导入器带到这儿显示，而不是只进日志。 -->
              <ul v-if="filesReport.notices.length" data-testid="import-files-notices">
                <li v-for="(n, i) in filesReport.notices" :key="i">{{ n }}</li>
              </ul>
            </div>
            <button type="button" class="btn" :disabled="settings.dataBusy" data-testid="backup-db" @click="doBackup">{{ t('settings.backup') }}</button>
            <button type="button" class="btn btn--danger" :disabled="settings.dataBusy" data-testid="restore-db" @click="doRestore">{{ t('settings.restore') }}</button>
          </div>
          <label class="row">
            <span class="text-sm">{{ t('settings.importModeEmpty') }}</span>
            <AppSelect
              :model-value="importMode"
              :options="importChoices"
              :label="t('settings.importModeEmpty')"
              testid="import-mode"
              @update:model-value="importMode = $event as 'merge' | 'intoEmpty'"
            />
          </label>
          <p v-if="report" class="field-hint" data-testid="data-report">{{ t('settings.report', { text: report }) }}</p>
          <p v-if="restoreHint" class="field-hint" data-testid="restore-hint">{{ restoreHint }}</p>
          <p class="field-hint">{{ t('settings.restoreNeedsRestart') }}</p>
        </div>

        <!-- 「清除一切数据」独立成块，不与导出/恢复挤在一起：
             那是不可撤销的操作，混在日常按钮行里迟早会被误触。
             两道闸门：① 先要点这颗按钮才展开确认区；② 确认区里还要再点一次。

             块级类用 `.card`（设置页其余 5 个块都是它）——原先我写的是
             `.section`，而那个类**在样式表里根本没有定义**，于是这一块的
             内边距/间距/背景全走浏览器默认，与上下几块对不齐（用户反馈"没对齐"）。 -->
        <div id="sec-danger" class="card card--danger">
          <h2 class="card__title">{{ t('settings.dangerZone') }}</h2>
          <p class="field-hint">{{ t('settings.eraseHint') }}</p>
          <button
            v-if="!eraseArmed"
            type="button"
            class="btn btn--danger"
            :disabled="settings.dataBusy"
            data-testid="erase-arm"
            @click="eraseArmed = true"
          >
            {{ t('settings.erase') }}
          </button>
          <div v-else class="erase-confirm" data-testid="erase-confirm">
            <p class="erase-confirm__text">{{ t('settings.eraseConfirm') }}</p>
            <div class="erase-confirm__row">
              <button
                type="button"
                class="btn"
                :disabled="settings.dataBusy"
                data-testid="erase-cancel"
                @click="eraseArmed = false"
              >
                {{ t('settings.eraseCancel') }}
              </button>
              <button
                type="button"
                class="btn btn--danger"
                :disabled="settings.dataBusy"
                data-testid="erase-confirm-btn"
                @click="doErase"
              >
                {{ t('settings.eraseConfirmBtn') }}
              </button>
            </div>
          </div>
          <p v-if="eraseReport" class="field-hint" data-testid="erase-report">{{ eraseReport }}</p>
        </div>

        <div id="sec-keys" class="card">
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

/* 导出范围的勾选行：行高按触摸目标下限（44pt）给，鼠标用户也不会觉得挤。 */
.pick {
  display: flex;
  min-height: 44px;
  align-items: center;
  gap: 0.5rem;
}

.folder-pick {
  margin-top: 0.25rem;
  border-left: 2px solid var(--line);
  padding-left: 0.25rem;
}

.settings__body {
  padding: var(--sp-4);
  display: flex;
  align-items: flex-start;
  gap: var(--sp-6);
}

/* 分节导航：只在放得下的宽度出现（窄屏时那一栏会把正文挤窄，
   而跳转在窄屏本来就不需要 —— 一屏就能滚到底）。 */
.settings__rail {
  position: sticky;
  top: 0;
  display: none;
  flex: 0 0 168px;
  width: 168px;
  flex-direction: column;
  gap: var(--sp-1);
  padding-block: var(--sp-2);
}

.settings__rail-item {
  min-height: var(--touch);
  padding: 0 var(--sp-3);
  border: 0;
  border-radius: var(--r-row, 6px);
  background: none;
  color: var(--body);
  font-size: var(--text-sm);
  text-align: left;
}

.settings__rail-item:hover {
  background: var(--hover);
  color: var(--ink);
}

@media (min-width: 1180px) {
  .settings__rail {
    display: flex;
  }
}

/**
 * 一栏到底，宽度封顶 —— 这一格以前是「2 列 3 列长度对不齐」那一句话的正身。
 *
 * 旧写法 `repeat(auto-fit, minmax(min(360px,100%),1fr))` + `align-items:start`
 * 的实测后果（`.logs/measure-ui.mjs`，量的是渲染后的几何）：
 *   宽 900 / 1100 → 2 列；1440 → 3 列；1800 → **4 列**，
 *   而同批卡片的实际高度是 892 / 331 / 253 / 649 / 172 / 532 ——
 *   底边落在 977 / 416 / 338 / 1642 / 1164 / 1525 六个不同的地方。
 *   ⇒ 视口每宽一点就多塞一列、每列底部都是台阶，短卡片下面是一整片空白。
 *
 * 知名笔记软件的设置页都不是这种"瀑布"：Obsidian / Notion 是"分类 + 单列内容"，
 * Apple Notes / UpNote 的偏好窗口是**一栏分组**。单列还有一个附带的好处 ——
 * 正文行宽封顶（这里 760px ≈ 表单字号下的可读上限），字段不会在 1800px 下被拉成
 * 一条扫不到尾的横线。
 */
.settings__grid {
  display: grid;
  grid-template-columns: minmax(0, 1fr);
  gap: var(--sp-4);
  width: min(760px, 100%);
  margin-inline: auto;
}

.card {
  display: flex;
  flex-direction: column;
  gap: var(--sp-2);
  padding: var(--sp-4);
  border: 1px solid var(--line);
  border-radius: var(--r-card);
  background: var(--canvas);
  box-shadow: var(--shadow-1);
}

/* 危险块：与其它块**同样的间距与圆角**（对齐的第一要求），
   只在边框与底色上做出区分 —— 不改变任何盒模型尺寸，
   否则"加了个危险区"就变成了"下面几块全被推歪了"。 */
.card--danger {
  border-color: var(--danger);
  background: var(--danger-soft);
}

.card__title {
  font-size: var(--text-md);
  font-weight: 700;
  margin-bottom: var(--sp-2);
}

.caps {
  display: flex;
  flex-direction: column;
  gap: var(--sp-2);
  padding: var(--sp-3);
  border: 1px solid var(--line);
  border-radius: var(--r-card);
  background: var(--sunken);
}

.caps__title {
  font-size: var(--text-sm);
  font-weight: 600;
  color: var(--body);
}

.caps__note {
  font-size: var(--text-sm);
  line-height: var(--leading-body);
  color: var(--ink);
}

/* S3 是本页少数"必须显眼"的提示：它讲的是覆盖风险，不是性能。 */
.caps__note--warn {
  color: var(--warn);
  font-weight: 600;
}

.caps__note--muted {
  color: var(--mute);
}

.caps__chips {
  display: flex;
  flex-wrap: wrap;
  gap: var(--sp-2);
  list-style: none;
  margin: 0;
  padding: 0;
}

.caps__chip {
  display: inline-flex;
  align-items: center;
  gap: var(--sp-1);
  padding: 2px var(--sp-2);
  border: 1px solid var(--line);
  border-radius: var(--r-chip);
  font-size: var(--text-xs);
}

/* on/off 只用已经在 tokens.spec.ts 里量过对比度的语义色，对新底 --sunken 两者均 ≥4.5:1。 */
.caps__chip--on {
  color: var(--ok);
  border-color: var(--ok);
}

.caps__chip--off {
  color: var(--mute);
}

.caps__when {
  font-size: var(--text-xs);
  color: var(--mute);
}

.stats {
  margin: var(--sp-2) 0 0;
  display: flex;
  flex-direction: column;
  gap: var(--sp-1);
  font-size: var(--text-sm);
  color: var(--body);
}

.stats dt {
  font-weight: 650;
  color: var(--ink);
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
  padding: var(--sp-1) 0;
  border-bottom: 1px solid var(--line);
}

.keys__combo {
  text-align: right;
  font-family: var(--font-mono);
  color: var(--body);
}

.erase-confirm {
  margin-top: var(--sp-2);
  padding: var(--sp-3);
  border: 1px solid var(--danger);
  border-radius: 8px;
  background: var(--canvas);
}

.erase-confirm__text {
  margin: 0 0 var(--sp-2);
  font-size: var(--text-sm);
  color: var(--ink);
}

.erase-confirm__row {
  display: flex;
  gap: var(--sp-2);
  justify-content: flex-end;
}

</style>
