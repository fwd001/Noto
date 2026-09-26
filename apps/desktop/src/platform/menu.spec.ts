/**
 * 原生菜单 id → 界面动作。
 *
 * id 集合的一致性由 Rust 侧守：`crates/notera-host/src/platform.rs` 里有一条测试
 * 直接读本文件的 `probes` 键集合并与 `menu_plan()` 的 id 比对 —— 壳加了一项而前端
 * 忘了挂号（点了没反应），或前端多挂一项（永远不会被触发），两边都会红。
 */
import { describe, expect, it, vi } from 'vitest';
import { dispatchMenu, type MenuDeps } from './menu';

function deps(): MenuDeps {
  return {
    newNote: vi.fn(),
    focusSearch: vi.fn(),
    syncNow: vi.fn(),
    gotoConflicts: vi.fn(),
    gotoTrash: vi.fn(),
    gotoSettings: vi.fn(),
  };
}

describe('原生菜单路由', () => {
  it('每一个 id 都恰好触发它那一个动作', () => {
    const probes: Record<string, keyof MenuDeps> = {
      'note.new': 'newNote',
      'note.search': 'focusSearch',
      'sync.now': 'syncNow',
      'view.conflicts': 'gotoConflicts',
      'view.trash': 'gotoTrash',
      'view.settings': 'gotoSettings',
    };
    expect(Object.keys(probes)).toHaveLength(6);
    for (const [id, key] of Object.entries(probes)) {
      const d = deps();
      expect(dispatchMenu(id, d), id).toBe(true);
      expect(d[key]).toHaveBeenCalledTimes(1);
      for (const [other, otherKey] of Object.entries(probes)) {
        if (other !== id) expect(d[otherKey], `${id} 顺带触发了 ${other}`).not.toHaveBeenCalled();
      }
    }
  });

  it('不认识的 id 明说"没人接"，而不是静默成功', () => {
    expect(dispatchMenu('nope.nothing', deps())).toBe(false);
  });
});
