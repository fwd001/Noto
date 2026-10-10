import { describe, expect, it } from 'vitest';

import { deviceTag } from './deviceLabel';

const ME = '01a10000-0000-7000-8000-0000000000ab';

/**
 * §6 第 12 格：三种答案各一条 —— 没有来源 / 本机 / 另一台设备。
 * 第三种那条**盯的是短 id 与全文都要有**：只给短 id 会让"另一台到底是哪台"没法核，
 * 只给全文在那一行里塞不下。
 */
describe('deviceTag（这一篇是哪台设备改的）', () => {
  it('没有来源 ⇒ null（界面那一格整条不画，不编一个"本机"）', () => {
    expect(deviceTag(null, ME)).toBe(null);
    expect(deviceTag(undefined, ME)).toBe(null);
    expect(deviceTag('', ME)).toBe(null);
    expect(deviceTag('   ', ME)).toBe(null);
  });

  it('与本机 id 一致 ⇒ this（大小写也归一）', () => {
    expect(deviceTag(ME, ME)).toEqual({ kind: 'this' });
    expect(deviceTag(ME.toUpperCase(), ME)).toEqual({ kind: 'this' });
  });

  it('另一台设备 ⇒ 短 id 前 8 位（去连字符），全文留在 full 里', () => {
    const other = '01a12324-3465-76eb-8ff3-29e7ee5a33f2';
    expect(deviceTag(other, ME)).toEqual({
      kind: 'other',
      short: '01a12324',
      full: other,
    });
  });

  it('本机 id 还不知道（stats 没回来）时，**不许**把别的设备说成本机', () => {
    expect(deviceTag(ME, null)).toEqual({ kind: 'other', short: '01a10000', full: ME });
    expect(deviceTag(ME, undefined)).toEqual({ kind: 'other', short: '01a10000', full: ME });
  });
});
