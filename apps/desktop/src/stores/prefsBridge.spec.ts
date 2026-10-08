/**
 * §6 第 6 格「偏好」的桥（缺口 G98 的前半）。
 *
 * 核心那份作用域偏好表（`settings(key, scope='ui')`，DATA-MODEL §4.1）一直存在且 `get_prefs` /
 * `set_pref` 两条命令都在 dispatch 里，而前端从没调用过 —— 主题与字号只躺在 WebView2 的
 * localStorage 里：profile 被重置、换用户目录、整库备份还原之后，笔记一条不少而设置回到默认。
 *
 * 这几条各自钉一件事：
 *  · **库是权威**：库里有值 ⇒ 用它，本机那份只是首帧的涂料；
 *  · **库里没值不许当"用户选了默认"**：第一次启动的人没写过偏好，那条空返回不能洗掉本机选择；
 *  · **读失败不许变成写失败**：桥不通时保留本机那份，而不是重置；
 *  · **改了要真落库**（调用边）：只换 localStorage 的实现——屏幕照样立刻变——这就是那颗假按钮的兄弟；
 *  · **滑杆不能一帧一次写事务**：一串变化合并成一次；
 *  · **hydrate 的回声不许写回去**。
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { nextTick } from 'vue';
import { stubLocalService } from '../testing/http';
import { FONT_SCALE_MAX, normalizePrefs, useSettingsStore, type UiPrefs } from './settings';

const STORAGE_KEY = 'notera.ui.v1';

function seedLocal(prefs: Partial<UiPrefs>): void {
  localStorage.setItem(STORAGE_KEY, JSON.stringify(prefs));
}

/** 等 Vue 的 watcher 跑完（deep watch 在 flush:'post' 之后），再放行 debounce 的定时器。 */
async function settle(ms = 400): Promise<void> {
  await nextTick();
  await nextTick();
  vi.advanceTimersByTime(ms);
  await nextTick();
}

beforeEach(() => {
  setActivePinia(createPinia());
  localStorage.clear();
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

describe('偏好过桥（§6 第 6 格）', () => {
  it('库里有值 ⇒ 库赢过本机那一份，且首屏的深浅跟着改', async () => {
    seedLocal({ theme: 'light' });
    const service = stubLocalService({ get_prefs: () => ({ theme: 'dark', fontScale: 1.2 }) });
    const settings = useSettingsStore();
    await settings.hydratePrefs();
    await settle();
    expect(settings.prefs.theme).toBe('dark');
    expect(settings.prefs.fontScale).toBe(1.2);
    expect(document.documentElement.dataset.theme).toBe('dark');
    expect(service.callsOf('get_prefs').length, '读偏好只发一次').toBe(1);
  });

  it('库里一条都没写过 ⇒ 保留本机那一份（第一次启动不该被洗成默认）', async () => {
    seedLocal({ theme: 'dark', fontScale: 1.4 });
    stubLocalService({ get_prefs: () => ({}) });
    const settings = useSettingsStore();
    await settings.hydratePrefs();
    await settle();
    expect(settings.prefs.theme).toBe('dark');
    expect(settings.prefs.fontScale).toBe(1.4);
  });

  it('核心没这条命令 / 桥不通 ⇒ 不抛错也不重置（读失败不该变成把用户的设置写坏）', async () => {
    seedLocal({ theme: 'dark' });
    const service = stubLocalService({}); // get_prefs 不在名单里 ⇒ 404 not_found
    const settings = useSettingsStore();
    await expect(settings.hydratePrefs()).resolves.toBeUndefined();
    await settle();
    expect(settings.prefs.theme).toBe('dark');
    expect(service.callsOf('set_pref').length, '读失败之后不许顺手写回去').toBe(0);
  });

  it('改主题 ⇒ 合并窗口之后真发一次 set_pref，键与值是那一格', async () => {
    const service = stubLocalService({ get_prefs: () => ({}), set_pref: () => null });
    const settings = useSettingsStore();
    settings.setTheme('dark');
    expect(service.callsOf('set_pref').length, '还没到合并窗口就不该写库').toBe(0);
    await settle();
    const sent = service.callsOf('set_pref');
    expect(sent.length).toBe(1);
    expect(sent[0].args).toEqual({ key: 'theme', value: 'dark' });
  });

  it('拖字号连发五下（跨五帧，像真拖） ⇒ 一次写事务，不是一帧一次', async () => {
    const service = stubLocalService({ get_prefs: () => ({}), set_pref: () => null });
    const settings = useSettingsStore();
    // 必须**跨 tick** 发：同一个 tick 里连改五次，Vue 的 watcher 本来就合并成一次 ——
    // 那样这条判据测的是 Vue 的调度，不是这里的合并窗口（M4 变异第一版就是因此照绿）。
    for (const value of [1.05, 1.1, 1.15, 1.2, 1.25]) {
      settings.setFontScale(value);
      await nextTick();
      await nextTick();
    }
    await settle();
    const sent = service.callsOf('set_pref');
    expect(sent.length, JSON.stringify(sent)).toBe(1);
    expect(sent[0].args).toEqual({ key: 'fontScale', value: 1.25 });
  });

  it('只发真的变了的那一格：改了主题之后不许把其余三格重发一遍', async () => {
    seedLocal({ theme: 'light', fontScale: 1.3, transparency: false, trayHint: true });
    const service = stubLocalService({ get_prefs: () => ({}), set_pref: () => null });
    const settings = useSettingsStore();
    settings.setTheme('dark');
    await settle();
    expect(service.callsOf('set_pref').map((c) => c.args.key)).toEqual(['theme']);
  });

  it('hydrate 读回来的那一份不许再写回去（回声会伪装成"界面真的改过"）', async () => {
    const service = stubLocalService({ get_prefs: () => ({ theme: 'dark', fontScale: 1.1 }), set_pref: () => null });
    const settings = useSettingsStore();
    await settings.hydratePrefs();
    await settle();
    expect(settings.prefs.theme).toBe('dark');
    expect(service.callsOf('set_pref').length, JSON.stringify(service.callsOf('set_pref'))).toBe(0);
  });

  it('归一化只有一份：库里来什么脏值都不能画到屏幕上', () => {
    expect(normalizePrefs({ theme: 'nope', fontScale: 99 }).theme).toBe('system');
    expect(normalizePrefs({ fontScale: 99 }).fontScale).toBe(FONT_SCALE_MAX);
    expect(normalizePrefs({ fontScale: '1.2' }).fontScale).toBe(1);
    expect(normalizePrefs(null).transparency).toBe(true);
  });
});
