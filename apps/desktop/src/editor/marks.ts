/**
 * 行内标记的按钮定义：顶部工具条和浮动选区条共用一份。
 * 放这里而不是各自写一遍，是为了让 glyph、文案键、顺序只有一个真相。
 */
import type { MessageKey } from '../i18n';
import type { IconName } from '../components/ui/icons';

export interface MarkButton {
  kind: string;
  /**
   * §2.4：「文字类（加粗/斜体）用 SVG 路径画，不用 B/I 字形」。
   * 这里存的必须是图标名，不是要显示的字符 —— 一旦存回字符，四端字体回退就会画出四种形状，
   * 而且与旁边 1.75px 描边的图标笔触不匹配。
   */
  icon: IconName;
  label: MessageKey;
}

export const MARK_BUTTONS: readonly MarkButton[] = [
  { kind: 'bold', icon: 'mark-bold', label: 'tb.bold' },
  { kind: 'italic', icon: 'mark-italic', label: 'tb.italic' },
  { kind: 'underline', icon: 'mark-underline', label: 'tb.underline' },
  { kind: 'strike', icon: 'mark-strike', label: 'tb.strike' },
  { kind: 'code', icon: 'mark-code', label: 'tb.code' },
  { kind: 'highlight', icon: 'mark-highlight', label: 'tb.highlight' },
];

/**
 * 顶部工具条只放"一眼看得懂"的四颗；`code` / `highlight` 留在**选中文字后浮出的那条**上
 * —— 它们必须先有选区才有意义，摆在常驻工具条上就是 16 颗里没人碰的那几颗。
 * 用户口径：「富文本选项应该像 Apple 便签那样只有几项」。
 */
export const TOOLBAR_MARK_BUTTONS: readonly MarkButton[] = MARK_BUTTONS.filter(
  (entry) => entry.kind === 'bold' || entry.kind === 'italic' || entry.kind === 'underline' || entry.kind === 'strike',
);

/**
 * 「文字大小」的档位（用户口径里 Apple 便签那套就是几个档，不是自由字号）。
 *
 * 值用 **em 而不是 px**：应用有自己的全局字号缩放（设置里那一档），写死 px 会让
 * "用户把字调大"这件事在这些行上失效。em 相对所在块的 font-size ⇒ 跟着缩放走。
 *
 * 表里**没有** `m`（标准）：标准就是"没有这个标记"。所以菜单里"标准"那颗发的是移除，
 * 不是加一个 `step:'m'` —— 后者会在文档里留一个没有任何视觉效果的标记，
 * 还白增一格内容哈希（同步侧每次都要为它算一遍）。
 */
export const FONT_SIZE_STEPS: Readonly<Record<string, string>> = Object.freeze({
  s: '0.8em',
  l: '1.3em',
  xl: '1.7em',
});

export const FONT_SIZE_MENU: ReadonlyArray<{ step: string; label: MessageKey }> = [
  { step: 's', label: 'tb.sizeSmall' },
  { step: 'm', label: 'tb.sizeDefault' },
  { step: 'l', label: 'tb.sizeLarge' },
  { step: 'xl', label: 'tb.sizeHuge' },
];

/**
 * 「文字颜色」的取值。**只存语义名，渲染成 `var(--ink-*)`**：
 * 存十六进制会让浅色主题里选的深红在深色主题下糊成一片，而深浅两套 token 是各自调过对比度的。
 * （导入的内容可能带裸 hex，渲染侧另外放行，见 `dom.ts` 的 `safeColor`。）
 *
 * 顺序即菜单顺序；第一项"默认色"= 移除标记，理由同字号的 `m`。
 */
export const INK_COLORS: Readonly<Record<string, string>> = Object.freeze({
  red: 'var(--ink-red)',
  orange: 'var(--ink-orange)',
  green: 'var(--ink-green)',
  blue: 'var(--ink-blue)',
  violet: 'var(--ink-violet)',
});

export const INK_COLOR_MENU: ReadonlyArray<{ name: string; label: MessageKey }> = [
  { name: 'default', label: 'tb.colorDefault' },
  { name: 'red', label: 'tb.colorRed' },
  { name: 'orange', label: 'tb.colorOrange' },
  { name: 'green', label: 'tb.colorGreen' },
  { name: 'blue', label: 'tb.colorBlue' },
  { name: 'violet', label: 'tb.colorViolet' },
];
