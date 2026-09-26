/**
 * 浮动选区工具条的定位：纯计算，不碰 DOM。
 * 坐标一律按视口算（组件里用 position: fixed 摆放），这样滚动容器内外的
 * 偏移、缩放、缩进都不用特判。
 */
export interface Box {
  top: number;
  bottom: number;
  left: number;
  right: number;
}

export interface Size {
  width: number;
  height: number;
}

export interface Placement {
  top: number;
  left: number;
}

const GAP = 8;

/** 默认浮在选区上方；上方放不下才翻到下方。水平居中于选区，但不越出可视区。 */
export function placeBar(sel: Box, bar: Size, view: Box): Placement {
  const above = sel.top - GAP - bar.height;
  const top = above >= view.top ? above : sel.bottom + GAP;
  const centered = (sel.left + sel.right) / 2 - bar.width / 2;
  const minLeft = view.left + GAP;
  const maxLeft = Math.max(minLeft, view.right - bar.width - GAP);
  return { top, left: Math.min(Math.max(centered, minLeft), maxLeft) };
}
