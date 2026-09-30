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
