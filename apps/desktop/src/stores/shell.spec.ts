/**
 * 导航落点。这几条盯的是"点了没反应"那一类：库内导航必须真的把人带回列表那一面，
 * 而不是只改一个 view 值 —— 窄屏（手机 / 半屏窗口）上一次停在编辑器，
 * 光 `goto('workspace')` 就会把人留在编辑器里，看着像按钮死了（缺口 G27）。
 */
import { beforeEach, describe, expect, it } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { useShellStore } from './shell';

beforeEach(() => {
  setActivePinia(createPinia());
});

describe('窄屏（390）', () => {
  it('停在编辑器那一面时，goto(workspace) 并不会把列表带回来 —— 所以导航要走 openList', () => {
    const shell = useShellStore();
    shell.width = 390;
    shell.goto('workspace');
    shell.openEditor();
    expect(shell.isCompact).toBe(true);
    expect(shell.listVisible).toBe(false);

    shell.goto('settings');
    shell.goto('workspace');
    expect(shell.listVisible).toBe(false); // 旧写法的全部作为：人还在编辑器那一面

    shell.openList();
    expect(shell.view).toBe('workspace');
    expect(shell.listVisible).toBe(true);
  });

  it('openList 会顺手收掉抽屉', () => {
    const shell = useShellStore();
    shell.width = 390;
    shell.openDrawer('sidebar');
    expect(shell.drawerTarget).toBe('sidebar');
    shell.openList();
    expect(shell.drawerTarget).toBe(null);
  });
});

describe('宽屏（1440）', () => {
  it('openList 不改变可见性：列表与编辑器本来就在一起', () => {
    const shell = useShellStore();
    shell.width = 1440;
    shell.openEditor();
    shell.openList();
    expect(shell.view).toBe('workspace');
    expect(shell.showsListAndEditorTogether).toBe(true);
    expect(shell.listVisible).toBe(true);
    expect(shell.editorVisible).toBe(true);
  });
});

/**
 * 退路的层级（Esc 与 Alt+← 共用的那条 `back()`）。
 * 修的是"设置页按 Esc 没反应"：返回键 `‹` 与 Alt+← 都在，唯独Esc 那一支只认抽屉。
 * 抽屉优先于退页面 —— 否则在抽屉里按一次Esc 会连带退回工作区，看着像"按一下走了两格"。
 */
describe('back() 的层级', () => {
  it('设置页/ 冲突页：back() 回到工作区并吃掉这次按键', () => {
    const shell = useShellStore();
    shell.width = 1440;
    for (const v of ['settings', 'conflicts'] as const) {
      shell.goto(v);
      expect(shell.back()).toBe(true);
      expect(shell.view).toBe('workspace');
    }
  });

  it('抽屉开着时 back() 只关抽屉，不带着页面一起退（Esc 的层级）', () => {
    const shell = useShellStore();
    // 抽屉只在窄屏下真的存在：宽屏侧栏常驻，`openDrawer` 会被 `drawerTarget` 直接清空
    // （实测 1440 下设上立刻变 null）。所以这条必须用 390，否则断言的是不存在的那条分支。
    shell.width = 390;
    // 先落到设置页再开抽屉：`goto('settings')` 自己会把 drawerTarget 清掉，
    // 而窄屏下 `back()` 还有一条"退到列表"的分支会先把 view 拽回 workspace ——
    // 那正是 G27 那条既有语义，不在这里跟抽屉层级搅在一起（下面窄屏那条单独盯它）。
    shell.goto('settings');
    shell.openDrawer('sidebar');
    expect(shell.drawerTarget).toBe('sidebar');
    expect(shell.back()).toBe(true);
    // 这一格是本条的全部意义：抽屉关掉了，页面**没有**被带走
    expect(shell.drawerTarget).toBe(null);
    expect(shell.view).toBe('settings');
  });

  it('已经在工作区且没有抽屉：返回 false，不抢占按键', () => {
    const shell = useShellStore();
    shell.width = 1440;
    expect(shell.back()).toBe(false);
    expect(shell.view).toBe('workspace');
  });

  it('窄屏停在编辑器：back() 回列表而不是把抽屉当成退路', () => {
    const shell = useShellStore();
    shell.width = 390;
    shell.openEditor();
    expect(shell.back()).toBe(true);
    expect(shell.mobilePane).toBe('list');
  });
});
