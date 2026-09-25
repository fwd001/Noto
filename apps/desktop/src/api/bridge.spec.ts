/**
 * 壳内通道的契约测试。
 *
 * 为什么单独钉这一组：Rust 侧只注册了 **一个** Tauri 命令
 * （`notera_command(name, args)` → `commands::dispatch`），两边靠字面量对齐。
 * 字面量漂了不会编译错、也不会有类型错，只会在真窗口里表现为"每个按钮都没反应"。
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { BridgeError, callCommand, currentTransport, inTauri, onUiEvent } from './bridge';

const invoke = vi.fn();
const listened: Array<{ name: string; handler: (e: { payload: unknown }) => void }> = [];
let fire: ((payload: unknown) => void) | null = null;

vi.mock('@tauri-apps/api/core', () => ({ invoke }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: (name: string, handler: (e: { payload: unknown }) => void) => {
    listened.push({ name, handler });
    fire = (payload: unknown) => handler({ payload });
    return Promise.resolve(() => {});
  },
}));

function asTauri() {
  Object.defineProperty(window, '__TAURI_INTERNALS__', { value: {}, configurable: true, writable: true });
}
function asBrowser() {
  Object.defineProperty(window, '__TAURI_INTERNALS__', { value: undefined, configurable: true, writable: true });
}
async function flush() {
  // 通道是 `await import('@tauri-apps/api/event')` 惰性建立的，需要让宏任务队列转一圈。
  for (let i = 0; i < 5; i++) await new Promise((r) => setTimeout(r, 0));
}

beforeEach(() => {
  invoke.mockReset();
  listened.length = 0;
  fire = null;
});

describe('Tauri 通道契约', () => {
  beforeEach(asTauri);

  it('命令名与载荷形状必须和壳里的 `notera_command(name, args)` 逐字对齐', async () => {
    invoke.mockResolvedValue({ ok: true, payload: { id: 'x' } });
    const args = { id: '0192a000-0000-7000-8000-000000000001', doc: { v: 1, content: [] }, expectedRev: 4 };
    await expect(callCommand('edit_note', args)).resolves.toEqual({ id: 'x' });

    expect(invoke).toHaveBeenCalledTimes(1);
    const [cmd, payload] = invoke.mock.calls[0];
    expect(cmd).toBe('notera_command');
    expect(Object.keys(payload as object).sort()).toEqual(['args', 'name']);
    expect(payload).toEqual({ name: 'edit_note', args });
  });

  it('ok:false 的拒绝载荷折成 BridgeError，并带上 messageKey 与 retryable', async () => {
    invoke.mockRejectedValue({ code: 'offline', messageKey: 'error.offline', retryable: true });
    await expect(callCommand('sync_status', {})).rejects.toMatchObject({
      name: 'BridgeError',
      code: 'offline',
      messageKey: 'error.offline',
      retryable: true,
    });
  });

  it('stale_edit 的真实 rev 从 detail 里取到（冲突流程要用它做"重新加载"）', async () => {
    invoke.mockRejectedValue({
      code: 'stale_edit',
      messageKey: 'note.staleEdit',
      retryable: false,
      detail: { expected: 4, actual: 7 },
    });
    const err = (await callCommand('edit_note', { id: 'a', doc: {}, expectedRev: 4 }).catch((e: unknown) => e)) as BridgeError;
    expect(err).toBeInstanceOf(BridgeError);
    expect(err.isStaleEdit).toBe(true);
    expect(err.actualRev).toBe(7);
  });

  it('事件名字面量与壳里的 EVENT_NAME 一致，且载荷能到达订阅者', async () => {
    const seen: unknown[] = [];
    onUiEvent((e) => seen.push(e));
    await flush();
    expect(listened.map((l) => l.name)).toContain('notera://event');

    fire?.({ kind: 'sync', badge: 'syncing', progress: null, errorCode: null });
    expect(seen).toHaveLength(1);
    expect(seen[0]).toMatchObject({ kind: 'sync', badge: 'syncing' });
  });

  it('通道判定：有 __TAURI_INTERNALS__ 才是 tauri，没有就是 http', async () => {
    expect(inTauri()).toBe(true);
    expect(currentTransport()).toBe('tauri');
    asBrowser();
    expect(inTauri()).toBe(false);
    expect(currentTransport()).toBe('http');
  });
});
