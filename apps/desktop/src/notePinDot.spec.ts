/**
 * 「置顶那颗点按下去没有任何变化」的回归门禁。
 *
 * 用户读到的事实：那颗 ◎ 藏在 `.row-item__actions` 里，而父级是 `opacity: 0`
 * （只有 `:hover` / `:focus-within` 才亮）—— 子元素没法把自己从父级的 opacity:0 里
 * 救回来，所以"点完再看那颗点"看到的还是原样；唯一的痕迹是标题前多了一个 ✓，
 * 而 ✓ 表达的是"这条被固定了"吗？读起来更像"已完成"。
 *
 * 修法是把那颗点搬到 hover 层之外，并让**图形本身**表状态（○ ↔ ● + 主色）。
 * 这里钉的是结构与行为（jsdom 量不到渲染后的 opacity，那一格在真浏览器里量，
 * 见 scripts/verify-layout.mjs 那一条通道）：
 *   ① 那颗点不是 `.row-item__actions` 的后代（否则它必然还是 hover 才出现）；
 *   ② 两态的 aria-pressed / 图形 / 状态类都不同（同一颗按钮要能读出两种状态）；
 *   ③ 点下去真发 `set_note_pinned`，且回包之后那颗点自己翻面；
 *   ④ 标题里不再出现 ✓（防止"两个地方各说一遍"又变成两套真相）。
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
import { createPinia, setActivePinia } from 'pinia';
import App from './App.vue';
import { noteFixture, stubLocalService } from './testing/http';

async function settle(times = 8): Promise<void> {
  for (let i = 0; i < times; i += 1) {
    await flushPromises();
  }
}

const PINNED = 'note-a';
const PLAIN = 'note-b';

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
  document.body.innerHTML = '';
});

async function mounted() {
  const service = stubLocalService({
    list_notes: () => [
      noteFixture({ id: PINNED, title: '已置顶的那篇', pinned: true }),
      noteFixture({ id: PLAIN, title: '没置顶的那篇', pinned: false }),
    ],
    list_folders: () => [{ id: 'f1', parentId: null, name: '工作', systemKind: null }],
    get_note: (args: Record<string, unknown>) => noteFixture({ id: String(args.id ?? PLAIN) }),
    account: () => null,
    stats: () => ({ notes: 2, notesInTrash: 0, folders: 1, attachments: 0, dbBytes: 4096 }),
    open_conflicts: () => [],
    set_note_pinned: (args: Record<string, unknown>) =>
      noteFixture({ id: String(args.id), pinned: Boolean(args.pinned), rev: 4 }),
  });
  const wrapper = mount(App, { attachTo: document.body });
  await settle();
  vi.advanceTimersByTime(50);
  await settle();
  return { wrapper, service };
}

function pinOf(wrapper: ReturnType<typeof mount>, id: string) {
  return wrapper.get(`[data-testid="note-row-${id}"]`).get('[data-testid="note-pin-toggle"]');
}

/** 那颗点里的 SVG 图标名（§2：图形本身表状态，现在是画出来的，不是字形）。 */
function iconOf(pin: ReturnType<typeof pinOf>): string {
  return pin.find('svg').attributes('data-icon') ?? '';
}

/** 那颗点的全部祖先 class（结构判据用得上）。 */
function ancestorClasses(el: Element): string[] {
  const out: string[] = [];
  let cur: Element | null = el.parentElement;
  while (cur) {
    out.push(...Array.from(cur.classList));
    cur = cur.parentElement;
  }
  return out;
}

describe('置顶那颗点：常显 + 状态写在图形上', () => {
  it('两行各有一颗点，且都不在 hover 才出现的那一层里', async () => {
    const { wrapper } = await mounted();
    for (const id of [PINNED, PLAIN]) {
      const pin = pinOf(wrapper, id);
      // 祖先里出现 row-item__actions 就等于它还是 hover 才亮（父级 opacity:0 压不掉）。
      expect(ancestorClasses(pin.element), `${id} 那颗点还藏在 .row-item__actions 里`).not.toContain('row-item__actions');
    }
    // 正对照：同一行里那颗"移入回收站"**确实**在 hover 层里 —— 上面那条判据不是空转。
    const trash = wrapper.get(`[data-testid="note-row-${PLAIN}"] [aria-label="移到最近删除"]`);
    expect(ancestorClasses(trash.element)).toContain('row-item__actions');
  });

  it('已置顶与未置顶那颗点读得出区别：aria-pressed、图形、状态类', async () => {
    const { wrapper } = await mounted();
    const on = pinOf(wrapper, PINNED);
    const off = pinOf(wrapper, PLAIN);
    expect(on.attributes('aria-pressed')).toBe('true');
    expect(off.attributes('aria-pressed')).toBe('false');
    // §2：那颗点的图形现在是 SVG（不是 ●/○ 字形）⇒ 判据改读 `data-icon`，
    //    语义没变：仍是"图形本身表状态"，只是不再依赖字体回退。
    expect(iconOf(on)).toBe('pin-on');
    expect(iconOf(off)).toBe('pin-off');
    expect(on.classes()).toContain('row-item__pin--on');
    expect(off.classes()).not.toContain('row-item__pin--on');
  });

  it('点未置顶那颗：发出 set_note_pinned(true)，并且这颗点自己翻成实心', async () => {
    const { wrapper, service } = await mounted();
    expect(service.callsOf('set_note_pinned')).toHaveLength(0);
    await pinOf(wrapper, PLAIN).trigger('click');
    await settle();

    const calls = service.callsOf('set_note_pinned');
    expect(calls).toHaveLength(1);
    expect(calls[0].args).toMatchObject({ id: PLAIN, pinned: true });
    expect(iconOf(pinOf(wrapper, PLAIN))).toBe('pin-on');
    expect(pinOf(wrapper, PLAIN).classes()).toContain('row-item__pin--on');
  });

  it('再点一次取消置顶：发的是 pinned=false，图形回到空心', async () => {
    const { wrapper, service } = await mounted();
    await pinOf(wrapper, PINNED).trigger('click');
    await settle();
    const last = service.callsOf('set_note_pinned').at(-1);
    expect(last?.args).toMatchObject({ id: PINNED, pinned: false });
    expect(iconOf(pinOf(wrapper, PINNED))).toBe('pin-off');
  });

  it('点那颗点不该顺手把这篇打开（点击不冒泡到行）', async () => {
    const { wrapper, service } = await mounted();
    const before = service.callsOf('get_note').length;
    await pinOf(wrapper, PLAIN).trigger('click');
    await settle();
    expect(service.callsOf('get_note')).toHaveLength(before);
  });

  it('标题前不再有 ✓：状态只由那颗点说一次', async () => {
    const { wrapper } = await mounted();
    const title = wrapper.get(`[data-testid="note-row-${PINNED}"] .row-item__title`);
    expect(title.text()).not.toContain('✓');
    expect(title.text()).toContain('已置顶的那篇');
  });
});
