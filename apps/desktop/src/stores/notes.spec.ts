import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { noteFixture, stubLocalService, stubServiceDown } from '../testing/http';
import { useNoteStore } from './notes';
import { useFolderStore } from './folders';

function row(id: string, title: string, pinned = false): Record<string, unknown> {
  return { id, title, summary: `${title} 的摘要`, pinned, charCount: 12, hasAttachment: false, updatedAt: '2026-01-02T00:00:00Z', deletedAt: null };
}

beforeEach(() => {
  setActivePinia(createPinia());
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

describe('即时搜索', () => {
  it('连续输入只在停止 200ms 后打一次请求', async () => {
    const service = stubLocalService({
      search: () => [{ noteId: 'note-1', score: 1, title: '第一条', snippetHtml: '<b>甲</b>乙' }],
      list_notes: () => [],
    });
    const notes = useNoteStore();
    notes.requestSearch('甲');
    notes.requestSearch('甲乙');
    notes.requestSearch('甲乙丙');
    await vi.advanceTimersByTimeAsync(80);
    notes.requestSearch('甲乙丙丁');
    expect(service.callsOf('search')).toHaveLength(0);
    await vi.advanceTimersByTimeAsync(220);
    expect(service.callsOf('search')).toHaveLength(1);
    expect(service.lastArgsOf('search')).toEqual({ text: '甲乙丙丁', limit: 80 });
    expect(notes.searching).toBe(false);
  });

  it('空查询不发请求，并清掉结果', async () => {
    const service = stubLocalService({ search: () => [], list_notes: () => [] });
    const notes = useNoteStore();
    notes.requestSearch('内容');
    await vi.advanceTimersByTimeAsync(220);
    expect(service.callsOf('search')).toHaveLength(1);
    notes.requestSearch('   ');
    await vi.advanceTimersByTimeAsync(500);
    expect(service.callsOf('search')).toHaveLength(1);
    expect(notes.hits).toBeNull();
    expect(notes.query).toBe('');
    expect(notes.searching).toBe(false);
  });

  /**
   * §3.3 的后一半（2026-10-09 用户拍板："搜索回满 80 就提示还有 N 条"）。
   * 两条腿一起才成形状：**回满时问一次**、**没回满时不问**（不出现那句 = 它不该出现时才不出现）。
   */
  it('回满一页（80 条）时才去问一次总数，读数留给界面说"还有 N 条"', async () => {
    const eighty = Array.from({ length: 80 }, (_, i) => ({
      noteId: `n${i}`, score: 1, title: `第 ${i} 篇`, snippetHtml: '片段', exact: true,
    }));
    const service = stubLocalService({
      search: () => eighty,
      search_total: () => ({ total: 120, cap: 200 }),
      list_notes: () => [],
    });
    const notes = useNoteStore();
    notes.requestSearch('同步');
    await vi.advanceTimersByTimeAsync(300);
    expect(service.callsOf('search_total').length, '回满了就必须问一次总数').toBe(1);
    expect(service.lastArgsOf('search_total')).toEqual({ text: '同步', cap: 200 });
    expect(notes.searchTotal).toEqual({ total: 120, cap: 200 });
  });

  it('没回满时不问总数 —— 界面上那句"还有"也就根本不会出现', async () => {
    const service = stubLocalService({
      search: () => [{ noteId: 'n1', score: 1, title: '一篇', snippetHtml: '片段', exact: true }],
      search_total: () => ({ total: 1, cap: 200 }),
      list_notes: () => [],
    });
    const notes = useNoteStore();
    notes.requestSearch('同步');
    await vi.advanceTimersByTimeAsync(300);
    expect(service.callsOf('search_total').length, '没回满还去问，就是每次搜索都白付一次往返').toBe(0);
    expect(notes.searchTotal).toBe(null);
  });

  it('一个字、emoji、超长查询都不报错，超长会被裁剪', async () => {
    const service = stubLocalService({ search: () => [], list_notes: () => [] });
    const notes = useNoteStore();
    notes.requestSearch('甲');
    await vi.advanceTimersByTimeAsync(220);
    notes.requestSearch('🙂');
    await vi.advanceTimersByTimeAsync(220);
    notes.requestSearch('长'.repeat(600));
    await vi.advanceTimersByTimeAsync(220);
    const args = service.callsOf('search').map((call) => String((call.args as { text: string }).text));
    expect(args[1]).toBe('🙂');
    expect((args[2] ?? '').length).toBe(200);
    expect(notes.searchErrorKey).toBeNull();
  });

  it('后端报错时给出文案键，不显示原始码', async () => {
    stubLocalService({
      search: () => ({ ok: false, error: { code: 'server_unavailable', messageKey: 'server_unavailable' } }),
      list_notes: () => [],
    });
    const notes = useNoteStore();
    notes.requestSearch('任何词');
    await vi.advanceTimersByTimeAsync(220);
    expect(notes.searchErrorKey).toBe('server_unavailable');
    expect(notes.hits).toEqual([]);
    expect(notes.searching).toBe(false);
  });

  /**
   * 命中行的标题那一格：核心一直在发 `title`（真 dev 桥实测：连"第一段是空的"那种笔记，
   * 它给的也是**正文第一块**那句话），而 store 里那份"拿片段前 40 字猜标题"是从前
   * DTO 还没这个键的年代留下来的（`git log -S` 到初始提交）。
   * 它只在**这篇不在已载入的列表里**时才露头（在文件夹视图里搜、或库里 200 条开外）——
   * 露出来的就是列表上「标题」与「摘要」两行写着同一段正文。
   */
  it('命中行的标题用核心给的那一格，不拿片段猜（缺口 G93）', async () => {
    stubLocalService({
      // 列表里没有这一篇，于是 titles 是空的 —— 正是那条旧退路会露头的形状。
      list_notes: () => [],
      search: () => [{ noteId: 'far-1', score: 1, title: '真正的标题', snippetHtml: '<mark>甲</mark>乙的正文片段', exact: true }],
    });
    const notes = useNoteStore();
    notes.requestSearch('甲乙');
    await vi.advanceTimersByTimeAsync(220);
    expect(notes.titles['far-1']).toBe('真正的标题');
  });

  it('核心说这篇没有标题，界面就不许给它编一个（退「未命名」）', async () => {
    stubLocalService({
      list_notes: () => [],
      search: () => [{ noteId: 'blank-1', score: 1, title: '', snippetHtml: '<mark>甲</mark>乙的正文片段', exact: true }],
    });
    const notes = useNoteStore();
    notes.requestSearch('甲乙');
    await vi.advanceTimersByTimeAsync(220);
    expect(notes.titles['blank-1']).toBe('');
  });

  it('搜索中/无结果两态可区分', async () => {
    stubLocalService({ search: () => [], list_notes: () => [] });
    const notes = useNoteStore();
    notes.requestSearch('不存在');
    expect(notes.searching).toBe(true);
    await vi.advanceTimersByTimeAsync(220);
    expect(notes.searching).toBe(false);
    expect(notes.hits).toEqual([]);
  });
});

describe('列表与回收站', () => {
  it('列表只取行数据，不发文档体请求', async () => {
    const service = stubLocalService({ list_notes: () => [row('note-1', '甲', true), row('note-2', '乙')] });
    const notes = useNoteStore();
    await notes.load();
    expect(notes.rows).toHaveLength(2);
    expect(notes.pinnedRows.map((item) => item.id)).toEqual(['note-1']);
    expect(service.callsOf('list_notes')[0]?.args).toEqual({ limit: 200, offset: 0, trash: false });
    expect(service.callsOf('get_note')).toHaveLength(0);
  });

  it('回收站视图带 trash 标记，恢复/彻底删除走对应命令', async () => {
    const service = stubLocalService({ list_notes: () => [row('note-9', '被删的')] });
    const notes = useNoteStore();
    await notes.setMode({ kind: 'trash' });
    expect(service.callsOf('list_notes')[0]?.args).toEqual({ limit: 200, offset: 0, trash: true });
    await notes.restore('note-9');
    await notes.purge('note-9');
    expect(service.callsOf('restore_note')[0]?.args).toEqual({ id: 'note-9' });
    expect(service.callsOf('purge_note')[0]?.args).toEqual({ id: 'note-9' });
  });

  it('新建笔记后自动选中', async () => {
    const service = stubLocalService({ create_note: (args) => noteFixture({ id: 'new-1', title: '无标题', doc: args.doc }), list_notes: () => [] });
    const notes = useNoteStore();
    const created = await notes.create('folder-1');
    expect(created?.id).toBe('new-1');
    expect(notes.selectedId).toBe('new-1');
    const args = service.lastArgsOf('create_note') as { folderId: string; doc: { v: number; content: unknown[] } };
    expect(args.folderId).toBe('folder-1');
    expect(args.doc.v).toBe(1);
    expect(args.doc.content).toHaveLength(1);
  });

  it('移动与固定命令使用契约字段名', async () => {
    const service = stubLocalService({
      set_note_folder: () => noteFixture({ folderId: 'folder-2' }),
      set_note_pinned: () => noteFixture({ pinned: true }),
      list_notes: () => [],
    });
    const notes = useNoteStore();
    await notes.moveTo('note-1', 'folder-2');
    await notes.setPinned('note-1', true);
    expect(service.lastArgsOf('set_note_folder')).toEqual({ id: 'note-1', folderId: 'folder-2' });
    expect(service.lastArgsOf('set_note_pinned')).toEqual({ id: 'note-1', pinned: true });
    expect(notes.rowById('note-1')?.pinned).toBe(true);
  });

  it('本地服务不可用时列表进入错误态而不是抛异常', async () => {
    const notes = useNoteStore();
    vi.stubGlobal('fetch', async () => {
      throw new TypeError('Failed to fetch');
    });
    await expect(notes.load()).resolves.toBeUndefined();
    expect(notes.errorKey).toBe('link.unreachable');
    expect(notes.rows).toEqual([]);
    expect(notes.loading).toBe(false);
  });

  it('文件夹树支持平铺输入（后端给哪种都能用）', async () => {
    stubLocalService({
      list_folders: () => [
        { id: 'f1', parentId: null, name: '工作' },
        { id: 'f2', parentId: 'f1', name: '项目' },
        { id: 'f3', parentId: 'f2', name: '细节' },
      ],
    });
    const folders = useFolderStore();
    await folders.load();
    expect(folders.nodes[0]?.name).toBe('工作');
    expect(folders.nodes[0]?.children[0]?.children[0]?.name).toBe('细节');
    expect(folders.flat.map((entry) => entry.node.name)).toEqual(['工作', '项目', '细节']);
  });
});

describe('「今天」这一格（日记）', () => {
  it('按一次只打一条 daily_note，并把核心认出的那一篇选上并打开', async () => {
    const service = stubLocalService({
      daily_note: () => ({
        note: noteFixture({ id: 'd-1', title: '2026-09-29' }),
        day: '2026-09-29',
        created: true,
      }),
      list_notes: () => [row('d-1', '2026-09-29')],
      get_note: () => noteFixture({ id: 'd-1', title: '2026-09-29' }),
    });
    const notes = useNoteStore();
    await notes.openToday();
    // 日子归核心算：界面这里一次调用都不许自己拼日期或按标题找（那是第二份判据）。
    expect(service.callsOf('daily_note')).toHaveLength(1);
    expect(service.lastArgsOf('daily_note')).toEqual({});
    expect(notes.selectedId).toBe('d-1');
    expect(service.callsOf('get_note').length).toBeGreaterThan(0);
  });

  it('在别的文件夹视图里按「今天」：视图要换到看得见它的位置', async () => {
    // 日记落在**默认本**，所以"当前正在看某个子文件夹"时，那一行不在眼前的列表里。
    // 编辑器开了而列表里没有它 = 用户读到的是"这颗按钮没反应"，所以视图必须换。
    const service = stubLocalService({
      daily_note: () => ({
        note: noteFixture({ id: 'd-1', title: '2026-09-29', folderId: 'f-default' }),
        day: '2026-09-29',
        created: true,
      }),
      list_notes: () => [row('d-1', '2026-09-29')],
      get_note: () => noteFixture({ id: 'd-1', title: '2026-09-29', folderId: 'f-default' }),
    });
    const notes = useNoteStore();
    await notes.setMode({ kind: 'folder', folderId: 'f-other' });
    await notes.openToday();
    expect(notes.mode.kind).toBe('all');
    expect(notes.selectedId).toBe('d-1');
    expect(service.callsOf('daily_note')).toHaveLength(1);
  });

  it('核心报错时把具名文案交出去，并且不选中任何一篇', async () => {
    const service = stubLocalService({
      daily_note: () => {
        throw Object.assign(new Error('cmd'), {
          code: 'no_default_folder',
          messageKey: 'error.no_default_folder',
          retryable: false,
        });
      },
      list_notes: () => [],
    });
    const notes = useNoteStore();
    const got = await notes.openToday();
    expect(got).toBeNull();
    expect(notes.selectedId).toBeNull();
    expect(service.callsOf('get_note')).toHaveLength(0);
  });
});

/**
 * §6「颜色」的笔记那一半（当「标签」用）。三条都打在**调用边**上：
 * 「屏幕上多了一颗点」可以是画上去的假象，真凭据是"命令发没发、那一行收成了什么"。
 */
describe('标记色：笔记那颗点是真去核心写过一笔的', () => {
  it('挑一支 → 命令里带的就是那一支，行上的颜色跟着落下来', async () => {
    const service = stubLocalService({
      list_notes: () => [row('note-1', '甲')],
      set_note_color: () => noteFixture({ id: 'note-1', color: '#c2410c' }),
    });
    const notes = useNoteStore();
    await notes.load();
    expect(notes.rows[0]?.color ?? null, '前置：一开始不该有颜色').toBe(null);

    await notes.setColor('note-1', '#c2410c');
    expect(service.callsOf('set_note_color').length, '一次都没发命令：那颗点是画上去的假象').toBe(1);
    expect(service.lastArgsOf('set_note_color')).toEqual({ id: 'note-1', color: '#c2410c' });
    expect(notes.rows[0]?.color).toBe('#c2410c');
  });

  it('清掉发的是空串（核心那边空串 = 清掉），行上的颜色回到 null', async () => {
    const service = stubLocalService({
      list_notes: () => [{ ...row('note-1', '甲'), color: '#c2410c' }],
      set_note_color: () => noteFixture({ id: 'note-1', color: null }),
    });
    const notes = useNoteStore();
    await notes.load();
    expect(notes.rows[0]?.color).toBe('#c2410c');

    await notes.setColor('note-1', '');
    expect(service.lastArgsOf('set_note_color')).toEqual({ id: 'note-1', color: '' });
    expect(notes.rows[0]?.color ?? null).toBe(null);
  });

  /** 失败时不许先把点抹掉再报错：那会让用户以为是自己点错了，而且下一次同步会拿这个假状态去比。 */
  it('命令没成（本地服务不在）时那一行保持原色，并把错留给界面说', async () => {
    stubLocalService({ list_notes: () => [{ ...row('note-1', '甲'), color: '#1e40af' }] });
    const notes = useNoteStore();
    await notes.load();
    stubServiceDown();
    await notes.setColor('note-1', '#c2410c');
    expect(notes.rows[0]?.color, '发失败了却已经把颜色改掉 —— 屏幕说的是假话').toBe('#1e40af');
    expect(typeof notes.errorKey === 'string' && notes.errorKey.length > 0, '失败没留下任何可显示的错误键').toBe(true);
  });
});
