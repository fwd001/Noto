/**
 * 「文字大小」「文字颜色」两颗控件（用户第 ③ 条：副文本选项要像 Apple 便签那样只有几项）。
 *
 * 盯的是**调用边**，不是"菜单里有没有那几行字"：
 *  · 档位与色名的表在 `editor/marks.ts`，渲染在 `editor/dom.ts` —— 三处只要有一处没接上，
 *    用户看到的就是"点了没反应"或"点了但没效果"（本项目反复踩过的那一族）。
 *  · "标准 / 默认色"必须发**移除**，不是发一个 `step:'m'` 的标记 —— 后者会在文档里
 *    留下一个没有任何视觉效果的标记，还白增一次内容哈希。
 */
import { beforeEach, describe, expect, it } from 'vitest';
import { mount } from '@vue/test-utils';
import EditorToolbar from './components/EditorToolbar.vue';

beforeEach(() => {
  // jsdom 没有 ResizeObserver；工具条挂载时用它量"右边还有没有内容"（缺口 G30 那条）。
  (globalThis as unknown as { ResizeObserver: unknown }).ResizeObserver = class {
    observe(): void {}
    unobserve(): void {}
    disconnect(): void {}
  };
});

function makeToolbar(props: Record<string, unknown> = {}) {
  return mount(EditorToolbar, {
    props: { disabled: false, activeMarks: [], blockType: 'paragraph', ...props },
  });
}

describe('两颗控件在不在、是不是有名字的按钮', () => {
  it('字号与颜色各有一颗带 aria-label 的按钮，且都不是系统控件', () => {
    const w = makeToolbar();
    const size = w.get('[data-testid="tb-size"]');
    const color = w.get('[data-testid="tb-color"]');
    expect(size.attributes('aria-label')).toBe('文字大小');
    expect(color.attributes('aria-label')).toBe('文字颜色');
    // 原生 select / color input 的面板由操作系统画 ⇒ 两端不可能一致（第 ⑪ 条口径）
    expect(size.element.tagName).toBe('BUTTON');
    expect(color.element.tagName).toBe('BUTTON');
    expect(w.find('select').exists()).toBe(false);
    expect(w.find('input[type="color"]').exists()).toBe(false);
  });

  it('编辑器不可写时两颗都禁用（不许出现"能点但改了没用"）', () => {
    const w = makeToolbar({ disabled: true });
    expect(w.get('[data-testid="tb-size"]').attributes('disabled')).toBeDefined();
    expect(w.get('[data-testid="tb-color"]').attributes('disabled')).toBeDefined();
  });
});

function menuItems(w: ReturnType<typeof makeToolbar>, menuTestId: string): string[] {
  // 只数菜单项：`[data-testid^="tb-size-"]` 会把那层容器（tb-size-menu）也圈进来，
  // 它的 text() 是全菜单的拼接 ⇒ 断言会"多出一格"，红得莫名其妙。
  return w
    .findAll(`[data-testid="${menuTestId}"] [role="menuitemradio"]`)
    .map((item) => item.text().trim());
}

describe('字号菜单', () => {
  it('展开后是小 / 标准 / 大 / 特大四档，不多不少', async () => {
    const w = makeToolbar();
    await w.get('[data-testid="tb-size"]').trigger('click');
    expect(menuItems(w, 'tb-size-menu')).toEqual(['小', '标准', '大', '特大']);
  });

  it('选「特大」发的是带档位的 mark，不是 kind 开关', async () => {
    const w = makeToolbar();
    await w.get('[data-testid="tb-size"]').trigger('click');
    await w.get('[data-testid="tb-size-xl"]').trigger('click');
    const events = w.emitted('mark');
    expect(events).toEqual([['fontSize', { step: 'xl' }]]);
  });

  it('选「标准」发移除 —— 文档里不许留一个没有视觉效果的 step:"m"', async () => {
    const w = makeToolbar();
    await w.get('[data-testid="tb-size"]').trigger('click');
    await w.get('[data-testid="tb-size-m"]').trigger('click');
    expect(w.emitted('mark')).toBeUndefined();
    expect(w.emitted('unmark')).toEqual([['fontSize']]);
  });

  it('当前档在菜单里是选中的（aria-checked 只许有一项为 true）', async () => {
    const w = makeToolbar({ activeFontSize: 'l' });
    await w.get('[data-testid="tb-size"]').trigger('click');
    const checked = w
      .findAll('[data-testid^="tb-size-"]')
      .filter((item) => item.attributes('aria-checked') === 'true')
      .map((item) => item.attributes('data-testid'));
    expect(checked).toEqual(['tb-size-l']);
  });
});

describe('颜色菜单', () => {
  it('展开后是 默认色 + 五个色名，每项都带文字（不许只靠颜色传达）', async () => {
    const w = makeToolbar();
    await w.get('[data-testid="tb-color"]').trigger('click');
    expect(menuItems(w, 'tb-color-menu')).toEqual(['默认色', '红', '橙', '绿', '蓝', '紫']);
  });

  it('选「红」发色名而不是十六进制（深浅主题各自有一份 token 值）', async () => {
    const w = makeToolbar();
    await w.get('[data-testid="tb-color"]').trigger('click');
    await w.get('[data-testid="tb-color-red"]').trigger('click');
    expect(w.emitted('mark')).toEqual([['color', { name: 'red' }]]);
  });

  it('选「默认色」发移除', async () => {
    const w = makeToolbar({ activeColor: 'red' });
    await w.get('[data-testid="tb-color"]').trigger('click');
    await w.get('[data-testid="tb-color-default"]').trigger('click');
    expect(w.emitted('mark')).toBeUndefined();
    expect(w.emitted('unmark')).toEqual([['color']]);
  });
});

describe('一次只开一个菜单', () => {
  it('开字号再开颜色，字号那层必须收起（两层叠着画会互相盖住）', async () => {
    const w = makeToolbar();
    await w.get('[data-testid="tb-size"]').trigger('click');
    expect(w.find('[data-testid="tb-size-menu"]').exists()).toBe(true);
    await w.get('[data-testid="tb-color"]').trigger('click');
    expect(w.find('[data-testid="tb-size-menu"]').exists()).toBe(false);
    expect(w.find('[data-testid="tb-color-menu"]').exists()).toBe(true);
  });
});
