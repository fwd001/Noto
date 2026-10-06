/**
 * 这次用户反馈的三个界面缺陷的回归门禁。
 *
 * 三条都来自真机截图，不是猜的：
 *  ① 列表行内容被下一行标题压住（时间那行溢出到版心之外）；
 *  ② 侧边栏收起后没法再打开（折叠按钮长在侧栏内部，被自己一起藏了）；
 *  ③ "正在同步"一直转 —— 那是"没配账户"，不是"在忙"。
 *
 * 为什么要有这三条：那三处都不是"编译能发现"的东西。
 * ① 靠一个数字（行高）同时承担"渲染高度"和"虚拟滚动偏移"，改小一点就溢出；
 * ② 结构上按钮在正确的组件里，只是那个组件被 v-show 藏了；
 * ③ 状态机少一个落点，多出来的路径全都停在最不误导的那一态之前。
 */
import { describe, expect, it } from 'vitest';

/**
 * 这份 spec 只读源码文本，不 import 被测模块。
 *
 * 为什么用 `import.meta.glob` 而不是 `node:fs`：
 * 本项目**没装 `@types/node`**（`vue-tsc` 会报 TS2591「Cannot find name 'node:fs'」，
 * tsconfig 的 types 里也没有 node）。而 `import.meta.glob` 是 Vite 编译期提供的，
 * 类型与运行时都在，且顺带把这几个文件打进依赖图 —— 源码改了这条会跟着重跑。
 *
 * `eager: false` = 只要路径不要内容，用 `read()` 自己去拿；
 * `query: '?raw'` = 按文本读，不经过模块解析（.vue 不是 JS 模块）。
 */
// 这份 spec 放在 `src/` 下，而要读的文件在 `src/components`、`src/stores`、`src/api`、
// `src/platform` 等**子目录**里 —— 所以 glob 用 `**` 而不是 `*`（`*` 只匹配同级，
// 会导致 glob 里一个键都没有，`read()` 直接抛）。
//
// `eager: true` 是必须的：非 eager 时 `FILES[key]` 是个**返回 Promise 的 loader**，
// 直接当字符串用会得到 `src.match is not a function`（实测踩过）。
// 代价是把这些源文件都读进内存 —— 几百 KB，对测试来说可以接受。
const FILES = import.meta.glob<string>('./**/*.{vue,ts}', {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>;

function read(rel: string): string {
  // 传进来的就是相对 `src/` 的路径（如 'components/NoteList.vue'）
  const key = `./${rel.replace(/^\.\//, '')}`;
  const text = FILES[key];
  if (typeof text !== 'string') {
    // 键名写错时要**响**，不能返回空串让断言"因为读不到所以过了"。
    throw new Error(`glob 里没有 ${key}；现有键：${Object.keys(FILES).slice(0, 8).join(', ')}…`);
  }
  return text;
}

describe('列表行高与溢出（用户截图：内容被下一行压住）', () => {
  it('行高常量必须容得下 标题 + 摘要 + 时间 三行', () => {
    const src = read('components/NoteList.vue');
    const m = src.match(/const ROW_HEIGHT = (\d+)/);
    expect(m, '必须显式声明 ROW_HEIGHT —— 虚拟滚动按它算偏移').not.toBeNull();
    const rowHeight = Number(m![1]);
    // 三行的实际高度：标题 text-base×1.35 + 摘要 text-sm×1.4 + 时间 text-xs×1.4
    // + gap(var(--sp-1)=4px，两处) + 上下 padding(var(--sp-2)=8px，两处)。
    // 实测装不下 76px（那正是被截图拍到的值），92px 有余量。
    // 若以后调了字号/间距，这条会先红 —— 那正是要人重新算一遍的时机。
    expect(rowHeight).toBeGreaterThanOrEqual(92);
  });

  it('标题与摘要都要有行高封顶，否则溢出压到相邻行', () => {
    const src = read('components/NoteList.vue');
    // `text-overflow: ellipsis` 只管"单行太长"，父级固定高度时内容仍会从上下溢出。
    // 必须同时有 `max-height`（或 `line-height` 封顶）才关得住。
    for (const sel of ['.row-item__title', '.row-item__summary']) {
      const block = src.slice(src.indexOf(sel));
      const body = block.slice(0, block.indexOf('}'));
      expect(body, `${sel} 缺 max-height`).toMatch(/max-height/);
    }
  });

  it('行高是唯一的：虚拟滚动的偏移与渲染必须用同一个数', () => {
    const src = read('components/NoteList.vue');
    // 出现过第二个字面行高就说明有人硬编码了偏移，那会让"渲染高度 ≠ 计算高度"。
    const literals = src.match(/height:\s*`\$\{[^}]+\}px`|scrollHeight \/ (\d+)/g) ?? [];
    expect(literals.every((l) => l.includes('${')), `发现硬编码的行高：${literals.join(' / ')}`).toBe(true);
  });
});

describe('侧边栏收起后必须还能打开，且全屏幕只有一颗把手', () => {
  it('主区那颗展开入口存在，且让位给自绘标题栏那一颗（不同时出现）', () => {
    const ws = read('views/WorkspaceView.vue');
    // 折叠按钮原本长在 SidebarPanel 里（sidebar-collapse），侧栏一收它自己就被藏了。
    // 展开入口必须由主区持有 —— 它不随侧栏生死；但那一行自绘时它必须不出，
    // 否则就是用户说的「折起之后下面一层还有一个折起」。
    expect(ws).toContain('data-testid="open-sidebar"');
    expect(ws).toMatch(/!shell\.sidebarOpen && !drawsTitleBar\(settings\.caps\)/);
  });

  it('列表头不许再有第三颗把手（回归：那颗和标题栏那颗叠着出现过）', () => {
    const list = read('components/NoteList.vue');
    expect(list, 'NoteList 里又出现了侧栏把手').not.toMatch(/toggleSidebar/);
  });

  it('标题栏那颗把手在，且它画不画由 caps 决定（不是按机型）', () => {
    const bar = read('components/TitleBar.vue');
    expect(bar).toContain('data-testid="sidebar-handle"');
    expect(bar).toMatch(/v-if="visible"/);
    const caps = read('platform/caps.ts');
    expect(caps).toMatch(/export function drawsTitleBar\(caps: PlatformCaps\): boolean/);
    expect(caps).toMatch(/windowChrome !== 'system'/);
  });

  it('展开按钮绝对定位，不占 flex 位（否则松手时列表宽度会跳）', () => {
    const ws = read('views/WorkspaceView.vue');
    const block = ws.slice(ws.indexOf('.workspace__reopen'));
    expect(block.slice(0, 400)).toContain('position: absolute');
  });

  it('展开入口只在宽屏出现 —— 窄屏已有抽屉按钮，不重复', () => {
    const ws = read('views/WorkspaceView.vue');
    expect(ws).toMatch(/v-if="!shell\.isCompact && !shell\.sidebarOpen && !drawsTitleBar\(settings\.caps\)"/);
  });
});

describe('未配置同步不该显示成"正在同步"（用户截图：一直转）', () => {
  it('第五态 idle 存在，且有静止的字形', () => {
    const types = read('api/types.ts');
    expect(types).toMatch(/SyncBadgeKind =[^;]*'idle'/);
    // §2.3 的五格 → 图标那张表在 `ui/icons.ts`（状态条与移动端底栏共用一份，两处各写一遍迟早分叉）。
    const icons = read('components/ui/icons.ts');
    const at = icons.indexOf('SYNC_ICONS');
    const block = icons.slice(at, at + 500);
    // 五格里只有"正在同步"会动 ⇒ idle 不许指到 syncing 那枚云
    expect(at, '找不到 SYNC_ICONS 那张表').toBeGreaterThan(-1);
    expect(block).toMatch(/idle:\s*'sync-idle'/);
    expect(block).not.toMatch(/idle:\s*'sync-syncing'/);
  });

  it('idle 有文案，且不是"正在同步"', () => {
    const i18n = read('i18n.ts');
    const m = i18n.match(/'sync\.idle':\s*'([^']+)'/);
    expect(m, "缺 'sync.idle' 文案键 —— 查不到键会静默印出键名").not.toBeNull();
    expect(m![1]).not.toContain('正在同步');
  });

  it('初始态是 idle 而不是 syncing（首帧并没有同步在跑）', () => {
    const sync = read('stores/sync.ts');
    const m = sync.match(/ref<FoldedSyncState>\(\{\s*badge:\s*'(\w+)'/);
    expect(m).not.toBeNull();
    expect(m![1]).toBe('idle');
  });

  it('no_account 要折到 idle，而不是留在 syncing', () => {
    const sync = read('stores/sync.ts');
    expect(sync).toMatch(/code === 'no_account'[\s\S]{0,120}markNoAccount\(\)/);
  });
});
