/**
 * 「新建文件夹」必须走弹窗、且落在根这一层的回归门禁。
 *
 * 两条各钉一个用户能看见的事实：
 *  ① 点"＋"不再在列表底下长出一行输入框（用户原话：「而不是底下突然补充一个新的」），
 *     而是弹出一个带焦点的模态；
 *  ② 发给核心的 `create_folder` 的 `parentId` 必须是 null ——
 *     以前它取"当前打开的那个文件夹"当父级，于是从子文件夹里点＋会长出孙层，
 *     与"只有一层文件夹用于归档"直接冲突。这一条打在**真请求的参数**上，不是打在上面板存在。
 *  ③ 名字为空时确认键禁用（弹窗不许造出一个空文件夹）。
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
import { createPinia, setActivePinia } from 'pinia';
import SidebarPanel from './components/SidebarPanel.vue';
import { noteFixture, stubLocalService, type LocalService } from './testing/http';

async function settle(times = 8): Promise<void> {
  for (let i = 0; i < times; i += 1) {
    await flushPromises();
  }
}

let service: LocalService;

beforeEach(() => {
  setActivePinia(createPinia());
  vi.useFakeTimers();
  vi.stubGlobal('EventSource', undefined);
  window.matchMedia = vi.fn().mockImplementation(() => ({ matches: false, addEventListener: vi.fn(), removeEventListener: vi.fn() }));
  service = stubLocalService({
    list_notes: () => [noteFixture({ id: 'note-1', folderId: 'folder-work' })],
    list_folders: () => [{ id: 'folder-work', parentId: null, name: '工作', systemKind: null, noteCount: 1 }],
    get_note: () => noteFixture({ id: 'note-1' }),
    create_folder: (args) => noteFixture({ id: 'folder-new', ...args }),
    account: () => null,
    stats: () => ({ notes: 1, notesInTrash: 0, folders: 1, attachments: 0, dbBytes: 4096 }),
    open_conflicts: () => [],
    sync_status: () => null,
  });
});

afterEach(() => {
  vi.useRealTimers();
  document.body.innerHTML = '';
});

/** Headless UI 的 Dialog 走 portal，节点落在 body 上而不是组件子树里 —— 查询得从 body 起。 */
function q<T extends Element>(sel: string): T {
  const el = document.querySelector(sel);
  if (!el) throw new Error(`找不到 ${sel}`);
  return el as T;
}

function typeInto(testid: string, value: string): void {
  const el = q<HTMLInputElement>(`[data-testid="${testid}"]`);
  el.value = value;
  el.dispatchEvent(new Event('input', { bubbles: true }));
}

async function openDialog() {
  const wrapper = mount(SidebarPanel, { attachTo: document.body });
  await settle();
  await wrapper.get('[data-testid="new-folder"]').trigger('click');
  await settle();
  return wrapper;
}

describe('新建文件夹走弹窗', () => {
  it('点＋弹出模态，而不是在列表底下长一行输入框', async () => {
    await openDialog();
    expect(document.querySelector('[data-testid="new-folder-dialog"]'), '弹窗没出现').not.toBeNull();
    expect(document.querySelector('[role="dialog"]')).not.toBeNull();
    // 底下长出来的那一行：输入框必须在 dialog 里面，而不是 `.pane-body` 的直接子节点。
    expect(q('[data-testid="new-folder-input"]').closest('[role="dialog"]'), '输入框没在弹窗里，还是在列表里长出来').not.toBeNull();
  });

  it('确认时发给核心的 parentId 是 null（新文件夹只落根，不长孙层）', async () => {
    await openDialog();
    typeInto('new-folder-input', '会议记录');
    // 等一拍再点：确认键的 disabled 是 `newName` 的派生绑定，不重渲染就还是禁用的，
    // 点一颗禁用的按钮什么也不会发生（这一拍不是等网络，是等 Vue）。
    await settle();
    q<HTMLButtonElement>('[data-testid="app-dialog-confirm"]').click();
    await settle();
    const calls = service.callsOf('create_folder');
    expect(calls).toHaveLength(1);
    expect(calls[0].args, 'parentId 不是 null：又从当前文件夹长出子层了').toEqual({ parentId: null, name: '会议记录' });
  });

  it('名字为空时确认键禁用', async () => {
    await openDialog();
    expect(q<HTMLButtonElement>('[data-testid="app-dialog-confirm"]').disabled, '空名字时确认键居然可点').toBe(true);
    typeInto('new-folder-input', 'x');
    await settle();
    expect(q<HTMLButtonElement>('[data-testid="app-dialog-confirm"]').disabled).toBe(false);
  });
});
