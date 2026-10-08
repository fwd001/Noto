/**
 * §6「版本历史」的 diff（缺口 G100 的后一半）：**按块比，不按字符比**。
 *
 * 为什么是块：本仓的顶层块有稳定 `id`（ADR-0008），它就是三方合并与冲突判定用的锚。
 * 拿它当 diff 的锚，界面上说的"这一段两边不一样"与核心合并时认的是同一件事；
 * 换成字符级 diff（Myers 那一类），段落一漂移就会把"两边各写不同段"判成一团乱改 ——
 * 那条路 ADR-0007 已经否过一次了，这里不再从界面侧绕回来。
 *
 * 四种状态各自一句话，**"字一样但格式不同"不能算一样**：加粗、标题级别、勾选这些
 * 变了而文字没变，用户看到的差别是真实的，说"这一版与现在相同"就是假话。
 */
import type { NoteDoc } from '../api/types';

export type BlockMark = 'same' | 'changed' | 'restyled' | 'removed';

export interface DocDiff {
  /** 旧版每一块相对**现在**的状态（键是块 id）。 */
  marks: Record<string, BlockMark>;
  /** 旧版有、现在没有的段数。 */
  removed: number;
  /** 两边文字不同的段数。 */
  changed: number;
  /** 字一样、格式不同的段数。 */
  restyled: number;
  /** 现在是后来新增的段数（旧版里没有这一位）。 */
  added: number;
}

function inlineText(block: unknown): string {
  const parts = (block as { content?: unknown } | null)?.content;
  if (!Array.isArray(parts)) return '';
  return parts
    .map((p) => (p as { text?: unknown })?.text)
    .filter((t): t is string => typeof t === 'string')
    .join('');
}

/**
 * 块的"内容签名"：去掉 id、去掉**结构噪声**之后的全部结构。
 *
 * 为什么必须去噪：等号两边不是同一份序列化 —— 一边是核心库里存的 JSON、一边是编辑器
 * 当场由块模型拼出来的 `currentDoc()`，后者会给每块带上 `attrs: {}`、给 inline 节点带上
 * `marks: []`。不剥掉这些空值，"覆盖之后重开那一版"会被说成"三段只是格式不同"
 * （腿 51 就是这么红的）—— 而那句界面话是**假话**：用户看到的字与格式都没变。
 * 只有**非空**的 attrs / marks 才算"格式不同"。
 */
function stripEmpty(value: unknown): unknown {
  if (Array.isArray(value)) {
    return value.map(stripEmpty).filter((v) => !isEmptyValue(v));
  }
  if (value && typeof value === 'object') {
    const out: Record<string, unknown> = {};
    for (const [k, v] of Object.entries(value as Record<string, unknown>)) {
      if (k === 'id') continue;
      if (isEmptyValue(v)) continue;
      out[k] = stripEmpty(v);
    }
    return out;
  }
  return value;
}

function isEmptyValue(v: unknown): boolean {
  if (v === null || v === undefined) return true;
  if (typeof v === 'string') return v === '';
  if (Array.isArray(v)) return v.length === 0;
  if (typeof v === 'object') return Object.keys(v as object).length === 0;
  return false;
}

function blockSignature(block: unknown): string {
  if (!block || typeof block !== 'object') return String(block);
  return JSON.stringify(stripEmpty(block));
}

function blocksOf(doc: NoteDoc | null | undefined): Array<Record<string, unknown>> {
  const content = (doc as { content?: unknown } | null)?.content;
  return Array.isArray(content) ? (content as Array<Record<string, unknown>>) : [];
}

export function diffDocs(oldDoc: NoteDoc | null | undefined, nowDoc: NoteDoc | null | undefined): DocDiff {
  const nowById = new Map<string, Record<string, unknown>>();
  for (const b of blocksOf(nowDoc)) {
    const id = typeof b.id === 'string' ? b.id : '';
    if (id) nowById.set(id, b);
  }
  const marks: Record<string, BlockMark> = {};
  let removed = 0;
  let changed = 0;
  let restyled = 0;
  for (const b of blocksOf(oldDoc)) {
    const id = typeof b.id === 'string' ? b.id : '';
    if (!id) continue;
    const now = nowById.get(id);
    let mark: BlockMark;
    if (!now) {
      mark = 'removed';
      removed += 1;
    } else if (inlineText(b) !== inlineText(now)) {
      mark = 'changed';
      changed += 1;
    } else if (blockSignature(b) !== blockSignature(now)) {
      mark = 'restyled';
      restyled += 1;
    } else {
      mark = 'same';
    }
    marks[id] = mark;
  }
  const oldIds = new Set(Object.keys(marks));
  let added = 0;
  for (const id of nowById.keys()) {
    if (!oldIds.has(id)) added += 1;
  }
  return { marks, removed, changed, restyled, added };
}
