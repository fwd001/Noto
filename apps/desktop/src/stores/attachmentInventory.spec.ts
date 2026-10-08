/**
 * §6「附件管理器」那一格的读侧（缺口 G99 的前半）。
 *
 * 与 `attachmentLedger.spec.ts` 的分工：那一格是**编辑器打开一篇笔记时**按 sha 问账（G74），
 * 这一格是**整台设备的全表账**（一次读、六个数）。两件事共用一条纪律：只搬运不加工。
 *
 * 为什么这条纪律值得钉：这一格有六个数，前端每多算一次就多一套真相
 * （备份清单那一格定的就是同一条口径 —— 顺序、可用性都在核心，前端再排一次迟早漂）。
 * 唯一允许前端算的是 `quarantineDaysLeft`：那是一个**给人看的紧迫度**（"还有 29 天"），
 * 不是判定；删除动作的判定在核心的宽限期 SQL 里，这里改了也不会多删一份字节。
 */
import { beforeEach, describe, expect, it } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { stubLocalService } from '../testing/http';
import { daysUntil } from '../util/format';
import { useSettingsStore } from './settings';

const DAY = 86_400_000;
const BASE = Date.UTC(2026, 9, 8, 12, 0, 0);

/** 核心写出来的那一种 UTC 串（无毫秒）。 */
function isoAt(offsetMs: number): string {
  return new Date(BASE + offsetMs).toISOString().replace(/\.\d{3}Z$/, 'Z');
}

/** 让 `isoAt(x)` 相对**真实的现在**表示 x 毫秒之后（`daysUntil` 用的是 `Date.now()`）。 */
function isoFromNow(offsetMs: number): string {
  return new Date(Date.now() + offsetMs).toISOString().replace(/\.\d{3}Z$/, 'Z');
}

function payload(over: Record<string, unknown> = {}) {
  return {
    rows: [
      { sha256: 'a'.repeat(64), bytes: 100, localState: 'available', remoteState: 'present', refs: 2, quarantinedUntil: null },
      { sha256: 'b'.repeat(64), bytes: 48, localState: 'missing', remoteState: 'absent', refs: 1, quarantinedUntil: null },
    ],
    totals: { count: 2, bytes: 148, unavailableCount: 1, unavailableBytes: 48, quarantinedCount: 0, quarantinedBytes: 0 },
    ...over,
  };
}

beforeEach(() => {
  setActivePinia(createPinia());
});

describe('附件全表账本的读侧', () => {
  it('六个数原样搬过来：前端一个都不重算', async () => {
    stubLocalService({ attachment_inventory: () => payload() });
    const settings = useSettingsStore();
    await settings.loadAttachmentInventory();
    expect(settings.attachmentsFailed).toBe(false);
    expect(settings.attachmentInventory?.totals).toEqual({
      count: 2, bytes: 148, unavailableCount: 1, unavailableBytes: 48, quarantinedCount: 0, quarantinedBytes: 0,
    });
    expect(settings.attachmentInventory?.rows.length).toBe(2);
    expect(settings.quarantineDaysLeft, '没有隔离行就不许有倒计时').toBeNull();
  });

  it('倒计时取的是**最早**到期的那一格，不是数组里的第一条、也不是最后一条', async () => {
    stubLocalService({
      attachment_inventory: () => payload({
        rows: [
          { sha256: 'a'.repeat(64), bytes: 100, localState: 'missing', remoteState: 'present', refs: 0, quarantinedUntil: isoFromNow(9 * DAY) },
          { sha256: 'b'.repeat(64), bytes: 48, localState: 'missing', remoteState: 'present', refs: 0, quarantinedUntil: isoFromNow(2 * DAY) },
          // 第三个位置刻意放"中间值"：只有三条在，"取第一条"与"取最后一条"才会与"取最小"分得开
          // （两条时最后一条恰好就是最早的那一条，M1 那种写法照样绿 —— 这是我这条断言自己踩过的坑）。
          { sha256: 'c'.repeat(64), bytes: 8, localState: 'missing', remoteState: 'present', refs: 0, quarantinedUntil: isoFromNow(5 * DAY) },
        ],
        totals: { count: 3, bytes: 156, unavailableCount: 3, unavailableBytes: 156, quarantinedCount: 3, quarantinedBytes: 156 },
      }),
    });
    const settings = useSettingsStore();
    await settings.loadAttachmentInventory();
    expect(settings.quarantineDaysLeft, '三格都在隔离区时，界面要说最快到期的那一个').toBe(2);
  });

  it('认不出来的时间戳不许把整句倒计时拖没（也别说 NaN 天）', async () => {
    // 垃圾串排在前面的时候，"按字符串排序取第一条"会算出 null ⇒ 界面一句倒计时都没有，
    // 而账上明明还有一份说得清"两天之后"。天数是换算出来比大小的，不是排字符。
    stubLocalService({
      attachment_inventory: () => payload({
        rows: [
          { sha256: 'a'.repeat(64), bytes: 100, localState: 'missing', remoteState: 'present', refs: 0, quarantinedUntil: '不是时间' },
          { sha256: 'b'.repeat(64), bytes: 48, localState: 'missing', remoteState: 'present', refs: 0, quarantinedUntil: isoFromNow(2 * DAY) },
        ],
        totals: { count: 2, bytes: 148, unavailableCount: 2, unavailableBytes: 148, quarantinedCount: 2, quarantinedBytes: 148 },
      }),
    });
    const settings = useSettingsStore();
    await settings.loadAttachmentInventory();
    expect(settings.quarantineDaysLeft).toBe(2);
  });

  it('读失败 ⇒ 说"没能取到"，并把上一次那份清掉（留着就是拿旧账当现状）', async () => {
    stubLocalService({ attachment_inventory: () => payload() });
    const settings = useSettingsStore();
    await settings.loadAttachmentInventory();
    expect(settings.attachmentInventory?.totals.count).toBe(2);
    stubLocalService({}); // 下一发不认这条命令了（桥断了 / 核心没这条命令）
    await settings.loadAttachmentInventory();
    expect(settings.attachmentsFailed).toBe(true);
    expect(settings.attachmentInventory).toBeNull();
    expect(settings.quarantineDaysLeft).toBeNull();
  });

  it('载荷形状不认（totals 齐但 rows 不是数组）也不能当"0 份"显示', async () => {
    // 这一发故意让 totals 看着完整：只按 totals 判形状就会把一份**没有行**的账当成
    // "扫过、0 份"显示出去 —— 而它其实是读坏了。rows 那一半必须自己站得住。
    stubLocalService({ attachment_inventory: () => ({ rows: null, totals: payload().totals }) });
    const settings = useSettingsStore();
    await settings.loadAttachmentInventory();
    expect(settings.attachmentInventory).toBeNull();
    expect(settings.attachmentsFailed, '"扫过且为空"与"没读到"必须是两句话').toBe(true);

    // 同一份 totals，rows 是数组（哪怕为空）⇒ 那才是真的"0 份"，不是失败。
    stubLocalService({ attachment_inventory: () => ({ rows: [], totals: payload().totals }) });
    await settings.loadAttachmentInventory();
    expect(settings.attachmentInventory?.rows.length).toBe(0);
    expect(settings.attachmentsFailed, '空库不是失败：这一格要说"这台设备上还没有附件"').toBe(false);
  });

  it('daysUntil：过去的读 0、未来的向上取整、读不出来的给 null', () => {
    expect(daysUntil(isoAt(0), BASE)).toBe(0);
    expect(daysUntil(isoAt(-DAY), BASE), '早就到期了也只给 0 —— "还有负几天"不是一句话').toBe(0);
    expect(daysUntil(isoAt(Math.round(DAY * 1.2)), BASE)).toBe(2);
    expect(daysUntil(isoAt(29 * DAY), BASE)).toBe(29);
    expect(daysUntil('不是时间', BASE)).toBeNull();
    expect(daysUntil(null, BASE)).toBeNull();
  });
});
