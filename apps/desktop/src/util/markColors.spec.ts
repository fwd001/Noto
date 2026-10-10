/**
 * 侧栏色板的判据（§6「颜色」，2026-10-09 拍板：只做侧栏小色点）。
 *
 * 这里问的三件事各自防一种坏法：
 *  · **每个 labelKey 都真登记**（`t()` 缺键会**原样返回键名** ⇒ 那颗点的 `title` 与读屏
 *    会念出 "sidebar.colorRed" 这种东西，界面上看不出来，只有对着它提问才知道）；
 *  · **形状是 `#rrggbb` 且小写**（核心只校形状不校表，写错一位就是同步出去的一个值；
 *    大写混进来会出现"同一个颜色两种哈希"，那一发改动就不传播了）；
 *  · **两张表不相交**（同色重复 = 用户点了两颗却得到同一个标记，界面上没有任何东西解释）。
 */
import { describe, expect, it } from 'vitest';
import { MARK_SWATCHES, dotStyle } from './markColors';
import { t } from '../i18n';

describe('侧栏文件夹色板', () => {
  it('八支，每支都是小写 #rrggbb', () => {
    expect(MARK_SWATCHES.length).toBe(8);
    for (const s of MARK_SWATCHES) {
      expect(s.hex, `色值形状不对：${s.hex}`).toMatch(/^#[0-9a-f]{6}$/);
    }
  });

  it('八支互相不重复（同色等于给用户两颗点了同一件事）', () => {
    expect(new Set(MARK_SWATCHES.map((s) => s.hex)).size).toBe(MARK_SWATCHES.length);
  });

  it('每一支的读屏名字都登记了，屏幕上不会印出键名', () => {
    for (const s of MARK_SWATCHES) {
      const text = t(s.labelKey);
      expect(text, `这一支没登记文案：${s.labelKey}`).not.toBe(s.labelKey);
      expect(text.length, `这一支的名字太空：${s.labelKey}`).toBeGreaterThan(1);
    }
  });

  it('那颗点照库里的值画；没有颜色就不给样式（不许留一个默认色的点）', () => {
    expect(dotStyle('#c2410c')).toEqual({ 'background-color': '#c2410c' });
    // 对面同步来、不在这张板上的值也要画得出来 —— 色板管"提供哪些"，不管"承认哪些"。
    expect(dotStyle('#123456')).toEqual({ 'background-color': '#123456' });
    expect(dotStyle(null)).toBeNull();
    expect(dotStyle(undefined)).toBeNull();
    expect(dotStyle('')).toBeNull();
  });
});
