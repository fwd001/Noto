/** 列表：文件夹/全部/最近删除三种视图 + 即时搜索 + 选中态。 */
import { defineStore } from 'pinia';
import { computed, ref } from 'vue';
import { callCommand } from '../api/bridge';
import {
  Commands,
  type Note,
  type NoteListRow,
  type NoteDoc,
  type SearchHit,
} from '../api/types';
import { createDebounced, SEARCH_DEBOUNCE_MS, type Debounced } from '../util/timing';
import { asBridgeError } from '../util/errors';
import { emptyDoc } from '../editor/model';
import { useEditorStore } from './editor';
import { useSettingsStore } from './settings';
import { useToastStore } from './toasts';
import { t } from '../i18n';

export type ListMode = { kind: 'all' } | { kind: 'folder'; folderId: string | null } | { kind: 'trash' };

const PAGE_SIZE = 200;

function rowFromNote(note: Note): NoteListRow {
  return {
    id: note.id,
    title: note.title,
    summary: note.summary ?? '',
    folderId: note.folderId ?? null,
    pinned: note.pinned ?? false,
    charCount: note.charCount ?? 0,
    hasAttachment: note.hasAttachment ?? false,
    updatedAt: note.updatedAt,
    createdAt: note.createdAt,
    deletedAt: note.deletedAt ?? null,
    color: note.color ?? null,
  };
}

export const useNoteStore = defineStore('notes', () => {
  const rows = ref<NoteListRow[]>([]);
  const mode = ref<ListMode>({ kind: 'all' });
  const loading = ref(false);
  const errorKey = ref<string | null>(null);
  const hasMore = ref(false);
  const selectedId = ref<string | null>(null);
  const query = ref('');
  const hits = ref<SearchHit[] | null>(null);
  const searching = ref(false);
  const searchErrorKey = ref<string | null>(null);
  const titles = ref<Record<string, string>>({});

  const inTrash = computed(() => mode.value.kind === 'trash');
  const searching_ = computed(() => query.value.trim().length > 0);
  const visibleRows = computed(() => rows.value);
  const pinnedRows = computed(() => rows.value.filter((row) => row.pinned === true));
  const restRows = computed(() => rows.value.filter((row) => row.pinned !== true));
  const titleOf = computed(() => (id: string) => titles.value[id] ?? '');
  /**
   * §3.3 的分档读数：**逐条读核心给的 `exact`**，不按位置切。
   *
   * 核心内部确实是用位置切的（`search.rs` 里 `if i < exact_count`），界面再抄一遍
   * 就是第二份判据 —— 哪天两档交叉返回，抄位置的这份会安静地数错。
   */
  const searchTiers = computed<{ exact: number; fuzzy: number } | null>(() => {
    if (hits.value === null) return null;
    let exact = 0;
    for (const hit of hits.value) if (hit.exact === true) exact += 1;
    return { exact, fuzzy: hits.value.length - exact };
  });

  let requestId = 0;

  function listArgs(offset: number): Record<string, unknown> {
    const args: Record<string, unknown> = { limit: PAGE_SIZE, offset };
    if (mode.value.kind === 'trash') args.trash = true;
    else {
      args.trash = false;
      if (mode.value.kind === 'folder') args.folder = mode.value.folderId ?? null;
    }
    return args;
  }

  async function load(offset = 0, append = false): Promise<void> {
    const mine = ++requestId;
    loading.value = true;
    errorKey.value = null;
    try {
      const result = await callCommand<NoteListRow[]>(Commands.listNotes, listArgs(offset));
      if (mine !== requestId) return;
      const list = Array.isArray(result) ? result : [];
      rows.value = append ? [...rows.value, ...list] : list;
      hasMore.value = list.length === PAGE_SIZE;
      const next: Record<string, string> = { ...titles.value };
      for (const row of list) next[row.id] = row.title || '';
      titles.value = next;
      if (!append && rows.value.length > 0 && !rows.value.some((row) => row.id === selectedId.value)) {
        selectedId.value = rows.value[0]?.id ?? null;
      } else if (!append && rows.value.length === 0) {
        // **列表空了就必须把选中项也清掉**（原来漏了这一支）。
        //
        // 漏掉的后果，用户实测撞到了：「清除一切数据」之后侧栏显示 0 篇，
        // 而编辑器里还**留着刚被删掉那一篇的正文**—— 因为 selectedId 仍指向
        // 一个已经不存在的 id，编辑器照旧渲染它的内容。在回收站里那一篇
        // 还在正文之前挂着一句「这条在"最近删除"里，恢复后才能继续编辑。」，
        // 而它**已经被永久删除了**，那句话是假的。
        //
        // 顺带把标题缓存也丢掉：留着已删 id 的标题，只会让列表重挂载时
        // 凭空冒出一行"有标题但点不开"的条目。
        selectedId.value = null;
        titles.value = {};
      }
    } catch (error) {
      if (mine !== requestId) return;
      const bridge = asBridgeError(error);
      errorKey.value = bridge.messageKey;
      if (!append) rows.value = [];
      hasMore.value = false;
    } finally {
      if (mine === requestId) loading.value = false;
    }
  }

  async function loadMore(): Promise<void> {
    if (!hasMore.value || loading.value) return;
    await load(rows.value.length, true);
  }

  async function setMode(next: ListMode, preferredId: string | null = null): Promise<void> {
    mode.value = next;
    hits.value = null;
    searching.value = false;
    query.value = '';
    debouncedSearch.cancel();
    selectedId.value = preferredId;
    await load();
    if (preferredId === null && rows.value.length === 0) selectedId.value = null;
  }

  async function refresh(): Promise<void> {
    await load();
  }

  /* ------------------------------------------------------------- 搜索 */

  async function runSearch(text: string): Promise<void> {
    const trimmed = text.trim();
    if (trimmed.length === 0) {
      clearSearch();
      return;
    }
    const mine = ++requestId;
    searching.value = true;
    searchErrorKey.value = null;
    try {
      const result = await callCommand<SearchHit[]>(Commands.search, { text: trimmed.slice(0, 200), limit: 80 });
      if (mine !== requestId) return;
      const list = Array.isArray(result) ? result : [];
      hits.value = list;
      const next: Record<string, string> = { ...titles.value };
      for (const hit of list) {
        // 标题只认核心发的 `title`（缺口 G93）：以前这里拿片段前 40 字猜标题，而那一带
        // 是 DTO 还没有 `title` 键的年代留下的。露头的形状很日常 —— 在文件夹视图里搜、
        // 或库里 200 条开外：只要这篇不在已载入的列表里，屏幕上「标题」与「摘要」两行
        // 就是同一段正文。空标题的那篇也照核心给的走（界面上退「未命名」），不替它编一个。
        if (hit.noteId && !next[hit.noteId]) next[hit.noteId] = hit.title ?? '';
      }
      titles.value = next;
    } catch (error) {
      if (mine !== requestId) return;
      hits.value = [];
      searchErrorKey.value = asBridgeError(error).messageKey;
    } finally {
      if (mine === requestId) searching.value = false;
    }
  }

  const debouncedSearch: Debounced<[string]> = createDebounced((text: string) => void runSearch(text), SEARCH_DEBOUNCE_MS);

  function requestSearch(text: string): void {
    query.value = text;
    if (text.trim().length === 0) {
      debouncedSearch.cancel();
      clearSearch();
      return;
    }
    searching.value = true;
    debouncedSearch(text);
  }

  function clearSearch(): void {
    query.value = '';
    hits.value = null;
    searching.value = false;
    searchErrorKey.value = null;
  }

  /* ------------------------------------------------------------- 写入 */

  /**
   * §1 的"日记"入口：**一天一篇**，日子由核心按**本地日历日**算并找回/新建。
   * 前端不自己算日子、也不自己按标题找 —— 那就是第二份判据，核心那边一改这里就悄悄不一致。
   */
  async function openToday(): Promise<Note | null> {
    try {
      const today = await callCommand<{ note: Note; day: string; created: boolean }>(
        Commands.dailyNote,
        {},
      );
      // 「按了却看不见」不算做完：回收站视图、以及"日记落在默认本而当前看的是别的文件夹"，
      // 都要把视图换到「全部」并直接选上那一篇 —— 否则编辑器开了，列表里却没有它。
      const elsewhere =
        mode.value.kind === 'trash' ||
        (mode.value.kind === 'folder' && mode.value.folderId !== today.note.folderId);
      if (elsewhere) {
        await setMode({ kind: 'all' }, today.note.id);
      } else {
        await load();
        selectedId.value = today.note.id;
      }
      titles.value = { ...titles.value, [today.note.id]: today.note.title };
      // 与"新建笔记"同一件事：只选中不打开，用户看到的是一块空白面板（真窗口实测踩过）。
      await useEditorStore().open(today.note.id);
      return today.note;
    } catch (error) {
      errorKey.value = asBridgeError(error).messageKey;
      return null;
    }
  }

  /**
   * 笔记的**条数**变了之后刷一次侧栏计数。
   *
   * 为什么必须在这里刷：侧栏「全部笔记 / 最近删除」旁边那两颗数读的是
   * `settings.stats`，而 `settings.loadStats()` 此前**只在 App.vue 启动时调一次**
   * （实测 `loadStats` 的调用点只有 App.vue 与 SettingsView 那几处）。
   * 于是新建/删除之后，列表里条目已经变了而侧栏数字还是旧的 ——
   * 用户看着"我明明建了 3 篇，数字却没动"，那是在说界面在说谎。
   *
   * 用 `void` 不await：统计是旁路，刷新失败不该把创建/删除这一步变成失败。
   */
  function refreshCounts(): void {
    void useSettingsStore().loadStats();
  }

  async function create(folderId: string | null, doc?: NoteDoc): Promise<Note | null> {
    try {
      const note = await callCommand<Note>(Commands.createNote, { folderId, doc: doc ?? emptyDoc() });
      refreshCounts();
      if (mode.value.kind === 'trash') await setMode({ kind: 'all' });
      else await load();
      selectedId.value = note.id;
      titles.value = { ...titles.value, [note.id]: note.title };
      // 新建之后立刻打开编辑器：只"选中"不"打开"，用户看到的是一块空白面板，
      // 连字都打不进去（真浏览器实测踩过：列表里有了条目，中间却没有光标）。
      await useEditorStore().open(note.id);
      return note;
    } catch (error) {
      errorKey.value = asBridgeError(error).messageKey;
      return null;
    }
  }

  async function setPinned(id: string, pinned: boolean): Promise<void> {
    try {
      const note = await callCommand<Note>(Commands.setNotePinned, { id, pinned });
      applyNoteUpdate(note);
      handRevToEditor(note);
    } catch (error) {
      errorKey.value = asBridgeError(error).messageKey;
    }
  }

  async function moveTo(id: string, folderId: string | null): Promise<void> {
    try {
      const note = await callCommand<Note>(Commands.setNoteFolder, { id, folderId });
      applyNoteUpdate(note);
      handRevToEditor(note);
      await load();
    } catch (error) {
      errorKey.value = asBridgeError(error).messageKey;
    }
  }

  async function moveToTrash(id: string): Promise<void> {
    try {
      // 移走之前先把**这一篇**在飞/排队的保存结清（缺口 G41）。不结清的两种坏法都量到过：
      // 那支自动保存随后打在"已经在回收站里"的它身上 ⇒ 核心按 `constraint` 拒（`retryable=false`），
      // 用户那边是"字打了、屏幕上没落、也没有一句话说明去哪了"（lane 第 49 步 Del 那一判的读数）；
      // 或者那一支随这次切换被丢掉（`stores/trashFlush.spec.ts` 里打出来的就是这一种，edit_note 零条）。
      // 顺序摆正之后那几个字进的是"还在正常列表里"的那一篇。
      const editorStore = useEditorStore();
      if (editorStore.noteId === id) await editorStore.flush();
      await callCommand<null>(Commands.deleteNote, { id });
      refreshCounts();
      await load();
      // §4.9「每条 Toast 要说清没有丢什么」。这一行从列表里消失是**读模型**的行为（软删之后不再投影，
      // 缺口 G63 那一族），于是用户看到的只有"它没了"。恢复这条路是真的存在（`restore_note`），
      // 所以这句话不是安慰，是事实陈述。
      useToastStore().pushText(t('state.movedToTrash'), 'info');
    } catch (error) {
      errorKey.value = asBridgeError(error).messageKey;
    }
  }

  async function restore(id: string): Promise<void> {
    try {
      await callCommand<null>(Commands.restoreNote, { id });
      refreshCounts();
      await load();
      // 恢复之后必须把这一篇**重读**进编辑器（缺口 G44）。不重读的读数（2026-10-02 真机）：
      // 回收站 → 点开它 → 点「恢复」→ 直接打字 ⇒ 屏幕上没字、桥那边一支写都没收到，
      // 也没有任何一句话说明为什么改不动 —— 因为编辑器的 `inTrash` 与 `rev` 是打开那一篇时
      // 从"还在回收站里"的状态 hydrate 来的，而 delete/restore 各自又推进了那一行的 rev。
      const editorStore = useEditorStore();
      if (editorStore.noteId === id) await editorStore.open(id, true);
    } catch (error) {
      errorKey.value = asBridgeError(error).messageKey;
    }
  }

  async function purge(id: string): Promise<void> {
    try {
      await callCommand<null>(Commands.purgeNote, { id });
      refreshCounts();
      await load();
      // 永久删除掉的那一篇如果正开在编辑器里，必须关掉（缺口 G45）。2026-10-02 真机读数：
      // 列表那边是对的（空态出现），但**编辑器还显示着那一篇的正文**，下面还挂着
      // 「这条在"最近删除"里，恢复后才能继续编辑。」—— 对一篇已经被永久删除的笔记，这句话是假话；
      // 往那块只读区打字，屏幕上连字都不出现，也没有任何一句话说明发生了什么。
      // （台账里我原先写"purge 靠选中项搬迁会自动关掉编辑器"，那是推的，量下来是反的。）
      const editorStore = useEditorStore();
      if (editorStore.noteId === id) await editorStore.open(null);
    } catch (error) {
      errorKey.value = asBridgeError(error).messageKey;
    }
  }

  function applyNoteUpdate(note: Note | null | undefined): void {
    if (!note || typeof note.id !== 'string') return;
    const existing = rows.value.find((row) => row.id === note.id);
    const row = rowFromNote(note);
    rows.value = existing ? rows.value.map((item) => (item.id === note.id ? { ...row, folderName: item.folderName ?? row.folderName } : item)) : [row, ...rows.value];
    titles.value = { ...titles.value, [note.id]: note.title };
  }

  /**
   * 把"同一行刚被推进的那格 rev"交给编辑器。
   *
   * 置顶与移到文件夹在核心里走的是同一条 `commit_edit`（pinned/folder 要能同步出去，就必须占一格 rev），
   * 而编辑器的 `rev` 平时只跟着自己的 `edit_note` 回包前进。不交接的实测形状（缺口 G43，2026-10-02 真机）：
   * 三次置顶把那一行推到 5，下一支自动保存仍报 `expectedRev=2` ⇒ 核心按 `stale_edit` 拒，
   * 用户刚打的字被回读换掉、哪儿也没落，屏幕上却写着"这条笔记在别处被改动了"。
   *
   * 只交给本地已成功的元数据写；远端同步下来的改动仍走编辑器的草稿冲突那一条路（`enterStale`），
   * 所以这里**不**放在 `applyNoteUpdate` 里。
   */
  function handRevToEditor(note: Note | null | undefined): void {
    if (!note || typeof note.id !== 'string' || typeof note.rev !== 'number') return;
    useEditorStore().adoptRev(note.id, note.rev);
  }

  function rowById(id: string | null | undefined): NoteListRow | undefined {
    if (!id) return undefined;
    return rows.value.find((row) => row.id === id);
  }

  /** 事件回流：后端说这些条目变了。若正是编辑器打开的那条，交由编辑器决定何时重载。 */
  async function invalidate(ids: readonly string[]): Promise<string | null> {
    await load();
    return ids.includes(selectedId.value ?? '') ? selectedId.value : null;
  }

  return {
    rows,
    visibleRows,
    pinnedRows,
    restRows,
    mode,
    inTrash,
    hasQuery: searching_,
    loading,
    errorKey,
    hasMore,
    selectedId,
    query,
    hits,
    searchTiers,
    searching,
    searchErrorKey,
    titles,
    titleOf,
    load,
    loadMore,
    setMode,
    refresh,
    runSearch,
    requestSearch,
    clearSearch,
    create,
    openToday,
    setPinned,
    moveTo,
    moveToTrash,
    restore,
    purge,
    applyNoteUpdate,
    rowById,
    invalidate,
  };
});
