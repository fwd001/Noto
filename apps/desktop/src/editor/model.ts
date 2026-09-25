/**
 * 编辑器模型：契约里的 Document{v,content:[Block]} 与 UI 内部块列表的双向映射。
 * 这里是纯函数（不碰 DOM、不碰网络），三条硬规则：
 *  1. 每个顶层块有稳定 id（8 位 base32）；编辑既有块必须保留其 id（块级合并靠它）。
 *  2. 未知块 / 未知字段 / 未知属性一律原样保留，绝不丢弃。
 *  3. doc.v 超出本客户端支持版本 → 只读闸门。
 */
import type { Block, Inline, Mark, NoteDoc } from '../api/types';

export const SUPPORTED_DOC_VERSION = 1;

export const TEXT_BLOCK_TYPES = [
  'paragraph',
  'heading',
  'blockquote',
  'codeBlock',
  'orderedList',
  'bulletList',
  'checklistItem',
] as const;

export const ATOMIC_BLOCK_TYPES = ['image', 'attachment', 'horizontalRule'] as const;

export type TextBlockType = (typeof TEXT_BLOCK_TYPES)[number];
export type BlockShape = 'text' | 'image' | 'attachment' | 'rule' | 'unknown';

export interface EditorBlock {
  id: string;
  type: string;
  shape: BlockShape;
  /** 完整属性表：未知键同样保留在里这里。 */
  attrs: Record<string, unknown>;
  content: Inline[];
  /** 除 id/type/attrs/content/unknown 之外的顶层字段（前向兼容）。 */
  rest: Record<string, unknown>;
  /** shape === 'unknown' 时的原始块 JSON，写回时一字不改。 */
  raw: Block | null;
}

const BASE32_ALPHABET = '0123456789abcdefghjkmnpqrstvwxyz';
const ID_LENGTH = 8;
const ID_PATTERN = /^[0-9a-z]{4,32}$/;

function randomBytes(count: number): Uint8Array {
  const bytes = new Uint8Array(count);
  const cryptoObject = typeof globalThis.crypto === 'undefined' ? undefined : globalThis.crypto;
  if (cryptoObject && typeof cryptoObject.getRandomValues === 'function') {
    cryptoObject.getRandomValues(bytes);
    return bytes;
  }
  for (let i = 0; i < count; i += 1) bytes[i] = Math.floor(Math.random() * 256);
  return bytes;
}

/** 稳定块 id：8 位 base32（Crockford 字母表，无 i/l/o/u）。 */
export function newBlockId(): string {
  const bytes = randomBytes(ID_LENGTH);
  let out = '';
  for (let i = 0; i < ID_LENGTH; i += 1) {
    out += BASE32_ALPHABET[(bytes[i] ?? 0) & 31] ?? '0';
  }
  return out;
}

export function isValidBlockId(id: string): boolean {
  return ID_PATTERN.test(id);
}

/** 未知块判定：type 以 unknown: 开头、不在表内，或后端标了 unknown。 */
export function isUnknownBlock(block: Block): boolean {
  if (block.unknown === true) return true;
  const type = typeof block.type === 'string' ? block.type : '';
  if (type.startsWith('unknown:') || type.startsWith('unknown:')) return true;
  return !(TEXT_BLOCK_TYPES as readonly string[]).includes(type) && !(ATOMIC_BLOCK_TYPES as readonly string[]).includes(type);
}

function shapeFor(block: Block): BlockShape {
  if (isUnknownBlock(block)) return 'unknown';
  switch (block.type) {
    case 'image':
      return 'image';
    case 'attachment':
      return 'attachment';
    case 'horizontalRule':
      return 'rule';
    default:
      return 'text';
  }
}

const RESERVED_KEYS = ['id', 'type', 'attrs', 'content', 'unknown'];

function splitRest(block: Block): Record<string, unknown> {
  const rest: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(block)) {
    if (!RESERVED_KEYS.includes(key)) rest[key] = value;
  }
  return rest;
}

function inlinesOf(block: Block): Inline[] {
  const content = Array.isArray(block.content) ? block.content : [];
  return content
    .filter((item): item is Inline => typeof item === 'object' && item !== null && typeof item.text === 'string')
    .map((item) => ({
      text: item.text,
      marks: Array.isArray(item.marks) ? item.marks.filter((m): m is Mark => typeof m === 'object' && m !== null && typeof m.kind === 'string') : undefined,
    }));
}

/** Document → 编辑器块列表（缺 id 的块补 id，其余原样保留）。 */
export function docToBlocks(doc: NoteDoc | null | undefined): EditorBlock[] {
  const content = Array.isArray(doc?.content) ? doc.content : [];
  const seen = new Set<string>();
  const blocks: EditorBlock[] = [];
  for (const block of content) {
    if (typeof block !== 'object' || block === null) continue;
    let id = typeof block.id === 'string' && isValidBlockId(block.id) ? block.id : newBlockId();
    while (seen.has(id)) id = newBlockId();
    seen.add(id);
    const shape = shapeFor(block);
    blocks.push({
      id,
      type: typeof block.type === 'string' ? block.type : 'paragraph',
      shape,
      attrs: typeof block.attrs === 'object' && block.attrs !== null ? { ...block.attrs } : {},
      content: inlinesOf(block),
      rest: splitRest(block),
      raw: shape === 'unknown' ? block : null,
    });
  }
  if (blocks.length === 0) blocks.push(emptyParagraph());
  return blocks;
}

function attrString(attrs: Record<string, unknown>, key: string): string | undefined {
  const value = attrs[key];
  return typeof value === 'string' ? value : undefined;
}

function attrNumber(attrs: Record<string, unknown>, key: string): number | undefined {
  const value = attrs[key];
  return typeof value === 'number' && Number.isFinite(value) ? value : undefined;
}

function attrBoolean(attrs: Record<string, unknown>, key: string): boolean | undefined {
  const value = attrs[key];
  return typeof value === 'boolean' ? value : undefined;
}

/** 编辑器块列表 → Document（未知块与未知字段一字不改地写回）。 */
export function blocksToDoc(blocks: readonly EditorBlock[], version = SUPPORTED_DOC_VERSION): NoteDoc {
  const content: Block[] = [];
  for (const block of blocks) {
    if (block.shape === 'unknown' && block.raw !== null) {
      content.push(block.raw);
      continue;
    }
    const attrs: Record<string, unknown> = { ...block.attrs };
    for (const key of ['level', 'indent', 'checked', 'lang', 'align']) {
      if (attrs[key] === undefined) continue;
      const value = attrs[key];
      if (typeof value === 'number' && Number.isInteger(value)) attrs[key] = value;
      else if (typeof value === 'boolean') attrs[key] = value;
      else if (typeof value === 'string') attrs[key] = value;
      else delete attrs[key];
    }
    const out: Block = { id: block.id, type: block.type, ...block.rest };
    if (Object.keys(attrs).length > 0) out.attrs = attrs;
    const keep = block.shape === 'text' ? normalizeInlines(block.content) : block.content;
    if (keep.length > 0) out.content = keep;
    content.push(out);
  }
  return { v: version, content };
}

/* ---------------------------------------------------------------- 块工厂 */

export function textBlock(type: TextBlockType, content: Inline[] = [], attrs: Record<string, unknown> = {}): EditorBlock {
  const shapeAttrs: Record<string, unknown> = { ...attrs };
  if (type === 'heading' && attrNumber(shapeAttrs, 'level') === undefined) shapeAttrs.level = 1;
  if (type === 'checklistItem' && attrBoolean(shapeAttrs, 'checked') === undefined) shapeAttrs.checked = false;
  if (attrNumber(shapeAttrs, 'indent') === undefined) shapeAttrs.indent = 0;
  return {
    id: newBlockId(),
    type,
    shape: 'text',
    attrs: shapeAttrs,
    content: normalizeInlines(content),
    rest: {},
    raw: null,
  };
}

export function emptyParagraph(content: Inline[] = []): EditorBlock {
  return textBlock('paragraph', content);
}

export function ruleBlock(): EditorBlock {
  return { id: newBlockId(), type: 'horizontalRule', shape: 'rule', attrs: {}, content: [], rest: {}, raw: null };
}

export function attachmentBlock(attrs: Record<string, unknown>): EditorBlock {
  return { id: newBlockId(), type: 'attachment', shape: 'attachment', attrs: { ...attrs }, content: [], rest: {}, raw: null };
}

export function imageBlock(attrs: Record<string, unknown>): EditorBlock {
  return { id: newBlockId(), type: 'image', shape: 'image', attrs: { ...attrs }, content: [], rest: {}, raw: null };
}

export function emptyDoc(): NoteDoc {
  return { v: SUPPORTED_DOC_VERSION, content: [{ id: newBlockId(), type: 'paragraph', content: [] }] };
}

/* ---------------------------------------------------------------- 行内文本 */

export function inlineText(inlines: readonly Inline[]): string {
  let out = '';
  for (const inline of inlines) out += inline.text;
  return out;
}

export function blockText(block: EditorBlock): string {
  if (block.shape !== 'text') return inlineText(block.content);
  return inlineText(block.content);
}

function sameMarks(a: readonly Mark[] | undefined, b: readonly Mark[] | undefined): boolean {
  const left = a ?? [];
  const right = b ?? [];
  if (left.length !== right.length) return false;
  for (let i = 0; i < left.length; i += 1) {
    const l = left[i];
    const r = right[i];
    if (!l || !r) return false;
    if (l.kind !== r.kind) return false;
    if (JSON.stringify(l.attrs ?? null) !== JSON.stringify(r.attrs ?? null)) return false;
  }
  return true;
}

/** 合并相邻同行内样式的片段、丢掉空片段（与后端 normalize 同向，不改变语义）。 */
export function normalizeInlines(inlines: readonly Inline[]): Inline[] {
  const out: Inline[] = [];
  for (const inline of inlines) {
    if (typeof inline.text !== 'string' || inline.text.length === 0) continue;
    const marks = inline.marks && inline.marks.length > 0 ? inline.marks.map((m) => ({ kind: m.kind, ...(m.attrs ? { attrs: { ...m.attrs } } : {}) })) : undefined;
    const last = out[out.length - 1];
    if (last && sameMarks(last.marks, marks)) {
      last.text += inline.text;
      continue;
    }
    out.push({ text: inline.text, ...(marks ? { marks } : {}) });
  }
  return out;
}

/** 在文本偏移处切分行内容（保留各片段的样式）。 */
export function sliceInlines(inlines: readonly Inline[], start: number, end = inlineText(inlines).length): Inline[] {
  const from = Math.max(0, Math.min(start, end));
  const to = Math.max(0, end);
  const out: Inline[] = [];
  let cursor = 0;
  for (const inline of inlines) {
    const inlineStart = cursor;
    const inlineEnd = cursor + inline.text.length;
    cursor = inlineEnd;
    if (inlineEnd <= from || inlineStart >= to) continue;
    const cutFrom = Math.max(0, from - inlineStart);
    const cutTo = Math.min(inline.text.length, to - inlineStart);
    if (cutFrom >= cutTo) continue;
    out.push({ text: inline.text.slice(cutFrom, cutTo), ...(inline.marks && inline.marks.length > 0 ? { marks: inline.marks.map((m) => ({ kind: m.kind, ...(m.attrs ? { attrs: { ...m.attrs } } : {}) })) } : {}) });
  }
  return out;
}

export function insertTextAt(inlines: readonly Inline[], offset: number, text: string): Inline[] {
  const before = sliceInlines(inlines, 0, offset);
  const after = sliceInlines(inlines, offset);
  const anchor = before[before.length - 1];
  const marks = anchor?.marks ? anchor.marks.map((m) => ({ kind: m.kind, ...(m.attrs ? { attrs: { ...m.attrs } } : {}) })) : undefined;
  const inserted: Inline = { text, ...(marks ? { marks } : {}) };
  return normalizeInlines([...before, inserted, ...after]);
}

export function deleteRange(inlines: readonly Inline[], start: number, end: number): Inline[] {
  const from = Math.min(start, end);
  const to = Math.max(start, end);
  return normalizeInlines([...sliceInlines(inlines, 0, from), ...sliceInlines(inlines, to)]);
}

function cloneMark(mark: Mark): Mark {
  return { kind: mark.kind, ...(mark.attrs ? { attrs: { ...mark.attrs } } : {}) };
}

/**
 * 对 [start,end) 应用/取消一种样式。
 * 取消=从该区间剥离；应用=补上（同 kind 同 attrs 视为已存在）。
 */
export function toggleMark(
  inlines: readonly Inline[],
  start: number,
  end: number,
  mark: Mark,
  options: { exclusive?: readonly string[] } = {},
): Inline[] {
  const from = Math.min(start, end);
  const to = Math.max(start, end);
  if (to <= from) return normalizeInlines(inlines);
  const exclusive = options.exclusive ?? [];

  // 互斥样式（如换链接地址）先剥离，再按"是否已具备该样式"决定加还是去。
  const working: Inline[] = inlines.map((inline) => {
    const kept = (inline.marks ?? []).map(cloneMark).filter((m) => !exclusive.some((kind) => kind === m.kind));
    return { text: inline.text, ...(kept.length > 0 ? { marks: kept } : {}) };
  });

  // 先量一下区间内的覆盖情况：整段都已有该样式 → 取消；否则 → 应用。
  let covered = 0;
  let coveredWithMark = 0;
  let cursor = 0;
  for (const inline of working) {
    const inlineStart = cursor;
    const inlineEnd = cursor + inline.text.length;
    cursor = inlineEnd;
    const overlap = Math.max(0, Math.min(inlineEnd, to) - Math.max(inlineStart, from));
    if (overlap === 0) continue;
    covered += overlap;
    if ((inline.marks ?? []).some((m) => m.kind === mark.kind)) coveredWithMark += overlap;
  }
  const shouldRemove = covered > 0 && covered === coveredWithMark;

  const out: Inline[] = [];
  cursor = 0;
  for (const inline of working) {
    const inlineStart = cursor;
    const inlineEnd = cursor + inline.text.length;
    cursor = inlineEnd;
    const original = (inline.marks ?? []).map(cloneMark);
    const overlapFrom = Math.max(0, from - inlineStart);
    const overlapTo = Math.min(inline.text.length, to - inlineStart);
    if (overlapTo <= overlapFrom) {
      out.push({ text: inline.text, ...(original.length > 0 ? { marks: original } : {}) });
      continue;
    }
    const existing = original.find((m) => m.kind === mark.kind);
    const bodyMarks = shouldRemove
      ? original.filter((m) => m.kind !== mark.kind)
      : [...original.filter((m) => m.kind !== mark.kind), cloneMark(mark)];
    const outside = existing ? original.filter((m) => m !== existing) : original;
    const head = inline.text.slice(0, overlapFrom);
    const body = inline.text.slice(overlapFrom, overlapTo);
    const tail = inline.text.slice(overlapTo);
    if (head.length > 0) out.push({ text: head, ...(outside.length > 0 ? { marks: outside } : {}) });
    if (body.length > 0) out.push({ text: body, ...(bodyMarks.length > 0 ? { marks: bodyMarks } : {}) });
    if (tail.length > 0) out.push({ text: tail, ...(outside.length > 0 ? { marks: outside } : {}) });
  }
  return normalizeInlines(out);
}

/** 该块是否含有某种样式（工具条高亮用）。 */
export function hasMarkInRange(inlines: readonly Inline[], start: number, end: number, kind: string): boolean {
  let cursor = 0;
  const from = Math.min(start, end);
  const to = Math.max(start, end);
  for (const inline of inlines) {
    const inlineStart = cursor;
    const inlineEnd = cursor + inline.text.length;
    cursor = inlineEnd;
    if (Math.max(inlineStart, from) >= Math.min(inlineEnd, to)) continue;
    if ((inline.marks ?? []).some((m) => m.kind === kind)) return true;
  }
  return false;
}

/** 光标处的样式集合（工具条高亮用）。 */
export function marksAtOffset(inlines: readonly Inline[], offset: number): Mark[] {
  let cursor = 0;
  for (const inline of inlines) {
    const inlineEnd = cursor + inline.text.length;
    if (offset >= cursor && offset <= inlineEnd) return (inline.marks ?? []).map(cloneMark);
    cursor = inlineEnd;
  }
  return [];
}

/* ---------------------------------------------------------------- 块操作 */

export function splitBlock(block: EditorBlock, offset: number): [EditorBlock, EditorBlock] {
  const text = inlineText(block.content);
  const at = Math.max(0, Math.min(offset, text.length));
  const left = sliceInlines(block.content, 0, at);
  const right = sliceInlines(block.content, at);
  const nextType: TextBlockType = block.type === 'heading' ? 'paragraph' : (block.type as TextBlockType);
  const inherit = ['orderedList', 'bulletList', 'checklistItem', 'blockquote'].includes(block.type);
  const nextAttrs: Record<string, unknown> = inherit ? { ...block.attrs } : { indent: block.attrs.indent ?? 0 };
  if (block.type === 'checklistItem') nextAttrs.checked = false;
  const leftBlock: EditorBlock = { ...block, content: normalizeInlines(left), attrs: { ...block.attrs } };
  const rightBlock: EditorBlock = {
    id: newBlockId(),
    type: nextType,
    shape: 'text',
    attrs: nextAttrs,
    content: normalizeInlines(right),
    rest: {},
    raw: null,
  };
  return [leftBlock, rightBlock];
}

/** 把 after 合并进 before（保留 before 的 id，删除 after）。 */
export function mergeBlocks(before: EditorBlock, after: EditorBlock): EditorBlock {
  return {
    ...before,
    content: normalizeInlines([...before.content, ...after.content]),
    attrs: { ...before.attrs },
  };
}

export function caretJoinOffset(before: EditorBlock): number {
  return inlineText(before.content).length;
}

export function setBlockType(block: EditorBlock, type: TextBlockType): EditorBlock {
  const attrs: Record<string, unknown> = { ...block.attrs };
  if (type === 'heading') attrs.level = attrs.level === 2 || attrs.level === 3 ? attrs.level : 1;
  else delete attrs.level;
  if (type === 'checklistItem' && typeof attrs.checked !== 'boolean') attrs.checked = false;
  if (type !== 'checklistItem') delete attrs.checked;
  if (type !== 'codeBlock') delete attrs.lang;
  if (type === 'codeBlock' && typeof attrs.lang !== 'string') attrs.lang = '';
  if (typeof attrs.indent !== 'number') attrs.indent = 0;
  return { ...block, type, shape: 'text', attrs };
}

export function cycleListType(block: EditorBlock): EditorBlock {
  if (block.type === 'checklistItem') return setBlockType(block, 'paragraph');
  return setBlockType(block, 'checklistItem');
}

export function setIndent(block: EditorBlock, indent: number): EditorBlock {
  const value = Math.max(0, Math.min(8, Math.trunc(indent)));
  return { ...block, attrs: { ...block.attrs, indent: value } };
}

export function indentOf(block: EditorBlock): number {
  const value = attrNumber(block.attrs, 'indent');
  return typeof value === 'number' ? Math.max(0, Math.min(8, value)) : 0;
}

export function headingLevel(block: EditorBlock): number {
  return attrNumber(block.attrs, 'level') ?? 1;
}

export function isChecked(block: EditorBlock): boolean {
  return attrBoolean(block.attrs, 'checked') === true;
}

export function stringAttr(block: EditorBlock, key: string): string | undefined {
  return attrString(block.attrs, key);
}

/* ---------------------------------------------------------------- 派生值 */

export function deriveTitle(blocks: readonly EditorBlock[]): string {
  for (const block of blocks) {
    const text = blockText(block).replace(/\s+/g, ' ').trim();
    if (text.length > 0) return text.length > 80 ? `${text.slice(0, 80)}…` : text;
  }
  return '';
}

export function docCharCount(blocks: readonly EditorBlock[]): number {
  let total = 0;
  for (const block of blocks) total += blockText(block).replace(/\n/g, '').length;
  return total;
}

export function blocksHaveAttachment(blocks: readonly EditorBlock[]): boolean {
  return blocks.some((block) => block.shape === 'attachment');
}

/** 只读闸门：文档版本高于本客户端支持版本时禁止写回。 */
export function docVersionSupported(doc: NoteDoc | null | undefined): boolean {
  const version = typeof doc?.v === 'number' ? doc.v : Number.NaN;
  if (!Number.isFinite(version)) return true;
  return version <= SUPPORTED_DOC_VERSION;
}

export function docVersionOf(doc: NoteDoc | null | undefined): number {
  return typeof doc?.v === 'number' ? doc.v : SUPPORTED_DOC_VERSION;
}

export function countUnknownBlocks(blocks: readonly EditorBlock[]): number {
  return blocks.filter((block) => block.shape === 'unknown').length;
}
