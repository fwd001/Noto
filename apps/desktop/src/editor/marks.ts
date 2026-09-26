/**
 * 行内标记的按钮定义：顶部工具条和浮动选区条共用一份。
 * 放这里而不是各自写一遍，是为了让 glyph、文案键、顺序只有一个真相。
 */
import type { MessageKey } from '../i18n';

export interface MarkButton {
  kind: string;
  glyph: string;
  label: MessageKey;
}

export const MARK_BUTTONS: readonly MarkButton[] = [
  { kind: 'bold', glyph: 'B', label: 'tb.bold' },
  { kind: 'italic', glyph: 'I', label: 'tb.italic' },
  { kind: 'underline', glyph: 'U', label: 'tb.underline' },
  { kind: 'strike', glyph: 'S', label: 'tb.strike' },
  { kind: 'code', glyph: '</>', label: 'tb.code' },
  { kind: 'highlight', glyph: 'H', label: 'tb.highlight' },
];
