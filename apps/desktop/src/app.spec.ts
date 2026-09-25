import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
import { createPinia, setActivePinia } from 'pinia';
import App from './App.vue';
import { noteFixture, stubLocalService } from './testing/http';

async function settle(times = 6): Promise<void> {
  for (let i = 0; i < times; i += 1) {
    await flushPromises();
  }
}

beforeEach(() => {
  setActivePinia(createPinia());
  vi.useFakeTimers();
  vi.stubGlobal('EventSource', undefined);
  window.matchMedia = vi.fn().mockImplementation(() => ({
    matches: false,
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
  }));
});

afterEach(() => {
  vi.useRealTimers();
});

describe('外壳首帧', () => {
  it('本地服务没起时显示明确状态，而不是白屏或控制台报错', async () => {
    const errors = vi.spyOn(console, 'error').mockImplementation(() => undefined);
    const warns = vi.spyOn(console, 'warn').mockImplementation(() => undefined);
    vi.stubGlobal('fetch', async () => {
      throw new TypeError('Failed to fetch');
    });

    const wrapper = mount(App, { attachTo: document.body });
    await settle();
    vi.advanceTimersByTime(100);
    await settle();

    expect(wrapper.find('[data-testid="banner-link"]').exists()).toBe(true);
    expect(wrapper.text()).toContain('未连接到本地服务');
    expect(wrapper.find('[data-testid="sync-badge"]').exists()).toBe(true);
    expect(wrapper.text()).toContain('离线');
    expect(wrapper.find('[data-testid="sidebar"]').exists()).toBe(true);
    expect(errors).not.toHaveBeenCalled();
    expect(warns).not.toHaveBeenCalled();

    wrapper.unmount();
    document.body.innerHTML = '';
  });

  it('有数据时渲染列表行，点行会打开编辑器并请求正文', async () => {
    const service = stubLocalService({
      list_notes: () => [noteFixture({ id: 'note-1', title: '第一条', summary: '摘要' })],
      list_folders: () => [{ id: 'f1', parentId: null, name: '工作' }],
      get_note: () => noteFixture({ id: 'note-1' }),
      account: () => null,
      stats: () => ({ notes: 1, notesInTrash: 0, folders: 1, attachments: 0, dbBytes: 4096 }),
      open_conflicts: () => [],
    });

    const errors = vi.spyOn(console, 'error').mockImplementation(() => undefined);
    const wrapper = mount(App, { attachTo: document.body });
    await settle();
    vi.advanceTimersByTime(50);
    await settle();

    expect(wrapper.text()).toContain('第一条');
    expect(wrapper.text()).toContain('工作');
    expect(wrapper.find('[data-testid="banner-link"]').exists()).toBe(false);

    await wrapper.find('[data-testid="note-row-note-1"]').trigger('click');
    await settle();
    expect(service.callsOf('get_note')).toHaveLength(1);
    expect(wrapper.find('[data-testid="editor-doc"]').exists()).toBe(true);
    expect(wrapper.text()).toContain('原始内容');
    expect(errors).not.toHaveBeenCalled();

    wrapper.unmount();
    document.body.innerHTML = '';
  });
});
