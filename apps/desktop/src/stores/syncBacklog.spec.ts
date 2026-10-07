/**
 * §6 第 4 格「同步详情面板」里剩下的那两位：**待处理任务数**与**开放冲突数**。
 *
 * 形状与 §3.3 的分档一模一样：核心在 `SyncStatusDto` 里一直在发
 * （`pending_ops` / `open_conflicts`，真序列化输出实测键名 `pendingOps` / `openConflicts`），
 * 而 `stores/sync.ts` 的 `refreshStatus()` 只取了 `lastSuccessAt` 与 `divergenceHeld` ——
 * 那一发查询带回来的其余几位在**调用点**上被丢掉（连 TS 的类型字面量里都没声明）。
 *
 * 但这一格有一处必须先问清的语义（缺口 G94）：`pendingOps` 数的是
 * **当前账户出箱里还有几条**（`lib.rs` 里那段 `outbox_len(账户…)`），没配账户时它恒为 0，
 * 而"这台设备上还有多少改动没落地"是笔记行上的 `dirty`。所以那句「改动都已经同步过去」
 * 只能对**配了账户**的设备说 —— 否则就是对一台根本没开同步的设备撒谎。下面两条 gate 判的就是这个。
 *
 * 其余三条是同一族"不许把未知说成事实"：问不到 ⇒ 那一行不出现；载荷缺键 ⇒ 也只能当未知
 * （`undefined` 不是 0）；数是真的 ⇒ 0 与非 0 各说各的话。
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { stubLocalService } from '../testing/http';
import { useSettingsStore } from './settings';
import { useSyncStore } from './sync';

/** 真产物的键名（camelCase）：这里漂了，下面几条就会红 —— 跨语言契约钉在这里的理由。 */
const status = (extra: Record<string, unknown>) => ({ phase: 'idle', badge: 'idle', ...extra });
const ACCOUNT = { id: 'acct-9', baseUrl: 'https://dav.invalid/dav', username: 'u', enabled: true };

/** 换一个账户形状 = 重开 pinia + 走真入口 `loadAccount()`，再问一次 sync_status。 */
async function withStatus(payload: Record<string, unknown>, account: unknown = ACCOUNT) {
  setActivePinia(createPinia());
  stubLocalService({ account: () => account, sync_status: () => payload });
  await useSettingsStore().loadAccount();
  const sync = useSyncStore();
  await sync.refreshStatus();
  return sync;
}

beforeEach(() => {
  vi.useRealTimers();
});

describe('同步账上的排队数（§6 第 4 格）', () => {
  it('把核心发的两个数读进来，那一句话说得出「还有 N 项改动等着同步」', async () => {
    const sync = await withStatus(status({ pendingOps: 7, openConflicts: 0, lastSuccessAt: null, divergenceHeld: null }));
    expect(sync.backlog).toEqual({ pendingOps: 7, openConflicts: 0 });
    expect(sync.backlogLine).toBe('还有 7 项改动等着同步');
  });

  it('两个数都是 0 ⇒ 说的是「都传上去了」，这句必须是核心给的事实', async () => {
    const sync = await withStatus(status({ pendingOps: 0, openConflicts: 0 }));
    expect(sync.backlogLine).toBe('改动都已经同步过去');
  });

  it('开放冲突那位单独说一句，和排队那句并存不互相吞掉', async () => {
    const sync = await withStatus(status({ pendingOps: 3, openConflicts: 2 }));
    expect(sync.backlogLine).toBe('还有 3 项改动等着同步 · 2 条版本等你处理');
  });

  it('只有冲突、没有排队时也只说事实的那半句', async () => {
    const sync = await withStatus(status({ pendingOps: 0, openConflicts: 1 }));
    expect(sync.backlogLine).toBe('改动都已经同步过去 · 1 条版本等你处理');
  });

  it('问不到（本地服务连不上）⇒ 那一行不出现，不许说成「都传上去了」', async () => {
    setActivePinia(createPinia());
    stubLocalService({
      account: () => ACCOUNT,
      sync_status: () => ({ ok: false, error: { code: 'server_unavailable', messageKey: 'server_unavailable', retryable: true } }),
    });
    await useSettingsStore().loadAccount();
    const sync = useSyncStore();
    await sync.refreshStatus();
    expect(sync.backlog).toBeNull();
    expect(sync.backlogLine).toBeNull();
  });

  it('回包里缺这两格（旧核心）⇒ 也只能当未知，不许把 undefined 读成 0', async () => {
    const sync = await withStatus(status({ lastSuccessAt: '2026-10-07T01:02:03Z' }));
    expect(sync.backlog).toBeNull();
    expect(sync.backlogLine).toBeNull();
  });

  /** 缺口 G94 那一格：核心的 0 是"没有账户可数"，不是"都传上去了"。 */
  it('没配账户时就算核心回 0/0，也不许说「改动都已经同步过去」', async () => {
    const sync = await withStatus(status({ pendingOps: 0, openConflicts: 0 }), null);
    expect(useSettingsStore().hasAccount).toBe(false);
    expect(sync.backlog, '数还是照原样读进来（未知与事实要分得开）').toEqual({ pendingOps: 0, openConflicts: 0 });
    expect(sync.backlogLine).toBeNull();
  });

  it('refresh 之后数字要跟着换（清完账不许还挂着旧的排队数）', async () => {
    let pending = 5;
    setActivePinia(createPinia());
    stubLocalService({
      account: () => ACCOUNT,
      sync_status: () => status({ pendingOps: pending, openConflicts: 0 }),
    });
    await useSettingsStore().loadAccount();
    const sync = useSyncStore();
    await sync.refreshStatus();
    expect(sync.backlogLine).toBe('还有 5 项改动等着同步');
    pending = 0;
    await sync.refreshStatus();
    expect(sync.backlogLine).toBe('改动都已经同步过去');
  });
});
