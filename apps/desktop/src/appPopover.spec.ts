/**
 * 悬浮操作面板（AppPopover）的行为门禁。
 *
 * 钉的是"确认层不许占版面、也不许顺手触发宿主行"这一族：
 *   ① 没点开之前，面板不在 DOM 里（不是藏起来，是真的没渲染 —— 藏着的占位块一样会顶行）；
 *   ② 点触发按钮 ⇒ 面板出现，带插槽内容；
 *   ③ 面板里点一下**不许**冒泡到宿主行（以前点"确认删除"会顺手把那一篇打开）；
 *   ④ 插槽拿得到的 close() 能把面板收掉（确认之后必须自己关，不能留着）。
 * 渲染后的 position/是否顶偏行，量在真浏览器那条腿里（scripts/verify-layout.mjs ⑧）。
 */
import { describe, expect, it, vi } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
import AppPopover from './components/ui/AppPopover.vue';

function mounted() {
  const host = { template: '<div><AppPopover icon="⌫" label="删除文件夹" testid="folder-delete"><template #default="{ close }"><p>删除后笔记移入默认本</p><button data-testid="yes" @click="close()">确认</button></template></AppPopover></div>' };
  return mount(host, { global: { components: { AppPopover } }, attachTo: document.body });
}

describe('AppPopover', () => {
  it('开合由触发按钮控制，面板里是插槽内容', async () => {
    const wrapper = mounted();
    expect(wrapper.find('[data-testid="app-popover-panel"]').exists()).toBe(false);
    await wrapper.get('[data-testid="folder-delete"]').trigger('click');
    await flushPromises();
    const panel = wrapper.get('[data-testid="app-popover-panel"]');
    expect(panel.text()).toContain('删除后笔记移入默认本');
    expect(wrapper.get('[data-testid="folder-delete"]').attributes('aria-expanded')).toBe('true');
  });

  it('插槽里的 close() 收得掉面板', async () => {
    const wrapper = mounted();
    await wrapper.get('[data-testid="folder-delete"]').trigger('click');
    await flushPromises();
    await wrapper.get('[data-testid="yes"]').trigger('click');
    await flushPromises();
    expect(wrapper.find('[data-testid="app-popover-panel"]').exists()).toBe(false);
  });

  it('点面板不冒泡：宿主行的点击处理器一次都不该被叫到', async () => {
    const spy = vi.fn();
    const wrapper = mount(
      {
        components: { AppPopover },
        methods: { onClickRow: spy },
        template: '<div @click="onClickRow"><AppPopover icon="⌫" label="删除" testid="t"><template #default><button data-testid="yes">确认</button></template></AppPopover></div>',
      },
      { attachTo: document.body },
    );
    await wrapper.get('[data-testid="t"]').trigger('click');
    await flushPromises();
    spy.mockClear();
    await wrapper.get('[data-testid="yes"]').trigger('click');
    await flushPromises();
    expect(spy, '点面板里的按钮冒泡到了宿主行').not.toHaveBeenCalled();
  });

  it('给了 #glyph 就画我那枚字形，且不再画图标（两颗同框会互相挤掉）', async () => {
    const wrapper = mount(
      {
        components: { AppPopover },
        template:
          '<AppPopover label="颜色" testid="t"><template #glyph><span data-testid="my-glyph" /></template></AppPopover>',
      },
      { attachTo: document.body },
    );
    const trigger = wrapper.get('[data-testid="t"]');
    expect(trigger.find('[data-testid="my-glyph"]').exists(), '插槽给的 glyph 没画出来').toBe(true);
    expect(trigger.find('svg').exists(), '用了 #glyph 还画一份图标，按钮上会多出第二枚字形').toBe(false);
  });

  it('不给 #glyph 时 icon 照旧（老调用点不许因为这次改动退化）', async () => {
    const wrapper = mount(
      {
        components: { AppPopover },
        template: '<AppPopover icon="trash" label="删除" testid="t"><template #default><p>x</p></template></AppPopover>',
      },
      { attachTo: document.body },
    );
    const trigger = wrapper.get('[data-testid="t"]');
    expect(trigger.find('svg').exists(), 'icon 那一支退化了：按钮里连字形都没有').toBe(true);
    expect(trigger.find('[data-testid="my-glyph"]').exists()).toBe(false);
  });

  it('换成自绘 glyph 后按钮仍有可读名字（颜色不能是一颗只有眼睛看得见的按钮）', async () => {
    const wrapper = mount(
      {
        components: { AppPopover },
        template: '<AppPopover label="为这个文件夹选颜色" testid="t"><template #glyph><span /></template></AppPopover>',
      },
      { attachTo: document.body },
    );
    expect(wrapper.get('[data-testid="t"]').attributes('aria-label')).toBe('为这个文件夹选颜色');
  });
});
