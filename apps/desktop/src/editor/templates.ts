/**
 * 快捷新建的笔记模板。
 *
 * 口径来自用户那句「也可以给上一些基础模板让快捷新建，**默认的模板要非常简洁，
 * 一进去就是"请输入标题和正文"**」—— 所以：
 *  · `blank` 就是默认那颗"新建笔记"走的路径（一个空段落，占位语提示输入）；
 *  · 其它模板**第一个块一律留空**：标题是从正文第一行推出来的（核心 `extract.rs`），
 *    模板要是把第一行占了，用户的标题就被模板文字顶掉，列表里看到的就不是他起的名字。
 *  · 模板只写"骨架"，不写示例文字 —— 示例最后都得删，等于给用户添活儿。
 */
import type { MessageKey } from '../i18n';
import type { Block, NoteDoc } from '../api/types';
import { emptyDoc, newBlockId, SUPPORTED_DOC_VERSION } from './model';

export interface NoteTemplate {
  id: string;
  label: MessageKey;
  build(): NoteDoc;
}

interface BlockSpec {
  type: string;
  attrs?: Record<string, unknown>;
  content?: Array<{ text: string }>;
}

function doc(blocks: BlockSpec[]): NoteDoc {
  const content: Block[] = blocks.map((b) => ({
    id: newBlockId(),
    type: b.type,
    ...(b.attrs ? { attrs: b.attrs } : {}),
    content: b.content ?? [],
  }));
  return { v: SUPPORTED_DOC_VERSION, content };
}

const paragraph = (): BlockSpec => ({ type: 'paragraph' });
const emptyCheck = (): BlockSpec => ({ type: 'checklistItem', attrs: { checked: false } });
const labeled = (text: string): BlockSpec => ({ type: 'paragraph', content: [{ text }] });

export const NOTE_TEMPLATES: readonly NoteTemplate[] = [
  { id: 'blank', label: 'template.blank', build: () => emptyDoc() },
  {
    id: 'todo',
    label: 'template.todo',
    build: () => doc([paragraph(), emptyCheck(), emptyCheck(), emptyCheck()]),
  },
  {
    id: 'meeting',
    label: 'template.meeting',
    build: () => doc([paragraph(), labeled('参会'), emptyCheck(), labeled('结论'), paragraph()]),
  },
];

export function templateById(id: string): NoteTemplate | undefined {
  return NOTE_TEMPLATES.find((tpl) => tpl.id === id);
}

/** 除默认模板外的候选：默认那颗按钮已经能建空白页，菜单里不该再列一遍。 */
export const TEMPLATE_CHOICES: readonly NoteTemplate[] = NOTE_TEMPLATES.filter((tpl) => tpl.id !== 'blank');
