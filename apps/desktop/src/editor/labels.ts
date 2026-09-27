import { t, type MessageKey } from '../i18n';

/**
 * 块型 → 文案键：**全项目只这一份**。
 *
 * 之前这张表在 `EditorToolbar.vue` 里，而且是用模板串把键拼出来（`editor.block` 前缀
 * 加类型名）—— 而 `MessageKey` 就是 `string`，拼错或漏键编译期一声不响，7 种块型里 5 种
 * 在工具条上直接显示成原始键名。现在必须是表，并且 `labels.spec.ts` 逐个核对
 * "键在 i18n 里真的登记了"。
 *
 * 为什么挪出组件：读屏要念同一个词。编辑区每个文本块是 `role="textbox"`，它的
 * `aria-label` 与工具条的类型名必须一致，两处各写一份迟早漂移。
 */
export const BLOCK_TYPE_LABELS: Record<string, MessageKey> = {
  paragraph: 'editor.blockParagraph',
  heading: 'editor.blockHeading',
  bulletList: 'editor.blockListBullet',
  orderedList: 'editor.blockListOrdered',
  checklistItem: 'editor.blockChecklist',
  blockquote: 'editor.blockQuote',
  codeBlock: 'editor.blockCode',
  image: 'editor.blockImage',
  attachment: 'editor.blockAttachment',
  rule: 'editor.blockRule',
  horizontalRule: 'editor.blockRule',
};

/**
 * 块型的可读名字。未知块型宁可返回原始类型名，也不假装是正文 —— 那会把"这有一段
 * 我们不认识的内容"这件事从用户眼前抹掉。
 */
export function blockTypeLabel(type: string, level?: number): string {
  const key = BLOCK_TYPE_LABELS[type];
  if (!key) return type;
  return type === 'heading' ? t(key, { level: level ?? 1 }) : t(key);
}
