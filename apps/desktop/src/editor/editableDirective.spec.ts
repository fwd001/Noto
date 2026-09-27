import { describe, expect, it } from 'vitest';
import { markAsParsed, vEditable } from './editableDirective';
import { renderBlockHtml } from './dom';

/**
 * 为什么单独一条：编辑区每个文本块是 `role="textbox"`，读屏必须念得出它是"正文/标题/代码"
 * 里的哪一个。这个名字由 `v-editable` 指令写，而不是模板里的 `:aria-label` —— 实测在那
 * 个元素上加响应式属性绑定会让打字内容进不了模型（黑盒 UAT 10/10 → 4/10，库里
 * `charCount: 0` 而屏幕上有字）。所以这里钉的是"名字确实落到元素上、且跟着块型走"。
 */
function mount(type: string, content: unknown[] = []) {
  const el = document.createElement('div');
  el.setAttribute('contenteditable', 'true');
  el.setAttribute('role', 'textbox');
  vEditable.mounted(el, { value: { content, type } as never });
  return el;
}

describe('v-editable 的读屏名字', () => {
  it('正文块叫「正文」', () => {
    expect(mount('paragraph').getAttribute('aria-label')).toBe('正文');
  });

  it('标题块叫出层级', () => {
    expect(mount('heading').getAttribute('aria-label')).toContain('标题');
  });

  it('代码块叫「代码」', () => {
    expect(mount('codeBlock').getAttribute('aria-label')).toBe('代码');
  });

  it('块型改了，名字也跟着改（updated 也要写）', () => {
    const el = mount('paragraph');
    const next = { value: { content: [], type: 'blockquote' } } as never;
    vEditable.updated(el, next);
    expect(el.getAttribute('aria-label')).toBe('引用');
  });

  it('没给块型时不写空名字（免得念成"无名称"）', () => {
    const el = document.createElement('div');
    vEditable.mounted(el, { value: { content: [] } } as never);
    expect(el.hasAttribute('aria-label')).toBe(false);
  });

  it('用户输入过的内容不被随后的刷新覆写（签名机制仍在）', () => {
    const el = mount('paragraph');
    const typed = [{ text: '刚敲的字' }];
    markAsParsed(el, typed as never);
    el.innerHTML = renderBlockHtml(typed as never);
    vEditable.updated(el, { value: { content: typed, type: 'paragraph' } } as never);
    expect(el.innerHTML).toContain('刚敲的字');
  });
});
