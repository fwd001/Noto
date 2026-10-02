/**
 * 把"正打开着的那一篇"移进最近删除时，编辑器在飞/排队的那一支要先结清（缺口 G41）。
 *
 * 现场（lane 的第 49 步 Del 那一判撞出来的，2026-10-02）：
 *   打字（debounce 还排着）→ 焦点还在正文里按 Del ⇒ `notes.moveToTrash(这一篇)`，
 *   核心把那一篇标成 deleted_at，而随后那支自动保存**还是打在它身上** ⇒
 *   `edit_note 400 {"code":"constraint","why":"笔记 … 在回收站中，请先恢复再编辑"}`。
 *   核心这一步是对的（回收站里的笔记不该被编辑），坏的是用户那边：那几个字打了、屏幕上没落，
 *   而除了右下角一次状态翻动之外没有任何一句话说明它去哪了。
 *
 * 修法方向按台账里倾向的那条：把顺序摆正 —— 移走之前先把这一篇的在飞保存 flush 掉，
 * 于是那几个字进的是"还在正常列表里"的那一篇，而不是打在回收站中的它身上。
 *
 * 跑法：`pnpm --dir apps/desktop test src/stores/trashFlush.spec.ts`
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

/**
 * 一座最小核心：`edit_note` 照产品行为**拒掉打在回收站里的那一篇**（回 `constraint`），
 * `delete_note` 把它标成已删除。所有写按时间记进 `seq`，顺序判据就看这个数组。
 */
function makeCore() {
  const trashed = new Set<string>();
  const seq: Array<{ cmd: string; id: string; text: string }> = [];
  const saved = new Map<string, string>();
  const note = (id: string) =>
    noteFixture({
      id,
      rev: 2,
      doc: docOf(saved.get(id) ?? (id === A ? 'A 的正文' : 'B 的正文')),
      deletedAt: trashed.has(id) ? '2026-10-02T00:00:00Z' : null,
    });
  const service = stubLocalService({
    get_note: (args) => note(String(args.id)),
    list_notes: () => [note(A), note(B)],
    edit_note: (args) => {
      const id = String(args.id);
      const content = (args.doc as { content?: Array<{ content?: Array<{ text?: string }> }> })?.content ?? [];
      const text = (content[0]?.content ?? []).map((item) => item.text ?? '').join('');
      if (trashed.has(id)) {
        // 与核心一致：这是"规则不允许"，不是"重试有用"（retryable=false）
        return { ok: false, error: { code: 'constraint', messageKey: 'cmd.constraint', retryable: false } };
      }
      seq.push({ cmd: 'edit_note', id, text });
      saved.set(id, text);
      return note(id);
    },
    delete_note: (args) => {
      const id = String(args.id);
      seq.push({ cmd: 'delete_note', id, text: saved.get(id) ?? '' });
      trashed.add(id);
      return null;
    },
  });
  return { service, seq, trashed, saved };
}

beforeEach(() => {
  setActivePinia(createPinia());
  vi.useFakeTimers();
});

describe('移进最近删除之前在飞的保存（缺口 G41）', () => {
  it('打完结清再移走：不许有一支 edit_note 落在回收站中的那一篇上，字要留在库里', async () => {
    const { seq, saved } = makeCore();
    const editor = useEditorStore();
    const notes = useNoteStore();

    await editor.open(A);
    editor.updateBlock({ ...editor.blocks[0], content: [{ text: '移走之前打的字' }] }); // debounce 还排着
    await notes.moveToTrash(A);
    await vi.advanceTimersByTimeAsync(3000);

    const editIndex = seq.findIndex((row) => row.cmd === 'edit_note' && row.id === A);
    const deleteIndex = seq.findIndex((row) => row.cmd === 'delete_note');
    expect(deleteIndex, 'delete_note 一次都没发 ⇒ 这条判据在空转').toBeGreaterThanOrEqual(0);
    expect(editIndex, '用户那几个字根本没落库 —— 移走之前没把在飞的那一支结清').toBeGreaterThanOrEqual(0);
    expect(editIndex, `写序是 ${JSON.stringify(seq)}：edit_note 排在了 delete_note 之后 ⇒ 它打在回收站中的笔记上`).toBeLessThan(deleteIndex);
    expect(saved.get(A), '移走之前打的那几个字要真的在库里').toContain('移走之前打的字');
    expect(editor.saveErrorKey, `移走之后编辑器留着一次失败态（${editor.saveErrorKey}）却没人说明那几个字去哪了`).toBeNull();
  });

  it('移走的是别的那一篇时，不许被当前这篇的在飞保存拦住（也不许多发一支写）', async () => {
    const { seq } = makeCore();
    const editor = useEditorStore();
    const notes = useNoteStore();

    await editor.open(A);
    editor.updateBlock({ ...editor.blocks[0], content: [{ text: 'A 里还没保存的字' }] });
    await notes.moveToTrash(B); // 移走的是 B
    await vi.advanceTimersByTimeAsync(3000);

    expect(seq.filter((row) => row.cmd === 'delete_note').map((row) => row.id)).toEqual([B]);
    expect(
      seq.filter((row) => row.cmd === 'edit_note').every((row) => row.id === A),
      '移走 B 却替 A 发了写以外的东西 ⇒ 这条边被搅了',
    ).toBe(true);
  });
});
