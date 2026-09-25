/** 小工具：可注入定时器的防抖器（测试里用假时钟即可精确断言触发次数）。 */

export interface Timers {
  set: (callback: () => void, ms: number) => number;
  clear: (handle: number) => void;
}

export const realTimers: Timers = {
  set: (callback, ms) => setTimeout(callback, ms) as unknown as number,
  clear: (handle) => clearTimeout(handle),
};

export interface Debounced<A extends unknown[]> {
  (...args: A): void;
  cancel: () => void;
  flush: () => void;
  pending: () => boolean;
}

export function createDebounced<A extends unknown[]>(
  run: (...args: A) => void,
  ms: number,
  timers: Timers = realTimers,
): Debounced<A> {
  let handle: number | null = null;
  let lastArgs: A | null = null;

  const cancel = () => {
    if (handle !== null) {
      timers.clear(handle);
      handle = null;
    }
    lastArgs = null;
  };

  const invoke = () => {
    handle = null;
    const args = lastArgs;
    lastArgs = null;
    if (args) run(...args);
  };

  const debounced = ((...args: A) => {
    lastArgs = args;
    if (handle !== null) timers.clear(handle);
    handle = timers.set(invoke, ms);
  }) as Debounced<A>;

  debounced.cancel = cancel;
  debounced.flush = () => {
    if (handle === null) return;
    invoke();
  };
  debounced.pending = () => handle !== null;
  return debounced;
}

export const AUTOSAVE_DEBOUNCE_MS = 1200;
export const SEARCH_DEBOUNCE_MS = 200;
export const LINK_PROBE_INTERVAL_MS = 5000;
