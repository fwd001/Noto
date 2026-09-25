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
        if (hit.noteId && !next[hit.noteId]) {
          const derived = hit.snippetHtml ? stripTags(hit.snippetHtml).slice(0, 40) : '';
          next[hit.noteId] = derived;
        }
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

  async function create(folderId: string | null, doc?: NoteDoc): Promise<Note | null> {
    try {
      const note = await callCommand<Note>(Commands.createNote, { folderId, doc: doc ?? emptyDoc() });
      if (mode.value.kind === 'trash') await setMode({ kind: 'all' });
      else await load();
      selectedId.value = note.id;
      titles.value = { ...titles.value, [note.id]: note.title };
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
    } catch (error) {
      errorKey.value = asBridgeError(error).messageKey;
    }
  }

  async function moveTo(id: string, folderId: string | null): Promise<void> {
    try {
      const note = await callCommand<Note>(Commands.setNoteFolder, { id, folderId });
      applyNoteUpdate(note);
      await load();
    } catch (error) {
      errorKey.value = asBridgeError(error).messageKey;
    }
  }

  async function moveToTrash(id: string): Promise<void> {
    try {
      await callCommand<null>(Commands.deleteNote, { id });
      await load();
    } catch (error) {
      errorKey.value = asBridgeError(error).messageKey;
    }
  }

  async function restore(id: string): Promise<void> {
    try {
      await callCommand<null>(Commands.restoreNote, { id });
      await load();
    } catch (error) {
      errorKey.value = asBridgeError(error).messageKey;
    }
  }

  async function purge(id: string): Promise<void> {
    try {
      await callCommand<null>(Commands.purgeNote, { id });
      await load();
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

function stripTags(html: string): string {
  return html.replace(/<[^>]*>/g, '').replace(/&[a-z]+;/gi, ' ');
}
