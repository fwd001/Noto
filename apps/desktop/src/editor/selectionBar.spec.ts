/**
 * 浮动条定位的纯逻辑测试：翻转、居中、贴边三种情形。
 */
import { describe, expect, it } from 'vitest';
import { placeBar, type Box } from './selectionBar';

const view: Box = { top: 0, bottom: 800, left: 600, right: 1400 };
const bar = { width: 260, height: 40 };
const mid: Box = { top: 300, bottom: 326, left: 900, right: 1000 };

describe('placeBar', () => {
  it('默认浮在选区上方并水平居中', () => {
    const p = placeBar(mid, bar, view);
    expect(p.top).toBe(300 - 8 - 40);
    expect(p.left).toBe(950 - 130);
  });

  it('上方放不下（选区贴着可视区顶）就翻到下方', () => {
    const p = placeBar({ top: 20, bottom: 46, left: 900, right: 1000 }, bar, view);
    expect(p.top).toBe(46 + 8);
  });

  it('选区靠边时贴边而不是溢出', () => {
    const leftEdge = placeBar({ top: 300, bottom: 326, left: 610, right: 660 }, bar, view);
    expect(leftEdge.left).toBe(608);
    const rightEdge = placeBar({ top: 300, bottom: 326, left: 1360, right: 1395 }, bar, view);
    expect(rightEdge.left + bar.width).toBeLessThanOrEqual(1400 - 8);
  });

  it('条形比可视区还宽时不产生反向夹取（left 不得大于 maxLeft）', () => {
    const p = placeBar(mid, { width: 5000, height: 40 }, view);
    expect(p.left).toBe(view.left + 8);
  });
});
