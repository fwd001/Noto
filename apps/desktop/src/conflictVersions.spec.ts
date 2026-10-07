/**
 * §4.5「必须说清是哪条笔记、本机是哪一版、对面是哪一版」里的**两个版本号**。
 *
 * 收件箱以前只说"我这台设备 / 另一处改动"，两边各是什么版本没说 —— 而 `localRev` / `remoteRev`
 * 早就从核心取回来并解析进卡片了，只是没往屏幕上放。裁决要的是"我知道我在选哪一版"，
 * 少了版本号，两栏就只是两段文字。
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { mount } from '@vue/test-utils';
import ConflictsView from './views/ConflictsView.vue';
import { stubLocalService } from './testing/http';

const card = {
  conflictId: 7,
  noteId: '01a10000-0000-7000-8000-000000000001',
  copyNoteId: '01a10000-0000-7000-8000-000000000002',
  copyRev: 3,
  noteTitle: '分歧夹具：值班表',
  localRev: 12,
  remoteRev: 7,
  localPreview: '本机这一版的正文',
  remotePreview: '服务器那一版的正文',
  createdAt: '2026-10-06T09:00:00Z',
};

async function renderWith(payload: Record<string, unknown>) {
  stubLocalService({ open_conflicts: () => [payload], preview_text: () => '' });
  const wrapper = mount(ConflictsView);
  await wrapper.vm.$nextTick();
  await vi.waitFor(() => expect(wrapper.text()).toContain('服务器那一版的正文'));
  return wrapper.text();
}

beforeEach(() => {
  setActivePinia(createPinia());
  vi.useRealTimers();
});

describe('分歧两栏各自说出是哪一版', () => {
  it('左栏带本机的 rev，右栏带对面的 rev', async () => {
    const text = await renderWith(card);
    expect(text).toContain('我这台设备 · 第 12 版');
    expect(text).toContain('另一处改动 · 第 7 版');
  });

  it('核心没给 rev 时整段不出现（不许写"第 ? 版"把缺信息伪装成有信息）', async () => {
    const { localRev: _l, remoteRev: _r, ...bare } = card;
    const text = await renderWith(bare);
    expect(text).toContain('我这台设备');
    expect(text).not.toMatch(/第\s*\d*\s*版/);
  });
});
