/**
 * 账户这条边的契约测试。它存在的理由是真实事故：设置页的"保存"从来没成功过，
 * 因为前端发的是 `{draft:{…}}` 而核心要平铺；回填按嵌套字段读而核心发的是平铺字段。
 * 这里钉的是**线格式本身**，改任何一侧都会在这里爆炸，而不是等到界面上静默失效。
 */
import { describe, expect, it } from 'vitest';
import { draftFromWire, labelFromBaseUrl, normalizeRootPrefix, toWire } from './accountWire';
import type { Account, AccountDraft } from '../api/types';

function draft(over: Partial<AccountDraft> = {}): AccountDraft {
  return {
    baseUrl: 'https://dav.example.com/dav',
    rootPrefix: '/.notes',
    username: 'notera',
    password: '',
    tlsPolicy: { kind: 'strict' },
    proxy: { mode: 'direct', bypass: [] },
    enabled: true,
    label: '',
    ...over,
  };
}

describe('toWire', () => {
  it('发出去的是平铺参数，不是包了一层 draft', () => {
    const wire = toWire(draft()) as unknown as Record<string, unknown>;
    expect(wire.draft).toBeUndefined();
    expect(Object.keys(wire).sort()).toEqual(
      ['authKind', 'baseUrl', 'bypass', 'enabled', 'id', 'label', 'proxyMode', 'rootPrefix', 'tlsPolicy', 'username'].slice().sort(),
    );
  });

  it('TLS 用核心的蛇形字面量，不是界面上的驼峰', () => {
    expect(toWire(draft({ tlsPolicy: { kind: 'caBundle' } })).tlsPolicy).toBe('ca_bundle');
    expect(toWire(draft({ tlsPolicy: { kind: 'insecureLocal' } })).tlsPolicy).toBe('insecure_local');
    expect(toWire(draft({ tlsPolicy: { kind: 'pin' } })).tlsPolicy).toBe('pin');
  });

  it('空口令不许把已存的凭据擦掉', () => {
    expect('password' in toWire(draft({ password: '' }))).toBe(false);
    expect(toWire(draft({ password: 'hunter2' })).password).toBe('hunter2');
  });

  it('直连/系统代理不发 host/port，SOCKS 才发', () => {
    expect(toWire(draft({ proxy: { mode: 'socks5', host: '127.0.0.1', port: 7890, bypass: [] } })).proxyHost).toBe('127.0.0.1');
    expect('proxyPort' in toWire(draft({ proxy: { mode: 'direct', host: '127.0.0.1', port: 7890, bypass: [] } }))).toBe(false);
  });

  it('缺 label 时用地址里的 host 顶上（核心要求非空）', () => {
    expect(toWire(draft({ baseUrl: 'https://dav.home.example:5006/dav' })).label).toBe('dav.home.example');
    expect(toWire(draft({ label: '家里那台' })).label).toBe('家里那台');
  });
});

describe('draftFromWire', () => {
  const stored: Account = {
    id: '0192e6c1-0000-7000-8000-000000000001',
    label: '内网',
    baseUrl: 'https://dav.example.com/dav',
    rootPrefix: '/.notes',
    username: 'notera',
    authKind: 'basic',
    tlsPolicy: 'insecure_local',
    proxyMode: 'http',
    proxyHost: '10.0.0.5',
    proxyPort: 3128,
    bypass: ['127.0.0.1'],
    enabled: true,
    hasCredential: true,
  };

  it('平铺字段要能读回来：改过的 TLS 与代理不能一刷新就变回默认', () => {
    const back = draftFromWire(stored);
    expect(back.tlsPolicy.kind).toBe('insecureLocal');
    expect(back.proxy.mode).toBe('http');
    expect(back.proxy.host).toBe('10.0.0.5');
    expect(back.proxy.port).toBe(3128);
    expect(back.username).toBe('notera');
    expect(back.proxy.bypass).toEqual(['127.0.0.1']);
  });

  it('口令永远不从后端回来，回填后是空串（表示"不改"）', () => {
    expect(draftFromWire(stored).password).toBe('');
  });

  it('发出去再读回来，关键设置不变形', () => {
    const original = draft({ tlsPolicy: { kind: 'pin' }, proxy: { mode: 'socks5', host: 'h', port: 1080, bypass: ['a'] } });
    const round = draftFromWire({ ...toWire(original) } as unknown as Account);
    expect(round.tlsPolicy.kind).toBe('pin');
    expect(round.proxy.mode).toBe('socks5');
    expect(round.proxy.host).toBe('h');
    expect(round.rootPrefix).toBe('/.notes');
  });

  it('没有账户时是一份可直接填的空草案，而不是 undefined 满天飞', () => {
    const empty = draftFromWire(null);
    expect(empty.baseUrl).toBe('');
    expect(empty.rootPrefix).toBe('/.notes');
    expect(empty.tlsPolicy.kind).toBe('strict');
    expect(empty.proxy.mode).toBe('direct');
  });
});

describe('rootPrefix 与 label', () => {
  it('前缀一律补上前导斜杠：.notes 与 /.notes 落到同一个库', () => {
    expect(normalizeRootPrefix('.notes')).toBe('/.notes');
    expect(normalizeRootPrefix('/.notes')).toBe('/.notes');
    expect(normalizeRootPrefix('')).toBe('/.notes');
    expect(normalizeRootPrefix(undefined)).toBe('/.notes');
    expect(normalizeRootPrefix('/weird//')).toBe('/weird');
  });

  it('label 认不出 host 时退回原串，不发空标签', () => {
    expect(labelFromBaseUrl('not-a-url')).toBe('not-a-url');
    expect(labelFromBaseUrl('  ')).toBe('');
  });
});
