import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { stubLocalService } from '../testing/http';
import { useSyncStore } from './sync';

/**
 * §4.3 那一格：「已同步 · 上一次：{时间}」。
 *
 * 后端 `sync_status` 一直带 `lastSuccessAt`（`commands.rs` 的 `SyncStatusDto`），
 * 而前端**从来没调过这条命令** —— 于是"上一次成功"这个用户最关心的事实只能等本次会话
 * 恰好跑完一轮才有值，重启后那一格是空的（看起来像"从没同步过"）。
 *
 * 判据打在调用边与渲染取值上：
 *  ① 问过一次 `sync_status`，且拿到的时间真的进了 store；
 *  ② 没拿到时间 ⇒ **不许编一个"刚刚"**（那是把未知说成事实）；
 *  ③ 命令失败 ⇒ 保持未知，不抛错、不影响徽标那五格。
 */
const STATUS = {
  phase: 'idle',
  badge: 'synced',
  lastSuccessAt: '2026-10-06T01:02:03Z',
  pendingOps: 0,
  openConflicts: 0,
  messageKey: null,
  retryable: false,
};

beforeEach(() => {
  setActivePinia(createPinia());
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('上一次成功时间', () => {
  it('启动时问一次 sync_status，拿到的时间进 store 并能读出一句人话', async () => {
    const service = stubLocalService({ sync_status: () => STATUS });
    const sync = useSyncStore();
    await sync.refreshStatus();
    expect(service.callsOf('sync_status')).toHaveLength(1);
    expect(sync.lastSuccessAt).toBe('2026-10-06T01:02:03Z');
    const line = sync.lastSuccessLine;
    expect(line, '那一格必须真说得出"上一次"').not.toBeNull();
    expect(line).toContain('上一次');
  });

  it('后端没给时间 ⇒ 那一格是空的，不许编一个"刚刚"', async () => {
    stubLocalService({ sync_status: () => ({ ...STATUS, lastSuccessAt: null }) });
    const sync = useSyncStore();
    await sync.refreshStatus();
    expect(sync.lastSuccessAt).toBeNull();
    expect(sync.lastSuccessLine).toBeNull();
  });

  it('命令失败保持未知，不抛错也不影响徽标', async () => {
    stubLocalService({});
    const sync = useSyncStore();
    await expect(sync.refreshStatus()).resolves.toBeUndefined();
    expect(sync.lastSuccessAt).toBeNull();
    expect(['idle', 'offline', 'synced', 'syncing', 'failed']).toContain(sync.badge);
  });
});
