/**
 * v-editable：把块内容写进 contenteditable，并且只在"模型真的变了"时改写 DOM。
 * 用户正在打字时绝不重写 DOM —— 否则光标会跳。输入路径解析出的模型用
 * markAsParsed 登记签名，随后的响应式刷新就会识别为"同一份内容"而跳过。
 */
import type { Inline } from '../api/types';
import { renderBlockHtml } from './dom';
import { blockTypeLabel } from './labels';
import { inlineText } from './model';

export interface EditableBinding {
  content: Inline[];
  placeholder?: string;
  /**
   * 块型（`paragraph` / `heading` …）。用来给这个 `role="textbox"` 的元素起读屏名字。
   *
   * 为什么由指令写这个属性，而不是模板里 `:aria-label="…"`：实测在**这个元素上加一个
   * 响应式属性绑定**，打字内容就进不了模型 —— 黑盒 UAT 从 10/10 掉到 4/10，库里的笔记
   * `charCount: 0` 而屏幕上明明有字（`aria-label` 换成静态写法就 10/10）。这个元素的内容
   * 归指令管，名字也归指令写，就别让 vdom 的补丁再碰它。真正的脆弱点在 autosave/签名那条
   * 路上，已单独记下待查。
   */
  type?: string;
}

const signatures = new WeakMap<HTMLElement, string>();

/**
 * 名字与内容是两件事：切换块型（正文 → 引用）时文本一个字没变，`updated` 会因为签名
 * 相同而**跳过覆写 DOM** —— 名字必须在那之前落好，否则读屏念的还是上一个块型。
 */
function name(el: HTMLElement, binding: EditableBinding): void {
  if (binding.type) el.setAttribute('aria-label', blockTypeLabel(binding.type));
}

function write(el: HTMLElement, binding: EditableBinding): void {
  name(el, binding);
  el.innerHTML = renderBlockHtml(binding.content);
  signatures.set(el, JSON.stringify(binding.content));
  el.dataset.empty = inlineText(binding.content).length === 0 ? 'true' : 'false';
}

export const vEditable = {
  mounted(el: HTMLElement, binding: { value: EditableBinding }): void {
    write(el, binding.value);
  },
  updated(el: HTMLElement, binding: { value: EditableBinding }): void {
    name(el, binding.value);
    const next = JSON.stringify(binding.value.content);
    if (signatures.get(el) === next) {
      el.dataset.empty = inlineText(binding.value.content).length === 0 ? 'true' : 'false';
      return;
    }
    write(el, binding.value);
  },
};

/** 解析 DOM 之后登记签名，阻止本轮刷新把 DOM 覆写回去。 */
export function markAsParsed(el: HTMLElement, content: Inline[]): void {
  signatures.set(el, JSON.stringify(content));
  el.dataset.empty = inlineText(content).length === 0 ? 'true' : 'false';
}

export function parsedSignature(el: HTMLElement): string | undefined {
  return signatures.get(el);
}
