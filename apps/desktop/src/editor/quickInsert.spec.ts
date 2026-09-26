/**
 * 快速插入的纯逻辑测试：缩写判定、marks 不被压平、面板过滤与选中。
 */
import { describe, expect, it } from 'vitest';
import { applySlash, filterSlash, markdownShortcut, slashQuery, SLASH_ITEMS } from './quickInsert';
import { textBlock, inlineText } from './model';
import type { EditorBlock } from './model';

function para(text: string): EditorBlock {
  return textBlock('paragraph', [{ text }]);
}
function boldPara(text: string): EditorBlock {
  return textBlock('paragraph', [{ text: '# ' }, { text, marks: [{ kind: 'bold' }] }]);
}

describe('markdown 输入缩写', () => {
  it.each([
    ['# 标题一', 'heading', 1],
    ['## 标题二', 'heading', 2],
    ['### 标题三', 'heading', 3],
    ['- 项目', 'bulletList', undefined],
    ['* 项目', 'bulletList', undefined],
    ['1. 第一', 'orderedList', undefined],
    ['> 引用', 'blockquote', undefined],
    ['[] 待办', 'checklistItem', undefined],
    ['[x] 完成', 'checklistItem', undefined],
  ])('"%s" 转成目标块型并吃掉前缀', (text, type, level) => {
    const edit = markdownShortcut([para(text)], 0);
    expect(edit).not.toBeNull();
    const block = edit!.blocks[0];
    expect(block.type).toBe(type);
    expect(inlineText(block.content)).not.toMatch(/^[#>*\[\]0-9-]/);
    if (level !== undefined) expect(block.attrs.level).toBe(level);
    expect(edit!.caret).toBe(0);
  });

  it('``` 单独一行转代码块', () => {
    const edit = markdownShortcut([para('```')], 0);
    expect(edit!.blocks[0].type).toBe('codeBlock');
  });

  it('[x] 勾上、[] 不勾', () => {
    expect(markdownShortcut([para('[x] 做完了')], 0)!.blocks[0].attrs.checked).toBe(true);
    expect(markdownShortcut([para('[] 没做完')], 0)!.blocks[0].attrs.checked).toBe(false);
  });

  it('没有空格就不转（"- 一条破折号" 与 "-3度" 是两回事）', () => {
    expect(markdownShortcut([para('#没有空格')], 0)).toBeNull();
    expect(markdownShortcut([para('-3 度今天')], 0)).toBeNull();
    expect(markdownShortcut([para('普通句子')], 0)).toBeNull();
  });

  it('转换不压平后续标记：前缀是纯文本，粗体仍然留着', () => {
    const edit = markdownShortcut([boldPara('重点')], 0);
    const block = edit!.blocks[0];
    expect(block.type).toBe('heading');
    const marks = block.content.flatMap((i) => (i as { marks?: unknown[] }).marks ?? []);
    expect(marks.some((m) => (m as { kind?: string }).kind === 'bold')).toBe(true);
  });

  it('非文本块（图片）不参与缩写', () => {
    const image: EditorBlock = { ...textBlock('paragraph'), shape: 'image', type: 'image' };
    expect(markdownShortcut([image], 0)).toBeNull();
  });
});

describe('"/" 命令面板', () => {
  it('只在行首且查询里没有空白时才算命令', () => {
    expect(slashQuery('/')).toBe('');
    expect(slashQuery('/cod')).toBe('cod');
    // 斜杠后紧跟空格 = 用户在写"/ 30 米"这类正文，不是命令
    expect(slashQuery('/ COD')).toBeNull();
    expect(slashQuery('先打字 /then')).toBeNull();
    expect(slashQuery('/代码 块')).toBeNull();
  });

  it('按名字与中英文关键词过滤，空查询给全表', () => {
    expect(filterSlash('')).toHaveLength(SLASH_ITEMS.length);
    expect(filterSlash('cod').map((i) => i.type)).toContain('codeBlock');
    expect(filterSlash('代码').map((i) => i.type)).toContain('codeBlock');
    expect(filterSlash('todo').map((i) => i.type)).toContain('checklistItem');
    expect(filterSlash('zzzz')).toHaveLength(0);
  });

  it('选中后换成目标块型，并把 "/查询" 本身清掉', () => {
    const item = SLASH_ITEMS.find((i) => i.type === 'heading' && i.level === 2)!;
    const edit = applySlash([para('/h2')], 0, item);
    expect(edit.blocks[0].type).toBe('heading');
    expect(edit.blocks[0].attrs.level).toBe(2);
    expect(inlineText(edit.blocks[0].content)).toBe('');
    expect(edit.caret).toBe(0);
  });
});
