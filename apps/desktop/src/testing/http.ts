/**
 * 测试用的传输桩：只替换"网络出口"这一层（fetch），
 * store / 编辑器 / 折叠逻辑跑的都是真实代码路径；应用内没有任何假数据层。
 */
import { vi } from 'vitest';

export interface RecordedCall {
  name: string;
  args: Record<string, unknown>;
}

interface StubResponse {
  ok: boolean;
  status: number;
  text: () => Promise<string>;
}

export interface LocalService {
  calls: RecordedCall[];
  callsOf: (name: string) => RecordedCall[];
  lastArgsOf: (name: string) => Record<string, unknown> | undefined;
}

type Handler = (args: Record<string, unknown>) => unknown | Promise<unknown>;

/** 后端未实现或返回错误的命令名 → 统一 CommandResult 结构。 */
export function stubLocalService(handlers: Record<string, Handler> = {}): LocalService {
  const calls: RecordedCall[] = [];

  const impl = async (input: unknown, init?: unknown): Promise<StubResponse> => {
    const url = String(input);
    const name = url.split('/cmd/')[1]?.split('?')[0] ?? '';
    const rawBody = (init as { body?: string } | undefined)?.body;
    const args = rawBody ? (JSON.parse(rawBody) as Record<string, unknown>) : {};
    calls.push({ name, args });
    const handler = handlers[name];
    if (!handler) {
      return { ok: false, status: 404, text: async () => JSON.stringify({ ok: false, error: { code: 'not_found' } }) };
    }
    const payload = await handler(args);
    return { ok: true, status: 200, text: async () => JSON.stringify(payload) };
  };

  vi.stubGlobal('fetch', impl);

  return {
    calls,
    callsOf: (name: string) => calls.filter((call) => call.name === name),
    lastArgsOf: (name: string) => [...calls].reverse().find((call) => call.name === name)?.args,
  };
}

/** 连本地服务都没起的场景：fetch 直接失败。 */
export function stubServiceDown(): LocalService {
  const calls: RecordedCall[] = [];
  vi.stubGlobal('fetch', async (input: unknown) => {
    calls.push({ name: String(input), args: {} });
    throw new TypeError('Failed to fetch');
  });
  return {
    calls,
    callsOf: (name: string) => calls.filter((call) => call.name.includes(name)),
    lastArgsOf: () => undefined,
  };
}

export function noteFixture(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    id: 'note-1',
    folderId: null,
    doc: { v: 1, content: [{ id: 'abcd1234', type: 'paragraph', content: [{ text: '原始内容' }] }] },
    title: '原始内容',
    summary: '',
    pinned: false,
    charCount: 4,
    hasAttachment: false,
    rev: 3,
    createdAt: '2026-01-01T00:00:00Z',
    updatedAt: '2026-01-01T00:00:00Z',
    deletedAt: null,
    ...overrides,
  };
}
