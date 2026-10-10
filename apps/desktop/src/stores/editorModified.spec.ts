/**
 * 「改于 {时间}」那一句的数据来源（设计稿第 1 页编辑器角上第三那句）。
 *
 * 判据全部打在**调用边**上：核心在 `NoteDto.updatedAt` 里发的这一位，
 * 以前前端压根没接（editor store 里连字段都没有），所以界面无从说"这篇是几点改的"。
 * 三条各盯一处漏点：读回来要存下、保存之后要跟上新时刻、没有"这一篇"时要清空。
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { noteFixture, stubLocalService } from '../testing/http';
import { useEditorStore } from './editor';

beforeEach(() => {
  setActivePinia(createPinia());
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

describe('编辑器里的"改于"（updatedAt 过桥）', () => {
  it('读回来的那一篇：核心给的时刻要存下（原样，不在 store 里格式化）', async () => {
    const fixture = noteFixture({ id: 'n1', updatedAt: '2026-10-05T01:02:03Z' });
    stubLocalService({ get_note: () => fixture });
    const editor = useEditorStore();
    await editor.open('n1');
    expect(editor.noteUpdatedAt).toBe('2026-10-05T01:02:03Z');
  });

  it('保存之后要跟上新时刻：屏幕上不许还停在被打开那一版的时间', async () => {
    const openAt = noteFixture({ id: 'n1', rev: 3, updatedAt: '2026-10-05T01:02:03Z' });
    const saved = noteFixture({ id: 'n1', rev: 4, updatedAt: '2026-10-07T06:22:00Z' });
    const service = stubLocalService({ get_note: () => openAt, edit_note: () => saved });
    const editor = useEditorStore();
    await editor.open('n1');
    expect(editor.noteUpdatedAt).toBe('2026-10-05T01:02:03Z');

    editor.updateBlock({ ...editor.blocks[0], content: [{ text: '打完这一句就存' }] });
    await editor.flush();
    expect(service.callsOf('edit_note')).toHaveLength(1);
    expect(editor.noteUpdatedAt).toBe('2026-10-07T06:22:00Z');
  });

  it('保存回包缺这一格时，不许把已知的时刻抹成空（宁缺毋滥，也不许倒退成 null）', async () => {
    const openAt = noteFixture({ id: 'n1', rev: 3, updatedAt: '2026-10-05T01:02:03Z' });
    const noStamp = (() => {
      const value = noteFixture({ id: 'n1', rev: 4 }) as Record<string, unknown>;
      delete value.updatedAt; // 旧核心/旧桥的回包就是这一形状
      return value;
    })();
    stubLocalService({ get_note: () => openAt, edit_note: () => noStamp });
    const editor = useEditorStore();
    await editor.open('n1');
    editor.updateBlock({ ...editor.blocks[0], content: [{ text: '这一版回包没有 updatedAt' }] });
    await editor.flush();
    expect(editor.noteUpdatedAt).toBe('2026-10-05T01:02:03Z');
  });

  it('关掉这一篇（没有"这一篇"了）必须清空，否则空编辑器会带着上一篇的时刻', async () => {
    stubLocalService({ get_note: () => noteFixture({ id: 'n1', updatedAt: '2026-10-05T01:02:03Z' }) });
    const editor = useEditorStore();
    await editor.open('n1');
    expect(editor.noteUpdatedAt).toBe('2026-10-05T01:02:03Z');
    await editor.open(null);
    expect(editor.noteUpdatedAt).toBeNull();
  });
});

/**
 * §6 第 12 格「设备身份」的前端那一半：`notes.updated_device` 到编辑器那一格。
 * 与"改于"完全同形（同一份详情 DTO、同一处 hydrate、同一处保存回包），所以判据也同形 ——
 * 唯一的差别是**缺位**那一支：核心没发这一格时回 `null`，界面那句整条不画。
 */
describe('编辑器里的"哪台设备改的"（updatedDevice 过桥）', () => {
  it('读回来的那一篇：核心给的那一串要存下（原样，比较与格式化都在呈现层）', async () => {
    const fixture = noteFixture({ id: 'n1', updatedDevice: '01a10000-0000-7000-8000-0000000000ab' });
    stubLocalService({ get_note: () => fixture });
    const editor = useEditorStore();
    await editor.open('n1');
    expect(editor.noteUpdatedDevice).toBe('01a10000-0000-7000-8000-0000000000ab');
  });

  it('保存之后要跟上新那一台（本机继续改，值仍是本机；核心换了谁写就换谁）', async () => {
    const openAt = noteFixture({ id: 'n1', rev: 3, updatedDevice: 'dev-a' });
    const saved = noteFixture({ id: 'n1', rev: 4, updatedDevice: 'dev-b' });
    const service = stubLocalService({ get_note: () => openAt, edit_note: () => saved });
    const editor = useEditorStore();
    await editor.open('n1');
    expect(editor.noteUpdatedDevice).toBe('dev-a');
    editor.updateBlock({ ...editor.blocks[0], content: [{ text: '打完这一句就存' }] });
    await editor.flush();
    expect(service.callsOf('edit_note')).toHaveLength(1);
    expect(editor.noteUpdatedDevice).toBe('dev-b');
  });

  it('核心没发这一格时是 null（界面那句整条不画），且**不许**拿空串冒充一台设备', async () => {
    const noDev = (() => {
      const value = noteFixture({ id: 'n1', rev: 4 }) as Record<string, unknown>;
      delete value.updatedDevice; // 旧核心/旧桥的回包就是这一形状
      return value;
    })();
    stubLocalService({ get_note: () => noDev, edit_note: () => noDev });
    const editor = useEditorStore();
    await editor.open('n1');
    expect(editor.noteUpdatedDevice).toBeNull();
  });

  it('关掉这一篇必须清空，否则空编辑器会带着上一篇的设备', async () => {
    stubLocalService({ get_note: () => noteFixture({ id: 'n1', updatedDevice: 'dev-a' }) });
    const editor = useEditorStore();
    await editor.open('n1');
    expect(editor.noteUpdatedDevice).toBe('dev-a');
    await editor.open(null);
    expect(editor.noteUpdatedDevice).toBeNull();
  });
});
