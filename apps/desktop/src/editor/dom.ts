/**
 * 行内容与 DOM 的双向映射（contenteditable 适配层）。
 * 约定：样式写在 data-mark / data-mark-attrs 上，解析以数据属性为准 ——
 * 这样未知样式也能原样往返（前向兼容），且不依赖浏览器的标签归一化行为。
 */
import type { Inline, Mark } from '../api/types';

const MARK_TAG: Record<string, string> = {
  bold: 'strong',
  italic: 'em',
  underline: 'u',
  strike: 's',
  code: 'code',
  highlight: 'mark',
  link: 'a',
};

const TAG_MARK: Record<string, string> = {
  strong: 'bold',
  b: 'bold',
  em: 'italic',
  i: 'italic',
  u: 'underline',
  ins: 'underline',
  s: 'strike',
  strike: 'strike',
  del: 'strike',
  code: 'code',
  mark: 'highlight',
  a: 'link',
};

const BOUNDARY_TAGS = new Set(['DIV', 'P', 'LI']);

export function escapeHtml(value: string): string {
  return value
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}

/** 只放行无脚本风险的链接协议。 */
export function safeHref(raw: unknown): string | null {
  if (typeof raw !== 'string') return null;
  const value = raw.trim();
  if (value.length === 0) return null;
  if (/^(https?:|mailto:|tel:|#)/i.test(value)) return value;
  if (/^[a-z][a-z0-9+.-]*:/i.test(value)) return null; // 其它协议一律不放行
  return value.startsWith('//') ? null : value;
}

function safeColor(raw: unknown): string | null {
  if (typeof raw !== 'string') return null;
  return /^#[0-9a-fA-F]{3,8}$/.test(raw.trim()) ? raw.trim() : null;
}

function markFromElement(element: Element): Mark | null {
  const declared = element.getAttribute('data-mark');
  const kind = typeof declared === 'string' && declared.length > 0 ? declared : (TAG_MARK[element.tagName.toLowerCase()] ?? null);
  if (!kind) return null;
  const rawAttrs = element.getAttribute('data-mark-attrs');
  let attrs: Record<string, unknown> | undefined;
  if (rawAttrs) {
    try {
      const parsed = JSON.parse(rawAttrs) as unknown;
      if (parsed && typeof parsed === 'object') attrs = parsed as Record<string, unknown>;
    } catch {
      attrs = undefined;
    }
  }
  if (!attrs && kind === 'link') {
    const href = element.getAttribute('href');
    if (href) attrs = { href };
  }
  return { kind, ...(attrs ? { attrs } : {}) };
}

function wrap(mark: Mark, inner: string): string {
  const attrs = mark.attrs ?? {};
  const data = Object.keys(attrs).length > 0 ? ` data-mark-attrs="${escapeHtml(JSON.stringify(attrs))}"` : '';
  switch (mark.kind) {
    case 'link': {
      const href = safeHref(attrs.href);
      if (!href) return inner;
      return `<a data-mark="link"${data} href="${escapeHtml(href)}" rel="noopener noreferrer nofollow" target="_blank">${inner}</a>`;
    }
    case 'highlight': {
      const color = safeColor(attrs.color);
      const style = color ? ` style="background-color:${color}"` : '';
      return `<mark data-mark="highlight"${data}${style}>${inner}</mark>`;
    }
    default: {
      const tag = MARK_TAG[mark.kind] ?? 'span';
      return `<${tag} data-mark="${escapeHtml(mark.kind)}"${data}>${inner}</${tag}>`;
    }
  }
}

/** 行内容 → HTML（文本一律转义，绝不让内容侧注入脚本）。 */
export function renderInlines(inlines: readonly Inline[]): string {
  let html = '';
  for (const inline of inlines) {
    const text = escapeHtml(inline.text).replace(/ /g, '\u00a0').replace(/\n/g, '<br>');
    let chunk = text;
    const marks = inline.marks ?? [];
    for (let i = marks.length - 1; i >= 0; i -= 1) {
      const mark = marks[i];
      if (mark) chunk = wrap(mark, chunk);
    }
    html += chunk;
  }
  return html.length > 0 ? html : '';
}

/** 空块的可编辑占位（contenteditable 需要至少一个可放置光标的节点）。 */
export function renderBlockHtml(inlines: readonly Inline[]): string {
  const html = renderInlines(inlines);
  return html.length > 0 ? html : '<br>';
}

/* ------------------------------------------------------------ DOM → 行内容 */

export function parseEditable(root: HTMLElement): Inline[] {
  const out: Inline[] = [];
  let emitted = false;

  const append = (text: string, marks: Mark[]) => {
    if (text.length === 0) return;
    const last = out[out.length - 1];
    if (last && sameMarkList(last.marks, marks)) last.text += text;
    else out.push({ text, ...(marks.length > 0 ? { marks: marks.map((m) => ({ kind: m.kind, ...(m.attrs ? { attrs: { ...m.attrs } } : {}) })) } : {}) });
    emitted = true;
  };

  const visit = (node: Node, marks: Mark[]) => {
    for (const child of Array.from(node.childNodes)) {
      if (child.nodeType === 3) {
        append((child.textContent ?? '').replace(/\u00a0/g, ' '), marks);
        continue;
      }
      if (child.nodeType !== 1) continue;
      const element = child as HTMLElement;
      if (element.tagName === 'BR') {
        append('\n', marks);
        continue;
      }
      if (BOUNDARY_TAGS.has(element.tagName) && emitted) append('\n', marks);
      const mark = markFromElement(element);
      visit(element, mark ? [...marks, mark] : marks);
    }
  };

  visit(root, []);
  return out;
}

function sameMarkList(a: readonly Mark[] | undefined, b: readonly Mark[]): boolean {
  const left = a ?? [];
  if (left.length !== b.length) return false;
  for (let i = 0; i < left.length; i += 1) {
    const l = left[i];
    const r = b[i];
    if (!l || !r) return false;
    if (l.kind !== r.kind) return false;
    if (JSON.stringify(l.attrs ?? null) !== JSON.stringify(r.attrs ?? null)) return false;
  }
  return true;
}

/* --------------------------------------------------------- 光标偏移映射 */

interface Segment {
  node: Node;
  start: number;
  end: number;
  kind: 'text' | 'break';
  parent: Node;
  index: number;
}

export interface EditableMeasure {
  root: HTMLElement;
  text: string;
  segments: Segment[];
}

/** 走一遍 DOM，产出与 parseEditable 完全同构的字符流及其位置映射。 */
export function measureEditable(root: HTMLElement): EditableMeasure {
  const segments: Segment[] = [];
  let cursor = 0;
  let emitted = false;

  const take = (length: number) => {
    cursor += length;
    emitted = true;
  };

  const visit = (node: Node) => {
    Array.from(node.childNodes).forEach((child, index) => {
      if (child.nodeType === 3) {
        const value = (child.textContent ?? '').replace(/\u00a0/g, ' ');
        if (value.length > 0) {
          segments.push({ node: child, start: cursor, end: cursor + value.length, kind: 'text', parent: node, index });
          take(value.length);
        }
        return;
      }
      if (child.nodeType !== 1) return;
      const element = child as HTMLElement;
      if (element.tagName === 'BR') {
        segments.push({ node: element, start: cursor, end: cursor + 1, kind: 'break', parent: node, index });
        take(1);
        return;
      }
      if (BOUNDARY_TAGS.has(element.tagName) && emitted) {
        segments.push({ node: element, start: cursor, end: cursor + 1, kind: 'break', parent: node, index });
        take(1);
      }
      visit(element);
    });
  };

  visit(root);
  return { root, text: textOf(segments), segments };
}

function textOf(segments: readonly Segment[]): string {
  let out = '';
  for (const segment of segments) {
    if (segment.kind === 'break') out += '\n';
    else out += (segment.node.textContent ?? '').replace(/\u00a0/g, ' ');
  }
  return out;
}

export interface CaretPoint {
  node: Node;
  offset: number;
}

/** DOM 选区位置 → 模型文本偏移。 */
export function toModelOffset(root: HTMLElement, point: CaretPoint, measure: EditableMeasure): number {
  const { segments } = measure;
  if (point.node.nodeType === 3) {
    for (const segment of segments) {
      if (segment.node === point.node) {
        const raw = (point.node.textContent ?? '').replace(/\u00a0/g, ' ');
        return segment.start + Math.max(0, Math.min(point.offset, raw.length));
      }
    }
  }
  const element = point.node as HTMLElement;
  const child = element.nodeType === 1 ? element.childNodes[point.offset] ?? null : null;
  for (const segment of segments) {
    if (child !== null && segment.node === child) return segment.start;
  }
  const previous = point.offset > 0 ? element.childNodes[point.offset - 1] ?? null : null;
  for (let i = segments.length - 1; i >= 0; i -= 1) {
    const segment = segments[i];
    if (segment && previous !== null && segment.node === previous) return segment.end;
  }
  if (element === root) return measure.text.length;
  for (const segment of segments) {
    if (segment.parent === element || element.contains(segment.node)) return segment.start;
  }
  return measure.text.length;
}

/** 模型文本偏移 → DOM 选区位置。 */
export function toDomPoint(measure: EditableMeasure, modelOffset: number): CaretPoint {
  const offset = Math.max(0, Math.min(modelOffset, measure.text.length));
  for (const segment of measure.segments) {
    if (offset < segment.start) continue;
    if (segment.kind === 'text' && offset <= segment.end) {
      return { node: segment.node, offset: offset - segment.start };
    }
    if (segment.kind === 'break' && offset < segment.end) {
      return { node: segment.parent, offset: segment.index };
    }
    if (offset === segment.end && segment.kind === 'break') {
      return { node: segment.parent, offset: segment.index + 1 };
    }
  }
  const last = measure.segments[measure.segments.length - 1];
  if (last && last.kind === 'text') {
    const length = (last.node.textContent ?? '').length;
    return { node: last.node, offset: length };
  }
  if (last) return { node: last.parent, offset: last.index + 1 };
  return { node: measure.root, offset: measure.root.childNodes.length };
}

/** 读取当前选区在该可编辑块内的 [start,end)。 */
export function selectionIn(root: HTMLElement): { start: number; end: number } | null {
  const selection = typeof window === 'undefined' ? null : window.getSelection();
  if (!selection || selection.rangeCount === 0) return null;
  const range = selection.getRangeAt(0);
  if (!root.contains(range.startContainer) && range.startContainer !== root) return null;
  const measure = measureEditable(root);
  const start = toModelOffset(root, { node: range.startContainer, offset: range.startOffset }, measure);
  const end = range.collapsed
    ? start
    : toModelOffset(root, { node: range.endContainer, offset: range.endOffset }, measure);
  return { start: Math.min(start, end), end: Math.max(start, end) };
}

export function applySelection(root: HTMLElement, start: number, end = start): void {
  const measure = measureEditable(root);
  const from = toDomPoint(measure, start);
  const to = toDomPoint(measure, end);
  if (from.node === null || to.node === null) return;
  const selection = typeof window === 'undefined' ? null : window.getSelection();
  if (!selection) return;
  try {
    const range = document.createRange();
    range.setStart(from.node, from.offset);
    range.setEnd(to.node, to.offset);
    selection.removeAllRanges();
    selection.addRange(range);
  } catch {
    // 选区越界（DOM 已被替换）时静默放弃：下一次输入会重新对齐
  }
}

export function caretInView(root: HTMLElement): void {
  const selection = typeof window === 'undefined' ? null : window.getSelection();
  if (!selection || selection.rangeCount === 0) return;
  const rect = selection.getRangeAt(0).getBoundingClientRect();
  if (rect.height === 0 && rect.width === 0) return;
  root.scrollIntoView({ block: 'nearest', inline: 'nearest' });
}
