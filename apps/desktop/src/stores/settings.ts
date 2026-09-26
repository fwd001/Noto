/** 设置：外观（design token 驱动）、账户（口令只写不读）、本地统计与数据进出。 */
import { defineStore } from 'pinia';
import { computed, ref, watch } from 'vue';
import { callCommand } from '../api/bridge';
import {
  Commands,
  type Account,
  type AccountDraft,
  type BackupInfo,
  type ExportRequest,
  type ImportRequest,
  type ProxyMode,
  type Report,
  type RestoreOutcome,
  type StoreStats,
  type TlsPolicyKind,
} from '../api/types';
import { asBridgeError } from '../util/errors';
import { localCaps, normalizeCaps, type PlatformCaps } from '../platform/caps';
import { useToastStore } from './toasts';

export type ThemeMode = 'system' | 'light' | 'dark';

const STORAGE_KEY = 'notera.ui.v1';
export const FONT_SCALE_MIN = 0.85;
export const FONT_SCALE_MAX = 1.6;

export interface UiPrefs {
  theme: ThemeMode;
  fontScale: number;
  transparency: boolean;
  trayHint: boolean;
}

const DEFAULT_PREFS: UiPrefs = { theme: 'system', fontScale: 1, transparency: true, trayHint: false };

function clampScale(value: number): number {
  if (!Number.isFinite(value)) return 1;
  return Math.min(FONT_SCALE_MAX, Math.max(FONT_SCALE_MIN, Math.round(value * 100) / 100));
}

function readPrefs(): UiPrefs {
  try {
    const raw = typeof localStorage !== 'undefined' ? localStorage.getItem(STORAGE_KEY) : null;
    if (!raw) return { ...DEFAULT_PREFS };
    const parsed = JSON.parse(raw) as Partial<UiPrefs>;
    return {
      theme: parsed.theme === 'light' || parsed.theme === 'dark' ? parsed.theme : 'system',
      fontScale: clampScale(typeof parsed.fontScale === 'number' ? parsed.fontScale : 1),
      transparency: parsed.transparency !== false,
      trayHint: parsed.trayHint === true,
    };
  } catch {
    return { ...DEFAULT_PREFS };
  }
}

function writePrefs(prefs: UiPrefs): void {
  try {
    if (typeof localStorage === 'undefined') return;
    localStorage.setItem(STORAGE_KEY, JSON.stringify(prefs));
  } catch {
    // 存储不可用（隐私模式等）时只影响下次启动的默认值，不影响功能
  }
}

export function emptyDraft(): AccountDraft {
  return {
    baseUrl: '',
    rootPrefix: '.notes',
    username: '',
    password: '',
    tlsPolicy: { kind: 'strict' },
    proxy: { mode: 'direct', bypass: [] },
    enabled: true,
  };
}

export function draftFromAccount(account: Account | null | undefined): AccountDraft {
  const draft = emptyDraft();
  if (!account) return draft;
  draft.id = account.id ?? null;
  draft.baseUrl = account.baseUrl ?? '';
  draft.rootPrefix = account.rootPrefix ?? '.notes';
  draft.username = account.username ?? '';
  draft.enabled = account.enabled !== false;
  draft.tlsPolicy = { kind: (account.tlsPolicy?.kind as TlsPolicyKind) ?? 'strict', fingerprints: account.tlsPolicy?.fingerprints ?? [] };
  draft.proxy = {
    mode: (account.proxy?.mode as ProxyMode) ?? 'direct',
    host: account.proxy?.host ?? '',
    port: typeof account.proxy?.port === 'number' ? account.proxy.port : undefined,
    username: account.proxy?.username ?? '',
    bypass: account.proxy?.bypass ?? [],
    resolveSystem: account.proxy?.resolveSystem ?? true,
  };
  return draft;
}

export const useSettingsStore = defineStore('settings', () => {
  const toasts = useToastStore();
  const prefs = ref<UiPrefs>(readPrefs());
  const systemDark = ref(false);
  const caps = ref<PlatformCaps>(localCaps());
  const account = ref<Account | null>(null);
  const draft = ref<AccountDraft>(emptyDraft());
  const accountLoading = ref(false);
  const accountSaving = ref(false);
  const accountErrorKey = ref<string | null>(null);
  const stats = ref<StoreStats | null>(null);
  const statsLoading = ref(false);
  const dataBusy = ref(false);
  const lastReport = ref<Report | null>(null);

  const resolvedTheme = computed<ThemeMode>(() => (prefs.value.theme === 'system' ? (systemDark.value ? 'dark' : 'light') : prefs.value.theme));
  const hasAccount = computed(() => account.value !== null && Boolean(account.value?.baseUrl));
  const passwordIsSet = computed(() => account.value?.hasCredential === true);
  const fontScale = computed(() => prefs.value.fontScale);
  const usesPlainHttp = computed(() => /^http:\/\//i.test(draft.value.baseUrl ?? ''));

  watch(
    prefs,
    (next) => {
      writePrefs(next);
      applyThemeToDocument();
    },
    { deep: true },
  );

  function applyThemeToDocument(): void {
    if (typeof document === 'undefined') return;
    const root = document.documentElement;
    root.dataset.theme = resolvedTheme.value;
    root.style.setProperty('--editor-font-scale', String(prefs.value.fontScale));
    root.classList.toggle('theme-dark', resolvedTheme.value === 'dark');
    root.classList.toggle('theme-light', resolvedTheme.value === 'light');
    root.classList.toggle('no-transparency', prefs.value.transparency !== true);
  }

  function trackSystemTheme(): () => void {
    if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') return () => undefined;
    const media = window.matchMedia('(prefers-color-scheme: dark)');
    const sync = () => {
      systemDark.value = media.matches === true;
    };
    sync();
    const onChange = (event: MediaQueryListEvent) => {
      systemDark.value = event.matches === true;
    };
    if (typeof media.addEventListener === 'function') {
      media.addEventListener('change', onChange);
      return () => media.removeEventListener('change', onChange);
    }
    return () => undefined;
  }

  function setTheme(mode: ThemeMode): void {
    prefs.value = { ...prefs.value, theme: mode };
  }

  function setFontScale(value: number): void {
    prefs.value = { ...prefs.value, fontScale: clampScale(value) };
  }

  function setCaps(next: unknown): void {
    caps.value = normalizeCaps(next);
  }

  async function loadAccount(): Promise<void> {
    accountLoading.value = true;
    accountErrorKey.value = null;
    try {
      const result = await callCommand<Account | null>(Commands.account, {});
      account.value = typeof result === 'object' && result !== null ? result : null;
      draft.value = draftFromAccount(account.value);
    } catch (error) {
      const bridge = asBridgeError(error);
      accountErrorKey.value = bridge.messageKey;
      if (bridge.code !== 'not_found') account.value = null;
    } finally {
      accountLoading.value = false;
    }
  }

  async function saveAccount(): Promise<boolean> {
    accountSaving.value = true;
    accountErrorKey.value = null;
    const payload: AccountDraft = { ...draft.value };
    // 口令只在用户本次输入了内容时才提交；提交后立即从内存草稿清掉。
    if (!payload.password || payload.password.length === 0) delete payload.password;
    try {
      const saved = await callCommand<Account>(Commands.configureAccount, { draft: payload });
      account.value = typeof saved === 'object' && saved !== null ? saved : account.value;
      draft.value = { ...draftFromAccount(account.value), password: '' };
      toasts.push('settings.saved', 'info');
      return true;
    } catch (error) {
      const bridge = asBridgeError(error);
      accountErrorKey.value = bridge.messageKey;
      toasts.push(bridge.messageKey, 'error');
      return false;
    } finally {
      accountSaving.value = false;
    }
  }

  function forgetPassword(): void {
    draft.value = { ...draft.value, password: '' };
  }

  async function loadStats(): Promise<void> {
    statsLoading.value = true;
    try {
      const result = await callCommand<StoreStats>(Commands.stats, {});
      stats.value = typeof result === 'object' && result !== null ? result : null;
    } catch {
      stats.value = null;
    } finally {
      statsLoading.value = false;
    }
  }

  async function exportData(request: ExportRequest): Promise<Report | null> {
    dataBusy.value = true;
    lastReport.value = null;
    try {
      const report = await callCommand<Report>(Commands.exportData, { req: request });
      lastReport.value = report ?? null;
      return report ?? null;
    } catch (error) {
      const bridge = asBridgeError(error);
      accountErrorKey.value = bridge.messageKey;
      toasts.push(bridge.messageKey, 'error');
      return null;
    } finally {
      dataBusy.value = false;
    }
  }

  async function importData(request: ImportRequest): Promise<Report | null> {
    dataBusy.value = true;
    lastReport.value = null;
    try {
      const report = await callCommand<Report>(Commands.importData, { req: request });
      lastReport.value = report ?? null;
      return report ?? null;
    } catch (error) {
      const bridge = asBridgeError(error);
      accountErrorKey.value = bridge.messageKey;
      toasts.push(bridge.messageKey, 'error');
      return null;
    } finally {
      dataBusy.value = false;
    }
  }

  /** 备份：产出一致快照并回报自证信息。失败要能看清是哪一步。 */
  async function backupDb(path?: string): Promise<BackupInfo | null> {
    dataBusy.value = true;
    lastReport.value = null;
    try {
      const info = await callCommand<BackupInfo>(Commands.backupDb, path ? { path } : {});
      if (info) lastReport.value = { path: info.path, sha256: info.sha256.slice(0, 12) };
      return info ?? null;
    } catch (error) {
      const bridge = asBridgeError(error);
      toasts.push(bridge.messageKey, 'error');
      return null;
    } finally {
      dataBusy.value = false;
    }
  }

  /** 恢复只排期：真正落地在下次启动，所以这里必须明说"要重启"。 */
  async function restoreDb(path: string): Promise<RestoreOutcome | null> {
    dataBusy.value = true;
    lastReport.value = null;
    try {
      const out = await callCommand<RestoreOutcome>(Commands.restoreDb, { path });
      if (out) {
        lastReport.value = { path: out.path, sha256: out.sha256.slice(0, 12) };
        toasts.push('settings.restoreStaged', 'info');
      }
      return out ?? null;
    } catch (error) {
      const bridge = asBridgeError(error);
      toasts.push(bridge.messageKey, 'error');
      return null;
    } finally {
      dataBusy.value = false;
    }
  }

  function describeReport(report: Report | null): string {
    if (!report) return '';
    const parts: string[] = [];
    if (typeof report.created === 'number') parts.push(`新增 ${report.created}`);
    if (typeof report.merged === 'number') parts.push(`并入 ${report.merged}`);
    if (typeof report.skipped === 'number') parts.push(`跳过 ${report.skipped}`);
    if (typeof report.conflicts === 'number' && report.conflicts > 0) parts.push(`需要处理 ${report.conflicts}`);
    if (typeof report.restoredAttachments === 'number') parts.push(`附件 ${report.restoredAttachments}`);
    if (report.path) parts.push(report.path);
    if (parts.length === 0 && report.counts) {
      for (const [key, value] of Object.entries(report.counts)) parts.push(`${key} ${value}`);
    }
    return parts.join(' · ');
  }

  return {
    prefs,
    caps,
    resolvedTheme,
    systemDark,
    account,
    draft,
    accountLoading,
    accountSaving,
    accountErrorKey,
    hasAccount,
    passwordIsSet,
    usesPlainHttp,
    fontScale,
    stats,
    statsLoading,
    dataBusy,
    lastReport,
    applyThemeToDocument,
    trackSystemTheme,
    setTheme,
    setFontScale,
    setCaps,
    loadAccount,
    saveAccount,
    forgetPassword,
    loadStats,
    exportData,
    importData,
    backupDb,
    restoreDb,
    describeReport,
  };
});
