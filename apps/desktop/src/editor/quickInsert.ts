/**
 * 快速插入的**纯逻辑**：markdown 输入缩写 + "/" 命令面板的候选与过滤。
 *
 * 为什么单独一个模块：判定不碰 DOM，才能被单元测试覆盖到位；
 * 组件里只留"显示什么、按键怎么走"，避免出现"只有真浏览器里才成立"的编辑器逻辑。
 */
import { changeType, type BlockEdit } from './commands';
import { inlineText } from './model';
import type { EditorBlock, TextBlockType } from './model';
import type { Inline } from '../api/types';

interface Prefix {
  readonly text: string;
  readonly type: TextBlockType;
  readonly level?: number;
  readonly checked?: boolean;
}

/** 顺序即优先级：`## ` 必须排在 `# ` 之前匹配到，所以按前缀长度降序排。 */
const PREFIXES: readonly Prefix[] = [
  { text: '### ', type: 'heading', level: 3 },
  { text: '## ', type: 'heading', level: 2 },
  { text: '# ', type: 'heading', level: 1 },
  { text: '[ ] ', type: 'checklistItem', checked: false },
  { text: '[] ', type: 'checklistItem', checked: false },
  { text: '[x] ', type: 'checklistItem', checked: true },
  { text: '[X] ', type: 'checklistItem', checked: true },
  { text: '- ', type: 'bulletList' },
  { text: '* ', type: 'bulletList' },
  { text: '+ ', type: 'bulletList' },
  { text: '1. ', type: 'orderedList' },
  { text: '> ', type: 'blockquote' },
];

export interface SlashItem {
  readonly type: TextBlockType;
  readonly labelKey: string;
  readonly hintKey: string;
  readonly keywords: readonly string[];
  readonly level?: number;
}

/** "/" 面板的条目。顺序 = 键盘不选时的默认可见顺序（最常用的在前）。 */
export const SLASH_ITEMS: readonly SlashItem[] = [
  { type: 'paragraph', labelKey: 'slash.paragraph', hintKey: 'slash.paragraphHint', keywords: ['text', 'p', '正文', '段落'] },
  { type: 'heading', labelKey: 'slash.heading', hintKey: 'slash.headingHint', keywords: ['h', 'title', '标题'], level: 1 },
  { type: 'heading', labelKey: 'slash.heading2', hintKey: 'slash.headingHint', keywords: ['h2', '标题2'], level: 2 },
  { type: 'heading', labelKey: 'slash.heading3', hintKey: 'slash.headingHint', keywords: ['h3', '标题3'], level: 3 },
  { type: 'bulletList', labelKey: 'slash.bullet', hintKey: 'slash.bulletHint', keywords: ['ul', 'list', '列表', '无序'] },
  { type: 'orderedList', labelKey: 'slash.ordered', hintKey: 'slash.orderedHint', keywords: ['ol', 'number', '有序', '编号'] },
  { type: 'checklistItem', labelKey: 'slash.checklist', hintKey: 'slash.checklistHint', keywords: ['todo', 'task', '待办', '任务'] },
  { type: 'blockquote', labelKey: 'slash.quote', hintKey: 'slash.quoteHint', keywords: ['quote', '引用'] },
  { type: 'codeBlock', labelKey: 'slash.code', hintKey: 'slash.codeHint', keywords: ['code', 'pre', '代码'] },
];

function stripLeading(content: readonly Inline[], n: number): Inline[] {
  const out = [...content];
  let left = n;
  while (left > 0 && out.length > 0) {
    const first = out[0];
    if (typeof (first as { text?: unknown }).text !== 'string') break;
    const text = (first as { text: string }).text;
    const take = Math.min(left, text.length);
    if (take >= text.length) {
      out.shift();
    } else {
      out[0] = { ...first, text: text.slice(take) } as Inline;
    }
    left -= take;
  }
  return out;
}

function retyped(block: EditorBlock, type: TextBlockType, content: Inline[], attrs: Record<string, unknown>): EditorBlock {
  const next = { ...block, type, content, attrs: { ...block.attrs, ...attrs } };
  if (type !== 'heading') delete next.attrs.level;
  return next;
}

/**
 * 块型转换 + 吃掉触发前缀。返回 null 表示"这不是缩写，正常输入"。
 *
 * 只在**行首**且前缀后紧跟内容时才转，避免把 "- 一条破折号开头的句子" 变成列表
 * —— 用户想写列表会打 "- "，想写破折号会连着写下去。
 */
export function markdownShortcut(blocks: readonly EditorBlock[], index: number): BlockEdit | null {
  const block = blocks[index];
  if (!block || block.shape !== 'text') return null;
  const text = inlineText(block.content);
  if (text === '```' || text === '```text') {
    return editWith(blocks, index, retyped(block, 'codeBlock', [], {}));
  }
  const hit = PREFIXES.find((p) => text.startsWith(p.text));
  if (!hit) return null;
  const rest = stripLeading(block.content, hit.text.length);
  const attrs: Record<string, unknown> = {};
  if (hit.level !== undefined) attrs.level = hit.level;
  if (hit.checked !== undefined) attrs.checked = hit.checked;
  return editWith(blocks, index, retyped(block, hit.type, rest, attrs));
}

function editWith(blocks: readonly EditorBlock[], index: number, nextBlock: EditorBlock): BlockEdit {
  const edit = changeType(blocks, index, nextBlock.type as TextBlockType);
  const out = [...edit.blocks];
  out[index] = nextBlock;
  return { blocks: out, focusId: nextBlock.id, caret: 0 };
}

/** 块内文本以 `/` 开头时给出查询串（`/` 后到第一个空格前）。 */
export function slashQuery(text: string): string | null {
  if (!text.startsWith('/')) return null;
  const rest = text.slice(1);
  return /\s/.test(rest) ? null : rest.toLowerCase();
}

export function filterSlash(query: string): SlashItem[] {
  const q = query.trim();
  if (q === '') return [...SLASH_ITEMS];
  return SLASH_ITEMS.filter((item) => {
    if (item.type.startsWith(q) || item.keywords.some((k) => k.startsWith(q))) return true;
    return item.keywords.some((k) => k.includes(q));
  });
}

/** 面板里选中一条 = 换成该块型并清掉 "/查询" 本身。 */
export function applySlash(blocks: readonly EditorBlock[], index: number, item: SlashItem): BlockEdit {
  const block = blocks[index];
  const attrs: Record<string, unknown> = {};
  if (item.level !== undefined) attrs.level = item.level;
  if (item.type === 'checklistItem') attrs.checked = false;
  return editWith(blocks, index, retyped(block, item.type, [], attrs));
}
