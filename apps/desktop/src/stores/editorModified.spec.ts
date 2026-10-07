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
