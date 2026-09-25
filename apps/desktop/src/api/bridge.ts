/**
 * 传输层：同一套 DTO，两种通道。
 *  1) Tauri 壳内：invoke(name, args) + listen('notera://event')
 *  2) 纯浏览器 dev：fetch(http://127.0.0.1:17323/cmd/<name>) + EventSource(<base>/events)
 *     —— 连的是本地核心服务（真实存储），不是假数据。
 * UI 其余部分只看到 callCommand / onUiEvent，不关心通道。
 */
import type { CommandResult, UiEvent } from './types';

export type Transport = 'tauri' | 'http';

/** 通道可达性：dev HTTP 下后端没起时，UI 必须显示"未连接到本地服务"而不是白屏。 */
export type LinkState = 'unknown' | 'connecting' | 'ready' | 'unreachable';

const DEFAULT_DEV_BASE = 'http://127.0.0.1:17323';
const CMD_TIMEOUT_MS = 8000;

export function inTauri(): boolean {
  return typeof window !== 'undefined' && typeof window.__TAURI_INTERNALS__ === 'object';
}

export function devBase(): string {
  const configured = import.meta.env?.VITE_NOTERA_DEV_BASE;
  const base = typeof configured === 'string' && configured.length > 0 ? configured : DEFAULT_DEV_BASE;
  return base.endsWith('/') ? base.slice(0, -1) : base;
}

export function currentTransport(): Transport {
  return inTauri() ? 'tauri' : 'http';
}

export const STALE_EDIT = 'stale_edit';

/** 归一化的通道错误：只携带用户可读信息（messageKey）与流程需要的少量结构化字段。 */
export class BridgeError extends Error {
  readonly code: string;
  readonly messageKey: string;
  readonly retryable: boolean;
  readonly actualRev: number | null;

  constructor(init: { code?: string; messageKey?: string; retryable?: boolean; actualRev?: number | null }) {
    const code = init.code ?? 'sync_failed';
    super(init.messageKey ?? code);
    this.name = 'BridgeError';
    this.code = code;
    this.messageKey = init.messageKey ?? defaultKeyForCode(code);
    this.retryable = init.retryable ?? code !== STALE_EDIT;
    this.actualRev = init.actualRev ?? null;
  }

  get isStaleEdit(): boolean {
    return this.code === STALE_EDIT;
  }

  get isOffline(): boolean {
    return this.code === 'offline' || this.code === 'transport_unreachable';
  }
}

function defaultKeyForCode(code: string): string {
  switch (code) {
    case STALE_EDIT:
      return 'note.staleEdit';
    case 'transport_unreachable':
      return 'link.unreachable';
    case 'db_too_new':
      return 'state.dbTooNew';
    default:
      return `error.${code}`;
  }
}

interface RawErrorShape {
  code?: string;
  kind?: string;
  messageKey?: string;
  errorCode?: string;
  retryable?: boolean;
  actualRev?: number;
  actual_rev?: number;
  detail?: { actual?: number; actualRev?: number };
  error?: RawErrorShape | string;
}

function normalizeError(raw: unknown, httpStatus?: number): BridgeError {
  if (raw instanceof BridgeError) return raw;
  let value: unknown = raw;
  if (typeof value === 'string') {
    value = parseJsonOrKeep(value);
  }
  const shape = (typeof value === 'object' && value !== null ? value : {}) as RawErrorShape;
  const nested = typeof shape.error === 'object' && shape.error !== null ? shape.error : shape;
  const code =
    nested.code ?? nested.kind ?? nested.errorCode ?? (typeof shape.error === 'string' ? shape.error : undefined) ??
    (httpStatus !== undefined && httpStatus >= 400 ? 'server_unavailable' : 'sync_failed');
  const revs = [nested.actualRev, nested.actual_rev, nested.detail?.actualRev, nested.detail?.actual];
  const actualRev = revs.find((r): r is number => typeof r === 'number') ?? null;
  return new BridgeError({
    code: String(code),
    messageKey: typeof nested.messageKey === 'string' ? nested.messageKey : undefined,
    retryable: typeof nested.retryable === 'boolean' ? nested.retryable : undefined,
    actualRev,
  });
}

function parseJsonOrKeep(text: string): unknown {
  const trimmed = text.trim();
  if (!trimmed.startsWith('{') && !trimmed.startsWith('[')) return text;
  try {
    return JSON.parse(trimmed) as unknown;
  } catch {
    return text;
  }
}

/** Tauri invoke 与 HTTP fetch 都返回 CommandResult 或裸 DTO；此处统一拆包。 */
function unwrap<T>(data: unknown): T {
  const result = data as CommandResult<T> | null;
  if (result !== null && typeof result === 'object' && typeof result.ok === 'boolean') {
    if (!result.ok) {
      throw normalizeError(result.error ?? { code: 'sync_failed' });
    }
    return (result.payload ?? (null as unknown as T)) as T;
  }
  return data as T;
}

/** 壳里只注册了这一个命令；命令名走参数，避免为 30 个命令写 30 个转发函数。 */
const TAURI_COMMAND = 'notera_command';

async function invokeTauri<T>(name: string, args: Record<string, unknown>): Promise<T> {
  const core = await import('@tauri-apps/api/core');
  try {
    return unwrap<T>(await core.invoke<T>(TAURI_COMMAND, { name, args }));
  } catch (error) {
    throw normalizeError(error);
  }
}

async function invokeHttp<T>(name: string, args: Record<string, unknown>): Promise<T> {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), CMD_TIMEOUT_MS);
  try {
    const response = await fetch(`${devBase()}/cmd/${encodeURIComponent(name)}`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(args),
      signal: controller.signal,
      cache: 'no-store',
    });
    const text = await response.text();
    const data = text.length > 0 ? parseJsonOrKeep(text) : null;
    if (!response.ok) {
      throw normalizeError(data ?? { code: response.status >= 500 ? 'server_unavailable' : 'sync_failed' }, response.status);
    }
    return unwrap<T>(data);
  } catch (error) {
    if (error instanceof BridgeError) throw error;
    const aborted = error instanceof Error && error.name === 'AbortError';
    throw new BridgeError({
      code: 'transport_unreachable',
      messageKey: aborted ? 'link.timeout' : 'link.unreachable',
      retryable: true,
    });
  } finally {
    clearTimeout(timer);
  }
}

/** 所有命令的唯一出口。 */
export async function callCommand<T>(name: string, args: Record<string, unknown> = {}): Promise<T> {
  return inTauri() ? invokeTauri<T>(name, args) : invokeHttp<T>(name, args);
}

type EventHandler = (event: UiEvent) => void;

function looksLikeUiEvent(value: unknown): value is UiEvent {
  if (typeof value !== 'object' || value === null) return false;
  const kind = (value as { kind?: unknown }).kind;
  return kind === 'sync' || kind === 'notes-changed' || kind === 'conflict' || kind === 'toast';
}

function dispatchPayload(payload: unknown, handlers: Set<EventHandler>): void {
  let value: unknown = payload;
  if (typeof value === 'string') value = parseJsonOrKeep(value);
  const wrapped = value as { payload?: unknown; event?: { payload?: unknown } } | null;
  const candidates: unknown[] = [];
  if (wrapped && typeof wrapped === 'object') {
    if (wrapped.payload !== undefined) candidates.push(wrapped.payload);
    if (wrapped.event?.payload !== undefined) candidates.push(wrapped.event.payload);
  }
  candidates.push(value);
  for (const candidate of candidates) {
    if (looksLikeUiEvent(candidate)) {
      for (const handler of [...handlers]) handler(candidate);
      return;
    }
  }
}

/**
 * 订阅事件回流。返回取消订阅函数。
 * dev HTTP 通道下若 SSE 连不上，调用方可依赖 probeLink() 的可达性结论。
 */
export function onUiEvent(handler: EventHandler): () => void {
  const handlers = handlerRegistry;
  handlers.add(handler);
  ensureSource();
  return () => {
    handlers.delete(handler);
    if (handlers.size === 0) teardownSource();
  };
}

const handlerRegistry = new Set<EventHandler>();
let unsubscribeTauri: (() => void) | null = null;
let eventSource: EventSource | null = null;
let starting = false;

function emit(raw: unknown): void {
  dispatchPayload(raw, handlerRegistry);
}

function ensureSource(): void {
  if (!inTauri()) {
    if (eventSource !== null || starting) return;
    if (typeof EventSource !== 'function') return;
    starting = true;
    try {
      const source = new EventSource(`${devBase()}/events`, { withCredentials: false });
      source.onmessage = (message: MessageEvent<string>) => emit(message.data);
      source.onerror = () => {
        // 连接失败由 linkState 统一呈现"未连接到本地服务"，不在此重试风暴。
        linkListeners.forEach((listener) => listener('unreachable'));
      };
      eventSource = source;
    } catch {
      eventSource = null;
    } finally {
      starting = false;
    }
    return;
  }
  if (unsubscribeTauri !== null || starting) return;
  starting = true;
  void import('@tauri-apps/api/event')
    .then((api) => api.listen<unknown>('notera://event', (event) => emit(event.payload)))
    .then((stop) => {
      unsubscribeTauri = stop;
      starting = false;
    })
    .catch(() => {
      starting = false;
    });
}

const linkListeners = new Set<(state: LinkState) => void>();

export function onLinkChange(listener: (state: LinkState) => void): () => void {
  linkListeners.add(listener);
  return () => {
    linkListeners.delete(listener);
  };
}

function teardownSource(): void {
  if (eventSource !== null) {
    eventSource.close();
    eventSource = null;
  }
  if (unsubscribeTauri !== null) {
    unsubscribeTauri();
    unsubscribeTauri = null;
  }
}

/** 通道可达性探测：用契约内的 stats 命令，不额外发明接口。 */
export async function probeLink(): Promise<LinkState> {
  if (!inTauri()) {
    try {
      await invokeHttp('stats', {});
      return 'ready';
    } catch {
      return 'unreachable';
    }
  }
  try {
    await invokeTauri('stats', {});
    return 'ready';
  } catch (error) {
    return error instanceof BridgeError && error.isOffline ? 'unreachable' : 'ready';
  }
}
