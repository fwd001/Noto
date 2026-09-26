/**
 * §5 界面表达的契约：位图 → 芯片、策略 → 该说哪句话。
 * 最要紧的一条是"没探过"和"探过但不支持"必须是两种说法 —— 后者要建议串行编辑，
 * 前者不该吓唬用户。
 */
import { describe, expect, it } from 'vitest';
import { CAP_BITS, capChips, capsState } from './serverCaps';

describe('capChips', () => {
  it('没探过时一个芯片都不出，而不是把全部标成 ✕', () => {
    expect(capChips(undefined)).toEqual([]);
    expect(capChips(null)).toEqual([]);
  });

  it('capMask=0（探过了，全不支持）要出芯片且全是 ✕，不能等同于没探过', () => {
    const chips = capChips(0);
    expect(chips).toHaveLength(5);
    expect(chips.every((c) => !c.on)).toBe(true);
  });

  it('位值与核心的 Caps 一致，且顺序按重要度排', () => {
    const chips = capChips(CAP_BITS.conditionalPut | CAP_BITS.strongEtag);
    expect(chips.map((c) => c.name)).toEqual(['conditionalPut', 'overwriteFMove', 'strongEtag', 'depthInfinity', 'range']);
    expect(chips.filter((c) => c.on).map((c) => c.name)).toEqual(['conditionalPut', 'strongEtag']);
    expect(chips.find((c) => c.name === 'overwriteFMove')?.on).toBe(false);
  });

  it('每一位都有登记过的文案键（漏一个界面上就是原始键名）', () => {
    for (const chip of capChips(CAP_BITS.strongEtag)) {
      expect(chip.labelKey).toMatch(/^sync\.cap\.[A-Za-z]+$/);
    }
  });

  it('核心以后加的新位被忽略，不会显示成"不支持"', () => {
    const chips = capChips(1 << 20);
    expect(chips.every((c) => !c.on)).toBe(true);
    expect(chips).toHaveLength(5);
  });
});

describe('capsState', () => {
  it('capMask 缺失 = 还没探过（不是"不支持"）', () => {
    expect(capsState({})).toBe('unknown');
    expect(capsState(null)).toBe('unknown');
    // 核心的 Option<u32> 序列化成 JSON null，这才是"刚配上"的真实线格式
    expect(capsState({ capMask: null, writeStrategy: null })).toBe('unknown');
    expect(capsState({ capMask: 0 })).not.toBe('unknown');
  });

  it('S3 才是不安全；S1/S2 都算有并发保护', () => {
    expect(capsState({ capMask: 0, writeStrategy: 'S3' })).toBe('unprotected');
    expect(capsState({ capMask: 2, writeStrategy: 'S1' })).toBe('protected');
    expect(capsState({ capMask: 4, writeStrategy: 'S2' })).toBe('protected');
  });

  it('策略由核心下发，前端不许自己从位图反推', () => {
    // 位图看起来"有条件写"但核心说 S3 —— 听核心的，因为判定表只该有一份
    expect(capsState({ capMask: CAP_BITS.conditionalPut, writeStrategy: 'S3' })).toBe('unprotected');
  });
});
