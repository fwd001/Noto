/**
 * 外壳：视图切换、栏位布局（按视口宽度自动三栏/两栏/单栏）、抽屉与返回栈。
 * 平台差异不看 UA，只看 PlatformCaps（在 settings store 里）。
 */
import { defineStore } from 'pinia';
import { computed, ref } from 'vue';

export type ViewName = 'workspace' | 'settings' | 'conflicts';
export type LayoutMode = 'three' | 'two' | 'one';
export type MobilePane = 'list' | 'editor';

export const THREE_PANE_MIN = 1180;
export const TWO_PANE_MIN = 820;

export function layoutFor(width: number): LayoutMode {
  if (width >= THREE_PANE_MIN) return 'three';
  if (width >= TWO_PANE_MIN) return 'two';
  return 'one';
}

export const useShellStore = defineStore('shell', () => {
  const view = ref<ViewName>('workspace');
  const width = ref(typeof window === 'undefined' ? 1280 : window.innerWidth);
  const height = ref(typeof window === 'undefined' ? 800 : window.innerHeight);
  const sidebarOpen = ref(true);
  const mobilePane = ref<MobilePane>('list');
  const reducedMotion = ref(false);
  /** 键盘占掉的那一段高度（px）。0 = 键盘没弹起。由 `observeViewport()` 写。 */
  const keyboardInset = ref(0);
  const drawerTarget = ref<'sidebar' | 'list' | null>(null);
  /**
   * 「整机只读（库过新）」这一格（§4.2 第四行）。放在 shell 而不是 sync：它说的是**整个库**，
   * 与同步那一轮没关系 —— 以前它挂在 `sync.dbTooNew` 上，而那个位只由一个核心从未发过的
   * 事件写（缺口 G85），于是这条横幅永远出不来。
   */
  const libraryReadOnly = ref(false);

  const layout = computed<LayoutMode>(() => layoutFor(width.value));
  const isCompact = computed(() => layout.value === 'one');
  /**
   * 侧栏**此刻到底在不在屏幕上** —— 一个位，两种布局都说得准。
   *
   * 为什么要新加这一位：侧栏的可见性本来有两套互不相干的机制
   * （三栏看 `data-sidebar`，两栏/单栏看 `data-drawer`），而 `toggleSidebar()`
   * 只翻前者 ⇒ 在 820–1179 那一档点 ☰ 什么都不动（空控件），而那一档又没有任何
   * 别的入口（☰ 长在侧栏自己内部，抽屉收起来时它跟着藏了；列表栏那个 ☰ 又只在
   * `isCompact` 才出现）⇒ 文件夹 / 同步 / 设置 整档够不着。
   */
  const sidebarShown = computed(() =>
    layout.value === 'three' ? sidebarOpen.value : drawerTarget.value === 'sidebar',
  );
  /** 侧栏是不是**占着版面**（三栏且没收起）。false 时界面上必须有个 ☰ 在侧栏外面。 */
  const sidebarInline = computed(() => layout.value === 'three' && sidebarOpen.value);
  const showsListAndEditorTogether = computed(() => layout.value !== 'one');
  const editorVisible = computed(() => layout.value === 'three' || layout.value === 'two' || mobilePane.value === 'editor');
  const listVisible = computed(() => layout.value !== 'one' || mobilePane.value === 'list');

  function goto(next: ViewName): void {
    view.value = next;
    drawerTarget.value = null;
  }

  function toggleSidebar(force?: boolean): void {
    const next = force ?? !sidebarShown.value;
    if (layout.value === 'three') {
      sidebarOpen.value = next;
      return;
    }
    // 两栏/单栏：侧栏是抽屉，`data-sidebar` 在那两种布局里没有对应规则（CSS 里只有
    // `[data-layout='three'][data-sidebar='collapsed']` 那一条），要开合就得动抽屉位。
    drawerTarget.value = next ? 'sidebar' : null;
  }

  function openEditor(): void {
    mobilePane.value = 'editor';
  }

  function backToList(): void {
    mobilePane.value = 'list';
  }

  /**
   * 库内导航的统一落点：回工作区、收抽屉，并且窄屏要停在**列表那一面**。
   * 以前各处只调 `goto('workspace')`，窄屏下如果上一次看的是编辑器，人就被留在编辑器里 ——
   * 点「全部笔记」看着像没反应（§6 的"按钮失效"）。三处重复的写法收在这一个函数里。
   */
  function openList(): void {
    goto('workspace');
    backToList();
  }

  function openDrawer(target: 'sidebar' | 'list'): void {
    drawerTarget.value = target;
  }

  function closeDrawer(): void {
    drawerTarget.value = null;
  }

  /** 系统返回/退格语义：抽屉 → 编辑器回列表 → 其余交给壳层。 */
  function back(): boolean {
    // 抽屉必须排在「退页面」之前，否则注释与实现说的不是一回事：
    // 窄屏下在设置页把侧栏抽屉拉出来，那一次返回/Alt+←/Esc 会连带把设置页也退掉，
    // 看着像"按一下走了两格"。抽屉开着就先只关抽屉。
    if (drawerTarget.value !== null) {
      closeDrawer();
      return true;
    }
    if (view.value !== 'workspace') {
      goto('workspace');
      return true;
    }
    if (isCompact.value && mobilePane.value === 'editor') {
      backToList();
      return true;
    }
    return false;
  }

  function observeViewport(): () => void {
    if (typeof window === 'undefined' || typeof window.addEventListener !== 'function') return () => undefined;
    const root = typeof document === 'undefined' ? null : document.documentElement;
    /**
     * 版心的高度只认 `visualViewport`。
     *
     * iOS 的键盘**不改** `innerHeight`（那是布局视口），只改可视视口 —— 于是以前那套
     * `window.resize` + `height: 100%` 在手机上永远收不到"键盘占了 300 px"这件事：
     * 界面照旧占满整屏，光标那一行就在键盘底下。Android 会 resize，iOS 不会，
     * 而 `visualViewport` 两端都说得准 ⇒ 只从它取，`innerHeight` 当没有它时的退路。
     */
    const sync = () => {
      width.value = window.innerWidth;
      height.value = window.innerHeight;
      const vv = window.visualViewport;
      const layout = window.innerHeight;
      const visible = vv && Number.isFinite(vv.height) ? Math.min(vv.height, layout) : layout;
      const inset = vv && Number.isFinite(vv.height) ? layout - vv.height - (vv.offsetTop ?? 0) : 0;
      keyboardInset.value = Math.max(0, Math.round(inset));
      root?.style.setProperty('--app-vh', `${Math.round(visible)}px`);
      root?.style.setProperty('--app-kb', `${keyboardInset.value}px`);
    };
    sync();
    window.addEventListener('resize', sync, { passive: true });
    let stopMotion: () => void = () => {};
    let stopVv: () => void = () => {};
    const vv = window.visualViewport;
    if (vv && typeof vv.addEventListener === 'function') {
      // iOS 上键盘弹出常只发 scroll（或两个都发），两个都听才不漏
      vv.addEventListener('resize', sync);
      vv.addEventListener('scroll', sync);
      stopVv = () => {
        vv.removeEventListener('resize', sync);
        vv.removeEventListener('scroll', sync);
      };
    }
    if (typeof window.matchMedia === 'function') {
      const motion = window.matchMedia('(prefers-reduced-motion: reduce)');
      const apply = (matches: boolean) => {
        reducedMotion.value = matches === true;
      };
      apply(motion.matches);
      const onChange = (event: MediaQueryListEvent) => apply(event.matches);
      if (typeof motion.addEventListener === 'function') {
        motion.addEventListener('change', onChange);
        stopMotion = () => motion.removeEventListener('change', onChange);
      }
    }
    return () => {
      window.removeEventListener('resize', sync);
      stopVv();
      stopMotion();
    };
  }

  /** 只进不退：库过新这件事在一次运行里不会因为某条命令成功而消失（要消失得重启应用）。 */
  function markLibraryReadOnly(): void {
    libraryReadOnly.value = true;
  }

  return {
    view,
    width,
    height,
    sidebarOpen,
    mobilePane,
    reducedMotion,
    keyboardInset,
    drawerTarget,
    libraryReadOnly,
    markLibraryReadOnly,
    layout,
    isCompact,
    sidebarShown,
    sidebarInline,
    showsListAndEditorTogether,
    editorVisible,
    listVisible,
    goto,
    toggleSidebar,
    openEditor,
    backToList,
    openList,
    openDrawer,
    closeDrawer,
    back,
    observeViewport,
  };
});
