import type { TextBlockType } from './model';
import type { IconName } from '../components/ui/icons';

/**
 * 工具条那一条横排里每个格子的**唯一描述处**（§3.4 的溢出、§2 的图标、§5 的可读名字都读它）。
 *
 * 为什么要一张表而不是把 markup 抄两遍：窄栏要"收进「更多 ›」"，同一个格子就要在两个位置
 * 之一出现。两处各写一份 markup 迟早分叉（一份有 aria-label、一份没有；一份 testid 与另一份不同），
 * 而那正是本项目反复踩的"同一个意思两条表征"。有了这张表，渲染只有一个组件，位置由溢出计划决定。
 */
export type ToolbarItemKind = 'mark' | 'menu' | 'icon' | 'text' | 'link';

export type ToolbarItem = {
  key: string;
  kind: ToolbarItemKind;
  /** i18n 键：既是 title/aria-label，也是「更多」面板里那一行的文字。 */
  label: string;
  /** 有 `data-testid` 的格子在这张表里登记 —— 门禁靠它定位，键名不许两处各写。 */
  testid?: string;
  /** mark：标记名；menu：菜单标识；icon/text/link：动作名。 */
  payload?: string;
  /** icon 格子的 SVG 名字（§2：界面里不许出现 Unicode 字形）。 */
  icon?: IconName;
  /** 只有选中文字才有意义的格子：块型/字号/颜色之外的"标记"类。 */
  requiresSelection?: boolean;
  /** 缩进两颗要看当前块型（列表项之外没有缩进）。 */
  needsIndent?: boolean;
  /** 顺序里的一处硬换行：这一格之后把剩下的推到右端。 */
  spacerAfter?: boolean;
};

/** 三个下拉的标识。与 `TextBlockType` 无关，只是"哪一个菜单开着"。 */
export type ToolbarMenu = 'type' | 'size' | 'color';

/** 顺序即工具条从左到右的顺序（也是溢出时"从右边开始收"的顺序）。 */
export const TOOLBAR_ITEMS: readonly ToolbarItem[] = [
  { key: 'bold', kind: 'mark', label: 'tb.bold', payload: 'bold', requiresSelection: true },
  { key: 'italic', kind: 'mark', label: 'tb.italic', payload: 'italic', requiresSelection: true },
  { key: 'underline', kind: 'mark', label: 'tb.underline', payload: 'underline', requiresSelection: true },
  { key: 'strike', kind: 'mark', label: 'tb.strike', payload: 'strike', requiresSelection: true },
  { key: 'link', kind: 'link', label: 'tb.link' },
  { key: 'type', kind: 'menu', label: 'tb.blockType', payload: 'type' satisfies ToolbarMenu },
  { key: 'size', kind: 'menu', label: 'tb.size', testid: 'tb-size', payload: 'size' satisfies ToolbarMenu },
  { key: 'color', kind: 'menu', label: 'tb.textColor', testid: 'tb-color', payload: 'color' satisfies ToolbarMenu },
  { key: 'outdent', kind: 'icon', label: 'tb.outdent', icon: 'indent-out', needsIndent: true },
  { key: 'indent', kind: 'icon', label: 'tb.indent', icon: 'indent-in', needsIndent: true },
  { key: 'rule', kind: 'icon', label: 'tb.rule', icon: 'rule' },
  { key: 'attach', kind: 'text', label: 'tb.attach', payload: 'file' },
  { key: 'image', kind: 'text', label: 'editor.blockImage', payload: 'inline' },
  { key: 'undo', kind: 'icon', label: 'tb.undo', icon: 'undo', spacerAfter: true },
  { key: 'redo', kind: 'icon', label: 'tb.redo', icon: 'redo' },
];

/** 类型菜单里可切换的文本块型；顺序即菜单顺序。 */
export const TEXT_TYPE_ORDER: readonly TextBlockType[] = [
  'paragraph',
  'blockquote',
  'codeBlock',
  'orderedList',
  'bulletList',
  'checklistItem',
];
