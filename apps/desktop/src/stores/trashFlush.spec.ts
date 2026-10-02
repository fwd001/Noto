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
  const purged = new Set<string>();
  const seq: Array<{ cmd: string; id: string; text: string }> = [];
  const saved = new Map<string, string>();
  const revs: Record<string, number> = { [A]: 2, [B]: 2 };
  const note = (id: string) =>
    noteFixture({
      id,
      rev: revs[id],
      doc: docOf(saved.get(id) ?? (id === A ? 'A 的正文' : 'B 的正文')),
      deletedAt: trashed.has(id) ? '2026-10-02T00:00:00Z' : null,
    });
  const service = stubLocalService({
    get_note: (args) => {
      const id = String(args.id);
      return purged.has(id) ? null : note(id);
    },
    list_notes: () => [note(A), note(B)].filter((n) => !purged.has(String(n.id))),
    edit_note: (args) => {
      const id = String(args.id);
      const content = (args.doc as { content?: Array<{ content?: Array<{ text?: string }> }> })?.content ?? [];
      const text = (content[0]?.content ?? []).map((item) => item.text ?? '').join('');
      if (trashed.has(id) || purged.has(id)) {
        // 与核心一致：这是"规则不允许"，不是"重试有用"（retryable=false）
        return { ok: false, error: { code: 'constraint', messageKey: 'cmd.constraint', retryable: false } };
      }
      seq.push({ cmd: 'edit_note', id, text });
      saved.set(id, text);
      revs[id] = Number(args.expectedRev ?? 0) + 1;
      return note(id);
    },
    delete_note: (args) => {
      const id = String(args.id);
      seq.push({ cmd: 'delete_note', id, text: saved.get(id) ?? '' });
      trashed.add(id);
      revs[id] += 1; // 软删也走 commit_edit ⇒ 同一行 rev 前进
      return null;
    },
    restore_note: (args) => {
      const id = String(args.id);
      seq.push({ cmd: 'restore_note', id, text: saved.get(id) ?? '' });
      trashed.delete(id);
      revs[id] += 1; // 恢复同样占一格
      return null;
    },
    purge_note: (args) => {
      const id = String(args.id);
      seq.push({ cmd: 'purge_note', id, text: saved.get(id) ?? '' });
      trashed.delete(id);
      purged.add(id);
      return null;
    },
  });
  return { service, seq, trashed, saved, revs, purged };
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

  it('在回收站里把它打开、再恢复：不许还停在"只读 + 旧 rev"那一格（恢复之后再打字要能落库）', async () => {
    // 真机动线：回收站 → 点开那一篇（编辑器 hydrate 成"在回收站里"⇒ 只读）→ 点「恢复」→ 直接打字。
    // 今天（未修）的两个后果都来自同一处：`notes.restore()` 只 `load()` 列表，**不碰编辑器**，
    // 而 `inTrash` 只在 `hydrate()` 里被赋值 —— 于是编辑器还留着"只读 + 恢复之前那格 rev"。
    // 只读那一半是"改不动"（§6 主流程走不通），旧 rev 那一半与 G43 同族（真打字会撞 stale_edit）。
    const { seq, revs, saved } = makeCore();
    const editor = useEditorStore();
    const notes = useNoteStore();

    await editor.open(B); // 编辑器停在别处，免得触发上一条判据里那句 flush
    await notes.moveToTrash(A);
    await editor.open(A); // 回收站视图里点开它
    expect(editor.inTrash, '回收站里打开应当是只读的（这条是前提，不是判据）').toBe(true);
    const revWhileTrashed = revs[A];

    await notes.restore(A);
    expect(revs[A], '恢复也走 commit_edit ⇒ 那一行又前进一格').toBe(revWhileTrashed + 1);
    expect(editor.inTrash, '已经恢复了，编辑器不许还说"这篇在回收站里"（用户那边是改了没反应）').toBe(false);
    expect(editor.rev, '恢复之后编辑器的 rev 要跟上，不许带旧格出门').toBe(revs[A]);

    editor.updateBlock({ ...editor.blocks[0], content: [{ text: '恢复之后打的字' }] });
    await vi.advanceTimersByTimeAsync(3000);
    const last = seq.filter((row) => row.cmd === 'edit_note' && row.id === A).pop();
    expect(last, '恢复之后打字一支写都没发出去 ⇒ 那几个字哪儿也没落').toBeTruthy();
    expect(editor.saveErrorKey, '恢复之后打字被挡下了，却没把原因摆在界面上（静默的"改了没反应"）').toBeNull();
    expect(saved.get(A), '恢复之后打的字要真的在库里').toContain('恢复之后打的字');
  });

  it('永久删除正开着的那一篇之后，编辑器不许留着它的正文（缺口 G45：幻影 + 一句已经作废的"在最近删除里"）', async () => {
    // 2026-10-02 真机读数（`.logs/probe-purge.mjs`，dev 桥 + 真浏览器、全新空库）：
    //   回收站 → 点开那一篇 → 「永久删除」→ 二次确认之后：
    //     列表对了（note-row=0、空态出现），**但编辑器还显示着那一篇的正文**，
    //     下面那句提示也还在：`这条在"最近删除"里，恢复后才能继续编辑。`
    //   —— 这篇已经不在"最近删除"里了，它被永久删掉了，那句话现在是假的；
    //   往那块只读区打字，屏幕上连字都不出现（只读），也没有任何一句话说明发生了什么。
    // 台账里我原先写的是"purge 靠选中项搬迁关掉编辑器，本轮实测无残留态" —— 那是**推**出来的，
    // 量下来是反的，所以这一格按 §45 更正并补判据。
    const { purged } = makeCore();
    const editor = useEditorStore();
    const notes = useNoteStore();

    await editor.open(B);
    await notes.moveToTrash(A);
    await editor.open(A); // 回收站里开着它（只读）
    expect(editor.inTrash, '前提：回收站里打开是只读的').toBe(true);

    await notes.purge(A);
    expect(purged.has(A), '核心那边这一篇真没了（前提）').toBe(true);
    expect(editor.noteId, '那一篇已被永久删除，编辑器不许还停在它身上').toBeNull();
    expect(editor.blocks.length, '屏幕上不许继续显示一篇已经不存在的正文').toBe(0);
    expect(editor.inTrash, '“这条在最近删除里”那句话对一篇已被永久删除的笔记是假话').toBe(false);
  });

  it('永久删除的是别的那一篇时，不许把当前这篇一起关掉（反向腿）', async () => {
    const { purged } = makeCore();
    const editor = useEditorStore();
    const notes = useNoteStore();

    await editor.open(A);
    await notes.moveToTrash(B);
    await notes.purge(B);
    expect(purged.has(B)).toBe(true);
    expect(editor.noteId, '关错篇 ⇒ 用户正在看的那一篇凭空消失').toBe(A);
    expect(editor.blocks.length, '当前这篇的正文不许被清掉').toBeGreaterThan(0);
  });
});
