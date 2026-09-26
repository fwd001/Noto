/**
 * 账户表单 ↔ 核心的 wire 形状（唯一的一份翻译）。
 *
 * 为什么单独一个文件：这条边此前有**三处**不一致，设置页的"保存"从来没成功过 ——
 *  1) 前端把草案包在 `{draft: …}` 里，核心的命令参数是**平铺**的；
 *  2) 前端发 `tlsPolicy: {kind:'caBundle'}` 对象，核心要的是 `"ca_bundle"` 这种蛇形字符串；
 *  3) 前端读回填时按 `account.proxy.host` 取嵌套字段，核心发的是 `proxyHost` 平铺字段
 *     —— 取不到就静默用默认值，用户改的 TLS/代理设置在下次打开时"自己变回去"。
 * 形状对不上不该由两侧各自 remember，所以集中在这里，并用契约测试钉住。
 */
import type { Account, AccountDraft, ProxyMode, TlsPolicyKind } from '../api/types';

/** 核心 `AccountDraftCmd` 的平铺线格式（camelCase）。 */
export interface AccountDraftWire {
  id: string;
  label: string;
  baseUrl: string;
  rootPrefix: string;
  authKind: string;
  username: string;
  password?: string;
  tlsPolicy: string;
  caPem?: string;
  proxyMode: string;
  proxyHost?: string;
  proxyPort?: number;
  proxyUsername?: string;
  bypass: string[];
  enabled: boolean;
}

/** 界面上的驼峰 → 核心认的蛇形（`commands.rs` 里 match 的是这些字面量）。 */
const TLS_TO_WIRE: Record<TlsPolicyKind, string> = {
  strict: 'strict',
  pin: 'pin',
  caBundle: 'ca_bundle',
  insecureLocal: 'insecure_local',
};
const TLS_FROM_WIRE: Record<string, TlsPolicyKind> = {
  strict: 'strict',
  pin: 'pin',
  ca_bundle: 'caBundle',
  caBundle: 'caBundle',
  insecure_local: 'insecureLocal',
  insecureLocal: 'insecureLocal',
};

/** 前缀按规范带前导斜杠、不带尾部斜杠：`/.notes`。少了斜杠会把库落到另一个目录上。 */
export function normalizeRootPrefix(prefix: string | null | undefined): string {
  const trimmed = (prefix ?? '').trim().replace(/^\/+/, '').replace(/\/+$/, '');
  return `/${trimmed || '.notes'}`;
}

/** 从地址取一个能认出来的名字当标签（核心要求 label 非空）。 */
export function labelFromBaseUrl(baseUrl: string): string {
  const host = /^https?:\/\/([^/:]+)/i.exec(baseUrl.trim())?.[1];
  return host ?? baseUrl.trim();
}

export function toWire(draft: AccountDraft): AccountDraftWire {
  const proxy = draft.proxy ?? {};
  const tls = draft.tlsPolicy ?? { kind: 'strict' };
  const mode: ProxyMode = proxy.mode ?? 'direct';
  const wire: AccountDraftWire = {
    id: draft.id ?? '',
    label: draft.label?.trim() || labelFromBaseUrl(draft.baseUrl),
    baseUrl: draft.baseUrl.trim(),
    rootPrefix: normalizeRootPrefix(draft.rootPrefix),
    authKind: 'basic',
    username: draft.username ?? '',
    tlsPolicy: TLS_TO_WIRE[tls.kind ?? 'strict'],
    proxyMode: mode,
    bypass: proxy.bypass ?? [],
    enabled: draft.enabled !== false,
  };
  // 空串按"不修改"处理：绝不用空口令把已存的凭据擦掉
  if (draft.password) wire.password = draft.password;
  if (tls.caBundlePem) wire.caPem = tls.caBundlePem;
  if (mode !== 'direct' && mode !== 'system') {
    if (proxy.host) wire.proxyHost = proxy.host;
    if (typeof proxy.port === 'number' && proxy.port > 0) wire.proxyPort = proxy.port;
    if (proxy.username) wire.proxyUsername = proxy.username;
  }
  return wire;
}

/** 回填表单。核心发的是平铺字段，这里不许再用嵌套取法"猜"。 */
export function draftFromWire(account: Account | null | undefined): AccountDraft {
  if (!account) {
    return { baseUrl: '', rootPrefix: '/.notes', username: '', password: '', tlsPolicy: { kind: 'strict' }, proxy: { mode: 'direct', bypass: [] }, enabled: true, label: '' };
  }
  const mode = (account.proxyMode as ProxyMode) ?? 'direct';
  return {
    id: account.id ?? null,
    label: account.label ?? '',
    baseUrl: account.baseUrl ?? '',
    rootPrefix: normalizeRootPrefix(account.rootPrefix),
    username: account.username ?? '',
    password: '',
    enabled: account.enabled !== false,
    tlsPolicy: { kind: TLS_FROM_WIRE[account.tlsPolicy ?? 'strict'] ?? 'strict', fingerprints: [] },
    proxy: {
      mode,
      host: account.proxyHost ?? '',
      port: typeof account.proxyPort === 'number' ? account.proxyPort : undefined,
      username: account.proxyUsername ?? '',
      bypass: account.bypass ?? [],
    },
  };
}
