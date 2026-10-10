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

/**
 * 系统文件对话框（**只在壳里存在** —— dev/浏览器通道没有插件 IPC）。
 *
 * 与插件官方的 JS 包装同一形状（`invoke('plugin:dialog|save' | 'open', { options })`）：
 * 少一个依赖，命令名与参数就写在调用点上，缺哪条权限一眼能对到
 * `src-tauri/capabilities/default.json`（2026-10-10 G75 实测：没有 capability 时这些命令
 * 会被 ACL 拒，而错误以前被 catch 吞掉 ⇒ "点了没反应"）。
 *
 * 三种结果**分开说**：picked / cancelled / unavailable（带原文）——
 * 调用方不许把后两种揉成同一件事（取消是用户的决定，不可用是环境的事）。
 */
export type PathPick =
  | { kind: 'picked'; path: string }
  | { kind: 'cancelled' }
  | { kind: 'unavailable'; why: string };

/**
 * 门禁探针钩子（真壳 lane 专用）：`window.__NOTERA_DEBUG_EVENTS__` 存在时，把桥上的关键动作报给它。
 *
 * 为什么要有这个钩子：Tauri 把 `window.__TAURI_INTERNALS__` 锁成**不可写、不可配置**
 * （2026-10-10 实测 `writable:false / configurable:false`，Proxy 套不上）—— 门外挂 invoke spy
 * 是挂不上的（早先那条"点了没反应"的假读数就是这么来的：spy 静默没装上，而真想弹的
 * 原生对话框已经弹出来了）。所以改成**由桥自己报**。不设钩子时这是一个 `typeof` 判断，
 * 产品路径一个字不变。
 */
function debugEvent(kind: string, detail: Record<string, unknown>): void {
  const hook = (window as unknown as { __NOTERA_DEBUG_EVENTS__?: (k: string, d: unknown) => void })
    .__NOTERA_DEBUG_EVENTS__;
  if (typeof hook === 'function') hook(kind, detail);
}

async function invokeDialog(
  command: 'plugin:dialog|save' | 'plugin:dialog|open',
  options: Record<string, unknown>,
): Promise<PathPick> {
  if (!inTauri()) return { kind: 'unavailable', why: 'not-in-tauri' };
  debugEvent('dialog-pick', { command, options });
  try {
    const core = await import('@tauri-apps/api/core');
    const pending = core.invoke<string | string[] | null>(command, { options });
    void pending.then(
      () => debugEvent('dialog-settled', { command, outcome: 'done' }),
      (e: unknown) => debugEvent('dialog-settled', { command, outcome: `rejected: ${e instanceof Error ? e.message : String(e)}` }),
    );
    const picked = await pending;
    const path = Array.isArray(picked) ? picked[0] : picked;
    return typeof path === 'string' && path.trim() !== '' ? { kind: 'picked', path } : { kind: 'cancelled' };
  } catch (error) {
    return { kind: 'unavailable', why: error instanceof Error ? error.message : String(error) };
  }
}

/** 选一个「保存到」的路径（导出/备份）。过滤器文案由调用点给（那是界面层的事）。 */
export function pickSavePath(suggested: string, filters: Array<{ name: string; extensions: string[] }>): Promise<PathPick> {
  return invokeDialog('plugin:dialog|save', {
    ...(suggested.trim() !== '' ? { defaultPath: suggested.trim() } : {}),
    filters,
  });
}

/** 选一个要**打开**的文件（导入用；单文件，不选目录）。 */
export function pickOpenPath(filters: Array<{ name: string; extensions: string[] }>): Promise<PathPick> {
  return invokeDialog('plugin:dialog|open', { multiple: false, directory: false, filters });
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

/**
 * 订阅**原生菜单**的点击。只在 Tauri 壳里有意义（dev 浏览器通道没有原生菜单），
 * 因此这里不假装支持它：非壳环境下返回一个什么都不做的取消函数。
 */
export function onMenuAction(handler: (id: string) => void): () => void {
  let stop: (() => void) | null = null;
  let disposed = false;
  if (inTauri()) {
    void import('@tauri-apps/api/event')
      .then((api) => api.listen<string>('notera://menu', (event) => handler(event.payload)))
      .then((unregister) => {
        if (disposed) unregister();
        else stop = unregister;
      })
      .catch(() => {
        /* 订阅失败：菜单点了没反应，但绝不能因此把界面卡住 */
      });
  }
  return () => {
    disposed = true;
    stop?.();
  };
}let unsubscribeTauri: (() => void) | null = null;
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
