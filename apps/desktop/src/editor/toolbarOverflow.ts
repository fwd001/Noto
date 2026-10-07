/**
 * §3.4 的工具条溢出：「横向放不下时右缘 28px 渐隐，最右的「更多 ›」收纳溢出项」。
 *
 * 这里只有**那一次决策**：给定每个条目的实测宽度与可用宽度，哪些留在条上、哪些进「更多」。
 * 刻意做成纯函数 —— 组件里那圈 ResizeObserver / ref / 弹层坐标是容易出事的格子
 * （G29 弹层被裁、G64 弹层画出视口、G30 看不见的按钮），而"到底放不放得下"这件事
 * 必须能脱离浏览器逐档量。
 *
 * 两条不显然的规矩：
 * ① 一旦要出「更多」，就得**先给它留出宽度**再摆别的 —— 否则摆到刚好塞满，
 *    「更多」自己把最后那颗顶出视口，用户点了个空；
 * ② 再窄也要**在条上留至少一颗**。整条只剩一颗「更多」时，工具条读起来像"这个编辑器没有工具"；
 *    窄到连一颗都放不下是病态输入，不是这一格的判据，那种情况交给渐隐与横向可滚兜。
 */

export type ToolbarSlot = {
  key: string;
  /** 实测宽度（含它自己的内边距），px。 */
  width: number;
};

export type ToolbarPlan = {
  /** 留在条上的 key，按原顺序。 */
  visible: string[];
  /** 进「更多 ›」的 key，按原顺序。 */
  overflow: string[];
  /** 条上真有东西被收走 —— 界面据此画那 28px 渐隐。 */
  overflowing: boolean;
};

/** 1px 以内的抖动不算溢出：ResizeObserver 与缩放都会给出一小数的读数。 */
const TOLERANCE_PX = 1;

export function planToolbar(
  slots: readonly ToolbarSlot[],
  available: number,
  moreWidth: number,
  gap: number,
): ToolbarPlan {
  if (slots.length === 0) return { visible: [], overflow: [], overflowing: false };

  const totalOf = (items: readonly ToolbarSlot[]): number =>
    items.reduce((sum, item) => sum + item.width, 0) + gap * Math.max(0, items.length - 1);

  // 全都放得下（含"正好等于"）：不出「更多」。
  if (totalOf(slots) <= available + TOLERANCE_PX) {
    return { visible: slots.map((item) => item.key), overflow: [], overflowing: false };
  }

  // 需要「更多」：它的宽度 + 它前面那一格 gap 先从预算里扣掉（规矩①）。
  const budget = available - moreWidth - gap;
  const taken: ToolbarSlot[] = [];
  for (const item of slots) {
    const candidate = [...taken, item];
    if (taken.length === 0 || totalOf(candidate) <= budget + TOLERANCE_PX) taken.push(item);
    else break;
  }

  const takenKeys = new Set(taken.map((slot) => slot.key));
  return {
    visible: slots.filter((slot) => takenKeys.has(slot.key)).map((slot) => slot.key),
    overflow: slots.filter((slot) => !takenKeys.has(slot.key)).map((slot) => slot.key),
    overflowing: taken.length < slots.length,
  };
}
