/**
 * 元数据写与编辑器之间的 rev 交接（缺口 G43 的真实根因）。
 *
 * 置顶与"移到文件夹"走的是核心里同一条 `commit_edit` ⇒ **同一行的 rev 被推进**（这是同步要 propagate
 *  pinned/folder 所必需的，不是副作用）。而编辑器缓存的 `rev` 只跟着自己的 `edit_note` 回包前进，
 * 于是按完置顶再打字，那一支带着**置顶之前**的 rev 出门。
 *
 * 2026-10-02 在真机（dev 桥 + 真浏览器）上量到的形状：
 *   edit_note expectedRev=1 → 200 rev=2
 *   set_note_pinned × 3     → 200 rev=3/4/5
 *   edit_note expectedRev=2 → 400          ← 与 lane 里那条 `actual 5 / expected 2` 同一对数字
 *   屏幕上只剩「这条笔记在别处被改动了」，而那几个字哪儿也没落。
 *
 * 跑法：`pnpm --dir apps/desktop test src/stores/noteRevHandoff.spec.ts`
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { noteFixture, stubLocalService } from '../testing/http';
import { useEditorStore } from './editor';
import { useNoteStore } from './notes';

const A = 'note-aaaa';
const B = 'note-bbbb';

function docOf(text: string) {
  return { v: 1, content: [{ id: 'abcd1234', type: 'paragraph', content: [{ text }] }] };
}

/** 照核心 `commit_edit` 的形状搭一座最小桩：每一笔写（含元数据）都把那一行的 rev 推进一格。 */
function makeCore(revs: Record<string, number>) {
  const edits: Array<{ id: string; expectedRev: number; text: string }> = [];
  const note = (id: string, extra: Record<string, unknown> = {}) =>
    noteFixture({ id, rev: revs[id], doc: docOf(id === A ? 'A 的正文' : 'B 的正文'), ...extra });
  const service = stubLocalService({
    get_note: (args) => note(String(args.id)),
    edit_note: (args) => {
      const id = String(args.id);
      const expected = Number(args.expectedRev ?? 0);
      const content = (args.doc as { content?: Array<{ content?: Array<{ text?: string }> }> })?.content ?? [];
      const text = (content[0]?.content ?? []).map((item) => item.text ?? '').join('');
      edits.push({ id, expectedRev: expected, text });
      // 核心只认"等于当前 rev"的那一支；这里如实把 rev 推进一格，判据看的是出门时带的是哪一格
      revs[id] = expected + 1;
      return note(id, { doc: (args.doc as never) ?? docOf('') });
    },
    set_note_pinned: (args) => {
      const id = String(args.id);
      revs[id] += 1; // 元数据写也推进同一行
      return note(id, { pinned: Boolean(args.pinned) });
    },
    set_note_folder: (args) => {
      const id = String(args.id);
      revs[id] += 1;
      return note(id, { folderId: args.folderId ?? null });
    },
    list_notes: () => [note(A), note(B)],
    list_folders: () => [],
  });
  return { service, edits };
}

beforeEach(() => {
  setActivePinia(createPinia());
  vi.useFakeTimers();
});

describe('置顶/移动之后接着打字（缺口 G43）', () => {
  it('置顶当前打开的那一篇之后，下一支自动保存要带着置顶之后的 rev 出门', async () => {
    const core = { [A]: 2, [B]: 5 };
    const { service, edits } = makeCore(core);
    const editor = useEditorStore();
    const notes = useNoteStore();

    await editor.open(A);
    expect(editor.rev, '开局编辑器认的是这一行当前的 rev').toBe(2);

    await notes.setPinned(A, true);
    expect(service.callsOf('set_note_pinned').length, '置顶那一次调用没发出去 ⇒ 这条判据是空转').toBe(1);
    expect(core[A], '核心把同一行的 rev 推进了一格（这是同步必需的，不是副作用）').toBe(3);

    editor.updateBlock({ ...editor.blocks[0], content: [{ text: '置顶之后打的字' }] });
    await vi.advanceTimersByTimeAsync(2000);

    const mine = edits.filter((edit) => edit.id === A);
    const last = mine[mine.length - 1];
    expect(last, '打字之后一支自动保存都没发出去 ⇒ 判据落空').toBeTruthy();
    expect(last.expectedRev, `带出去的是 ${last.expectedRev}，而置顶已经把那一行推到 3 ⇒ 核心会按 stale_edit 拒`).toBe(3);
    expect(last.text, '认领新 rev 不许顺手把用户没保存的正文换成核心那份').toContain('置顶之后打的字');
  });

  it('移到别的文件夹之后再打字，同样不许带旧 rev 出门', async () => {
    const core = { [A]: 2, [B]: 5 };
    const { service, edits } = makeCore(core);
    const editor = useEditorStore();
    const notes = useNoteStore();

    await editor.open(A);
    await notes.moveTo(A, 'folder-x');
    expect(service.callsOf('set_note_folder').length, '移动那一次调用没发出去 ⇒ 这条判据是空转').toBe(1);
    expect(core[A]).toBe(3);

    editor.updateBlock({ ...editor.blocks[0], content: [{ text: '移动之后打的字' }] });
    await vi.advanceTimersByTimeAsync(2000);

    const last = edits.filter((edit) => edit.id === A).pop();
    expect(last?.expectedRev, '移动之后还在用移动之前的 rev 提交').toBe(3);
  });

  it('别的篇的 rev 更新不许改掉当前这篇的 rev（认领只按 id 匹配）', async () => {
    const core = { [A]: 2, [B]: 5 };
    const { edits } = makeCore(core);
    const editor = useEditorStore();
    const notes = useNoteStore();

    await editor.open(A);
    // B 被别处推进了三格（对面设备同步下来的、或另一篇的置顶）
    notes.applyNoteUpdate(noteFixture({ id: B, rev: 9, doc: docOf('B 的新正文') }) as never);
    expect(editor.rev, '当前打开的是 A，A 的 rev 不许跟着 B 走').toBe(2);

    editor.updateBlock({ ...editor.blocks[0], content: [{ text: 'A 自己的改动' }] });
    await vi.advanceTimersByTimeAsync(2000);
    const last = edits.filter((edit) => edit.id === A).pop();
    expect(last?.expectedRev, '把 B 的 rev 认领给了 A ⇒ 串篇').toBe(2);
  });
});
