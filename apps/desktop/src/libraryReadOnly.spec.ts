/**
 * §4.2 第四格「整机只读（库过新）」以前**一次都出不来**（缺口 G85），而且原因比"横幅读错了位"深一层：
 * 核心在 `user_version > 支持值` 时让 `Store::open` 直接失败 ⇒ `App::boot` 跟着失败 ⇒ 桌面壳的
 * `setup()` 顶死、dev 桥整个进程退出 —— 于是**没有任何一条命令发得出 `db_too_new`**，
 * 而界面那一位只挂在一个核心从不发出的同步事件上。ADR-0012 的 M4 写的是「进入只读模式」。
 *
 * 现在这一格只有一条生产者：首帧那次 `stats` 带回来的 `libraryReadOnly`（读侧留着、写侧拒了，
 * 所以它既一定成功又一定说得出这件事）。这里断言的是**调用边**，不是被调方的绿。
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { mount } from '@vue/test-utils';
import BannerHost from './components/BannerHost.vue';
import { noteFixture, stubLocalService } from './testing/http';
import type { Note } from './api/types';
import { useEditorStore } from './stores/editor';
import { useSettingsStore } from './stores/settings';
import { useShellStore } from './stores/shell';
import { inlineText } from './editor/model';

beforeEach(() => {
  setActivePinia(createPinia());
  vi.useRealTimers();
});

describe('库过新那一格（§4.2 第四行）', () => {
  it('生产者只有一条：首帧的 `stats` 报 libraryReadOnly，这一位就要立起来', async () => {
    const service = stubLocalService({ stats: () => ({ notes: 7, libraryReadOnly: true }) });

    await useSettingsStore().loadStats();

    expect(service.callsOf('stats'), '这一格靠的是首帧那次 stats —— 没发出去就没人知道').toHaveLength(1);
    expect(useShellStore().libraryReadOnly).toBe(true);
  });

  it('正对照：这一位不在载荷里就不许立起来（否则"库过新"会退化成"什么都算库过新"）', async () => {
    stubLocalService({ stats: () => ({ notes: 7 }) });

    await useSettingsStore().loadStats();

    expect(useShellStore().libraryReadOnly).toBe(false);
  });

  it('横幅真的画出来了，而且给的是**下一步**（§4.2 那句"请升级以编辑"）', () => {
    useShellStore().markLibraryReadOnly();

    const banner = mount(BannerHost).find('[data-testid="banner-db"]');

    expect(banner.exists(), '整机只读必须有一条全局横幅').toBe(true);
    expect(banner.text()).toContain('请升级以编辑');
    expect(banner.text()).toContain('不会写');
  });

  it('整机只读时编辑区关掉写：一次 `edit_note` 都不许出门（"绝不降级写"的界面那一半）', async () => {
    vi.useFakeTimers();
    const service = stubLocalService({ edit_note: () => noteFixture({ rev: 4 }) });
    const editor = useEditorStore();
    editor.hydrate(noteFixture() as unknown as Note);
    useShellStore().markLibraryReadOnly();

    expect(editor.writeBlocked).toBe(true);
    expect(editor.readOnlyReason).toBe('libraryReadOnly');

    editor.updateBlock({ ...editor.blocks[0], content: [{ text: '这一笔不许落' }] });
    editor.replaceBlocks([{ ...editor.blocks[0], content: [{ text: '这一笔也不许落' }] }]);
    await editor.flush();
    await vi.advanceTimersByTimeAsync(10_000);

    expect(service.callsOf('edit_note'), '只读闸门下写命令一发都不许出门').toHaveLength(0);
    expect(inlineText(editor.blocks[0]?.content ?? [])).not.toBe('这一笔不许落');
    vi.useRealTimers();
  });

  it('正对照：没进只读时同一支调用照常写（挡住的是这一位，不是整条路径）', async () => {
    vi.useFakeTimers();
    const service = stubLocalService({ edit_note: () => noteFixture({ rev: 4 }) });
    const editor = useEditorStore();
    editor.hydrate(noteFixture() as unknown as Note);

    editor.updateBlock({ ...editor.blocks[0], content: [{ text: '照常写' }] });
    await editor.flush();

    expect(service.callsOf('edit_note')).toHaveLength(1);
    vi.useRealTimers();
  });
});
