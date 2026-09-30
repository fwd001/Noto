import { describe, expect, it } from 'vitest';
import { t } from '../i18n';
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

/**
 * 快捷键表是"设置页教用户怎么按"的那张表 —— 它说错一次，用户就按错一次。
 * 缺口 G40：`Del` 那一行以前借的是 `sidebar.deleteFolder`（"删除文件夹"）的文案，
 * 可按下去干的是"把**当前这条笔记**移到最近删除"（`App.vue` 里的 `trashCurrent()`）。
 * 错的动词 + 错的宾语，而且这句话是设置页上唯一的说明 —— 同一处先修的是列表里那颗按钮，
 * 这张表是漏掉的那一处。
 */
describe('快捷键表说的必须就是那一下干的事', () => {
  it('Del / ⌘⌫ 那行说的是「移到最近删除」，不许说「删除文件夹」', () => {
    const row = SHORTCUTS.find((s) => s.id === 'delete');
    if (!row) throw new Error('快捷键表里没了 delete 这一行：Del 仍然有效，但用户在设置页查不到它');
    const label = t(row.labelKey);
    expect(label, `Del 那行的文案是「${label}」`).toContain('最近删除');
    expect(label, `Del 那行的文案是「${label}」`).not.toContain('文件夹');
  });

  it('每一行的文案都真取得到（缺键会在界面上印出键名本身）', () => {
    expect(SHORTCUTS.length).toBeGreaterThan(10);
    for (const s of SHORTCUTS) {
      const label = t(s.labelKey);
      expect(label, `${s.id} 的文案键没登记`).not.toBe(s.labelKey);
      expect(label.length, `${s.id} 的文案是空串`).toBeGreaterThan(0);
    }
  });
});
