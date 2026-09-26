/**
 * 块把手的落点几何：纯计算，不碰 DOM —— 用假 rect 就能断言"过了中线才算落到下一缝"。
 */
export interface BlockBox {
  top: number;
  bottom: number;
}

/** 指针落在哪道插入缝：0 = 第一个块之前，n = 最后一个块之后。以块中线为界。 */
export function gapAt(boxes: readonly BlockBox[], clientY: number): number {
  for (let index = 0; index < boxes.length; index += 1) {
    const box = boxes[index];
    if (!box) continue;
    if (clientY < (box.top + box.bottom) / 2) return index;
  }
  return boxes.length;
}

/** 缝号 → 落点下标：被拖的块从数组抽掉后，它下方的每一道缝都左移一格。 */
export function destinationFor(gap: number, from: number): number {
  return gap > from ? gap - 1 : gap;
}
