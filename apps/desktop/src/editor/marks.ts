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

/**
 * 顶部工具条只放"一眼看得懂"的四颗；`code` / `highlight` 留在**选中文字后浮出的那条**上
 * —— 它们必须先有选区才有意义，摆在常驻工具条上就是 16 颗里没人碰的那几颗。
 * 用户口径：「富文本选项应该像 Apple 便签那样只有几项」。
 */
export const TOOLBAR_MARK_BUTTONS: readonly MarkButton[] = MARK_BUTTONS.filter(
  (entry) => entry.kind === 'bold' || entry.kind === 'italic' || entry.kind === 'underline' || entry.kind === 'strike',
);
