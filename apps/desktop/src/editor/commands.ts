/**
 * 编辑器命令：对块数组的纯变换（不依赖 DOM），返回新数组 + 光标落点。
 * 所有函数都保留既有块 id，只为新建块分配 id。
 */
import type { Mark } from '../api/types';
import {
  blockText,
  emptyParagraph,
  inlineText,
  indentOf,
  insertTextAt,
  deleteRange,
  mergeBlocks,
  ruleBlock,
  setBlockType,
  setIndent,
  splitBlock,
  textBlock,
  toggleMark,
  type EditorBlock,
  type TextBlockType,
} from './model';

export interface BlockEdit {
  blocks: EditorBlock[];
  focusId: string | null;
  caret: number;
}

const LIST_TYPES: readonly string[] = ['orderedList', 'bulletList', 'checklistItem'];

export function blockAt(blocks: readonly EditorBlock[], index: number): EditorBlock | undefined {
  return blocks[index];
}

/** Enter：在当前偏移处切分（清单类型自动延续）。 */
export function splitAt(blocks: readonly EditorBlock[], index: number, offset: number): BlockEdit {
  const current = blocks[index];
  if (!current || current.shape !== 'text') return { blocks: [...blocks], focusId: current?.id ?? null, caret: offset };
  const [left, right] = splitBlock(current, offset);
  const next = [...blocks];
  next[index] = left;
  next.splice(index + 1, 0, right);
  return { blocks: next, focusId: right.id, caret: 0 };
}

/** Backspace 在块首：与上一块合并（保留上一块 id）。 */
export function mergeWithPrevious(blocks: readonly EditorBlock[], index: number): BlockEdit | null {
  if (index <= 0) return null;
  const current = blocks[index];
  const previous = blocks[index - 1];
  if (!current || !previous || previous.shape !== 'text' || current.shape !== 'text') return null;
  const caret = inlineText(previous.content).length;
  const merged = mergeBlocks(previous, current);
  const next = [...blocks];
  next[index - 1] = merged;
  next.splice(index, 1);
  if (next.length === 0) next.push(emptyParagraph());
  return { blocks: next, focusId: merged.id, caret };
}

/** 块内删除：有选区删选区；否则删前一个字符（块空了则整块删除）。 */
export function backspace(blocks: readonly EditorBlock[], index: number, range: { start: number; end: number }): BlockEdit {
  const current = blocks[index];
  if (!current || current.shape !== 'text') return { blocks: [...blocks], focusId: current?.id ?? null, caret: range.start };
  if (range.start !== range.end) {
    const content = deleteRange(current.content, range.start, range.end);
    const next = [...blocks];
    next[index] = { ...current, content };
    return { blocks: next, focusId: current.id, caret: Math.min(range.start, range.end) };
  }
  const text = inlineText(current.content);
  if (range.start > 0) {
    const content = deleteRange(current.content, range.start - 1, range.start);
    const next = [...blocks];
    next[index] = { ...current, content };
    return { blocks: next, focusId: current.id, caret: range.start - 1 };
  }
  if (text.length === 0) {
    const previous = blocks[index - 1];
    if (previous?.shape === 'text') {
      const merged = mergeWithPrevious(blocks, index);
      if (merged) return merged;
    }
    const next = blocks.filter((_, i) => i !== index);
    if (next.length === 0) next.push(emptyParagraph());
    const before = blocks[index - 1];
    return { blocks: next, focusId: before?.id ?? next[0]?.id ?? null, caret: before ? blockText(before).length : 0 };
  }
  return { blocks: [...blocks], focusId: current.id, caret: 0 };
}

export function insertText(blocks: readonly EditorBlock[], index: number, range: { start: number; end: number }, text: string): BlockEdit {
  const current = blocks[index];
  if (!current || current.shape !== 'text') return { blocks: [...blocks], focusId: current?.id ?? null, caret: range.start };
  const withoutSelection = range.start === range.end ? current.content : deleteRange(current.content, range.start, range.end);
  const at = Math.min(range.start, range.end);
  const parts = text.split('\n');
  let content = withoutSelection;
  let caret = at;
  parts.forEach((part, partIndex) => {
    if (partIndex > 0) {
      content = insertTextAt(content, caret, '\n');
      caret += 1;
    }
    if (part.length === 0) return;
    content = insertTextAt(content, caret, part);
    caret += part.length;
  });
  const next = [...blocks];
  next[index] = { ...current, content };
  return { blocks: next, focusId: current.id, caret };
}

export function applyMark(blocks: readonly EditorBlock[], index: number, range: { start: number; end: number }, mark: Mark, exclusive: readonly string[] = []): BlockEdit {
  const current = blocks[index];
  if (!current || current.shape !== 'text' || range.start === range.end) return { blocks: [...blocks], focusId: current?.id ?? null, caret: range.end };
  const content = toggleMark(current.content, range.start, range.end, mark, { exclusive });
  const next = [...blocks];
  next[index] = { ...current, content };
  return { blocks: next, focusId: current.id, caret: range.end };
}

export function changeType(blocks: readonly EditorBlock[], index: number, type: TextBlockType): BlockEdit {
  const current = blocks[index];
  if (!current || current.shape !== 'text') return { blocks: [...blocks], focusId: current?.id ?? null, caret: 0 };
  const next = [...blocks];
  next[index] = setBlockType(current, type);
  return { blocks: next, focusId: current.id, caret: inlineText(current.content).length };
}

/** Ctrl+Enter：在段落与清单之间切换；对连续的同类清单块一起切换。 */
export function cycleChecklist(blocks: readonly EditorBlock[], index: number): BlockEdit {
  const current = blocks[index];
  if (!current || current.shape !== 'text') return { blocks: [...blocks], focusId: current?.id ?? null, caret: 0 };
  const target: TextBlockType = current.type === 'checklistItem' ? 'paragraph' : 'checklistItem';
  const next = [...blocks];
  next[index] = setBlockType(current, target);
  return { blocks: next, focusId: current.id, caret: inlineText(current.content).length };
}

export function setChecked(blocks: readonly EditorBlock[], index: number, checked: boolean): BlockEdit {
  const current = blocks[index];
  if (!current || current.type !== 'checklistItem') return { blocks: [...blocks], focusId: current?.id ?? null, caret: 0 };
  const next = [...blocks];
  next[index] = { ...current, attrs: { ...current.attrs, checked } };
  return { blocks: next, focusId: current.id, caret: 0 };
}

export function shiftIndent(blocks: readonly EditorBlock[], index: number, delta: number): BlockEdit {
  const current = blocks[index];
  if (!current || current.shape !== 'text') return { blocks: [...blocks], focusId: current?.id ?? null, caret: 0 };
  const next = [...blocks];
  next[index] = setIndent(current, indentOf(current) + delta);
  return { blocks: next, focusId: current.id, caret: inlineText(current.content).length };
}

export function insertRule(blocks: readonly EditorBlock[], index: number): BlockEdit {
  const rule = ruleBlock();
  const next = [...blocks];
  const anchor = blocks[index + 1];
  if (anchor && anchor.shape === 'text' && inlineText(anchor.content).length === 0) next.splice(index + 1, 0, rule);
  else next.splice(index + 1, 0, rule, textBlock('paragraph'));
  return { blocks: next, focusId: rule.id, caret: 0 };
}

export function removeBlockAt(blocks: readonly EditorBlock[], index: number): BlockEdit {
  if (index < 0 || index >= blocks.length) return { blocks: [...blocks], focusId: blocks[0]?.id ?? null, caret: 0 };
  const next = blocks.filter((_, i) => i !== index);
  if (next.length === 0) next.push(emptyParagraph());
  const focus = next[Math.min(index, next.length - 1)];
  return { blocks: next, focusId: focus?.id ?? null, caret: focus ? blockText(focus).length : 0 };
}

export function moveBlock(blocks: readonly EditorBlock[], index: number, delta: number): BlockEdit {
  const target = index + delta;
  if (target < 0 || target >= blocks.length) return { blocks: [...blocks], focusId: blocks[index]?.id ?? null, caret: 0 };
  const next = [...blocks];
  const [moved] = next.splice(index, 1);
  next.splice(target, 0, moved ?? blocks[index] as EditorBlock);
  return { blocks: next, focusId: blocks[index]?.id ?? null, caret: 0 };
}

/** 编号列表的可见序号：按"同缩进连续同类型"分组计数。 */
export function orderedNumbers(blocks: readonly EditorBlock[]): number[] {
  const out: number[] = [];
  let counter = 0;
  let previousType: string | null = null;
  let previousIndent = 0;
  for (const block of blocks) {
    const isOrdered = block.type === 'orderedList';
    const indent = indentOf(block);
    if (!isOrdered) {
      out.push(0);
      if (block.type !== 'codeBlock') {
        previousType = null;
        counter = 0;
      }
      continue;
    }
    if (previousType !== 'orderedList' || indent !== previousIndent) counter = 0;
    counter += 1;
    previousType = 'orderedList';
    previousIndent = indent;
    out.push(counter);
  }
  return out;
}

export function isListType(block: EditorBlock): boolean {
  return LIST_TYPES.includes(block.type);
}
