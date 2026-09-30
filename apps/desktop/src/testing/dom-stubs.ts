/**
 * jsdom 没有的浏览器 API，在这里补一个"存在但什么都不做"的版本。
 *
 * **为什么补在这里而不是组件里写 `if (typeof ResizeObserver === 'undefined')`**：
 * 那个判断在真实浏览器里永远为假，等于把一段"只在测试里走的分支"种进产品代码（§39 不许的形状）。
 * 而布局观察这件事的**行为**归 L4 真浏览器那条 lane 判（`verify-app.mjs` 的「工具条放不下…」一步），
 * 单测只需要组件能挂载。
 */
class NoopResizeObserver {
  constructor(private readonly cb: ResizeObserverCallback) {}

  observe(): void {
    this.cb([], this as unknown as ResizeObserver);
  }

  unobserve(): void {}

  disconnect(): void {}
}

if (typeof globalThis.ResizeObserver === 'undefined') {
  globalThis.ResizeObserver = NoopResizeObserver as unknown as typeof ResizeObserver;
}
