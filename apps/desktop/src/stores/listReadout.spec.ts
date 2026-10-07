/**
 * 列表栏底部那一格（设计稿第 1 页列表栏最后一行「共 128 条 · 按更新时间排列」）。
 *
 * 三条不是装饰而是判据的理由：
 *  · 「共 N 条」在**还有下一页没取回来**时就是谎话（一次只取 200 行），所以那一格必须换句子；
 *  · 搜索时不许出现这一格 —— 上面已经在说「找到 N 条」，两个数一起说的是两件不同的事；
 *  · 那句"按更新时间"在本产品里是半句假话（核心的真序是 `pinned DESC, updated_at DESC, id`，
 *    见 `notera-store/src/store.rs:1506`），所以文案写的是「置顶在前，按更新时间」，
 *    而顺序本身由 `verify-layout` 腿 ㊷ 打在渲染后的行序上（文案声称了顺序，就得有人验顺序）。
 * 顺带把那颗**定义了却零调用**的死键 `list.rowsLoaded` 上岗。
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { stubLocalService } from '../testing/http';
import { t } from '../i18n';
import { useNoteStore } from './notes';

function row(id: string, pinned = false): Record<string, unknown> {
  return { id, title: `条 ${id}`, summary: '', pinned, charCount: 3, hasAttachment: false, updatedAt: '2026-10-01T00:00:00Z', deletedAt: null, folderId: 'f1' };
}

beforeEach(() => {
  setActivePinia(createPinia());
  vi.useRealTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

describe('列表底部读数（设计稿那一行）', () => {
  it('一页就装完了 ⇒ 说「共 N 条」，并带上真实的那句顺序', async () => {
    stubLocalService({ list_notes: () => [row('a'), row('b'), row('c')] });
    const notes = useNoteStore();
    await notes.load();
    expect(notes.listReadout).toEqual({ key: 'list.totalCount', count: 3 });
    expect(t(notes.listReadout!.key, { count: notes.listReadout!.count })).toBe('共 3 条 · 置顶在前，按更新时间');
  });

  it('回满一页（还有下一页）⇒ 只能说「已载入 N 条」，不许报「共」', async () => {
    stubLocalService({ list_notes: () => Array.from({ length: 200 }, (_, i) => row(`n${i}`)) });
    const notes = useNoteStore();
    await notes.load();
    expect(notes.hasMore).toBe(true);
    expect(notes.listReadout).toEqual({ key: 'list.rowsLoaded', count: 200 });
    expect(t(notes.listReadout!.key, { count: notes.listReadout!.count })).toBe('已载入 200 条 · 置顶在前，按更新时间');
  });

  it('搜索态不画这一格（那句归「找到 N 条」说）', async () => {
    stubLocalService({
      list_notes: () => [row('a')],
      search: () => [{ noteId: 'a', score: 1, title: '条 a', snippetHtml: '<mark>甲</mark>', exact: true }],
    });
    const notes = useNoteStore();
    await notes.load();
    expect(notes.listReadout).not.toBeNull();
    await notes.runSearch('甲');
    expect(notes.hits).toHaveLength(1);
    expect(notes.listReadout).toBeNull();
  });

  it('空库不画（那一格归空态文案）', async () => {
    stubLocalService({ list_notes: () => [] });
    const notes = useNoteStore();
    await notes.load();
    expect(notes.listReadout).toBeNull();
  });

  it('换到回收站那一栏时数的是这一栏的行，不是整库', async () => {
    stubLocalService({
      list_notes: (args) => (args.trash === true ? [row('t1'), row('t2')] : [row('a'), row('b'), row('c')]),
    });
    const notes = useNoteStore();
    await notes.setMode({ kind: 'trash' });
    expect(notes.listReadout).toEqual({ key: 'list.totalCount', count: 2 });
  });
});
