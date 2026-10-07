/**
 * §4.6 凭据三时点里的"存之后"那一格。
 *
 * 口令字段是只写的，读回来只有"有没有 / 这一轮还在不在 / 能不能持久"三个位 ——
 * 所以保存成功之后界面说什么，完全取决于核心回的那三个位。这一族谎报的代价很具体：
 * 用户看到"已保存"，下次开机同步必然失败，而他已经想不起来自己填过什么。
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { stubLocalService } from '../testing/http';
import { useSettingsStore } from './settings';
import { useToastStore } from './toasts';

const texts = () => useToastStore().items.map((item) => item.text);

beforeEach(() => {
  setActivePinia(createPinia());
  vi.useRealTimers();
});

describe('存之后那句话', () => {
  it('这台设备留不住口令：要说「已记住。退出后需要重新填写。」，不许只说"已保存"', async () => {
    stubLocalService({
      configure_account: () => ({
        id: 'acct-1',
        baseUrl: 'https://dav.home.example/dav',
        enabled: true,
        hasCredential: true,
        credentialLive: true,
        credentialPersistent: false,
      }),
    });
    const settings = useSettingsStore();

    expect(await settings.saveAccount()).toBe(true);

    expect(texts()).toContain('已记住。退出后需要重新填写。');
    expect(texts(), '只报"已保存"就是当着用户说假话').not.toContain('已保存');
  });

  it('正对照：这台设备留得住口令时，才说「已保存」', async () => {
    stubLocalService({
      configure_account: () => ({
        id: 'acct-2',
        baseUrl: 'https://dav.home.example/dav',
        enabled: true,
        hasCredential: true,
        credentialLive: true,
        credentialPersistent: true,
      }),
    });
    const settings = useSettingsStore();

    expect(await settings.saveAccount()).toBe(true);

    expect(texts()).toContain('已保存');
    expect(texts()).not.toContain('已记住。退出后需要重新填写。');
  });
});
