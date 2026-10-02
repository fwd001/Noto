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
  type Report,
  type RestoreOutcome,
  type StoreStats,
} from '../api/types';
import { draftFromWire, toWire } from '../sync/accountWire';
import { asBridgeError } from '../util/errors';
import { localCaps, normalizeCaps, type PlatformCaps } from '../platform/caps';
import { useToastStore } from './toasts';

export type ThemeMode = 'system' | 'light' | 'dark';

/** `import_files` 的回报。`notices` 是"没坏但用户该知道"的那部分，必须显示出来。 */
export interface ImportFilesReport {
  folderName: string;
  created: { label: string; title: string; id: string }[];
  duplicates: number;
  failed: { label: string; why: string }[];
  notices: string[];
}

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

/**
 * 表单的初始空值。回填/发送的翻译规则全在 `sync/accountWire.ts` —— 那里记着这条边
 * 曾经错在哪（嵌套 vs 平铺），别在这里再写第二份。
 */
export function emptyDraft(): AccountDraft {
  return {
    label: '',
    baseUrl: '',
    rootPrefix: '/.notes',
    username: '',
    password: '',
    tlsPolicy: { kind: 'strict' },
    proxy: { mode: 'direct', bypass: [] },
    enabled: true,
  };
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
  /**
   * 口令格上那句"已保存口令（留空则不修改）"的依据。
   *
   * 用的是 `credentialLive` 而**不是** `hasCredential`：后者只说明配置里挂着一条引用，
   * 而缺口 G38 选 B 之后，没有系统凭据库的平台上口令只活在这次进程里 —— 重启后引用还在、
   * 东西已经没了。照 `hasCredential` 显示就等于当着用户说"你的口令还在"，
   * 而他下一次同步必然失败（`hasCredential` 的原始语义正是本项目踩过的那类谎报）。
   */
  const passwordIsSet = computed(() => account.value?.credentialLive === true);
  /** 配置里挂着引用、这一轮却拿不到了 = 重启过 / 换机器 —— 界面要说的是"请重填"，不是"已保存"。 */
  const credentialSavedButGone = computed(
    () => account.value?.hasCredential === true && account.value?.credentialLive === false,
  );
  /** 现在拿得到，但只在这次运行里有效（这台设备没有系统凭据库）：退出后要重填。 */
  const credentialVolatile = computed(
    () => account.value?.credentialLive === true && account.value?.credentialPersistent === false,
  );
  /**
   * 这台设备到底有没有**系统**凭据库（`caps.keychain`）。
   *
   * 这句话必须在用户敲口令**之前**说出来：没有它时口令只留在本次进程的内存里，
   * 退出即失效。核心那侧不再因此拒绝保存（缺口 G38 选 B），所以这里说的不是"配不了同步"，
   * 而是"这次运行有效、下次要重填" —— 说错的那一半曾经写在这里，照文档对代码扫出来的。
   */
  const credentialStoreUnavailable = computed(() => caps.value.keychain === 'none');
  /**
   * 根证书 PEM 本体核心不回传（一份 CA 证书几 KB，每次回填都端过去没意义），
   * 传的是"存过没有"。界面上必须有这一位，否则选了 `ca_bundle` 的账户重开设置页时
   * 那一格看着像空的 —— 而"留空 = 不改"的语义又会把它说成已配置，两件事对不上就是误导。
   */
  const caPemIsSet = computed(() => account.value?.hasCaPem === true);
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
      draft.value = draftFromWire(account.value);
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
    // 命令参数是平铺的：直接把翻译好的线格式当 body（此前包了一层 {draft:…}，
    // 核心反序列化不认，"保存"一直回 bad_args —— 也就是这条配置从来没生效过）。
    const payload = toWire(draft.value);
    try {
      const saved = await callCommand<Account>(Commands.configureAccount, { ...payload });
      account.value = typeof saved === 'object' && saved !== null ? saved : account.value;
      draft.value = { ...draftFromWire(account.value), password: '' };
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

  /** 导入散文件（`.enex` / Markdown / 纯文本）到默认本。与"整库还原"不是一回事。 */
  async function importFiles(paths: string[]): Promise<ImportFilesReport | null> {
    dataBusy.value = true;
    lastReport.value = null;
    try {
      const report = await callCommand<ImportFilesReport>(Commands.importFiles, { paths });
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
    // 导出报告必须自己说清这是整库还是子树：拿一份子树包当"整库备份"是最危险的误用。
    if (report.scope === 'folders') parts.push(`子树 · ${report.scopeFolders ?? '?'} 个文件夹 · 不能用于整库还原`);
    else if (report.scope === 'full') parts.push('整库');
    if (report.counts) {
      const c = report.counts;
      parts.push(`笔记 ${c.notes ?? 0} · 文件夹 ${c.folders ?? 0} · 附件 ${c.attachments ?? 0}`);
    }
    if (report.path) parts.push(report.path);
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
    credentialSavedButGone,
    credentialVolatile,
    credentialStoreUnavailable,
    caPemIsSet,
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
    importFiles,
    backupDb,
    restoreDb,
    describeReport,
  };
});
