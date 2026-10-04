/**
 * 统一选择控件的行为门禁。
 *
 * 这条 spec 存在的唯一理由：原生 `<select>` 的展开面板由**操作系统**绘制，
 * 所以"两端 UI 一致"这句话用原生控件永远验不了。换成无头库之后，
 * 面板是我们 DOM 里的节点 —— 于是"是不是我们的面板"变成可断言的事实。
 *
 * 判据刻意打在"行为"而不是"源码形状"上：
 *   ① 触发器是 BUTTON（不是 SELECT）；
 *   ② 点开后选项列表在**我们自己的 DOM** 里，条数与传入的 options 一致；
 *   ③ 选一项 ⇒ 发出 update:modelValue，值是那一项的 value；
 *   ④ 键盘（↓ + Enter）能改值 —— 这条是"我们没自己重写下拉行为"的证据：
 *      它由库提供，我们只是没去实现它；
 *   ⑤ 当前值显示在按钮上（不选的时候也要读得出选了什么）。
 */
import { describe, expect, it } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
import AppSelect from './components/ui/AppSelect.vue';

const OPTIONS = [
  { value: 'strict', label: '严格校验' },
  { value: 'pin', label: '固定指纹' },
  { value: 'insecure_local', label: '仅本机放行' },
];

async function open(wrapper: ReturnType<typeof mount>) {
  await wrapper.get('[data-testid="app-select"]').trigger('click');
  await flushPromises();
}

describe('AppSelect：面板是我们画的', () => {
  it('触发器是按钮，不是原生 select', () => {
    const wrapper = mount(AppSelect, { props: { modelValue: 'pin', options: OPTIONS, label: 'TLS 策略', testid: 'account-tls' } });
    const trigger = wrapper.get('[data-testid="account-tls"]').element;
    expect(trigger.tagName).toBe('BUTTON');
    expect(wrapper.find('select').exists(), '还留着原生 select 就等于没换干净').toBe(false);
  });

  it('按钮上显示当前值的文字（不是 value）', () => {
    const wrapper = mount(AppSelect, { props: { modelValue: 'pin', options: OPTIONS, label: 'TLS 策略' } });
    expect(wrapper.text()).toContain('固定指纹');
    expect(wrapper.text()).not.toContain('insecure_local');
  });

  it('点开之后，选项在自家 DOM 里且条数一致', async () => {
    const wrapper = mount(AppSelect, { props: { modelValue: 'strict', options: OPTIONS, label: 'TLS 策略' } });
    expect(wrapper.find('[data-testid="app-select-panel"]').exists(), '没点开之前不该有面板').toBe(false);
    await open(wrapper);
    const panel = wrapper.find('[data-testid="app-select-panel"]');
    expect(panel.exists(), '点了没开面板').toBe(true);
    const items = panel.findAll('[role="option"]');
    expect(items).toHaveLength(OPTIONS.length);
    expect(items.map((i) => i.text())).toEqual(OPTIONS.map((o) => o.label));
  });

  it('点某一项 ⇒ 发出该项的 value', async () => {
    const wrapper = mount(AppSelect, { props: { modelValue: 'strict', options: OPTIONS, label: 'TLS 策略' } });
    await open(wrapper);
    const items = wrapper.findAll('[role="option"]');
    await items[2].trigger('click');
    await flushPromises();
    const emitted = wrapper.emitted('update:modelValue');
    expect(emitted?.at(-1)?.[0]).toBe('insecure_local');
  });

  it('键盘 ↓ + Enter 也能改值（行为由库提供，不是我们重写的）', async () => {
    const wrapper = mount(AppSelect, { props: { modelValue: 'strict', options: OPTIONS, label: 'TLS 策略' } });
    // 打开之后焦点在面板上（库自己的焦点管理），按键要发给它。
    await open(wrapper);
    const panel = wrapper.get('[role="listbox"]');
    await panel.trigger('keydown', { key: 'ArrowDown' });
    await flushPromises();
    await panel.trigger('keydown', { key: 'Enter' });
    await flushPromises();
    const emitted = wrapper.emitted('update:modelValue');
    expect(emitted?.at(-1)?.[0], '键盘选值没发出来').toBe('pin');
  });
});
