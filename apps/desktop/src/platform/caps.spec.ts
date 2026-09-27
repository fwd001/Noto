import { describe, expect, it } from 'vitest';
import { localCaps, shouldHideOnClose, SHORTCUTS, shortcutsFor } from './caps';

/**
 * 钉的是"关窗到底是不是收进托盘"这一条判断，以及设置页那张快捷键表的过滤。
 * 这两处都是**只看界面看不出来**的：开关与托盘能力必须同时成立才隐藏窗口。
 */
describe('关闭窗口 = 收进托盘 的判据', () => {
  it('开关开着且托盘真的挂上了，才收进托盘', () => {
    expect(shouldHideOnClose({ tray: true }, { trayHint: true })).toBe(true);
  });

  it('开关是默认关的 → 照常退出（不许替用户决定常驻）', () => {
    expect(shouldHideOnClose({ tray: true }, { trayHint: false })).toBe(false);
  });

  it('托盘没挂上时绝不隐藏：那会得到一个关不掉也找不回的进程', () => {
    expect(shouldHideOnClose({ tray: false }, { trayHint: true })).toBe(false);
  });

  it('能力缺失（undefined）按"没挂上"处理，不按缺省值猜', () => {
    expect(shouldHideOnClose({} as { tray: boolean }, { trayHint: true })).toBe(false);
  });
});

describe('快捷键表的过滤', () => {
  it('本机兜底能力下不显示任何需要托盘/全局快捷键的行', () => {
    const caps = localCaps();
    const shown = shortcutsFor(caps).map((s) => s.id);
    const gated = SHORTCUTS.filter((s) => s.requires).map((s) => s.id);
    expect(gated.length).toBeGreaterThan(0);
    for (const id of gated) expect(shown).not.toContain(id);
  });

  it('两项能力都到位后，那些行才出现', () => {
    const shown = shortcutsFor({ ...localCaps(), tray: true, globalShortcuts: true }).map((s) => s.id);
    expect(shown).toContain('tray');
    expect(shown).toContain('global.quick-note');
    expect(shown).toContain('global.toggle-window');
  });
});
