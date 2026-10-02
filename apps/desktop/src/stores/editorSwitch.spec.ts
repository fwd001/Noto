/**
 * 切篇时的两条**存储层不变式**（写它们的动机是缺口 G43：`verify-app` 第 49 步连四次撞出
 * `edit_note 400 stale_edit（actual 5 / expected 2）`）。
 *
 * 读数要说准：这两条在 2026-10-02 这一版代码上是**绿的** —— 也就是说 G43 的那条 400
 * **不是**这两条路径造出来的（"排队中的保存带着别一篇的 rev 出门"与"B 收到 A 的正文"
 * 在 store 层都构造不出来）。所以它们的身份是**不变式守卫**，不是 G43 的复现，也不是它的解药：
 * 谁以后把 `open()` 的 flush/cancel 顺序或 `doSave()` 读 `rev` 的时机改动而破了这两条，这里会红。
 * G43 仍在台账里开着（PRODUCTION-READINESS §7），下一步要往下查的是 DOM 那一侧：
 * 编辑器还显示上一篇时，一次 commit 会把哪一份正文交进来。
 *
 * 跑法：`pnpm --dir apps/desktop test src/stores/editorSwitch.spec.ts`
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { noteFixture, stubLocalService } from '../testing/http';
import { useEditorStore } from './editor';

const A = 'note-aaaa';
const B = 'note-bbbb';

function docOf(text: string) {
  return { v: 1, content: [{ id: 'abcd1234', type: 'paragraph', content: [{ text }] }] };
}

/** 每篇笔记各自的"核心侧 rev"，由 edit_note 的落地推进 —— 这就是判据要对的东西。 */
function makeCore(revs: Record<string, number>) {
  const seen: Array<{ id: string; expectedRev: number }> = [];
  const service = stubLocalService({
    get_note: (args) => {
      const id = String(args.id);
      return noteFixture({ id, rev: revs[id] ?? 1, doc: docOf(id === A ? 'A 的正文' : 'B 的正文') });
    },
    edit_note: (args) => {
      const id = String(args.id);
      const expected = Number(args.expectedRev ?? 0);
      seen.push({ id, expectedRev: expected });
      // 核心只接受"等于当前 rev"的那一支；不等就照产品行为回 stale_edit（400），
      // 但这里让桩件先记账再按当前值推进，判据看的是记账，不用管回包形状。
      revs[id] = Math.max(revs[id] ?? 1, expected + 1);
      return noteFixture({ id, rev: revs[id], doc: (args.doc as never) ?? docOf('') });
    },
  });
  return { service, seen };
}

beforeEach(() => {
  setActivePinia(createPinia());
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

describe('切篇时的保存排队（缺口 G43）', () => {
  it('每支 edit_note 出门时，id 与 expectedRev 必须来自同一篇笔记', async () => {
    const core = { [A]: 2, [B]: 5 };
    const { service, seen } = makeCore(core);
    const editor = useEditorStore();

    await editor.open(A);
    expect(editor.noteId).toBe(A);
    editor.updateBlock({ ...editor.blocks[0], content: [{ text: 'A 的第一次改动' }] });
    await vi.advanceTimersByTimeAsync(1300);

    // 在飞的这一支还没回包时又改了一次：回包会走"正文又变了"那支，重新武装一次 debounce。
    editor.updateBlock({ ...editor.blocks[0], content: [{ text: 'A 的第二次改动' }] });
    const switching = editor.open(B);
    await vi.advanceTimersByTimeAsync(2000);
    await switching;

    expect(service.callsOf('edit_note').length, '这一趟至少该有一次自动保存，否则判据是空转').toBeGreaterThan(0);
    // 判据：出门的 (id, expectedRev) 不许跨篇。B 从 5 起，A 在 2..4 之间。
    for (const call of seen) {
      if (call.id === A) {
        expect(
          call.expectedRev,
          `A 的保存带着 ${call.expectedRev} 出门（B 的 rev 是 5）—— 这就是"带着别一篇的 rev 提交"`,
        ).toBeLessThan(5);
      } else {
        expect(
          call.expectedRev,
          `B 的保存带着 A 的 rev ${call.expectedRev} 出门 —— 核心会按 stale_edit 拒，用户那边字就没了`,
        ).toBeGreaterThanOrEqual(5);
      }
    }
  });

  it('切篇之后不许还把上一篇的正文写进库里（同一支的 doc 也要跟着 id）', async () => {
    const core = { [A]: 2, [B]: 5 };
    const { service } = makeCore(core);
    const editor = useEditorStore();

    await editor.open(A);
    editor.updateBlock({ ...editor.blocks[0], content: [{ text: '还没落库的 A 结尾' }] });
    // 不 flush 就切：debounce 还排着
    await editor.open(B);
    await vi.advanceTimersByTimeAsync(3000);

    for (const call of service.callsOf('edit_note')) {
      const args = call.args as { id: string; doc: { content: Array<{ content?: Array<{ text?: string }> }> } };
      const text = (args.doc?.content?.[0]?.content ?? []).map((i) => i.text ?? '').join('');
      if (args.id === B) {
        expect(text, 'B 不许收到 A 的正文（那是把两篇的内容串了）').not.toContain('还没落库的 A 结尾');
      }
    }
  });

  it('切篇的往返还没回来时，上一篇补上来的那一支不许打到下一篇（G43 的稳定复现）', async () => {
    // 这一条与上面两条不同：上面两条是**负读数**（构造不出来），这一条照 DOM 的真实节奏
    // 把窗口摆出来 —— `open(B)` 已经把 noteId 换成 B、blocks 清空，而 `get_note(B)` 还在飞
    // （真机上就是一个往返的窗口；lane 里 `Ctrl+N` 之后 700 ms 打字撞的正是它）。
    // 那一刻屏幕上还是上一篇的 ProseMirror 视图，它补上来的 `updateBlock` 用的是**A 的块 id**。
    //
    // 现状（2026-10-02，未修）：那一支会以 `{id: B, expectedRev: 2}` 出门 —— 2 是 **A 的 rev**，
    // 而 B 在核心已经是 5 ⇒ 正是 lane 里那条 `stale_edit（actual 5 / expected 2）`。
    // 所以这条判据在修复之前**必须是红的**；它红了才算复现被锁死。
    const core: Record<string, number> = { [A]: 2, [B]: 5 };
    const gate: { release: ((value: unknown) => void) | null } = { release: null };
    const seen: Array<{ id: string; expectedRev: number }> = [];
    const service = stubLocalService({
      get_note: (args) => {
        const id = String(args.id);
        if (id === B) {
          // 手动掌握这一支什么时候落地 —— 窗口要多长由测试说了算，不靠 sleep 猜
          return new Promise((resolve) => {
            gate.release = resolve;
          });
        }
        return noteFixture({ id, rev: core[id], doc: docOf('A 的正文') });
      },
      edit_note: (args) => {
        const id = String(args.id);
        const expected = Number(args.expectedRev ?? 0);
        seen.push({ id, expectedRev: expected });
        core[id] = Math.max(core[id], expected + 1);
        return noteFixture({ id, rev: core[id], doc: (args.doc as never) ?? docOf('') });
      },
    });
    const editor = useEditorStore();

    await editor.open(A);
    expect(editor.noteId).toBe(A);

    const switching = editor.open(B);
    await vi.advanceTimersByTimeAsync(0); // 让 open() 跑到那个 pending 的 get_note 上
    // 上一篇的视图还在屏幕上，用户这几个字打进了它
    editor.updateBlock({ ...editor.blocks[0], content: [{ text: '用户刚打的字' }] });
    await vi.advanceTimersByTimeAsync(2000); // 自动保存的计时器到点

    try {
      const toB = seen.filter((call) => call.id === B);
      for (const call of toB) {
        expect(
          call.expectedRev,
          `B 收到了一支 expectedRev=${call.expectedRev} 的写（B 在核心是 5）—— 带着上一篇的 rev 出门，` +
            '核心按 stale_edit 拒，用户那几个字哪儿也没落',
        ).toBeGreaterThanOrEqual(5);
      }
      // 第二条腿：不许静默。窗口期被挡下的输入必须在状态里有交代，
      // 否则修完只是把"打进去的字"从"报错"换成"什么都不发生"。
      if (toB.length === 0) {
        expect(editor.saveErrorKey, '没有把字写进任何一篇，却没给出原因（静默丢输入）').not.toBeNull();
      }
    } finally {
      gate.release?.(noteFixture({ id: B, rev: core[B], doc: docOf('B 的正文') }));
      await switching;
    }
    expect(service.callsOf('get_note').length, 'get_note 至少发过 A 与 B 各一次').toBeGreaterThanOrEqual(2);
  });
});
