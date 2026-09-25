import { describe, expect, it } from 'vitest';
import type { NoteDoc } from '../api/types';
import {
  SUPPORTED_DOC_VERSION,
  blocksToDoc,
  docToBlocks,
  deriveTitle,
  docVersionSupported,
  newBlockId,
  splitBlock,
  toggleMark,
  docCharCount,
  countUnknownBlocks,
} from './model';
import { changeType, cycleChecklist, splitAt } from './commands';

const SOURCE: NoteDoc = {
  v: 1,
  content: [
    { id: 'a1b2c3d4', type: 'heading', attrs: { level: 2, align: 'left', customKey: { deep: true } }, content: [{ text: '标题', marks: [{ kind: 'bold' }] }] },
    { id: 'e5f6g7h8', type: 'paragraph', attrs: { indent: 0 }, content: [{ text: '普通' }, { text: '加粗', marks: [{ kind: 'bold' }] }] },
    { id: 'j9k0l1m2', type: 'checklistItem', attrs: { checked: true, indent: 1 }, content: [{ text: '已完成' }] },
    { id: 'n3o4p5q6', type: 'codeBlock', attrs: { lang: 'rust' }, content: [{ text: 'fn main() {}\n' }] },
    { id: 'r7s8t9u0', type: 'attachment', attrs: { sha256: 'ab'.repeat(32), name: 'a.pdf', size: 1234 } },
    { id: 'v1w2x3y4', type: 'unknown:mermaid', attrs: { diagram: 'graph TD' }, content: [{ text: 'keep me' }], sinceVersion: 3 },
    { id: 'z5a6b7c8', type: 'blockquote', content: [{ text: '引用', marks: [{ kind: 'fontSize', attrs: { size: 3 } }] }] },
  ],
};

describe('Document ↔ 编辑器模型往返', () => {
  it('往返后与源文档逐字段相等（含未知块、未知属性、未知顶层字段）', () => {
    const round = blocksToDoc(docToBlocks(SOURCE));
    expect(round).toEqual(SOURCE);
  });

  it('未知块即使周围被编辑也原样保留', () => {
    const blocks = docToBlocks(SOURCE);
    const unknown = blocks.find((block) => block.shape === 'unknown');
    expect(unknown?.raw).toEqual(SOURCE.content[5]);
    const edited = changeType(blocks, 1, 'heading');
    const doc = blocksToDoc(edited.blocks);
    expect(doc.content[5]).toEqual(SOURCE.content[5]);
    expect(countUnknownBlocks(edited.blocks)).toBe(1);
  });

  it('type 不在表内的块也算未知块', () => {
    const doc: NoteDoc = { v: 1, content: [{ id: 'abcd1234', type: 'somedayTable', attrs: { x: 1 } }] };
    const blocks = docToBlocks(doc);
    expect(blocks[0]?.shape).toBe('unknown');
    expect(blocksToDoc(blocks)).toEqual(doc);
  });

  it('缺 id 的块被补上合法 id，其余字段不动', () => {
    const doc: NoteDoc = { v: 1, content: [{ id: '', type: 'paragraph', content: [{ text: 'x' }] }] };
    const blocks = docToBlocks(doc);
    expect(blocks[0]?.id).toMatch(/^[0-9a-z]{8}$/);
    expect(blocks[0]?.content).toEqual([{ text: 'x' }]);
  });

  it('空文档至少有一个段落，保证可输入', () => {
    expect(docToBlocks({ v: 1, content: [] })).toHaveLength(1);
  });
});

describe('块 id 稳定性', () => {
  it('样式、类型、清单切换都不改既有 id', () => {
    const before = docToBlocks(SOURCE).map((block) => block.id);
    let blocks = docToBlocks(SOURCE);
    blocks = changeType(blocks, 0, 'paragraph').blocks;
    blocks = cycleChecklist(blocks, 1).blocks;
    expect(blocks.map((block) => block.id)).toEqual(before);
  });

  it('切分保留左块 id，右块拿新 id', () => {
    const blocks = docToBlocks(SOURCE);
    const original = blocks[1]?.id;
    const edit = splitAt(blocks, 1, 2);
    expect(edit.blocks[1]?.id).toBe(original);
    expect(edit.blocks[2]?.id).not.toBe(original);
    expect(edit.blocks[2]?.id).toMatch(/^[0-9a-z]{8}$/);
  });

  it('id 只在文档内唯一即可：8 位 base32、字母表不含 i/l/o/u', () => {
    const ids = new Set<string>();
    for (let i = 0; i < 500; i += 1) ids.add(newBlockId());
    expect(ids.size).toBeGreaterThan(480);
    for (const id of ids) {
      expect(id).toMatch(/^[0123456789abcdefghjkmnpqrstvwxyz]{8}$/);
    }
  });
});

describe('派生与版本闸门', () => {
  it('标题取第一个非空块；切分后类型保持', () => {
    expect(deriveTitle(docToBlocks(SOURCE))).toBe('标题');
    const target = docToBlocks(SOURCE)[1];
    expect(target).toBeDefined();
    if (!target) return;
    const [left, right] = splitBlock(target, 2);
    expect([left, right].map((block) => block.type)).toEqual(['paragraph', 'paragraph']);
  });

  it('字符数不含换行', () => {
    expect(docCharCount(docToBlocks(SOURCE))).toBeGreaterThan(0);
  });

  it('doc.v 超过支持版本 → 只读闸门', () => {
    expect(docVersionSupported({ v: SUPPORTED_DOC_VERSION, content: [] })).toBe(true);
    expect(docVersionSupported({ v: SUPPORTED_DOC_VERSION + 1, content: [] })).toBe(false);
    expect(docVersionSupported(null)).toBe(true);
  });
});

describe('样式切换', () => {
  it('整段已加粗时再切换等于取消', () => {
    const start = [{ text: 'abc', marks: [{ kind: 'bold' }] }];
    const off = toggleMark(start, 0, 3, { kind: 'bold' });
    expect(off).toEqual([{ text: 'abc' }]);
    const on = toggleMark(off, 0, 3, { kind: 'bold' });
    expect(on).toEqual(start);
  });

  it('部分选区会把选区切成三段并保持顺序', () => {
    const result = toggleMark([{ text: 'abcdef' }], 2, 4, { kind: 'italic' });
    expect(result.map((inline) => inline.text)).toEqual(['ab', 'cd', 'ef']);
    expect(result[1]?.marks?.[0]?.kind).toBe('italic');
    expect(result[2]?.marks).toBeUndefined();
  });

  it('链接样式互斥：换地址不留下旧链接', () => {
    const first = toggleMark([{ text: 'x' }], 0, 1, { kind: 'link', attrs: { href: 'https://a.test' } });
    const second = toggleMark(first, 0, 1, { kind: 'link', attrs: { href: 'https://b.test' } }, { exclusive: ['link'] });
    expect(second).toHaveLength(1);
    expect(second[0]?.marks).toEqual([{ kind: 'link', attrs: { href: 'https://b.test' } }]);
  });
});
