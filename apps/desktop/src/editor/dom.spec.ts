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
      // 以前这一格用的是 `fontSize` —— 那时它是"未来可能出现的样式"的替身。
      // 现在 fontSize 是**认识**的档了（不认识的值就该被丢掉，见下面那组断言），
      // 所以换成一个模型里真没有的名字，这条测的还是它原本要测的前向兼容。
      { text: '未来样式', marks: [{ kind: 'futureStyle', attrs: { size: 3 } }] },
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

describe('文字大小与颜色（用户第 ③ 条口径：像 Apple 便签那样只有"大小、颜色"几项）', () => {
  it('字号档落进 style，值是 em（跟着用户的字号缩放走，不是写死的 px）', () => {
    const html = renderInlines([{ text: '大', marks: [{ kind: 'fontSize', attrs: { step: 'l' } }] }]);
    expect(html).toContain('data-mark="fontSize"');
    expect(html).toContain('font-size:1.3em');
    const html2 = renderInlines([{ text: '小', marks: [{ kind: 'fontSize', attrs: { step: 's' } }] }]);
    expect(html2).toContain('font-size:0.8em');
    const html3 = renderInlines([{ text: '特大', marks: [{ kind: 'fontSize', attrs: { step: 'xl' } }] }]);
    expect(html3).toContain('font-size:1.7em');
  });

  it('认不出的档值一律不画 —— 不许留一个"看着设了其实没效果"的标记', () => {
    for (const bad of [{}, { step: 'huge' }, { step: 3 }, { step: 'javascript:alert(1)' }]) {
      const html = renderInlines([{ text: 'x', marks: [{ kind: 'fontSize', attrs: bad }] }]);
      expect(html).not.toContain('font-size');
      expect(html).not.toContain('data-mark="fontSize"');
    }
  });

  it('颜色走 token（var(--ink-*)），这样深浅色主题都是同一份真相', () => {
    const html = renderInlines([{ text: '红', marks: [{ kind: 'color', attrs: { name: 'red' } }] }]);
    expect(html).toContain('data-mark="color"');
    expect(html).toContain('color:var(--ink-red)');
  });

  it('导入内容带来的裸十六进制也放行，但只放行颜色形状的东西', () => {
    const ok = renderInlines([{ text: 'x', marks: [{ kind: 'color', attrs: { color: '#A1B2C3' } }] }]);
    expect(ok).toContain('color:#A1B2C3');
    for (const bad of ['red; } body{display:none}', 'url(http://x)', 'javascript:alert(1)', 'rgb(1,2,3)', '']) {
      const html = renderInlines([{ text: 'x', marks: [{ kind: 'color', attrs: { color: bad } }] }]);
      expect(html).not.toContain('data-mark="color"');
    }
  });

  it('往返不掉属性：画出去再解析回来，step / name 还在', () => {
    const element = document.createElement('div');
    element.innerHTML = renderInlines([
      { text: '大小', marks: [{ kind: 'bold' }, { kind: 'fontSize', attrs: { step: 'xl' } }] },
      { text: '颜色', marks: [{ kind: 'color', attrs: { name: 'blue' } }] },
    ]);
    const parsed = parseEditable(element);
    expect(parsed[0]?.marks).toEqual([
      { kind: 'bold' },
      { kind: 'fontSize', attrs: { step: 'xl' } },
    ]);
    expect(parsed[1]?.marks).toEqual([{ kind: 'color', attrs: { name: 'blue' } }]);
  });
});
