/**
 * 壳内通道的契约测试。
 *
 * 为什么单独钉这一组：Rust 侧只注册了 **一个** Tauri 命令
 * （`notera_command(name, args)` → `commands::dispatch`），两边靠字面量对齐。
 * 字面量漂了不会编译错、也不会有类型错，只会在真窗口里表现为"每个按钮都没反应"。
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { BridgeError, callCommand, currentTransport, inTauri, onUiEvent, pickOpenPath, pickSavePath } from './bridge';

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

describe('系统文件对话框（G75）', () => {
  it('浏览器通道：unavailable 且一个 invoke 都不发（插件 IPC 只在壳里）', async () => {
    asBrowser();
    const r = await pickSavePath('', [{ name: 'z', extensions: ['zip'] }]);
    expect(r).toEqual({ kind: 'unavailable', why: 'not-in-tauri' });
    expect(invoke).not.toHaveBeenCalled();
  });

  it('壳内 save：命令名与选项逐字对齐（少一个依赖，命令名就写在调用点上）', async () => {
    asTauri();
    invoke.mockResolvedValue('D:/x/备份.zip');
    const r = await pickSavePath('D:/seed.zip', [{ name: 'Noto 备份包（.zip）', extensions: ['zip'] }]);
    expect(r).toEqual({ kind: 'picked', path: 'D:/x/备份.zip' });
    expect(invoke).toHaveBeenCalledTimes(1);
    const [cmd, payload] = invoke.mock.calls[0];
    expect(cmd).toBe('plugin:dialog|save');
    expect(payload).toEqual({
      options: { defaultPath: 'D:/seed.zip', filters: [{ name: 'Noto 备份包（.zip）', extensions: ['zip'] }] },
    });
  });

  it('壳内 open：单文件、非目录（这两个参数错了会在真壳里变成"选目录时也当文件"）', async () => {
    asTauri();
    invoke.mockResolvedValue('C:/a.enex');
    await expect(pickOpenPath([{ name: 'n', extensions: ['enex'] }])).resolves.toEqual({ kind: 'picked', path: 'C:/a.enex' });
    const [cmd, payload] = invoke.mock.calls[0];
    expect(cmd).toBe('plugin:dialog|open');
    expect(payload).toEqual({
      options: { multiple: false, directory: false, filters: [{ name: 'n', extensions: ['enex'] }] },
    });
  });

  it('取消（null）与空串都算 cancelled —— 那是用户的决定，不是错误', async () => {
    asTauri();
    invoke.mockResolvedValueOnce(null);
    await expect(pickOpenPath([{ name: 'n', extensions: ['zip'] }])).resolves.toEqual({ kind: 'cancelled' });
    invoke.mockResolvedValueOnce('   ');
    await expect(pickOpenPath([{ name: 'n', extensions: ['zip'] }])).resolves.toEqual({ kind: 'cancelled' });
  });

  it('被 ACL 拒（G75 的 before 原文）⇒ unavailable 且带着原文，绝不静默', async () => {
    asTauri();
    invoke.mockRejectedValue(
      new Error('dialog.save not allowed. Permissions associated with this command: dialog:allow-save, dialog:default'),
    );
    const r = await pickSavePath('', [{ name: 'n', extensions: ['zip'] }]);
    expect(r.kind).toBe('unavailable');
    expect((r as { why: string }).why).toContain('not allowed');
  });
});
