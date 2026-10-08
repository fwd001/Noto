/**
 * 块级 diff 的判据（§6「版本历史」的 diff 那一半，缺口 G100）。
 *
 * 这一族的教训是"每个状态要有一个只触发它自己的输入"（M74 那次是一个夹具混两条规则，
 * 一条的兜底替另一条背书）。所以下面第一份夹具里 **same / changed / restyled / removed / added
 * 各占一块，且五块的文本互不相同** —— 任何一格把两种状态混起来，计数就会对不上。
 */
import { describe, expect, it } from 'vitest';
import type { NoteDoc } from '../api/types';
import { diffDocs } from './docDiff';

const para = (id: string, text: string, extra: Record<string, unknown> = {}) => ({
  id,
  type: 'paragraph',
  ...extra,
  content: [{ text }],
});

const doc = (...blocks: unknown[]): NoteDoc => ({ v: 1, content: blocks } as NoteDoc);

describe('块级 diff', () => {
  it('五种状态各占一块时，四个数分别对得上', () => {
    const oldDoc = doc(
      para('b-same', '两边一模一样'),
      para('b-changed', '旧的文字'),
      para('b-restyled', '字一样但格式不同'),
      para('b-removed', '现在没有了的那段'),
    );
    const nowDoc = doc(
      para('b-same', '两边一模一样'),
      para('b-changed', '新的文字'),
      para('b-restyled', '字一样但格式不同', { attrs: { bold: true } }),
      para('b-added', '后来新增的那段'),
    );
    const d = diffDocs(oldDoc, nowDoc);
    expect(d).toEqual({
      marks: {
        'b-same': 'same',
        'b-changed': 'changed',
        'b-restyled': 'restyled',
        'b-removed': 'removed',
      },
      removed: 1,
      changed: 1,
      restyled: 1,
      added: 1,
    });
  });

  it('字一样、格式不同不许算"一样"（加粗变了用户看得见）', () => {
    const d = diffDocs(doc(para('x', '一段话')), doc(para('x', '一段话', { marks: ['bold'] })));
    expect(d.changed, '文字没变就不该报"文字不同"').toBe(0);
    expect(d.restyled).toBe(1);
    expect(d.marks.x).toBe('restyled');
  });

  it('块换了位置还是 same（id 是锚，顺序不是内容）', () => {
    const a = doc(para('p1', '第一段'), para('p2', '第二段'));
    const b = doc(para('p2', '第二段'), para('p1', '第一段'));
    const d = diffDocs(a, b);
    expect([d.changed, d.restyled, d.removed, d.added], JSON.stringify(d)).toEqual([0, 0, 0, 0]);
  });

  it('同一句话被拆成两段 inline ⇒ 文字相同、结构不同：说"格式不同"，不说"一样"', () => {
    const split = { id: 's1', type: 'paragraph', content: [{ text: '粗 ' }, { text: '斜' }] };
    const whole = { id: 's1', type: 'paragraph', content: [{ text: '粗 斜' }] };
    const d = diffDocs(doc(split), doc(whole));
    expect(d.changed, '拼起来文字相同 ⇒ 不是"文字不同"').toBe(0);
    expect(d.restyled).toBe(1);
  });

  it('一边带空 attrs/marks、一边什么都不带 ⇒ 算"一样"（结构噪声不许说成"格式不同"）', () => {
    // 这条是被浏览器腿打红之后补的：等号两边不是同一份序列化 —— 库里存的那份没有空 `attrs`，
    // 编辑器现拼的那份每块都带 `attrs:{}` 与 inline 的 `marks:[]`。
    // 不剥空值，"覆盖之后重开同一版"就会被界面上说成"三段只是格式不同"，那是假话。
    const fromCore = doc({ id: 'n1', type: 'paragraph', content: [{ text: '同样一句话' }] });
    const fromEditor = doc({ id: 'n1', type: 'paragraph', attrs: {}, content: [{ text: '同样一句话', marks: [] }] });
    const d = diffDocs(fromCore, fromEditor);
    expect(d.restyled, '空 attrs / 空 marks 不算格式差别').toBe(0);
    expect(d.marks.n1).toBe('same');
  });

  it('非空的 marks 才算格式不同（真的加粗了要看得见）', () => {
    const plain = doc({ id: 'n1', type: 'paragraph', content: [{ text: '一段话' }] });
    const bold = doc({ id: 'n1', type: 'paragraph', content: [{ text: '一段话', marks: [{ type: 'bold' }] }] });
    expect(diffDocs(plain, bold).marks.n1).toBe('restyled');
  });
  it('旧版与现在完全相同 ⇒ 四个数全 0（界面上那句"这一版就是现在"的根据）', () => {
    const same = doc(para('a', '一'), para('b', '二'));
    expect(diffDocs(same, doc(para('a', '一'), para('b', '二')))).toEqual({
      marks: { a: 'same', b: 'same' },
      removed: 0,
      changed: 0,
      restyled: 0,
      added: 0,
    });
  });

  it('缺 id 的块与空文档都不炸、也不计数', () => {
    expect(diffDocs(null, doc(para('a', '一')))).toEqual({ marks: {}, removed: 0, changed: 0, restyled: 0, added: 1 });
    expect(diffDocs(doc({ type: 'paragraph', content: [{ text: '没 id' }] } as never), doc())).toEqual({
      marks: {}, removed: 0, changed: 0, restyled: 0, added: 0,
    });
  });
});
