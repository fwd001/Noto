/**
 * §6 最后一格「设备身份 —— 每条记录带 device_id，界面上从没出现过"是哪台设备改的"」的前半：
 * 存储层发的这一串要原样到界面。
 *
 * 核心那侧的契约另有测试（`notera-host` 的 stats 键集合那条：非空、与配置一致、两次调用同一个）。
 * 这里只管**前端这一头**两件事：
 *  · 键名不许漂（存储层发 `deviceId`，界面读 `deviceId` —— 这条边历史上漏过一次整组键，
 *    设置页三行恒为「—」就是这么来的）；
 *  · **缺位不许兜底成空串**：`undefined` 让模板那一行整条不出现；
 *    在这里"贴心地"补一个 `''`，界面上就会画出「这台设备：」后面空着 —— 那是宣称这台设备没有身份。
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { stubLocalService } from '../testing/http';
import { useSettingsStore } from './settings';

const STATS = {
  notes: 3, notesInTrash: 1, folders: 2, attachments: 4, ftsEntries: 9,
  dbBytes: 131072, searchGeneration: 7, inflightOps: 0, libraryReadOnly: false,
};

beforeEach(() => {
  setActivePinia(createPinia());
  vi.useRealTimers();
});

describe('设备身份到界面这一头（§6）', () => {
  it('存储层发的那一串原样带回来', async () => {
    stubLocalService({ stats: () => ({ ...STATS, deviceId: '01a10000-0000-7000-8000-0000000000ab' }) });
    const settings = useSettingsStore();
    await settings.loadStats();
    expect(settings.stats?.deviceId).toBe('01a10000-0000-7000-8000-0000000000ab');
  });

  it('对面是个没这一格的老核心：保持 undefined，不许兜底成空串', async () => {
    stubLocalService({ stats: () => ({ ...STATS }) });
    const settings = useSettingsStore();
    await settings.loadStats();
    expect(settings.stats, '其余读数照常在（那一行只是不出现，不是整块没了）').not.toBeNull();
    expect(settings.stats?.deviceId, '补成空串会让界面画出「这台设备：」后面空着').toBeUndefined();
  });

  it('问不到 stats 时整块归未知（这一位不能活成上一次的残留）', async () => {
    stubLocalService({
      stats: () => ({ ok: false, error: { code: 'server_unavailable', messageKey: 'server_unavailable' } }),
    });
    const settings = useSettingsStore();
    await settings.loadStats();
    expect(settings.stats).toBeNull();
  });
});
