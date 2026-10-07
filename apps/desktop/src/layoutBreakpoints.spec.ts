/**
 * §3.1 的那几个数以前**一个都没钉**：`layoutFor` 有实现、`--sidebar-w/--list-w` 在 tokens 里，
 * 但把 1180 改成 1200、把侧栏改成 240，全仓不会红 —— 而 §3.1 是"三栏/两栏/单栏"这一整套
 * 布局承诺的地基（第 3 刀那次量的都是"某一档下长什么样"，没有一条量"档在哪儿分"）。
 *
 * 这一份钉三件事：① 边界值本身；② 边界上那一像素归哪一档（`≥` 与 `<` 写反是这一族最常见的错）；
 * ③ CSS 里出现的宽度断点必须是**已登记的那几个**（新增一个魔数断点要先过这里）。
 */
import { describe, expect, it } from 'vitest';
import { THREE_PANE_MIN, TWO_PANE_MIN, layoutFor } from './stores/shell';

const VUES = import.meta.glob<string>('../**/*.vue', {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>;

const CSS = import.meta.glob<string>('./styles/*.css', {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>;

describe('§3.1 断点', () => {
  it('两个边界值就是规范写的那两个数（改数要先改规范）', () => {
    expect(THREE_PANE_MIN).toBe(1180);
    expect(TWO_PANE_MIN).toBe(820);
  });

  it('边界那一像素归上一档：1180 与 820 各自是"三栏"和"两栏"的第一像素', () => {
    expect(layoutFor(1180)).toBe('three');
    expect(layoutFor(1179)).toBe('two');
    expect(layoutFor(820)).toBe('two');
    expect(layoutFor(819)).toBe('one');
  });

  it('档内任意宽度不会跳档（只有边界能换档）', () => {
    for (const w of [390, 520, 700, 819]) expect(layoutFor(w)).toBe('one');
    for (const w of [821, 900, 1100, 1179]) expect(layoutFor(w)).toBe('two');
    for (const w of [1181, 1440, 1800, 2560]) expect(layoutFor(w)).toBe('three');
  });
});

describe('§3.1 栏宽', () => {
  it('侧栏 260 / 列表 340 写在 tokens 里，且真的读到了值（读不到就是正则坏了）', () => {
    const tokens = Object.values(CSS).join('\n');
    const side = /--sidebar-w:\s*(\d+)px/.exec(tokens);
    const list = /--list-w:\s*(\d+)px/.exec(tokens);
    expect(side, '没找到 --sidebar-w，这条判据在空转').not.toBeNull();
    expect(list, '没找到 --list-w，这条判据在空转').not.toBeNull();
    expect(Number(side?.[1])).toBe(260);
    expect(Number(list?.[1])).toBe(340);
  });
});

/**
 * 宽度媒体查询的漂移守卫。为什么不是"只许用 §3.1 那两个数"：
 * `ConflictsView` 的 560 是**分歧卡那一格自己的**换列点，不是壳的断点（壳在 820 才单栏），
 * 那一档在 §3.1 之外，但它是登记过的。所以这里判的是"没登记的不许悄悄进来"。
 */
describe('§3.1 之外不许再有宽度断点', () => {
  const REGISTERED = new Set([560, 820, 1180]);
  const found: Array<{ file: string; px: number; kind: string }> = [];

  for (const [file, raw] of Object.entries({ ...VUES, ...CSS })) {
    for (const m of raw.matchAll(/@media[^{]*?\((min-width|max-width):\s*(\d+)px\)/g)) {
      found.push({ file: file.replace(/^\.\.\//, '').replace(/^\.\//, ''), px: Number(m[2]), kind: m[1] ?? '' });
    }
  }

  it('扫描真的覆盖到了这些查询（扫到 0 条等于没检查）', () => {
    expect(found.length).toBeGreaterThanOrEqual(3);
  });

  it('每一个宽度断点都在登记表里', () => {
    const rogue = found.filter((f) => !REGISTERED.has(f.px));
    expect(rogue, `未登记的宽度断点：${JSON.stringify(rogue)}`).toEqual([]);
  });

  /**
   * 已知的那一格（登记为 G90，不当已完成）：`max-width: 820px` 与 JS 的 `>= 820`
   * 压在同一个数上 —— 宽度**正好** 820 时壳是两栏，而分歧那一格按"更窄的那档"排。
   * 这一条不判红（差异只有 1px 宽的一带，且不撒谎），但把它写成断言，
   * 免得下一次有人以为这两个 820 是巧合。
   */
  it('820 那一格是"已知重叠"，不是笔误（改了要同步改这条）', () => {
    const atBoundary = found.filter((f) => f.px === 820);
    expect(atBoundary.map((f) => f.kind).sort()).toEqual(['max-width']);
  });
});
