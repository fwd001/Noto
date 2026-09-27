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

describe('parseEditable 跳过空块的占位 <br>', () => {
  const box = (html: string) => {
    const node = document.createElement('div');
    node.innerHTML = html;
    return node;
  };

  it('只剩一个 <br> 的空块 → 空内容，不是一条换行', () => {
    expect(parseEditable(box('<br>'))).toEqual([]);
    expect(inlineText(parseEditable(box('<br>')))).toBe('');
  });

  it('占位 <br> 打头时跳过，后面的文字照旧保留', () => {
    expect(inlineText(parseEditable(box('<br>正文')))).toBe('正文');
  });

  it('文字之后的 <br> 仍然是换行（没把真换行一起砍掉）', () => {
    expect(inlineText(parseEditable(box('甲<br>乙')))).toBe('甲\n乙');
  });
});
