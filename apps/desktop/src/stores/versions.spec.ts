/**
 * §6「版本历史浏览」的前端读侧与回滚调用边（缺口 G100）。
 *
 * 与 `note_revisions.rs`（Rust 那五条）的分工：那边钉"桥上有哪些键、谁算的哪一位"，
 * 这边钉"界面拿这份载荷说的三句话对不对、以及覆盖那一发到底怎么出门"。
 *
 * 覆盖这一条最值得钉：它**必须**走 `edit_note` 且 `expectedRev` 用的是列表里那个 currentRev。
 * 写成"直接 PUT 一份正文"或"不带 expectedRev"的实现，功能上今天也能动 ——
 * 代价是绕过 CAS，两个窗口都开着时后一次会悄悄盖掉前一次（这一族在编辑器保存那边踩过）。
 */
import { beforeEach, describe, expect, it } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { stubLocalService } from '../testing/http';
import { docToLines, useVersionsStore } from './versions';

const docOf = (...lines: string[]) => ({
  v: 1,
  content: lines.map((text, i) => ({ id: `blk${i}`, type: 'paragraph', content: [{ text }] })),
});

function payload(over: Record<string, unknown> = {}) {
  return {
    rows: [
      { rev: 3, origin: 'local', deviceId: 'dev-a', createdAt: '2026-10-08T09:15:30Z', sameAsNow: true },
      { rev: 2, origin: 'conflict_copy', deviceId: 'dev-b', createdAt: '2026-10-07T09:15:30Z', sameAsNow: false },
      { rev: 1, origin: 'restored', deviceId: 'dev-a', createdAt: '2026-10-06T09:15:30Z', sameAsNow: false },
    ],
    currentRev: 3,
    syncRev: 2,
    truncated: false,
    ...over,
  };
}

beforeEach(() => {
  setActivePinia(createPinia());
});

describe('版本历史的读侧', () => {
  it('列表原样搬过来：倒序、六个数之外不加工，"有没有没公告的版"由 rev 与 syncRev 比出来', async () => {
    stubLocalService({ note_revisions: () => payload() });
    const versions = useVersionsStore();
    await versions.load('note-1');
    expect(versions.failed).toBe(false);
    expect(versions.rows.map((r) => r.rev)).toEqual([3, 2, 1]);
    expect(versions.hasUnpublished, 'rev 3 > syncRev 2 ⇒ 最新一版还没传上去').toBe(true);
    expect(versions.truncated).toBe(false);

    // 第二个数据点：追平之后就不许再说"还有没公告的"（只有上面那一个点，`>` 写成 `>=` 照样绿）。
    stubLocalService({ note_revisions: () => payload({ syncRev: 3 }) });
    await versions.load('note-1');
    expect(versions.hasUnpublished, 'rev 与 syncRev 相等 = 这一版已经公告出去了').toBe(false);
  });

  it('换一篇笔记要先把上一份的现场清掉（留着就会在这一篇的格子里看到那一篇的版本）', async () => {
    stubLocalService({ note_revisions: () => payload() });
    const versions = useVersionsStore();
    await versions.load('note-1');
    await versions.openPreview('note-1', 2);
    stubLocalService({
      note_revisions: () => payload({ rows: [], currentRev: 0, syncRev: 0 }),
      note_revision: () => ({ rev: 2, doc: docOf('不该还在') }),
    });
    await versions.load('note-2');
    expect(versions.rows).toEqual([]);
    expect(versions.preview, '预览是上一篇文章的 ⇒ 必须跟着清').toBeNull();
    expect(versions.noteId).toBe('note-2');
  });

  it('读失败 ⇒ failed 为真且不留旧账；"扫过且只有一版"与"没读到"是两句话', async () => {
    stubLocalService({ note_revisions: () => payload() });
    const versions = useVersionsStore();
    await versions.load('note-1');
    stubLocalService({}); // 桥不认这条命令了
    await versions.load('note-1');
    expect(versions.failed).toBe(true);
    expect(versions.rows).toEqual([]);
    expect(versions.currentRev).toBe(0);
  });

  it('某一版读不出来要说"读不出来"，不许画成"这一版什么都没写"，也不许留着上一版的正文', async () => {
    stubLocalService({ note_revision: () => ({ rev: 2, doc: docOf('第二版的正文') }) });
    const versions = useVersionsStore();
    await versions.load('note-1');
    await versions.openPreview('note-1', 2);
    expect(versions.previewLines).toEqual(['第二版的正文']);

    stubLocalService({ note_revision: () => ({ ok: false, code: 'not_found' }) });
    await versions.openPreview('note-1', 9);
    expect(versions.preview, '上一版的正文还挂在格子里 ⇒ 用户会把第二版看成第九版').toBeNull();
    expect(versions.previewFailed).toBe(true);
  });

  it('docToLines：只取有字的块，顺序与正文一致（预览那一格读的就是它）', () => {
    expect(docToLines(docOf('第一行', '', '第三行') as never)).toEqual(['第一行', '第三行']);
    expect(docToLines(null)).toEqual([]);
    expect(docToLines({ v: 1 } as never)).toEqual([]);
  });
});

describe('覆盖那一发的调用边', () => {
  it('走 edit_note，expectedRev 用的是列表里那个 currentRev，正文就是那一版的 doc', async () => {
    const old = docOf('第一版的正文');
    const calls: { name: string; args: Record<string, unknown> }[] = [];
    stubLocalService({
      note_revisions: () => payload(),
      note_revision: () => ({ rev: 1, doc: old }),
      edit_note: (args: Record<string, unknown>) => {
        calls.push({ name: 'edit_note', args });
        return { id: 'note-1', rev: 4 };
      },
      get_note: () => ({ id: 'note-1', doc: old, rev: 4, syncRev: 2, folderId: null }),
    });
    const versions = useVersionsStore();
    await versions.load('note-1');
    const to = await versions.restore('note-1', 1);
    expect(to).toBe(4);
    expect(calls).toHaveLength(1);
    expect(calls[0].args.expectedRev, '不带 CAS 的覆盖 = 绕过唯一写出口的旁路').toBe(3);
    expect(calls[0].args.doc).toEqual(old);
  });

  it('核心拒了（stale_edit）就回 null，界面据此说"没覆盖成功"，不假装成功', async () => {
    stubLocalService({
      note_revisions: () => payload(),
      note_revision: () => ({ rev: 1, doc: docOf('旧正文') }),
      edit_note: () => ({ ok: false, code: 'stale_edit' }),
    });
    const versions = useVersionsStore();
    await versions.load('note-1');
    expect(await versions.restore('note-1', 1)).toBeNull();
  });

  it('那一版的正文读不出来 ⇒ 一发 edit_note 都不许出门（拿空正文覆盖就是把笔记清空）', async () => {
    const calls: string[] = [];
    stubLocalService({
      note_revisions: () => payload(),
      note_revision: () => ({ ok: false, code: 'not_found' }),
      edit_note: () => {
        calls.push('edit_note');
        return { rev: 99 };
      },
    });
    const versions = useVersionsStore();
    await versions.load('note-1');
    expect(await versions.restore('note-1', 42)).toBeNull();
    expect(calls, '读不到旧正文还去覆盖 ⇒ 会写出一份空正文').toEqual([]);
  });
});

// 覆盖成功之后"编辑器要按新的那一版重读"这一条归浏览器腿 51 管：
// 它量的是屏幕上真的是那一版正文，而不是某个函数被叫到。
