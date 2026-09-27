import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import type { Note } from '../api/types';
import { noteFixture, stubLocalService } from '../testing/http';
import { useEditorStore } from './editor';
import { useNoteStore } from './notes';
import { inlineText, textBlock } from '../editor/model';

function asNote(value: Record<string, unknown>): Note {
  return value as unknown as Note;
}

beforeEach(() => {
  setActivePinia(createPinia());
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

describe('自动保存', () => {
  it('停止输入 1200ms 后才保存，并且只保存一次', async () => {
    const service = stubLocalService({ edit_note: () => noteFixture({ rev: 4 }) });
    const editor = useEditorStore();
    editor.hydrate(asNote(noteFixture()));

    const block = editor.blocks[0];
    expect(block).toBeDefined();
    if (!block) return;

    editor.updateBlock({ ...block, content: [{ text: '第一段改动' }] });
    await vi.advanceTimersByTimeAsync(600);
    expect(service.callsOf('edit_note')).toHaveLength(0);

    editor.updateBlock({ ...editor.blocks[0], content: [{ text: '第一段改动，还在打' }] });
    await vi.advanceTimersByTimeAsync(600);
    expect(service.callsOf('edit_note')).toHaveLength(0);

    await vi.advanceTimersByTimeAsync(1400);
    expect(service.callsOf('edit_note')).toHaveLength(1);

    const args = service.lastArgsOf('edit_note') as { id: string; expectedRev: number; doc: { content: Array<{ content?: Array<{ text: string }> }> } };
    expect(args.id).toBe('note-1');
    expect(args.expectedRev).toBe(3);
    expect(inlineText(args.doc.content[0]?.content ?? [])).toBe('第一段改动，还在打');

    await vi.advanceTimersByTimeAsync(10_000);
    expect(service.callsOf('edit_note')).toHaveLength(1);
    expect(editor.dirty).toBe(false);
  });

  it('失焦立即提交，不等安静期', async () => {
    const service = stubLocalService({ edit_note: () => noteFixture({ rev: 5 }) });
    const editor = useEditorStore();
    editor.hydrate(asNote(noteFixture()));
    editor.updateBlock({ ...editor.blocks[0], content: [{ text: '马上走' }] });
    await editor.flush();
    expect(service.callsOf('edit_note')).toHaveLength(1);
  });

  it('版本过新的文档只读，不发出任何写命令', async () => {
    const service = stubLocalService({ edit_note: () => noteFixture() });
    const editor = useEditorStore();
    editor.hydrate(asNote(noteFixture({ doc: { v: 99, content: [{ id: 'abcd1234', type: 'paragraph', content: [{ text: '未来的' }] }] } })));
    expect(editor.writeBlocked).toBe(true);
    expect(editor.readOnlyReason).toBe('versionTooNew');
    editor.updateBlock({ ...editor.blocks[0], content: [{ text: '不该写进去' }] });
    await vi.advanceTimersByTimeAsync(3000);
    expect(service.callsOf('edit_note')).toHaveLength(0);
  });
});

describe('分歧保护（stale_edit）', () => {
  it('重载对方版本但保留本机版本，且绝不静默覆盖', async () => {
    const remote = noteFixture({
      rev: 9,
      title: '别人改的',
      doc: { v: 1, content: [{ id: 'abcd1234', type: 'paragraph', content: [{ text: '别人改的' }] }] },
    });
    const service = stubLocalService({
      edit_note: () => ({ ok: false, error: { code: 'stale_edit', actualRev: 9, messageKey: 'note.staleEdit' } }),
      get_note: () => remote,
    });
    const editor = useEditorStore();
    editor.hydrate(asNote(noteFixture()));

    editor.updateBlock({ ...editor.blocks[0], content: [{ text: '我这台设备的改动' }] });
    await vi.advanceTimersByTimeAsync(1400);

    expect(service.callsOf('edit_note')).toHaveLength(1);
    expect(editor.hasDraftConflict).toBe(true);
    expect(inlineText(editor.blocks[0]?.content ?? [])).toBe('别人改的');
    expect(inlineText(editor.localDraft?.content[0]?.content ?? [])).toBe('我这台设备的改动');
    expect(editor.saveErrorKey).toBe('note.staleEdit');

    // 不会自己重试覆盖：安静期过后仍然只有一次写
    await vi.advanceTimersByTimeAsync(5000);
    expect(service.callsOf('edit_note')).toHaveLength(1);

    await editor.useLocalDraft();
    expect(service.callsOf('edit_note')).toHaveLength(2);
    const args = service.lastArgsOf('edit_note') as { expectedRev: number };
    expect(args.expectedRev).toBe(9);
  });

  it('放弃本机版本后回到对方内容，横幅消失', async () => {
    stubLocalService({
      edit_note: () => ({ ok: false, error: { code: 'stale_edit', actualRev: 4 } }),
      get_note: () => noteFixture({ rev: 4, doc: { v: 1, content: [{ id: 'abcd1234', type: 'paragraph', content: [{ text: '对方' }] }] } }),
    });
    const editor = useEditorStore();
    editor.hydrate(asNote(noteFixture()));
    editor.updateBlock({ ...editor.blocks[0], content: [{ text: '我的' }] });
    await vi.advanceTimersByTimeAsync(1400);
    editor.discardLocalDraft();
    expect(editor.hasDraftConflict).toBe(false);
    expect(editor.localDraft).toBeNull();
  });
});

describe('事件回流校正', () => {
  it('保存成功后列表行同步更新', async () => {
    stubLocalService({
      edit_note: () => noteFixture({ rev: 7, title: '新标题内容', summary: '新标题内容' }),
      list_notes: () => [],
    });
    const editor = useEditorStore();
    const notes = useNoteStore();
    editor.hydrate(asNote(noteFixture()));
    notes.applyNoteUpdate(asNote(noteFixture()));
    editor.updateBlock({ ...editor.blocks[0], content: [{ text: '新标题内容' }] });
    await vi.advanceTimersByTimeAsync(1400);
    expect(notes.rowById('note-1')?.title).toBe('新标题内容');
    expect(editor.rev).toBe(7);
  });
});

describe('插入附件与自动保存的先后', () => {
  /**
   * 核心的附件写入会把笔记 rev 推进一格（它改了派生列与引用表）。曾经编辑器让附件
   * 先写、自己排队的自动保存随后带着旧 rev 出发 → 被判定 stale_edit：用户插一张图，
   * 得到的却是"这条笔记在别处被改动了"并把界面切走。端到端里它就是那条 expected 7 /
   * actual 8。顺序与"接住新 rev"两件事都必须被测到。
   */
  it('先落地待保存内容，再让核心写附件，并接住附件推进后的新 rev', async () => {
    const service = stubLocalService({
      edit_note: () => noteFixture({ rev: 8 }),
      attach_file: () => ({ sha256: 'ab'.repeat(32), size: 3, mediaType: 'image/png', rev: 9 }),
    });
    const editor = useEditorStore();
    editor.hydrate(asNote(noteFixture({ rev: 7 })));
    editor.updateBlock({ ...editor.blocks[0], content: [{ text: '改一段再插图' }] });

    await editor.attachFile('inline', new File([new Uint8Array([1, 2, 3])], '图.png', { type: 'image/png' }));
    // 附件写之前先落编辑；随后取显示字节；再之后才是把附件块写进正文
    expect(service.calls.map((c) => c.name)).toEqual(['edit_note', 'attach_file', 'attachment_data']);

    await vi.advanceTimersByTimeAsync(1400);
    const last = service.lastArgsOf('edit_note') as { expectedRev: number; doc: { content: unknown[] } };
    // 必须用核心回的新 rev：带着旧的出发就是自己跟自己造 stale_edit
    expect(last.expectedRev).toBe(9);
    expect(JSON.stringify(last.doc)).toContain('sha256');
  });

  it('核心没回 rev 时不许把本地 rev 改成 undefined', async () => {
    stubLocalService({
      edit_note: () => noteFixture({ rev: 4 }),
      attach_file: () => ({ sha256: 'ab'.repeat(32), size: 3, mediaType: 'image/png' }),
    });
    const editor = useEditorStore();
    editor.hydrate(asNote(noteFixture({ rev: 4 })));
    await editor.attachFile('file', new File([new Uint8Array([9])], 'a.bin', { type: '' }));
    expect(editor.rev).toBe(4);
    expect(editor.blocks.some((b) => b.shape === 'attachment')).toBe(true);
  });

  it('附件写失败要撤掉占位块，不许留一个永远转圈的假附件', async () => {
    stubLocalService({
      edit_note: () => noteFixture({ rev: 4 }),
      attach_file: () => {
        throw new Error('boom');
      },
    });
    const editor = useEditorStore();
    editor.hydrate(asNote(noteFixture({ rev: 4 })));
    const before = editor.blocks.length;
    await expect(editor.attachFile('inline', new File([new Uint8Array([1])], 'x.png', { type: 'image/png' }))).resolves.toBeNull();
    expect(editor.blocks).toHaveLength(before);
  });
});

describe('写被挡下时不许静默', () => {
  /**
   * 钉的是一条真实踩过的形态：笔记还没就绪时用户已经开始打字，输入被丢掉，
   * 而右下角仍然显示"已保存"。纯黑盒 UAT 三轮里红两轮就是这么来的。
   * 挡下是允许的，谎报不是。
   */
  it('noteId 还不存在时 updateBlock 丢掉输入，就把状态改成明确的错误', async () => {
    const service = stubLocalService({});
    const editor = useEditorStore();
    // 故意不 hydrate：noteId === null → writeBlocked
    const dropped = textBlock('paragraph', [{ text: '刚敲却没落库的字' }]);

    editor.updateBlock(dropped);
    await vi.advanceTimersByTimeAsync(3000);

    expect(editor.dirty).toBe(false);
    expect(editor.saveState).toBe('error');
    expect(editor.saveLabel).toContain('没有被接受');
    expect(service.callsOf('edit_note')).toHaveLength(0);
  });

  it('就绪之后同样的写入照常落库（错误态不是常态化的挡路）', async () => {
    const service = stubLocalService({ edit_note: () => noteFixture({ rev: 2 }) });
    const editor = useEditorStore();
    editor.hydrate(asNote(noteFixture()));
    const block = editor.blocks[0];
    if (!block) return;
    editor.updateBlock({ ...block, content: [{ text: '正常输入' }] });
    await vi.advanceTimersByTimeAsync(2000);
    expect(service.callsOf('edit_note')).toHaveLength(1);
    expect(editor.saveState).toBe('saved');
  });
});

describe('同一笔记的回读不许盖掉未保存的输入', () => {
  /**
   * 实测缺陷的形态：新建笔记后立刻打字，某次"保存前的旧快照"回读把正文换回空文档，
   * 于是屏幕上有字、库里是空正文、右下角写着"已保存"（纯黑盒 UAT 三轮里两轮红）。
   */
  it('本地有未保存编辑、回读的 rev 不更新 → 保留本地正文与 dirty', async () => {
    const stale = noteFixture({ rev: 1 });
    stubLocalService({ get_note: () => stale, edit_note: () => stale });
    const editor = useEditorStore();
    await editor.open('note-1');
    const block = editor.blocks[0];
    if (!block) return;
    editor.updateBlock({ ...block, content: [{ text: '刚打还没保存的字' }] });
    expect(editor.dirty).toBe(true);

    editor.hydrate(asNote(stale));

    expect(inlineText(editor.blocks[0].content)).toBe('刚打还没保存的字');
    expect(editor.dirty).toBe(true);
  });

  it('回读带来更新的 rev（真的是远端改动）→ 照常应用，不挡同步', async () => {
    const fresh = noteFixture({
      rev: 9,
      doc: { v: 1, content: [{ id: 'abcd1234', type: 'paragraph', content: [{ text: '对方版本' }] }] },
    });
    stubLocalService({ get_note: () => noteFixture({ rev: 1 }), edit_note: () => fresh });
    const editor = useEditorStore();
    await editor.open('note-1');
    const block = editor.blocks[0];
    if (!block) return;
    editor.updateBlock({ ...block, content: [{ text: '我的改动' }] });

    editor.hydrate(asNote(fresh));

    expect(inlineText(editor.blocks[0].content)).toBe('对方版本');
    expect(editor.dirty).toBe(false);
  });
});

describe('在飞的保存不许把状态打回旧的一版', () => {
  /**
   * 这条钉的是实测到的完整链条（纯黑盒 UAT 反复红的第 2 步就是它）：
   * ① 第一次 autosave 带着"刚打了一半"的正文出发；
   * ② 在飞期间用户继续打字；
   * ③ 第一次的回包到了 —— 它代表的是**过去**那一版；
   * ④ 此时若把 dirty 清掉，随后任何一次同一笔记的回读就有理由把正文换回旧版，
   *    库里最后只剩半截（实测只剩一条换行），而屏幕上是一整句、右下角写着"已保存"。
   */
  const docOf = (text: string) => ({ v: 1, content: [{ id: 'b1', type: 'paragraph', content: [{ text }] }] });

  it('回包对不上当前正文时：保持 dirty、应用回包不许抹掉新输入，并把新版本存出去', async () => {
    const sent: unknown[] = [];
    const waiters: Array<(v: unknown) => void> = []; // 数组装：避开 TS 对闭包内赋值的窄化
    const service = stubLocalService({
      get_note: () => noteFixture({ rev: 1, doc: docOf('') }),
      edit_note: (args) => {
        sent.push((args as { doc: unknown }).doc);
        if (sent.length === 1) return new Promise((res) => { waiters.push(res as (v: unknown) => void); });
        return noteFixture({ rev: 5, doc: (args as { doc: unknown }).doc });
      },
    });
    const editor = useEditorStore();
    await editor.open('note-1');
    const first = editor.blocks[0];
    if (!first) return;

    editor.updateBlock({ ...first, content: [{ text: '半' }] });
    await vi.advanceTimersByTimeAsync(1300);
    expect(service.callsOf('edit_note')).toHaveLength(1); // 第一次保存现在"在飞"

    editor.updateBlock({ ...editor.blocks[0], content: [{ text: '半句完整的话' }] }); // 在飞期间继续打

    // 回包到达，代表的是过去那一版
    waiters[0]?.(asNote(noteFixture({ rev: 2, doc: docOf('半') })));
    await vi.advanceTimersByTimeAsync(1400);

    expect(editor.dirty).toBe(false); // 新版本必须已经存出去
    expect(inlineText(editor.blocks[0].content)).toBe('半句完整的话');
    const lastArgs = service.callsOf('edit_note').at(-1)?.args as { doc: unknown };
    expect(JSON.stringify(lastArgs.doc)).toContain('半句完整的话');
    expect(editor.saveState).toBe('saved');
  });
});
