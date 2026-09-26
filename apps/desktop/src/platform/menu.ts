/**
 * 原生菜单 → 界面动作的路由表。
 *
 * 菜单项由壳（`src-tauri/src/platform.rs`）声明，动作却住在前端 —— 这正是最容易漂
 * 的那条边：壳加一项"最近删除"，忘了在这里挂号，用户点了什么也不会发生，而且
 * 两边各自看都是好的。所以 `menu.spec.ts` 直接去读那份 Rust 源码里的 id 列表，
 * 拿它当唯一事实源比对（TS 类型断言拦不住这种漂移）。
 */
export type MenuDeps = {
  newNote: () => void;
  focusSearch: () => void;
  syncNow: () => void;
  gotoConflicts: () => void;
  gotoTrash: () => void;
  gotoSettings: () => void;
};

/** `id` 不认识时返回 false：调用方要能区分"办了"和"没人接"。 */
export function dispatchMenu(id: string, deps: MenuDeps): boolean {
  const action = {
    'note.new': deps.newNote,
    'note.search': deps.focusSearch,
    'sync.now': deps.syncNow,
    'view.conflicts': deps.gotoConflicts,
    'view.trash': deps.gotoTrash,
    'view.settings': deps.gotoSettings,
  }[id];
  if (!action) return false;
  action();
  return true;
}
