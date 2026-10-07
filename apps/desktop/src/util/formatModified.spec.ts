/**
 * 「改于 {时间}」那一格（设计稿第 1 页编辑器角上：`已存在本机` `412 字` `改于 14:22`）。
 *
 * 口径是"这一篇最后一次被改的时刻"，那就不许说成"刚刚"这类会自己老掉的相对时间 ——
 * `formatWhen` 走的是相对那套（§4.3 的"上一次：3 分钟前"用它），这一格要的是**墙上时刻**：
 * 同一天给 `HH:MM`，同年往前给 `M月D日`，再往前带年份。
 * 取不到时间时返回空串，由调用方**根本不画那一格**（写"刚刚"是编的 —— 同 §4.3 那条口径）。
 *
 * 夹具全部用**本地** `Date` 造再转 ISO：直接写 UTC 串的话，判据的正确性会跟着跑测机器的
 * 时区漂（这里期望的是本地日历天，而 `2025-12-31T16:00Z` 在 UTC+8 已经是 2026 年）。
 */
import { expect, it } from 'vitest';
import { formatModified } from './format';

const NOW = new Date(2026, 9, 7, 15, 30, 0).getTime(); // 2026-10-07 15:30 本地
const iso = (y: number, m: number, d: number, hh = 10, mm = 20) => new Date(y, m - 1, d, hh, mm, 0).toISOString();

it('同一天给墙上时刻（设计稿那句就是 HH:MM）', () => {
  expect(formatModified(iso(2026, 10, 7, 14, 22), NOW)).toBe('14:22');
  expect(formatModified(iso(2026, 10, 7, 0, 5), NOW)).toBe('00:05');
});

it('同一年往前退到日期，不许假装是今天', () => {
  expect(formatModified(iso(2026, 9, 28), NOW)).toBe('9月28日');
  expect(formatModified(iso(2026, 1, 1), NOW)).toBe('1月1日');
});

it('跨年要带年份', () => {
  expect(formatModified(iso(2025, 12, 31), NOW)).toBe('2025年12月31日');
});

it('未来的时间戳也照墙上时刻读，不许退化成空（服务器时钟偏一点是真实存在的）', () => {
  expect(formatModified(iso(2026, 10, 8, 9, 0), NOW)).toBe('10月8日');
});

it('拿不到时间就是空串（调用方据此不画那一格，界面不许编一个"刚刚"）', () => {
  expect(formatModified(null, NOW)).toBe('');
  expect(formatModified(undefined, NOW)).toBe('');
  expect(formatModified('', NOW)).toBe('');
  expect(formatModified('不是时间', NOW)).toBe('');
});
