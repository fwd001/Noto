/**
 * 编辑器"记事本化"这一批的形状门禁。
 * 每条都对应用户一句具体要求，且都是"少了什么/多了什么"能一眼对上的那种。
 */
import { describe, expect, it } from 'vitest';
import { TOOLBAR_MARK_BUTTONS } from './editor/marks';

/** 用 `import.meta.glob` 而不是 `node:fs`：本项目没装 @types/node，`vue-tsc` 会直接红（同 uiFeedback.spec 的注释）。 */
const FILES = import.meta.glob<string>('./**/*.{vue,ts}', { query: '?raw', import: 'default', eager: true }) as Record<string, string>;

function src(rel: string): string {
  const text = FILES[`./${rel}`];
  if (typeof text !== 'string') throw new Error(`glob 里读不到 ${rel}（路径写错了？这条门禁会静默空转）`);
  return text;
}

describe('工具条只剩看得懂的那几颗', () => {
  it('常驻工具条的行内标记 = B / I / U / S 四颗', () => {
    expect(TOOLBAR_MARK_BUTTONS.map((b) => b.kind)).toEqual(['bold', 'italic', 'underline', 'strike']);
  });

  it('code 与 highlight 没被删掉，只是挪到"选中文字之后"那条浮动条上', async () => {
    const { MARK_BUTTONS } = await import('./editor/marks');
    const kinds = MARK_BUTTONS.map((b) => b.kind);
    expect(kinds).toContain('code');
    expect(kinds).toContain('highlight');
    // 浮动条仍用全量那张表（RichEditor 里），不然这两颗就是真没了。
    const editor = src('components/RichEditor.vue');
    expect(editor).toMatch(/from '\.\.\/editor\/marks'|from '\.\/marks'/);
  });

  it('清单/缩进不再各占一颗：清单进"类型"菜单，缩进留 ⇤ ⇥', () => {
    const bar = src('components/EditorToolbar.vue');
    expect(bar).not.toMatch(/emit\('checklist'\)/);
    expect(bar).toMatch(/emit\('indent', -1\)/);
    expect(bar).toMatch(/emit\('indent', 1\)/);
    // 但清单这个能力还在（类型菜单里那一档）。
    expect(bar).toMatch(/checklistItem/);
  });
});

describe('行首不再有一列加减号', () => {
  it('`+`（在这行下面插一块）已经拿掉，拖动那颗 ⠿ 保留', () => {
    const editor = src('components/RichEditor.vue');
    expect(editor).not.toContain('insert-below');
    expect(editor).toContain('drag-handle');
  });
});

describe('空笔记的提示语', () => {
  it('文案是"请输入标题和正文"，而且只提在第一行', () => {
    const i18n = src('i18n.ts');
    expect(i18n).toMatch(/'editor\.placeholder': '请输入标题和正文'/);
    const editor = src('components/RichEditor.vue');
    expect(editor).toMatch(/:data-placeholder="index === 0 \? t\('editor\.placeholder'\) : ''"/);
  });
});
