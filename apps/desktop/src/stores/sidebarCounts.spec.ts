/**
 * 侧栏计数与真实数据一致。
 *
 * 这条是被真机走查打出来的：点「新建笔记」，列表里条目确实多了，
 * 而侧栏「全部笔记」旁边那颗数字**一动不动**（实测 25 → 25，连点三次都是 25）。
 *
 * 根因：侧栏数字读的是 `settings.stats`，而 `settings.loadStats()` 此前
 * **只在 App.vue 启动时调一次** —— 新建/删除/恢复/永久删除四条路径都没刷。
 * 界面于是"列表说 12 条、侧栏说 25 条"，两块对不上，且没有任何一句话解释。
 *
 * 判据钉的是**那四条路径各自都会触发一次 `stats`**：不是"列表变长"
 * （那个 `rows` 会被分页掩盖），而是统计命令真的被发了。
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { stubLocalService } from '../testing/http';
import { useNoteStore } from './notes';
import { useSettingsStore } from './settings';

const note = (id: string, title: string): Record<string, unknown> => ({
  id,
  title,
  summary: '',
  pinned: false,
  charCount: 0,
  hasAttachment: false,
  updatedAt: '2026-01-02T00:00:00Z',
  deletedAt: null,
});

/**
 * `stubLocalService` **自己**就调了 `vi.stubGlobal('fetch', impl)`（见 testing/http.ts:44），
 * 返回的是 `{ calls, callsOf, lastArgsOf }` —— **没有 `fetch` 字段**。
 * 我第一版写 `vi.stubGlobal('fetch', svc.fetch)` ⇒挂上去的是 undefined，
 * 于是所有命令都没真走到桩上，6 条全红且读数是"0 次调用"。
 */
let svc: ReturnType<typeof stubLocalService>;

function statsCalls(): number {
  return svc.callsOf('stats').length;
}

beforeEach(() => {
  setActivePinia(createPinia());
  svc = stubLocalService({
    create_note: () => ({ ok: true, payload: note('11111111-1111-7111-8111-111111111111', '新笔记') }),
    delete_note: () => ({ ok: true, payload: null }),
    restore_note: () => ({ ok: true, payload: null }),
    purge_note: () => ({ ok: true, payload: null }),
    list_notes: () => ({ ok: true, payload: [] }),
    get_note: () => ({ ok: true, payload: note('11111111-1111-7111-8111-111111111111', '新笔记') }),
    stats: () => ({ ok: true, payload: { notes: 26, notesInTrash: 0, folders: 1, attachments: 0, bytes: 4096 } }),
  });
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('笔记条数变了就要刷侧栏计数', () => {
  it('新建笔记会拉一次 stats', async () => {
    const before = statsCalls();
    await useNoteStore().create(null);
    expect(statsCalls()).toBe(before + 1);
  });

  it('移到最近删除会拉一次 stats（总数不变但回收站变了）', async () => {
    const before = statsCalls();
    await useNoteStore().moveToTrash('11111111-1111-7111-8111-111111111111');
    expect(statsCalls()).toBe(before + 1);
  });

  it('从回收站恢复会拉一次 stats（两处计数都变了）', async () => {
    const before = statsCalls();
    await useNoteStore().restore('11111111-1111-7111-8111-111111111111');
    expect(statsCalls()).toBe(before + 1);
  });

  it('永久删除会拉一次 stats（总数真的少了）', async () => {
    const before = statsCalls();
    await useNoteStore().purge('11111111-1111-7111-8111-111111111111');
    expect(statsCalls()).toBe(before + 1);
  });

  it('统计刷新不阻塞主流程：命令失败也要让 create 返回笔记', async () => {
    // stats 是旁路。若把它 await 进主流程，一个统计失败就会把"建好了笔记"
    // 报成失败 —— 那是拿一件可有可无的事去否决已经完成的事。
    svc = stubLocalService({
      create_note: () => ({ ok: true, payload: note('11111111-1111-7111-8111-111111111111', '新笔记') }),
      list_notes: () => ({ ok: true, payload: [] }),
      get_note: () => ({ ok: true, payload: note('11111111-1111-7111-8111-111111111111', '新笔记') }),
      // stats 故意不给 handler → 该桩返回 404
    });
    const made = await useNoteStore().create(null);
    expect(made, 'stats 失败不该让新建笔记变成失败').not.toBeNull();
  });

  it('设置页拿到的 stats 会被侧栏消费（口径一致，不是两套数）', async () => {
    const settings = useSettingsStore();
    await settings.loadStats();
    expect(settings.stats?.notes).toBe(26);
  });
});
