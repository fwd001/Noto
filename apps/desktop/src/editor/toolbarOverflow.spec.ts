/**
 * §3.4 工具条溢出那一次决策的判据。
 * 每个用例都先算出"人眼看得懂的那个数"再写进断言，读数错了要能一眼看出错在哪。
 */
import { describe, expect, it } from 'vitest';
import { planToolbar, type ToolbarSlot } from './toolbarOverflow';

/** 十颗一样宽的按钮，方便手算。 */
function slots(count: number, width = 44): ToolbarSlot[] {
  return Array.from({ length: count }, (_, i) => ({ key: `i${i}`, width }));
}

const MORE = 64;
const GAP = 4;

describe('planToolbar（§3.4 工具条溢出）', () => {
  it('放得下就不出「更多」：10 颗 × 44 + 9 个 gap = 476 ≤ 500', () => {
    const plan = planToolbar(slots(10), 500, MORE, GAP);
    expect(plan.overflow).toEqual([]);
    expect(plan.overflowing).toBe(false);
    expect(plan.visible).toHaveLength(10);
  });

  it('正好等于可用宽度也不算溢出；短 2px 就要收（1px 容差之外不许自己加戏）', () => {
    // 5 颗 × 44 + 4 × 4 = 236
    expect(planToolbar(slots(5), 236, MORE, GAP).overflow).toEqual([]);
    // 容差是 1px：短 1px 仍算放得下，短 2px 才开始收。
    expect(planToolbar(slots(5), 235, MORE, GAP).overflow).toEqual([]);
    expect(planToolbar(slots(5), 234, MORE, GAP).overflow.length).toBeGreaterThan(0);
  });

  it('要收就先给「更多」留出宽度：不能摆到刚好塞满再让它顶掉最后一颗', () => {
    // 条上给 300。4 颗 = 44*4 + 3*4 = 188，加「更多」(4 + 64) = 256 ≤ 300；
    // 5 颗 = 236，加「更多」= 304 > 300 → 只能留 4 颗。
    // 不预留的话会留 6 颗（284 ≤ 300），那时「更多」自己就把第 6 颗顶出视口了。
    const plan = planToolbar(slots(10), 300, MORE, GAP);
    const widthOnBar = plan.visible.length * 44 + Math.max(0, plan.visible.length - 1) * GAP;
    expect(plan.visible.length).toBe(4);
    expect(widthOnBar + GAP + MORE).toBeLessThanOrEqual(300 + 1);
  });

  it('按原顺序从左边塞，收走的还是右边那一段', () => {
    const plan = planToolbar(slots(6, 100), 300, MORE, GAP);
    expect(plan.visible).toEqual(['i0', 'i1']);
    expect(plan.overflow).toEqual(['i2', 'i3', 'i4', 'i5']);
  });

  it('再窄也在条上留至少一颗：整条只剩一颗「更多」等于"这个编辑器没有工具"', () => {
    const plan = planToolbar(slots(4, 200), 60, MORE, GAP);
    expect(plan.visible).toEqual(['i0']);
    expect(plan.overflow).toEqual(['i1', 'i2', 'i3']);
    expect(plan.overflowing).toBe(true);
  });

  it('空表不炸（工具条在只读且没有可切换块型时可能就是空的）', () => {
    expect(planToolbar([], 400, MORE, GAP)).toEqual({ visible: [], overflow: [], overflowing: false });
  });

  it('溢出与不溢出两档的读数都不许丢条目：visible + overflow 始终是全集', () => {
    for (const width of [1000, 476, 300, 120, 40]) {
      const plan = planToolbar(slots(10), width, MORE, GAP);
      expect([...plan.visible, ...plan.overflow].sort()).toEqual(slots(10).map((s) => s.key).sort());
    }
  });

  it('真实那一档读数当基准：窄栏要收、宽栏一颗都不收', () => {
    // 用当前工具条的真实宽度量：4 颗标记 44 + 链接 84 + 块型 96 + 字号 84 + 颜色 84
    // + 五颗图标 44 + 撤销/重做 88 ≈ 832，再加 14 个 gap ≈ 888 的内容总长。
    const real: ToolbarSlot[] = [
      ...slots(4, 44),
      { key: 'link', width: 84 },
      { key: 'type', width: 96 },
      { key: 'size', width: 84 },
      { key: 'color', width: 84 },
      ...slots(5, 44),
      { key: 'undo', width: 44 },
      { key: 'redo', width: 44 },
    ];
    const total = real.reduce((sum, item) => sum + item.width, 0) + GAP * (real.length - 1);
    expect(total, '内容总长要在 800 上下，否则这组基准读数早就与界面分叉了').toBeGreaterThan(800);

    const at390 = planToolbar(real, 374, MORE, GAP);
    const atWide = planToolbar(real, total, MORE, GAP);
    expect(at390.overflowing, '390 那一档必须真的放不下（否则这格是空判据）').toBe(true);
    expect(atWide.overflowing, '刚好放得下时一颗都不许收').toBe(false);
  });
});
