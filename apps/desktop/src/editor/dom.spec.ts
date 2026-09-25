import { describe, expect, it } from 'vitest';
import { measureEditable, parseEditable, renderBlockHtml, renderInlines, safeHref, toDomPoint, toModelOffset } from './dom';
import { inlineText, normalizeInlines } from './model';
import type { Inline } from '../api/types';

function host(html: string): HTMLElement {
  const element = document.createElement('div');
  element.innerHTML = html;
  return element;
}

describe('行内容 ↔ DOM', () => {
  it('文本一律转义，内容里带脚本也只会变成文字', () => {
    const html = renderInlines([{ text: '<img src=x onerror=alert(1)>' }]);
    expect(html).not.toContain('<img');
    expect(parseEditable(host(html))[0]?.text).toBe('<img src=x onerror=alert(1)>');
  });

  it('样式往返：已知与未知样式都能原样回来', () => {
    const inlines: Inline[] = [
      { text: '粗', marks: [{ kind: 'bold' }] },
      { text: '斜+码', marks: [{ kind: 'italic' }, { kind: 'code' }] },
      { text: '高亮', marks: [{ kind: 'highlight', attrs: { color: '#ffe9a8' } }] },
      { text: '链接', marks: [{ kind: 'link', attrs: { href: 'https://example.test/a?b=1' } }] },
      { text: '未来样式', marks: [{ kind: 'fontSize', attrs: { size: 3 } }] },
    ];
    const round = normalizeInlines(parseEditable(host(renderBlockHtml(inlines))));
    expect(round).toEqual(normalizeInlines(inlines));
  });

  it('换行以 <br> 与 \\n 双向一致', () => {
    const inlines: Inline[] = [{ text: '第一行\n第二行' }];
    expect(renderBlockHtml(inlines)).toContain('<br>');
    expect(parseEditable(host(renderBlockHtml(inlines)))).toEqual(inlines);
  });

  it('浏览器自己插进来的 div 也读成换行，不丢字', () => {
    const parsed = parseEditable(host('<span>甲</span><div>乙</div><div>丙</div>'));
    expect(inlineText(parsed)).toBe('甲\n乙\n丙');
  });

  it('危险协议链接被剥离，但文字保留', () => {
    expect(safeHref('javascript:alert(1)')).toBeNull();
    expect(safeHref('https://ok.test')).toBe('https://ok.test');
    const html = renderBlockHtml([{ text: '点这里', marks: [{ kind: 'link', attrs: { href: 'javascript:alert(1)' } }] }]);
    expect(html).not.toContain('javascript:');
    expect(inlineText(parseEditable(host(html)))).toBe('点这里');
  });

  it('空块渲染出可放置光标的节点', () => {
    expect(renderBlockHtml([])).toBe('<br>');
  });
});

describe('光标偏移映射', () => {
  const inlines: Inline[] = [
    { text: '甲乙', marks: [{ kind: 'bold' }] },
    { text: '\n' },
    { text: '丙丁戊' },
  ];

  it('字符流与模型一致', () => {
    const measure = measureEditable(host(renderBlockHtml(inlines)));
    expect(measure.text).toBe(inlineText(normalizeInlines(inlines)));
  });

  it('模型偏移 → DOM → 模型偏移，逐位往返', () => {
    const element = host(renderBlockHtml(inlines));
    const measure = measureEditable(element);
    for (let offset = 0; offset <= measure.text.length; offset += 1) {
      const point = toDomPoint(measure, offset);
      expect(toModelOffset(element, point, measure)).toBe(offset);
    }
  });
});
