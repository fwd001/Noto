/**
 * 移动端键盘弹起的视口账（用户口径：「你要考虑好移动端的这种体验，就是输入框弹起带来的这种体验」）。
 *
 * 症状是这类问题里最难自证的一种：**代码全对，屏幕上就是看不见在输入**。iOS WKWebView 里
 * 键盘盖住版心，而 `window.innerHeight` **纹丝不动**（它量的是布局视口）—— 于是整套
 * `height: 100%` 的外壳照旧占满整屏，光标所在那一行永远在键盘底下。Android 会 resize，
 * iOS 不会 ⇒ 只有 `visualViewport` 是两端都说了算的那个来源。
 *
 * 这里盯四件事：
 *  ① 变量真从 `visualViewport` 来（不是 innerHeight）；
 *  ② 键盘那一格（inset）算得对，且**不为负**（iOS 页面被顶上去时 offsetTop > 0）；
 *  ③ 变化要真的传到位（dispatch resize ⇒ 变量跟着变）与 detach 之后不再传（调用边）；
 *  ④ CSS 那一侧确实在用这两个变量 —— 只在 JS 里设了变量而样式没读，是本项目反复踩过的
 *    "写的那一位不是被读的那一位"。
 *
 * 这一条**不**等于真机验收：jsdom 与桌面 Chromium 都没有真键盘。它验的是"键盘一改变视口，
 * 界面这一侧会跟着缩"这条链路；iOS 真机那一格仍记 BLOCKED（本机无设备）。
 */
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { useShellStore } from './stores/shell';

const FILES = import.meta.glob<string>('./**/*.{vue,ts,css}', {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>;

function read(rel: string): string {
  const key = `./${rel.replace(/^\.\//, '')}`;
  const text = FILES[key];
  if (typeof text !== 'string') throw new Error(`读不到 ${key}（glob 没打进这个文件或键名写错）`);
  return text;
}

const LAYOUT_H = 800;
const LAYOUT_W = 390;

/** 装一个最小的 `visualViewport`：真对象是 EventTarget + 一组只读数，这里只要这两样。 */
function stubViewport(height: number, offsetTop = 0): EventTarget & { height: number; offsetTop: number } {
  const vv = Object.assign(new EventTarget(), { height, offsetTop });
  Object.defineProperty(window, 'visualViewport', { value: vv, configurable: true, writable: true });
  return vv;
}

function setLayout(w: number, h: number): void {
  Object.defineProperty(window, 'innerWidth', { value: w, configurable: true, writable: true });
  Object.defineProperty(window, 'innerHeight', { value: h, configurable: true, writable: true });
}

function varOf(name: string): string {
  return document.documentElement.style.getPropertyValue(name);
}

let stop: (() => void) | null = null;
const savedVv = Object.getOwnPropertyDescriptor(window, 'visualViewport');
const savedW = Object.getOwnPropertyDescriptor(window, 'innerWidth');
const savedH = Object.getOwnPropertyDescriptor(window, 'innerHeight');

beforeEach(() => {
  setActivePinia(createPinia());
  document.documentElement.style.removeProperty('--app-vh');
  document.documentElement.style.removeProperty('--app-kb');
  setLayout(LAYOUT_W, LAYOUT_H);
});

afterEach(() => {
  stop?.();
  stop = null;
  Object.defineProperty(window, 'innerWidth', savedW ?? { value: 1024, configurable: true, writable: true });
  Object.defineProperty(window, 'innerHeight', savedH ?? { value: 768, configurable: true, writable: true });
  if (savedVv) Object.defineProperty(window, 'visualViewport', savedVv);
  else delete (window as unknown as { visualViewport?: unknown }).visualViewport;
});

describe('键盘弹起时界面要真的缩起来', () => {
  it('视口变量取自 visualViewport，不是那个纹丝不动的 innerHeight', () => {
    // 键盘占了 300px：布局视口没变，可视视口变了 —— 只有后者说真话
    stubViewport(500, 0);
    stop = useShellStore().observeViewport();
    expect(varOf('--app-vh')).toBe('500px');
    expect(varOf('--app-kb')).toBe('300px');
  });

  it('inset 不许为负，版心也不许比窗口还高', () => {
    const vv = stubViewport(500, 120); // 500 + 120 = 620 < 800 ⇒ 剩 180 是被顶上去的那一段
    stop = useShellStore().observeViewport();
    expect(varOf('--app-kb')).toBe('180px');

    // 键盘比版心还高（小屏横屏）：inset 夹到 0，vh 夹到布局视口 ——
    // 给一个负的 padding 或比窗口还高的一屏，都是把界面顶飞。
    vv.height = 900;
    vv.dispatchEvent(new Event('resize'));
    expect(varOf('--app-kb')).toBe('0px');
    expect(varOf('--app-vh')).toBe('800px');
  });

  it('键盘收起之后要复原（变化真的传到位，不是只在挂载时算一次）', () => {
    const vv = stubViewport(500, 0);
    stop = useShellStore().observeViewport();
    expect(varOf('--app-vh')).toBe('500px');

    vv.height = LAYOUT_H; // 键盘收起
    vv.dispatchEvent(new Event('resize'));
    expect(varOf('--app-vh')).toBe('800px');
    expect(varOf('--app-kb')).toBe('0px');

    vv.height = 500; // 再弹起来
    vv.dispatchEvent(new Event('scroll')); // iOS 上也常只发 scroll
    expect(varOf('--app-vh')).toBe('500px');
    expect(varOf('--app-kb')).toBe('300px');
  });

  it('detach 之后不再改这两个变量（卸载留了监听器就是第二份真相）', () => {
    const vv = stubViewport(500, 0);
    const s = useShellStore().observeViewport();
    s();
    vv.height = 200;
    vv.dispatchEvent(new Event('resize'));
    expect(varOf('--app-vh')).toBe('500px');
  });

  it('没有 visualViewport 的老 WebView：退回 innerHeight，且不抛', () => {
    delete (window as unknown as { visualViewport?: unknown }).visualViewport;
    stop = useShellStore().observeViewport();
    expect(varOf('--app-vh')).toBe(`${LAYOUT_H}px`);
    expect(varOf('--app-kb')).toBe('0px');
  });

  it('keyboardInset 这一位在 store 上也读得到（别的组件要按它让位）', () => {
    stubViewport(500, 0);
    const shell = useShellStore();
    stop = shell.observeViewport();
    expect(shell.keyboardInset).toBe(300);
  });
});

describe('样式那一侧真的在读这两个变量', () => {
  it('外壳的高度用 --app-vh', () => {
    const css = read('styles/base.css');
    const block = css.slice(css.indexOf('.app-shell'));
    expect(block.slice(0, 220)).toContain('height: var(--app-vh)');
  });

  it(':root 给了兜底值，JS 没跑起来也不能没有高度', () => {
    const css = read('styles/base.css');
    expect(css).toMatch(/--app-vh:\s*100dvh/);
    expect(css).toMatch(/--app-kb:\s*0px/);
  });

  it('版心不许再用 100vh —— 那是"键盘不算数"的那种高度', () => {
    const offenders = Object.entries(FILES)
      .filter(([path, text]) => !path.includes('viewport.spec') && /100vh/.test(text))
      .map(([path]) => path);
    expect(offenders).toEqual([]);
  });

  it('浮层（toast）给键盘让位', () => {
    // `.toast-host` 是 `position: fixed` —— 固定层相对**布局视口**定位，
    // iOS 上键盘不改它 ⇒ 不加这一格，toast 会弹在键盘底下。
    const css = read('styles/base.css');
    const block = css.slice(css.indexOf('.toast-host'));
    expect(block.slice(0, 400)).toContain('var(--app-kb)');
  });
});
