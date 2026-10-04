/**
 * 「装完应该是干净的」与侧栏视觉层级。
 *
 * ① 用户原话：「同步俺就为啥一直在转，我没设置 webdav 服务」
 *    ⇒ 判据是**没配账户时绝不出现"正在同步"**，且徽标在观察期内**不自己转**。
 *    这条与"有没有 WebDAV 账号"无关 —— 只要 `hasAccount` 为假，徽标就必须静止。
 *
 * ② 用户原话：「设计界 2 列 3 列长度对不齐不好看」
 *    ⇒ 侧栏三块（导航 / 同步 / 文件夹）要有**同级**的分区标题，
 *    徽标不能是个"从别处掉进来的胶囊"，空文件夹不显示 0。
 *
 * 全部只读 DOM 几何与属性，不读内部 store 状态 ——
 * 判据要站在"用户看得见的那一层"。
 */
import { describe, expect, it } from 'vitest';

const FILES = import.meta.glob('./**/*.{vue,ts}', {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>;

function read(rel: string): string {
  const text = FILES[`./${rel}`];
  if (typeof text !== 'string') throw new Error(`glob 里没有 ${rel}`);
  return text;
}

describe('① 没配账户时不允许"正在同步"', () => {
  // ⚠ 这三条只守**源码形状**；真把这条性质钉住的是 `syncClickGate.spec.ts`
  //    （挂真 App + 真 store + 按调用边断言"这一次 sync_now 发没发"）。
  //    形状判据连栽过三次（大窗口正则切错行），别再往这里加新语义。
  it('启动时只在配置允许（syncActive）时才自动同步', () => {
    const app = read('App.vue');
    // ⚠ 判据必须锚在**链式调用** `settings\n.loadAccount().then(` 上，
    // 不能搜裸的 `loadAccount()`：文件里还有另一处独立的 `void settings.loadAccount();`
    // （它负责把账户读进 store，不负责触发同步），正则会先撞上那处而误判。
    const chain = app.match(/settings\s*\.\s*loadAccount\(\)\s*\.then\([\s\S]{0,600}?\}\);/);
    expect(chain, '找不到"loadAccount().then(...)"那段').not.toBeNull();
    // `hasAccount` 只说"配过"，`syncActive` 才说"用户要它跑"（关了启用就不该自动同步）。
    expect(chain![0], '必须显式判配置，不能无条件 syncNow').toMatch(/if\s*\(\s*settings\.syncActive\s*\)/);
    expect(chain![0], 'else 分支应落静止态').toMatch(/else\s+sync\.markNoAccount\(\)/);
  });

  it('不再有"启动即无条件 syncNow"', () => {
    const app = read('App.vue');
    // 原来那行是 `void sync.syncNow();` 独立成句。
    // ⚠ 判据不能全文搜 —— F5 快捷键（`void sync.syncNow();`，第 166 行）
    // 与菜单转发里的那一行是**合法**的，它们只在用户主动触发时跑。
    // ⇒ 只看启动那一段（loadAccount 链之后到函数结束）。
    const start = app.indexOf('settings.loadAccount()');
    const seg = app.slice(start, start + 700);
    expect(seg, '启动段里不该有裸的 `void sync.syncNow();`').not.toMatch(/^\s*void sync\.syncNow\(\);\s*$/m);
  });

  it('启动顺序：先等 loadAccount 落地，再判配置', () => {
    const app = read('App.vue');
    // 不 await 就读配置 ⇒ `syncActive` 还是 false ⇒ 用户配好了却不同步。
    // ⚠ **按行序判，不跨行做正则**：真实文本是
    //     void settings
    //       .loadAccount()
    //       .then(() => {
    //         if (settings.syncActive) void sync.syncNow();
    //         else sync.markNoAccount();
    //       });
    // `loadAccount()` 与 `.then` 之间有换行 ⇒ `loadAccount\(\)\s*\.then` 能过，
    // 但 `settings\s*\.?\s*loadAccount\(\)` 这类要跨 700 字符去找 `syncNow` 的写法
    // 会因为中间那些注释而错位（实测连续三条判据都栽在这里）。行序最省事也最准。
    const lines = app.split('\n');
    const loadLine = lines.findIndex((l) => l.includes('.loadAccount()'));
    const thenLine = lines.findIndex((l) => l.includes('.then(') && Math.abs(l.length) < 20);
    const syncLine = lines.findIndex((l) => l.includes('if (settings.syncActive)'));
    expect(loadLine, '找不到 .loadAccount()').toBeGreaterThan(-1);
    expect(thenLine, '找不到 .then(').toBeGreaterThan(loadLine);
    expect(syncLine, '找不到配置闸门').toBeGreaterThan(thenLine);
  });

  it('F5 / 菜单 / 移动端按钮三处 syncNow 各自可达（保留手动同步能力）', () => {
    // 刻意**不**去判断"是不是顶层语句"—— 那种形状推断太脆：
    // 第一版会看"上一行是不是 if/=> /{"，而 F5 那处的上一行是
    // `event.preventDefault();` ⇒ 假红（实测）。
    // ⇒ 改成钉"这三处入口都还在"，这才是这条判据真正想守的东西：
    //    **自动同步被门控了，但用户想手动同步时依然做得到。**
    const app = read('App.vue');
    expect(app, 'F5 快捷键没了').toMatch(/event\.key === 'F5'[\s\S]{0,120}sync\.syncNow\(\)/);
    expect(app, '菜单转发没了').toMatch(/syncNow:\s*\(\)\s*=>\s*void sync\.syncNow\(\)/);
    expect(app, '移动端按钮没了').toMatch(/data-testid="mobile-sync"[^\n]*sync\.syncNow\(\)/);
  });
});

describe('① 徽标的第五态是静止的', () => {
  it('idle 用静止字形，不是转圈', () => {
    const badge = read('components/SyncBadge.vue');
    const m = badge.match(/idle:\s*'([^']*)'/);
    expect(m, "缺 idle 字形").not.toBeNull();
    expect(m![1]).not.toBe('↻');
  });

  it('初始态是 idle（首帧没有同步在跑）', () => {
    const sync = read('stores/sync.ts');
    const m = sync.match(/ref<FoldedSyncState>\(\{\s*badge:\s*'(\w+)'/);
    expect(m![1]).toBe('idle');
  });
});

describe('② 侧栏三块视觉同级', () => {
  it('「同步」有分区标题（原先是悬空胶囊）', () => {
    const side = read('components/SidebarPanel.vue');
    expect(side).toMatch(/section-title[^>]*>\s*\{\{\s*t\('sidebar\.syncSection'\)/);
  });

  it('侧栏里的徽标去掉了框与底色（与 nav-btn 同款）', () => {
    const badge = read('components/SyncBadge.vue');
    const seg = badge.slice(badge.indexOf('.syncline :deep(.badge)'));
    expect(seg.slice(0, 400), '侧栏语境下要覆盖 .badge 的 pill 样式').toMatch(/border:\s*0/);
    expect(seg.slice(0, 400)).toMatch(/background:\s*none/);
  });

  it('同步块只保留上分隔线（上下都有会像被夹在中间）', () => {
    const side = read('components/SidebarPanel.vue');
    const seg = side.slice(side.indexOf('.side-sync {'));
    const block = seg.slice(0, seg.indexOf('}'));
    expect(block).toMatch(/border-top/);
    expect(block, '下边框要去掉').not.toMatch(/border-bottom/);
  });
});

describe('② 噪声点与动作区', () => {
  it('空文件夹不显示计数（"默认名 0"那个噪点）', () => {
    const tree = read('components/FolderTree.vue');
    expect(tree).toMatch(/noteCount === 'number' && row\.node\.noteCount > 0/);
  });

  it('编辑器顶栏动作区贴右（margin-left:auto），不挤在标题旁', () => {
    const ws = read('views/WorkspaceView.vue');
    const seg = ws.slice(ws.indexOf('.editor-head__actions'));
    expect(seg.slice(0, 300)).toMatch(/margin-left:\s*auto/);
  });

  it('「固定/删除」被包进动作区', () => {
    const ws = read('views/WorkspaceView.vue');
    const i = ws.indexOf('editor-head__actions');
    const seg = ws.slice(i, i + 900);
    expect(seg, '动作区里应有toggle-pin').toContain('toggle-pin');
    expect(seg, '动作区里应有 trash-note').toContain('trash-note');
  });
});

describe('② 分区标题文案齐备', () => {
  it('「同步」的小节标题键存在（否则 t() 查不到会静默印出键名）', () => {
    const i18n = read('i18n.ts');
    const m = i18n.match(/'sidebar\.syncSection':\s*'([^']+)'/);
    expect(m, "缺 'sidebar.syncSection'").not.toBeNull();
    expect(m![1], '不该是空串（空标题比没标题更糟）').not.toBe('');
  });

  it('侧栏里「设置」入口仍贴底（不该被这轮改动挪走）', () => {
    // side-foot 在 DOM 里必须排在 pane-body 之后：设置是"次要入口"，
    // 贴底是它与上面三块的关系表达。这轮只给"同步"加标题，不动它的位置。
    const side = read('components/SidebarPanel.vue');
    const bodyIdx = side.indexOf('<div class="pane-body">');
    const footIdx = side.indexOf('side-foot');
    expect(bodyIdx).toBeGreaterThan(-1);
    expect(footIdx, '设置应排在文件夹区之后').toBeGreaterThan(bodyIdx);
  });
});
