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
  const drawerTarget = ref<'sidebar' | 'list' | null>(null);

  const layout = computed<LayoutMode>(() => layoutFor(width.value));
  const isCompact = computed(() => layout.value === 'one');
  const showsListAndEditorTogether = computed(() => layout.value !== 'one');
  const editorVisible = computed(() => layout.value === 'three' || layout.value === 'two' || mobilePane.value === 'editor');
  const listVisible = computed(() => layout.value !== 'one' || mobilePane.value === 'list');

  function goto(next: ViewName): void {
    view.value = next;
    drawerTarget.value = null;
  }

  function toggleSidebar(force?: boolean): void {
    sidebarOpen.value = force ?? !sidebarOpen.value;
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
    if (view.value !== 'workspace') {
      goto('workspace');
      return true;
    }
    if (drawerTarget.value !== null) {
      closeDrawer();
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
    const onResize = () => {
      width.value = window.innerWidth;
      height.value = window.innerHeight;
    };
    onResize();
    window.addEventListener('resize', onResize, { passive: true });
    let stopMotion: () => void = () => {};
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
      window.removeEventListener('resize', onResize);
      stopMotion();
    };
  }

  return {
    view,
    width,
    height,
    sidebarOpen,
    mobilePane,
    reducedMotion,
    drawerTarget,
    layout,
    isCompact,
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
