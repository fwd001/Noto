/**
 * v-editable：把块内容写进 contenteditable，并且只在"模型真的变了"时改写 DOM。
 * 用户正在打字时绝不重写 DOM —— 否则光标会跳。输入路径解析出的模型用
 * markAsParsed 登记签名，随后的响应式刷新就会识别为"同一份内容"而跳过。
 */
import type { Inline } from '../api/types';
import { renderBlockHtml } from './dom';
import { inlineText } from './model';

export interface EditableBinding {
  content: Inline[];
  placeholder?: string;
}

const signatures = new WeakMap<HTMLElement, string>();

function write(el: HTMLElement, binding: EditableBinding): void {
  el.innerHTML = renderBlockHtml(binding.content);
  signatures.set(el, JSON.stringify(binding.content));
  el.dataset.empty = inlineText(binding.content).length === 0 ? 'true' : 'false';
}

export const vEditable = {
  mounted(el: HTMLElement, binding: { value: EditableBinding }): void {
    write(el, binding.value);
  },
  updated(el: HTMLElement, binding: { value: EditableBinding }): void {
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
