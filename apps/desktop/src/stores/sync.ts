/**
 * 同步状态：把后端事件折叠成用户可见的四个状态（✓ 已同步 / ↻ 正在同步 / ○ 离线 / ! 同步失败）。
 * 任何未知的内部状态都归入"同步失败"，绝不出现第 5 态；错误只以文案键呈现。
 */
import { defineStore } from 'pinia';
import { computed, ref } from 'vue';
import { callCommand, type LinkState } from '../api/bridge';
import { Commands, type SyncBadgeKind, type SyncProgress } from '../api/types';
import { messageFor } from '../i18n';
import { asBridgeError } from '../util/errors';

export interface FoldedSyncState {
  badge: SyncBadgeKind;
  progress: SyncProgress | null;
  messageKey: string | null;
  retryable: boolean;
  finishedAt: number | null;
  rounds: number;
}

export interface SyncSignal {
  badge: string;
  progress?: Partial<SyncProgress> | null;
  errorCode?: string;
  messageKey?: string;
}

const KNOWN_BADGES: readonly SyncBadgeKind[] = ['synced', 'syncing', 'offline', 'failed'];

export function normalizeBadge(value: string | undefined | null): SyncBadgeKind {
  return (KNOWN_BADGES as readonly string[]).includes(value ?? '') ? (value as SyncBadgeKind) : 'failed';
}

function readProgress(value: Partial<SyncProgress> | null | undefined): SyncProgress | null {
  if (!value || typeof value !== 'object') return null;
  const done = typeof value.done === 'number' && Number.isFinite(value.done) ? Math.max(0, Math.trunc(value.done)) : 0;
  const total = typeof value.total === 'number' && Number.isFinite(value.total) ? Math.max(0, Math.trunc(value.total)) : 0;
  if (total === 0 && done === 0) return null;
  const bytes = typeof value.bytes === 'number' && Number.isFinite(value.bytes) ? Math.max(0, Math.trunc(value.bytes)) : undefined;
  return { done, total: Math.max(total, done), ...(bytes === undefined ? {} : { bytes }) };
}

/** 纯函数折叠：给定当前状态与一条同步信号，产出新的四态状态。 */
export function foldSyncEvent(state: FoldedSyncState, signal: SyncSignal, at: number): FoldedSyncState {
  const badge = normalizeBadge(signal.badge);
  switch (badge) {
    case 'syncing':
      return {
        ...state,
        badge: 'syncing',
        progress: readProgress(signal.progress) ?? state.progress,
        messageKey: null,
        retryable: false,
      };
    case 'synced':
      return { badge: 'synced', progress: null, messageKey: null, retryable: false, finishedAt: at, rounds: state.rounds + 1 };
    case 'offline':
      return { ...state, badge: 'offline', progress: null, messageKey: null, retryable: false };
    case 'failed':
    default:
      return {
        ...state,
        badge: 'failed',
        progress: null,
        messageKey: signal.messageKey ?? signal.errorCode ?? null,
        retryable: true,
      };
  }
}

/** 通道可达性也折叠进四态：连不上本地服务 = 离线。 */
export function foldLinkState(state: FoldedSyncState, link: LinkState): FoldedSyncState {
  if (link === 'unreachable') return { ...state, badge: 'offline', progress: null, retryable: false, messageKey: 'link.unreachable' };
  if (link === 'ready' && state.messageKey === 'link.unreachable') {
    return { ...state, badge: 'syncing', messageKey: null, progress: null };
  }
  return state;
}

export const useSyncStore = defineStore('sync', () => {
  const state = ref<FoldedSyncState>({ badge: 'syncing', progress: null, messageKey: null, retryable: false, finishedAt: null, rounds: 0 });
  const link = ref<LinkState>('unknown');
  const busy = ref(false);

  const badge = computed(() => state.value.badge);
  const dbTooNew = ref(false);
  const label = computed(() => messageFor(`sync.${state.value.badge}`));
  const detail = computed(() => (state.value.messageKey ? messageFor(state.value.messageKey) : null));
  const showRetry = computed(() => state.value.badge === 'failed' && state.value.retryable);
  const percent = computed(() => {
    const progress = state.value.progress;
    if (!progress || progress.total <= 0) return null;
    return Math.min(100, Math.round((progress.done / progress.total) * 100));
  });
  const offline = computed(() => state.value.badge === 'offline');
  const linkDown = computed(() => link.value === 'unreachable');

  function applySignal(signal: SyncSignal): void {
    if (signal.errorCode === 'db_too_new' || signal.messageKey === 'db_too_new') dbTooNew.value = true;
    state.value = foldSyncEvent(state.value, signal, Date.now());
  }

  function setLink(next: LinkState): void {
    if (link.value === next) return;
    link.value = next;
    state.value = foldLinkState(state.value, next);
  }

  function markBusy(next: boolean): void {
    busy.value = next;
    if (next) state.value = foldSyncEvent(state.value, { badge: 'syncing' }, Date.now());
  }

  function noteSavedLocally(): void {
    // 本地写入成功后，未同步期间的可见态仍是"离线/正在同步"，不伪装成已同步。
    if (state.value.badge === 'synced') return;
    if (link.value === 'unreachable') state.value = foldLinkState(state.value, 'unreachable');
  }

  /** 手动同步：命令本身不等网络，结果一律由事件回流。 */
  async function syncNow(): Promise<void> {
    if (busy.value) return;
    markBusy(true);
    try {
      await callCommand<null>(Commands.syncNow, {});
    } catch (error) {
      const bridge = asBridgeError(error);
      // 连不上 = 离线，不是"同步失败"：四态折叠在这里发生，不暴露内部码
      state.value = foldSyncEvent(
        state.value,
        { badge: bridge.isOffline ? 'offline' : 'failed', ...(bridge.isOffline ? {} : { messageKey: bridge.messageKey }) },
        Date.now(),
      );
    } finally {
      busy.value = false;
    }
  }

  return {
    state,
    link,
    badge,
    label,
    detail,
    dbTooNew,
    showRetry,
    percent,
    offline,
    linkDown,
    busy,
    applySignal,
    setLink,
    markBusy,
    noteSavedLocally,
    syncNow,
  };
});
