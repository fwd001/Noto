/**
 * §3.3「搜索分档是新能力：后端已经算出"精准/模糊"…但被丢掉了。精准排前，模糊在后。」
 * §6 第 5 条把「搜索结果分档」列为"已实现未开放"。
 *
 * 核心那一侧**早就算完了**（`notera_store::SearchHit.match_kind`，`search.rs` 里
 * `if i < exact_count` 那一格），丢的是**过桥那一刀**：`SearchHitDto` 只带
 * noteId/score/snippetHtml/title 四个键（真产物实测，不是从 TS 类型推的）。
 *
 * 这里盯两件事，缺一不可：
 *  1. 分档结论必须**逐条**从载荷里的 `exact` 字段读，不许按位置猜
 *     （所以有一条**模糊在前**的载荷：按 `i < exact_count` 那种写法这里必红，
 *     而它恰好就是核心内部的那句判据 —— 界面抄一遍位置逻辑就是第二份判据）；
 *  2. 界面不许自己重排序（"精准排前"是核心的承诺，界面重排会把它掩盖掉）。
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { stubLocalService } from '../testing/http';
import { useNoteStore } from './notes';

/** 一条命中：`exact` 就是核心 DTO 里那一格。 */
function hit(noteId: string, exact: boolean): Record<string, unknown> {
  return { noteId, score: 1, title: `标题 ${noteId}`, snippetHtml: '<mark>甲</mark>乙', exact };
}

beforeEach(() => {
  setActivePinia(createPinia());
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

/** 发一次搜索并等过防抖。 */
async function search(text: string): Promise<void> {
  useNoteStore().requestSearch(text);
  await vi.advanceTimersByTimeAsync(220);
}

describe('搜索结果分档（§3.3）', () => {
  it('按载荷里的 exact 逐条计数，不是"前 N 条算精准"', async () => {
    stubLocalService({
      list_notes: () => [],
      // 模糊穿插在两处：位置写法在这里有两种，都会数错 ——
      //  「前 exact_count 条算精准」（抄核心内部那句 `i < exact_count`）数出 1，
      //  「除最后一条都算精准」数出 4。真字段是 **3**。
      search: () => [hit('e1', true), hit('f1', false), hit('e2', true), hit('f2', false), hit('e3', true)],
    });
    await search('甲乙');
    const notes = useNoteStore();
    expect(notes.searchTiers).toEqual({ exact: 3, fuzzy: 2 });
  });

  it('原样保留核心给的回顺序：界面不重排，也不自己算档', async () => {
    const service = stubLocalService({
      list_notes: () => [],
      search: () => [hit('f1', false), hit('e1', true)],
    });
    await search('甲乙');
    const notes = useNoteStore();
    // 顺序是核心的承诺（精准排前）。这一条载荷本身就"不规范"，
    // 界面必须照原样交给列表，否则列表上看到的顺序就不再是核心的顺序了。
    expect(notes.hits?.map((h) => h.noteId)).toEqual(['f1', 'e1']);
    expect(notes.searchTiers).toEqual({ exact: 1, fuzzy: 1 });
    expect(service.callsOf('search')).toHaveLength(1);
  });

  it('没搜索时没有分档；搜到零条是两个 0（不是"没数据"）', async () => {
    stubLocalService({ list_notes: () => [], search: () => [] });
    const notes = useNoteStore();
    expect(notes.searchTiers).toBeNull();
    await search('甲乙');
    expect(notes.searchTiers).toEqual({ exact: 0, fuzzy: 0 });
  });

  it('旧版核心的载荷缺 exact 这一格时，不许把全部命中报成精准', async () => {
    // 退回路径（用户机器上跑着没升级的核心/桥）：缺字段只能读成 false，
    // 于是「精准 0 · 模糊 N」。反过来（缺字段当成精准）会当着用户面说谎。
    stubLocalService({
      list_notes: () => [],
      search: () => [{ noteId: 'x1', score: 1, title: '甲', snippetHtml: '<mark>甲</mark>' }],
    });
    await search('甲');
    expect(useNoteStore().searchTiers).toEqual({ exact: 0, fuzzy: 1 });
  });

  it('清掉搜索与换列表模式都把分档归零', async () => {
    stubLocalService({ list_notes: () => [], search: () => [hit('e1', true)] });
    const notes = useNoteStore();
    await search('甲乙');
    expect(notes.searchTiers).toEqual({ exact: 1, fuzzy: 0 });
    notes.clearSearch();
    expect(notes.searchTiers).toBeNull();
    await search('甲乙');
    await notes.setMode({ kind: 'all' });
    expect(notes.searchTiers).toBeNull();
  });
});
