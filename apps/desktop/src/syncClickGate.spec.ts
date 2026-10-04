/**
 * 「没配同步，点一下却开始转」的回归门禁。
 *
 * 为什么这份 spec 存在（真机读数，不是推测）：
 *   空库 + 无账户，点侧栏徽标之后连采 24 次（12 秒）——
 *   `syncing,syncing,…,syncing` 一个不落，`aria-busy=true`，控制台零 error。
 *   F5 同形。见 `.logs/repro-click-spin.mjs`。
 *
 * 根因是**一条没人守的调用边**，不是状态机少一个落点：
 *   ① `sync_now` 命令只做 `dirty_ticks += 1` 然后回 `null`（HTTP 200），
 *      **它从不回错** —— 所以 `stores/sync.ts` 里那条 `no_account` 分支是死代码；
 *   ② 调度器只在壳里「首帧之后 + 当时已配好账户」才 spawn 一次，
 *      没配账户时**没有任何消费者**，于是永远不会有一条事件来把徽标从 `syncing` 带走。
 *   ⇒ 徽标进了 `syncing` 就是**永久**的。
 *
 * 因此判据打在「这一次调用发没发」与「徽标进没进 syncing」两层，
 * 而不是去正则切源码形状（那种写法在 `syncIdleAndSidebar.spec.ts` 里连栽三次）。
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
import { createPinia, setActivePinia } from 'pinia';
import App from './App.vue';
import { stubLocalService, type LocalService } from './testing/http';
import { useSyncStore } from './stores/sync';

const ACCOUNT_CONFIGURED = {
  id: 'acct-1',
  label: '家里',
  baseUrl: 'https://dav.home.example/dav',
  rootPrefix: '/.notes',
  authKind: 'basic',
  username: 'me',
  tlsPolicy: 'strict',
  proxyMode: 'direct',
  bypass: [],
  enabled: true,
  hasCredential: true,
  credentialLive: true,
  credentialPersistent: true,
};

async function settle(times = 8): Promise<void> {
  for (let i = 0; i < times; i += 1) {
    await flushPromises();
  }
}

/** 空库、无账户 —— 与 `notera-cli serve` 的真读数一致：`account` 回 null，`sync_now` 回 null。 */
function makeService(account: unknown = null): LocalService {
  return stubLocalService({
    account: () => account,
    sync_now: () => null,
    stats: () => ({ notes: 0, folders: 1, attachments: 0, dbBytes: 1, notesInTrash: 0, inflightOps: 0, searchGeneration: 0, ftsRows: 0, userVersion: 1, tombstones: 0, tombstonesPurged: 0, dirtyNotes: 0, outboxPending: 0, conflictsOpen: 0 }),
    list_notes: () => ({ items: [], total: 0, truncated: false }),
    search: () => ({ items: [], total: 0, truncated: false, generation: 0 }),
    list_folders: () => [],
    list_trash: () => ({ items: [], total: 0, truncated: false }),
    sync_status: () => ({ phase: 'unconfigured', badge: 'offline', lastSuccessAt: null, pendingOps: 0, openConflicts: 0, messageKey: null, retryable: false }),
    list_conflicts: () => [],
    caps: () => ({}),
  });
}

const badgeKind = (wrapper: ReturnType<typeof mount>) =>
  wrapper.find('[data-testid="sync-badge"]').attributes('data-badge');

beforeEach(() => {
  setActivePinia(createPinia());
  vi.stubGlobal('EventSource', undefined);
  window.matchMedia = vi.fn().mockImplementation(() => ({
    matches: false,
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
  }));
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('没配同步时，点击不许假装在同步', () => {
  it('未配置账户：点徽标既不进 syncing，也不发 sync_now', async () => {
    const service = makeService(null);
    const wrapper = mount(App, { attachTo: document.body });
    await settle();
    expect(badgeKind(wrapper), '首帧该是静止的 idle').toBe('idle');

    await wrapper.find('[data-testid="sync-badge"]').trigger('click');
    await settle();

    expect(badgeKind(wrapper), '没配账户时点一下绝不能进"正在同步"').not.toBe('syncing');
    expect(badgeKind(wrapper)).toBe('idle');
    expect(service.callsOf('sync_now'), '根本不该发这一次同步请求').toHaveLength(0);
    expect(wrapper.text()).toContain('未配置同步');
    wrapper.unmount();
    document.body.innerHTML = '';
  });

  it('未配置账户：F5 同形（快捷键也是手动同步入口）', async () => {
    const service = makeService(null);
    const wrapper = mount(App, { attachTo: document.body });
    await settle();

    document.dispatchEvent(new KeyboardEvent('keydown', { key: 'F5', bubbles: true }));
    await settle();

    expect(badgeKind(wrapper)).not.toBe('syncing');
    expect(service.callsOf('sync_now'), 'F5 不该在没有账户时发同步请求').toHaveLength(0);
    wrapper.unmount();
    document.body.innerHTML = '';
  });

  it('未配置账户：点徽标把用户带到配置它的地方（设置页），而不是点了没反应', async () => {
    const wrapper = mount(App, { attachTo: document.body });
    await settle();
    expect(wrapper.find('[data-testid="account-save"]').exists()).toBe(false);

    await wrapper.find('[data-testid="sync-badge"]').trigger('click');
    await settle();

    expect(wrapper.find('[data-testid="account-save"]').exists(), '要点该有去处：同步在设置页配').toBe(true);
    wrapper.unmount();
    document.body.innerHTML = '';
  });

  it('配了但关了「启用同步」：静止，并且说的是"已关闭"而不是"未配置"', async () => {
    const service = makeService({ ...ACCOUNT_CONFIGURED, enabled: false });
    const wrapper = mount(App, { attachTo: document.body });
    await settle();

    await wrapper.find('[data-testid="sync-badge"]').trigger('click');
    await settle();

    expect(badgeKind(wrapper)).not.toBe('syncing');
    expect(service.callsOf('sync_now'), '用户明确关了同步，就不该有任何一轮').toHaveLength(0);
    expect(wrapper.text(), '关着却说"未配置"是另一句假话').toContain('同步已关闭');
    wrapper.unmount();
    document.body.innerHTML = '';
  });
});

describe('静止态要说清是哪一种静止（PROXY.md §7 那句"徽标停在需要凭据"）', () => {
  it('核心给了原因，那一句话必须**在屏幕上看得见**，不能只活在 aria-live 与 title 里', async () => {
    makeService(null);
    const wrapper = mount(App, { attachTo: document.body });
    await settle();

    useSyncStore().applySignal({ badge: 'offline', messageKey: 'sync.needsCredentials' });
    await settle();

    const detail = wrapper.find('[data-testid="sync-detail"]');
    expect(detail.exists(), '"为什么"那一格根本没渲染 ⇒ 用户只看见"离线"两个字').toBe(true);
    expect(detail.classes()).not.toContain('visually-hidden');
    expect(detail.text(), '要念得出的是核心给的那句具名原因').toContain('凭据');
    wrapper.unmount();
    document.body.innerHTML = '';
  });
});

describe('配好了同步，手动入口必须真的还能用', () => {
  it('已配置且启用：点徽标进 syncing 并发出 sync_now', async () => {
    const service = makeService(ACCOUNT_CONFIGURED);
    const wrapper = mount(App, { attachTo: document.body });
    await settle();
    // 配好了，启动那一轮本来就该发一次 —— 所以要量的是"这一次点击有没有加上去"，
    // 不是总数（钉总数会把"启动自动同步"这条合法调用也算成我的错）。
    const atBoot = service.callsOf('sync_now').length;
    expect(atBoot, '配好且开着：启动该自动同步一轮').toBe(1);

    await wrapper.find('[data-testid="sync-badge"]').trigger('click');
    await settle();

    expect(service.callsOf('sync_now'), '配好了就该真发这一次请求').toHaveLength(atBoot + 1);
    expect(badgeKind(wrapper)).toBe('syncing');
    expect(wrapper.find('[data-testid="account-save"]').exists(), '配好了就不该被踢到设置页').toBe(false);
    wrapper.unmount();
    document.body.innerHTML = '';
  });

  it('启动时的自动同步也只跟配置走：关了就不自动同步', async () => {
    const service = makeService({ ...ACCOUNT_CONFIGURED, enabled: false });
    const wrapper = mount(App, { attachTo: document.body });
    await settle();

    expect(badgeKind(wrapper)).toBe('idle');
    expect(service.callsOf('sync_now'), '关掉启用之后，启动不该自动同步').toHaveLength(0);
    wrapper.unmount();
    document.body.innerHTML = '';
  });
});
