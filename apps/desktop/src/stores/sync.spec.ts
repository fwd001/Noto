import { describe, expect, it } from 'vitest';
import { foldLinkState, foldSyncEvent, normalizeBadge, type FoldedSyncState } from './sync';
import { messageFor } from '../i18n';

const START: FoldedSyncState = { badge: 'syncing', progress: null, messageKey: null, retryable: false, finishedAt: null, rounds: 0 };
const FOUR = ['synced', 'syncing', 'offline', 'failed'];

describe('同步四态折叠', () => {
  it('正常轮次：syncing → synced，进度与错误被清空', () => {
    const busy = foldSyncEvent(START, { badge: 'syncing', progress: { done: 3, total: 10, bytes: 2048 } }, 10);
    expect(busy.badge).toBe('syncing');
    expect(busy.progress?.done).toBe(3);
    const done = foldSyncEvent(busy, { badge: 'synced' }, 20);
    expect(done.badge).toBe('synced');
    expect(done.progress).toBeNull();
    expect(done.messageKey).toBeNull();
    expect(done.finishedAt).toBe(20);
    expect(done.rounds).toBe(START.rounds + 1);
  });

  it('离线不吞掉进度以外的信息，且不显示错误文案', () => {
    const offline = foldSyncEvent(START, { badge: 'offline' }, 1);
    expect(offline.badge).toBe('offline');
    expect(offline.messageKey).toBeNull();
    expect(offline.retryable).toBe(false);
  });

  it('失败带 messageKey：文案查表，绝不出现原始码', () => {
    const failed = foldSyncEvent(START, { badge: 'failed', errorCode: 'CertUntrusted' }, 5);
    expect(failed.badge).toBe('failed');
    expect(failed.retryable).toBe(true);
    const text = messageFor(failed.messageKey ?? 'error.fallback');
    expect(text).toBe(messageFor('CertUntrusted'));
    expect(text).not.toMatch(/HTTP|412|500|status/i);
  });

  it('任何未知状态都折进四态之一（不会出现第 5 态）', () => {
    const inputs = ['synced', 'syncing', 'offline', 'failed', 'relisted', 'cas_retry', '', 'undefined', 'RETRY_LATER'];
    for (const badge of inputs) {
      const next = foldSyncEvent(START, { badge }, 1);
      expect(FOUR).toContain(next.badge);
      expect(FOUR).toContain(normalizeBadge(badge));
    }
    expect(normalizeBadge('weird')).toBe('failed');
  });

  it('进度非法数值不炸，并且不会超过 total', () => {
    const weird = foldSyncEvent(START, { badge: 'syncing', progress: { done: 7, total: 3 } }, 1);
    expect(weird.progress).toEqual({ done: 7, total: 7 });
    const none = foldSyncEvent(START, { badge: 'syncing', progress: { done: 0, total: 0 } }, 1);
    expect(none.progress).toBeNull();
    const nan = foldSyncEvent(START, { badge: 'syncing', progress: { done: Number.NaN, total: 4 } }, 1);
    expect(nan.progress?.done).toBe(0);
  });

  it('连不上本地服务 = 离线；恢复后回到"正在同步"', () => {
    const down = foldLinkState(START, 'unreachable');
    expect(down.badge).toBe('offline');
    expect(FOUR).toContain(down.badge);
    const back = foldLinkState(down, 'ready');
    expect(back.badge).toBe('syncing');
    expect(back.messageKey).toBeNull();
    const ready = foldLinkState(START, 'ready');
    expect(ready).toEqual(START);
  });
});
