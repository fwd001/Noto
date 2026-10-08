/**
 * 「移到」那颗下拉的落点（缺口 G96）。
 *
 * 三条判据各自钉一件事：
 *  · **产品里没有"未归类"这一态**：`list_notes` 把笔记**内连接**到 folders（不属于任何文件夹的笔记
 *    根本列不出来），`create_note` / `set_note_folder` 的入参都是非空 uuid。所以下拉里那一格
 *    `value: ''` 选下去必然 `bad_args` ⇒ 一次 400、屏幕上位置一个字没变 —— 那是颗会响的假按钮。
 *    真正的"还没整理"是核心 bootstrap 出来的**默认本**（`system_kind='default'`）。
 *  · 认它要**按角色不按名字**：默认本的名字是用户可改的显示名（`folders.ts:26` 早就为这条写过一次）。
 *  · 删除文件夹那句确认文案说的是"笔记回到哪儿"，那个去处必须是**读出来的真名字**，不是写死的词。
 */
import { beforeEach, describe, expect, it } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { stubLocalService } from '../testing/http';
import { MESSAGE_KEYS, t } from '../i18n';
import { useFolderStore } from './folders';
import { useNoteStore } from './notes';

function folder(id: string, name: string, systemKind: string | null): Record<string, unknown> {
  return { id, parentId: null, name, children: [], systemKind };
}

beforeEach(() => {
  setActivePinia(createPinia());
});

describe('「移到」的落点（G96）', () => {
  it('默认本按 systemKind 认，不按名字认（名字是用户可改的显示名）', async () => {
    // 顺序是判据的一部分：那颗名字叫"默认"却**不是**系统格的排在前面 ——
    // "挑第一颗"这种坏实现在这里必须红（不然这条测的是数组顺序，不是角色）。
    stubLocalService({
      list_folders: () => [folder('f2', '默认', null), folder('fd1', '收件箱', 'default')],
    });
    const folders = useFolderStore();
    await folders.load();
    expect(folders.defaultNode?.node.id).toBe('fd1');
    expect(folders.defaultNode?.node.name).toBe('收件箱');
  });

  it('库里没有默认本那一格时 defaultNode 是 null，而不是"随便挑一颗"', async () => {
    stubLocalService({ list_folders: () => [folder('f2', '默认', null)] });
    const folders = useFolderStore();
    await folders.load();
    expect(folders.defaultNode).toBeNull();
  });

  it('moveTo 发给核心的 folderId 就是选中的那个真 id（调用边：不许把空串当"未归类"发出去）', async () => {
    const service = stubLocalService({
      list_notes: () => [],
      set_note_folder: (args) => ({ id: args.id, folderId: args.folderId, title: 'x', pinned: false, updatedAt: '2026-10-01T00:00:00Z' }),
    });
    const notes = useNoteStore();
    await notes.moveTo('n1', 'fd1');
    expect(service.lastArgsOf('set_note_folder')).toEqual({ id: 'n1', folderId: 'fd1' });
  });

  it('文案表里不许再出现「未归类」—— 产品里没这个状态，说了就是句假话', () => {
    const offenders = MESSAGE_KEYS.filter((key) => t(key).includes('未归类'));
    expect(offenders, JSON.stringify(offenders)).toEqual([]);
  });

  it('删除文件夹那句确认文案必须把去处做成参数（写死的名字改了就会说谎）', () => {
    const hint = t('sidebar.deleteFolderHint', { folder: '收件箱' });
    expect(hint).toContain('收件箱');
    expect(hint, '不传参数就不该画出一句带占位符的话').not.toContain('{folder}');
  });
});
