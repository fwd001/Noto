/**
 * 块把手的纯逻辑测试：落点几何 + 重排不丢块。
 * 重排是少数能"改数据结构"的交互，所以 id 集合守恒这条比手感优先。
 */
import { describe, expect, it } from 'vitest';
import { destinationFor, gapAt } from './interaction';
import { insertBelow, moveBlock } from './commands';
import { blockText, ruleBlock, textBlock, type EditorBlock } from './model';

function para(text: string): EditorBlock {
  return textBlock('paragraph', [{ text }]);
}

/** 每块高 40，块间不留缝：0-40 / 40-80 / 80-120 */
const boxes = [{ top: 0, bottom: 40 }, { top: 40, bottom: 80 }, { top: 80, bottom: 120 }];

describe('落点几何', () => {
  it('指针在某个块的上半段 → 落到它前面', () => {
    expect(gapAt(boxes, 10)).toBe(0);
    expect(gapAt(boxes, 50)).toBe(1);
    expect(gapAt(boxes, 90)).toBe(2);
  });

  it('过了最后一块的中线 → 落到末尾缝', () => {
    expect(gapAt(boxes, 115)).toBe(3);
    expect(gapAt(boxes, 999)).toBe(3);
  });

  it('空文档只有 0 这一道缝', () => {
    expect(gapAt([], 500)).toBe(0);
  });

  it('往下拖时，抽掉自己会让下方缝号左移一格', () => {
    // 从 0 拖到"第 2 块之后"（缝 2）：它自己不在数组里了，落点是下标 1
    expect(destinationFor(2, 0)).toBe(1);
    expect(destinationFor(3, 1)).toBe(2);
    // 往上拖不偏移
    expect(destinationFor(0, 2)).toBe(0);
    expect(destinationFor(1, 2)).toBe(1);
    // 落在自己原来的缝 = 原地
    expect(destinationFor(1, 0)).toBe(0);
  });
});

describe('重排', () => {
  it('保序地把块搬到目标下标', () => {
    const [a, b, c] = [para('a'), para('b'), para('c')];
    const edit = moveBlock([a, b, c], 2, 0);
    expect(edit.blocks.map((block) => blockText(block))).toEqual(['c', 'a', 'b']);
    expect(edit.focusId).toBe(c.id);
  });

  it('id 集合守恒：既不丢块也不复制块', () => {
    const src = [para('a'), para('b'), ruleBlock(), para('c'), para('d')];
    const ids = src.map((block) => block.id);
    for (const from of [0, 1, 2, 3, 4]) {
      for (const to of [-3, 0, 1, 2, 3, 4, 9]) {
        const next = moveBlock(src, from, to).blocks;
        expect(next).toHaveLength(src.length);
        expect(next.map((block) => block.id).sort()).toEqual([...ids].sort());
      }
    }
  });

  it('越界的落点夹到两端而不是丢块', () => {
    const src = [para('a'), para('b'), para('c')];
    expect(moveBlock(src, 0, 99).blocks.map((b) => b.id)).toEqual([src[1]!.id, src[2]!.id, src[0]!.id]);
    expect(moveBlock(src, 2, -99).blocks.map((b) => b.id)).toEqual([src[2]!.id, src[0]!.id, src[1]!.id]);
  });

  it('不改入参：原数组仍是那份可回滚的旧数据', () => {
    const src = [para('a'), para('b')];
    const snapshot = src.map((block) => block.id);
    moveBlock(src, 0, 1);
    insertBelow(src, 0);
    expect(src.map((block) => block.id)).toEqual(snapshot);
    expect(src).toHaveLength(2);
  });

  it('非文本块也能搬：附件/分隔线不是段落，不该被跳过', () => {
    const rule = ruleBlock();
    const edit = moveBlock([para('a'), rule], 1, 0);
    expect(edit.blocks[0]?.id).toBe(rule.id);
    expect(edit.blocks[0]?.shape).toBe('rule');
  });

  it('"插入下方"新建空段并聚焦它，且不动原有块的内容', () => {
    const a = para('a');
    const edit = insertBelow([a], 0);
    expect(edit.blocks).toHaveLength(2);
    expect(blockText(edit.blocks[1]!)).toBe('');
    expect(edit.focusId).toBe(edit.blocks[1]!.id);
    expect(blockText(edit.blocks[0]!)).toBe('a');
  });

  it('在最后一块下方插入仍然追加到末尾', () => {
    const edit = insertBelow([para('a'), para('b')], 1);
    expect(edit.blocks).toHaveLength(3);
    expect(blockText(edit.blocks[2]!)).toBe('');
  });
});
