/**
 * 同步状态：把后端事件折叠成用户可见的状态。
 *
 * 口径（2026-10-04 与用户重定，见 docs/CI-CD.md「装完应该是干净的」）：
 * `✓ 已同步 / ↻ 正在同步 / ○ 离线 / ! 同步失败` 四态**只描述一轮同步**，
 * 而"这一台设备根本没在同步"是第五种事实 —— 它必须有**静止**的落点（`idle`），
 * 否则"没配账户"会被显示成"正在忙"。协议细节一律折进这五格，不外露第 6 态。
 * 任何未知的内部状态都归入"同步失败"。
 */
import { defineStore } from 'pinia';
import { computed, ref } from 'vue';
import { callCommand, type LinkState } from '../api/bridge';
import { Commands, type SyncBadgeKind, type SyncProgress } from '../api/types';
import { messageFor, t } from '../i18n';
import { asBridgeError } from '../util/errors';
import { formatWhen } from '../util/format';
import { useSettingsStore } from './settings';

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

const KNOWN_BADGES: readonly SyncBadgeKind[] = ['synced', 'syncing', 'offline', 'failed', 'idle'];

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
      // 「离线」这一格**必须把原因带上**。核心在缺凭据 / 协议不匹配 / root 不匹配那几处
      // 发的都是 `badge: Offline` + 一个具名 `message_key`（`crates/notera-host/src/lib.rs`
      // 的 `Phase::NeedsCredentials` 那一段），而旧写法把 `messageKey` 一律清成 `null`
      // ⇒ PROXY.md §7 那句"徽标停在『需要凭据』"在界面上根本不存在，用户只看见"离线"，
      // 既不知道是口令没了，也不知道去哪儿修。（这条是补"原因要看得见"那格时被自己的门禁抓出来的）
      return { ...state, badge: 'offline', progress: null, messageKey: signal.messageKey ?? signal.errorCode ?? null, retryable: false };
    case 'idle':
      // 核心当前只有 4 个 `Badge`（没有 Idle），所以这条路暂时只能由本地上来。
      // 但折叠必须是**全函数**：少这一支时 `'idle'` 会掉进 `default` 变成"同步失败"——
      // 把"没在同步"报成"同步坏了"，正是用户投诉的那类谎。
      return { ...state, badge: 'idle', progress: null, messageKey: null, retryable: false };
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

/** 四态 → 文案键。写成表而不是 `sync.${badge}` 拼接：拼出来的键查不到就静默印键名，
 *  而 Record 的键是联合类型，少一态编译期就报错。 */
const BADGE_LABEL_KEYS: Record<SyncBadgeKind, string> = {
  synced: 'sync.synced',
  syncing: 'sync.syncing',
  offline: 'sync.offline',
  failed: 'sync.failed',
  idle: 'sync.idle',
};

export const useSyncStore = defineStore('sync', () => {
  // 初始态用 `idle` 而不是 `syncing`：**首帧时还没有任何一轮同步在跑**，
  // 写 `syncing` 等于开口就说谎 —— 而且在没有事件来纠正它的界面上，
  // 那颗圈会一直转到用户以为"卡住了"（用户反馈"那个按钮一直转"）。
  // 等真的一轮开始（`markBusy(true)`）再变 `syncing`。
  const state = ref<FoldedSyncState>({ badge: 'idle', progress: null, messageKey: null, retryable: false, finishedAt: null, rounds: 0 });
  const link = ref<LinkState>('unknown');
  const busy = ref(false);

  const badge = computed(() => state.value.badge);
  /**
   * 静止态（`idle`）要说清是**哪一种**静止：没配过 vs 配了但关了 vs 配了但这一轮没口令。
   *
   * 三者都该静止、都不该转圈，但把"已关闭"念成"未配置同步"是另一句假话 —— 用户会去翻一个
   * 填得满满当当的表单（`sync.needsCredentials` 踩过同一形：G38）。第三句是 §2.3 明写的
   * 第五格三句之一，而它此前在界面上**不存在**：核心缺凭据时发的是 `offline` +
   * `sync.needsCredentials`，于是屏幕上只有"离线" —— 说的是"网络断了"，
   * 真相是"这台设备的口令只活在上一次运行里"（缺口 G86）。
   */
  const syncActive = computed(() => useSettingsStore().syncActive);
  const idleReason = computed<'unconfigured' | 'disabled' | 'password' | null>(() => {
    const settings = useSettingsStore();
    if (!settings.hasAccount) return 'unconfigured';
    if (!settings.syncActive) return 'disabled';
    if (settings.credentialSavedButGone) return 'password';
    return null;
  });
  /**
   * 屏幕上那一格。"这一轮没有口令"归**第五格**（根本没在同步），不归"离线" ——
   * 门没开和线路断了是两件不同的事，而 §2.3 把它列在第五格的三句里。
   * 「本地服务连不上」优先级更高（§4.3），所以 linkDown 时一个字都不改。
   */
  const shownBadge = computed(() =>
    link.value !== 'unreachable' && badge.value === 'offline' && idleReason.value === 'password'
      ? 'idle'
      : badge.value,
  );
  const label = computed(() => {
    if (shownBadge.value === 'idle') {
      if (idleReason.value === 'disabled') return messageFor('sync.disabled');
      if (idleReason.value === 'password') return messageFor('sync.needsPassword');
    }
    return messageFor(BADGE_LABEL_KEYS[shownBadge.value]);
  });
  /** 折成第五格时，原因那句也要换成说得准的那一句（核心给的是"还没有可用的登录凭据"）。 */
  const detail = computed(() => {
    if (shownBadge.value === 'idle' && idleReason.value === 'password') {
      return messageFor('settings.credentialGone');
    }
    return state.value.messageKey ? messageFor(state.value.messageKey) : null;
  });
  const showRetry = computed(() => state.value.badge === 'failed' && state.value.retryable);
  const percent = computed(() => {
    const progress = state.value.progress;
    if (!progress || progress.total <= 0) return null;
    return Math.min(100, Math.round((progress.done / progress.total) * 100));
  });
  const offline = computed(() => state.value.badge === 'offline');
  const linkDown = computed(() => link.value === 'unreachable');

  /**
   * 上一次同步成功的时间（§4.3 的「已同步 · 上一次：{时间}」）。
   *
   * 来源是核心 `sync_status` 的 `lastSuccessAt` —— 那个字段一直有，前端从来没调过那条命令，
   * 于是这一格只能靠本次会话的事件凑，重启之后就是空的（看起来像"从没同步过"）。
   * **没拿到就是 null**：写"刚刚"或拿启动时间冒充，都是把未知说成事实。
   */
  const lastSuccessAt = ref<string | null>(null);
  const lastSuccessLine = computed(() => {
    const when = formatWhen(lastSuccessAt.value);
    return when.length > 0 ? t('sync.lastSuccess', { time: when }) : null;
  });

  async function refreshStatus(): Promise<void> {
    try {
      const status = await callCommand<{ lastSuccessAt?: string | null }>(Commands.syncStatus, {});
      lastSuccessAt.value = status?.lastSuccessAt ?? null;
    } catch {
      // 问不到就继续未知：这一格缺席不影响徽标那五格，也不该抛到界面上。
    }
  }

  function applySignal(signal: SyncSignal): void {
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

  /**
   * 没有同步账户时，把徽标落到一个**诚实**的状态。
   *
   * 为什么必须单独处理：用户截图里那颗"正在同步"一直转，原因就是
   * `markBusy(true)` 把它置成 `syncing`，而核心对"没配账户"返回的是
   * `no_account` 错误（crates/notera-host/src/lib.rs 的sync 分支）——
   * **那不是一次同步失败，而是一次根本没发生的同步**。`busy` 复位后没有事件
   * 把它带走，于是它永远停在"正在同步"。
   *
   * 而"转不停的圈"是最坏的答案：用户无法判断是在忙、在卡、还是根本没配账户。
   * 这里让它落到 `idle`（文案是"未配置同步"），那颗圈自然停。
   */
  function markNoAccount(): void {
    // **链接不可达时不要盖成"未配置同步"。**
    //
    // 两件事要同时说清：① 本地服务连不上（`unreachable`）；② 没配同步账户（`idle`）。
    // 而"连本地服务都连不上"是**更前置**的条件 —— 此时界面能不能读到笔记都还没保证，
    // 报"未配置同步"是**转移焦点**：用户会去设置里翻同步，而真正的问题是服务没起。
    // 旧测试（app.spec「本地服务没起时显示明确状态」）就是钉这一条的。
    //
    // ⚠ 而这条也正好服务于用户的要求「没配 WebDAV 就该是静止的」——
    // `unreachable` 同样是静止字形（○），不会转。
    if (link.value === 'unreachable') {
      state.value = foldLinkState(state.value, 'unreachable');
      return;
    }
    state.value = { ...state.value, badge: 'idle', progress: null, messageKey: null, retryable: false };
  }

  function noteSavedLocally(): void {
    // 本地写入成功后，未同步期间的可见态仍是"离线/正在同步"，不伪装成已同步。
    if (state.value.badge === 'synced') return;
    if (link.value === 'unreachable') state.value = foldLinkState(state.value, 'unreachable');
  }

  /** 手动同步：命令本身不等网络，结果一律由事件回流。 */
  async function syncNow(): Promise<void> {
    if (busy.value) return;
    // **先看配置，再决定要不要亮"正在同步"。**
    //
    // 这一道闸门是用户那条投诉的正解（「我没配置同步，咋一点击未同步就开始转了」）：
    // `sync_now` 命令只做 `dirty_ticks += 1` 就回 `null`（HTTP 200，**从不回错**），
    // 而调度器只在壳里"首帧之后、当时已配好账户"那一次 spawn ——
    // 没配账户时没有任何消费者，于是 `markBusy(true)` 亮起来的 `syncing`
    // **永远不会被一条事件带走**。实测（`.logs/repro-click-spin.mjs`）：
    // 空库点一下，连采 12 秒全是 `syncing`，`aria-busy=true`，控制台零 error。
    //
    // 所以 `no_account` 那条 catch 在旧形状上是死代码（仍然留着：核心那侧同步补了具名码，
    // 真回错时得接住）。真正守位置的是这里 —— **徽标只说真话**，
    // 而"点了该有去处"（把人带去设置页）归 `SyncBadge` 自己判，store 不偷偷换视图。
    // 而"这一台设备的口令只剩个引用"必须走**同一道**闸门（缺口 G86 的另一半）：
    // `syncActive` 看的是配置，配置确实是开着的那一格 —— 可这一轮注定 407。
    // 放它出门的结果是徽标从"需要重新填写口令"翻成"同步失败"，
    // 用户照着"失败"去查网络，而该做的动作是重填一次口令。
    if (!syncActive.value || idleReason.value === 'password') {
      markNoAccount();
      return;
    }
    markBusy(true);
    try {
      await callCommand<null>(Commands.syncNow, {});
    } catch (error) {
      const bridge = asBridgeError(error);
      // 三条路要分开说，别都塞进"同步失败"：
      // ① 没配账户 ⇒ `no_account`。那是"根本没在同步"，落 `idle`，那颗圈会停 ——
      //    以前这里落回 `syncing` 且没有事件来纠正，于是永远转（用户截图里的那颗）。
      // ② 连不上本地服务 ⇒ 离线。
      // ③ 其余才是真失败。
      if (bridge.code === 'no_account') {
        markNoAccount();
        return;
      }
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
    shownBadge,
    idleReason,
    label,
    detail,
    syncActive,
    showRetry,
    percent,
    lastSuccessAt,
    lastSuccessLine,
    refreshStatus,
    offline,
    linkDown,
    busy,
    applySignal,
    setLink,
    markBusy,
    // 导出给启动流程用：没有账户时首帧就该落`idle`，
    // 否则用户一打开就看到"正在同步"转起来（那轮同步根本不会发生）。
    markNoAccount,
    noteSavedLocally,
    syncNow,
  };
});
