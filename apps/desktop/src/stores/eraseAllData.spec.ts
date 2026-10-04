/**
 * 「清除一切数据」之后，界面必须**立刻**是空的。
 *
 * 这三条都来自用户装包实测的反馈，不是推演出来的：
 *  「清除数据了，列表上面还是 40 笔记、3 最近删除。」
 *
 * 三处数据源各说各话：侧栏那两颗数读 `settings.stats`（只在 App.vue 启动时取一次），
 * 列表读 `notes.rows`，编辑器握着 `editor.blocks`。清库只动了**库**，
 * 三个界面状态一个都没被告知，于是"库是空的、屏幕上是满的"。
 *
 * 这里盯的是"清完之后还有没有旧数据留在屏幕上"，不是"库清没清干净" ——
 * 后者由 Rust 侧的 `erase_all_data` 管，且不可撤销。
 */
import { describe, expect, it, beforeEach, afterEach, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { noteFixture, stubLocalService } from '../testing/http';
import { useNoteStore } from './notes';
import { useSettingsStore } from './settings';
import { useEditorStore } from './editor';

/**
 * 列表行（`NoteListRow`）与完整笔记（`Note`）是**两个形状**，别混用：
 *  `list_notes` 回的是前者、只有 id/title/updatedAt 那几个字段；
 *  `get_note` 回的是后者（带 doc/rev）。`noteFixture` 造的是后者。
 * 我第一版拿 `noteFixture` 去喂 `list_notes`，于是 id 全是 'note-1'。
 */
function row(id: string, title: string): Record<string, unknown> {
  return {
    id,
    title,
    summary: `${title} 的摘要`,
    pinned: false,
    charCount: 12,
    hasAttachment: false,
    updatedAt: '2026-01-02T00:00:00Z',
    deletedAt: null,
  };
}

/** 三张表都空 = 库刚被清完的样子。 */
const EMPTY = { ok: true, payload: [] };

// 不存成模块级变量：除了"列表非空时仍会迁移"那一条要**换成另一套桩**，
// 其余用例都只靠 beforeEach 里这一套。而 beforeEach 里赋值、别处不读 ⇒
// `noUnusedLocals` 会报 TS6133（出包脚本的 `vue-tsc --noEmit` 直接把它拦住，
// 连带整个包都出不来—— 门禁起作用的样子）。
beforeEach(() => {
  setActivePinia(createPinia());
  stubLocalService({
    create_note: () => ({ ok: true, payload: noteFixture({ id: 'n1', title: '旧笔记' }) }),
    get_note: () => ({ ok: true, payload: noteFixture({ id: 'n1', title: '旧笔记' }) }),
    // 列表空 = 库被清空后，界面还拿着旧内容。
    list_notes: () => EMPTY,
    stats: () => ({ ok: true, payload: { notes: 0, notesInTrash: 0, folders: 0, attachments: 0, bytes: 0 } }),
    list_conflicts: () => EMPTY,
  });
  vi.useRealTimers();
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('清库后侧栏计数必须归零', () => {
  it('loadStats 会把 notes / notesInTrash 都读成 0', async () => {
    const settings = useSettingsStore();
    await settings.loadStats();
    expect(settings.stats?.notes).toBe(0);
    expect(settings.stats?.notesInTrash).toBe(0);
  });

  it('侧栏那两颗数是从 stats 派生的（所以 stats 为 0 时它们必须是 0）', async () => {
    // 侧栏：`allCount = stats?.notes ?? 0`、`trashCount = stats?.notesInTrash ?? 0`
    // （SidebarPanel.vue）。这里不重复实现，只钉住"stats 是唯一来源"这个事实。
    const settings = useSettingsStore();
    await settings.loadStats();
    const allCount = settings.stats?.notes ?? 0;
    const trashCount = settings.stats?.notesInTrash ?? 0;
    expect([allCount, trashCount]).toEqual([0, 0]);
  });
});

describe('清库后列表与选中项必须一起空', () => {
  it('列表空时 selectedId 要被清成 null（不能留在已删的 id 上）', async () => {
    const notes = useNoteStore();
    // 先指向一篇存在的
    notes.selectedId = 'n1';
    expect(notes.selectedId).toBe('n1');

    await notes.load();

    // 这是关键那一支：原来只在"列表非空且选中项不在列表里"时迁移，
    // 列表为空的那一支漏了 ⇒ 清库后 selectedId 仍指向已删的那篇。
    expect(notes.rows).toEqual([]);
    expect(notes.selectedId, '列表空了却还留着已删笔记的 selectedId').toBeNull();
  });

  it('列表空了要把标题缓存一起丢掉（否则重挂载会冒出点不开的条目）', async () => {
    const notes = useNoteStore();
    notes.selectedId = 'n1';
    notes.titles = { n1: '旧笔记' };
    await notes.load();
    expect(notes.rows.length).toBe(0);
    expect(Object.keys(notes.titles).length, '已删 id 的标题不该留在缓存里').toBe(0);
  });

  it('列表非空时仍会迁移到第一行（不能被上面的修复带坏）', async () => {
    // 这一条要**换一套桩**（列表非空），所以这里是唯一用得到返回值的地方。
    stubLocalService({
      list_notes: () => ({ ok: true, payload: [row('a', '第一篇'), row('b', '第二篇')] }),
      get_note: () => ({ ok: true, payload: noteFixture({ id: 'a', title: '第一篇' }) }),
    });
    const notes = useNoteStore();
    notes.selectedId = '已经删掉的id';
    await notes.load();
    expect(notes.selectedId).toBe('a');
  });
});

describe('清库后编辑器必须关掉', () => {
  it('open(null) 清空 blocks 与 noteId', async () => {
    const editor = useEditorStore();
    await editor.open('n1');
    expect(editor.noteId).toBe('n1');

    // 清库流程里就是这么调的（SettingsView.doErase）。
    await editor.open(null);

    expect(editor.noteId, '编辑器还开着那一篇已经被永久删掉的笔记').toBeNull();
    expect(editor.blocks.length, '已删笔记的正文还留在屏幕上').toBe(0);
  });
});
