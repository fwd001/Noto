/**
 * 编辑器状态：Document ↔ 块模型、自动保存（乐观并发）、分歧保护。
 * 铁律：保存失败为 stale_edit 时绝不静默覆盖 —— 本机版本完整留在 localDraft，
 * 界面切到对方版本并给出可操作的取舍入口。
 */
import { defineStore } from 'pinia';
import { computed, ref } from 'vue';
import { callCommand } from '../api/bridge';
import { Commands, type Attachment, type Note, type NoteDoc } from '../api/types';
import { messageFor, t } from '../i18n';
import { asBridgeError } from '../util/errors';
import { AUTOSAVE_DEBOUNCE_MS, createDebounced } from '../util/timing';
import {
  blocksToDoc,
  docToBlocks,
  emptyParagraph,
  newBlockId,
  SUPPORTED_DOC_VERSION,
  textBlock,
  type EditorBlock,
} from '../editor/model';
import { useNoteStore } from './notes';
import { useSyncStore } from './sync';
import { useToastStore } from './toasts';

export type SaveState = 'idle' | 'pending' | 'saving' | 'saved' | 'error';

export interface EditorReadiness {
  loading: boolean;
  empty: boolean;
}

export const useEditorStore = defineStore('editor', () => {
  const notes = useNoteStore();
  const sync = useSyncStore();
  const toasts = useToastStore();

  const noteId = ref<string | null>(null);
  const blocks = ref<EditorBlock[]>([]);
  const rev = ref(0);
  const docVersion = ref(SUPPORTED_DOC_VERSION);
  const dirty = ref(false);
  const saveState = ref<SaveState>('idle');
  const saveErrorKey = ref<string | null>(null);
  const loading = ref(false);
  const inTrash = ref(false);
  const localDraft = ref<NoteDoc | null>(null);
  const lastSavedAt = ref<number | null>(null);
  const pendingAttachments = ref<Record<string, Attachment>>({});

  const versionTooNew = computed(() => docVersion.value > SUPPORTED_DOC_VERSION);
  const writeBlocked = computed(() => versionTooNew.value || inTrash.value || noteId.value === null);
  const readOnlyReason = computed<string | null>(() => {
    if (versionTooNew.value) return 'versionTooNew';
    if (inTrash.value) return 'inTrash';
    return null;
  });
  const saveLabel = computed(() => {
    if (saveState.value === 'saving' || saveState.value === 'pending') return t('editor.saving');
    if (dirty.value) return t('editor.unsaved');
    if (saveState.value === 'error') return messageFor(saveErrorKey.value ?? 'error.sync_failed');
    if (lastSavedAt.value !== null) return t('editor.saved');
    return '';
  });
  const hasDraftConflict = computed(() => localDraft.value !== null);
  const isEmpty = computed(() => blocks.value.length === 0);
  const ready = computed<EditorReadiness>(() => ({ loading: loading.value, empty: noteId.value === null }));

  const debouncedSave = createDebounced(() => {
    void save();
  }, AUTOSAVE_DEBOUNCE_MS);

  /** 所有 save 依次排队，杜绝两条自动保存同时出发把彼此打成"冲突"。 */
  let saveChain: Promise<void> = Promise.resolve();

  function hydrate(note: Note): void {
    noteId.value = note.id;
    rev.value = typeof note.rev === 'number' ? note.rev : 0;
    docVersion.value = typeof note.doc?.v === 'number' ? note.doc.v : SUPPORTED_DOC_VERSION;
    inTrash.value = note.deletedAt !== null && note.deletedAt !== undefined;
    blocks.value = docToBlocks(note.doc);
    dirty.value = false;
    saveState.value = 'idle';
    saveErrorKey.value = null;
  }

  async function open(id: string | null): Promise<void> {
    if (id === noteId.value) return;
    await flush();
    debouncedSave.cancel();
    localDraft.value = null;
    saveErrorKey.value = null;
    if (id === null) {
      noteId.value = null;
      blocks.value = [];
      dirty.value = false;
      saveState.value = 'idle';
      return;
    }
    noteId.value = id;
    blocks.value = [];
    loading.value = true;
    try {
      const note = await callCommand<Note | null>(Commands.getNote, { id });
      if (noteId.value !== id) return;
      if (!note) {
        saveErrorKey.value = 'not_found';
        return;
      }
      hydrate(note);
    } catch (error) {
      if (noteId.value !== id) return;
      const bridge = asBridgeError(error);
      saveErrorKey.value = bridge.messageKey;
      toasts.push(bridge.messageKey, 'error');
    } finally {
      if (noteId.value === id) loading.value = false;
    }
  }

  function currentDoc(): NoteDoc {
    return blocksToDoc(blocks.value, Math.min(docVersion.value, SUPPORTED_DOC_VERSION));
  }

  function replaceBlocks(next: readonly EditorBlock[]): void {
    if (writeBlocked.value) return;
    blocks.value = next.map((block) => ({ ...block }));
    dirty.value = true;
    saveState.value = 'pending';
    debouncedSave();
  }

  /** 单个块就地更新（保留其它块的 DOM 与 id）。 */
  function updateBlock(block: EditorBlock): void {
    if (writeBlocked.value) return;
    blocks.value = blocks.value.map((item) => (item.id === block.id ? { ...block } : item));
    dirty.value = true;
    saveState.value = 'pending';
    debouncedSave();
  }

  function commitStructural(next: readonly EditorBlock[], focusId: string | null, caret = 0): void {
    replaceBlocks(next);
    focusHint.value = focusId === null ? null : { id: focusId, caret };
  }

  const focusHint = ref<{ id: string; caret: number } | null>(null);
  function consumeFocusHint(): { id: string; caret: number } | null {
    const hint = focusHint.value;
    focusHint.value = null;
    return hint;
  }

  function insertBlockAfter(index: number, block: EditorBlock): void {
    const next = [...blocks.value];
    next.splice(index + 1, 0, block);
    commitStructural(next, block.id, 0);
  }

  function appendBlock(): void {
    const block = emptyParagraph();
    commitStructural([...blocks.value, block], block.id, 0);
  }

  /** 失焦 / 切换笔记 / 关闭前：立即提交未保存改动。 */
  async function flush(): Promise<void> {
    if (!debouncedSave.pending()) return;
    debouncedSave.cancel();
    await save();
  }

  async function save(): Promise<void> {
    // 串行化：并发两次 save 会都带着同一个 expectedRev 出发，先回来的把 rev 推进，
    // 后回来的就被核心判成 stale_edit —— 于是用户看到一条"这条笔记在别处被改动了"，
    // 而"别处"其实是我们自己的第二次自动保存。
    const next = saveChain.then(() => doSave());
    saveChain = next.then(
      () => undefined,
      () => undefined,
    );
    return next;
  }

  async function doSave(): Promise<void> {
    if (noteId.value === null || !dirty.value || writeBlocked.value) return;
    const doc = currentDoc();
    const targetId = noteId.value;
    saveState.value = 'saving';
    try {
      const note = await callCommand<Note>(Commands.editNote, { id: targetId, doc, expectedRev: rev.value });
      if (noteId.value !== targetId) return;
      if (typeof note?.rev === 'number') rev.value = note.rev;
      dirty.value = false;
      saveState.value = 'saved';
      lastSavedAt.value = Date.now();
      saveErrorKey.value = null;
      notes.applyNoteUpdate(note);
      sync.noteSavedLocally();
      localDraft.value = null;
    } catch (error) {
      const bridge = asBridgeError(error);
      if (bridge.isStaleEdit) {
        await enterStale(doc, targetId);
        return;
      }
      if (noteId.value !== targetId) return;
      saveState.value = 'error';
      saveErrorKey.value = bridge.messageKey;
      if (bridge.retryable) debouncedSave();
    }
  }

  async function enterStale(localDoc: NoteDoc, targetId: string): Promise<void> {
    localDraft.value = localDoc;
    saveState.value = 'error';
    saveErrorKey.value = 'note.staleEdit';
    toasts.pushText(t('editor.staleTitle'), 'warn');
    if (noteId.value === targetId) await reloadRemote();
  }

  async function reloadRemote(): Promise<void> {
    const targetId = noteId.value;
    if (!targetId) return;
    try {
      const note = await callCommand<Note | null>(Commands.getNote, { id: targetId });
      if (!note || noteId.value !== targetId) return;
      rev.value = note.rev;
      docVersion.value = typeof note.doc?.v === 'number' ? note.doc.v : SUPPORTED_DOC_VERSION;
      blocks.value = docToBlocks(note.doc);
      dirty.value = false;
      notes.applyNoteUpdate(note);
    } catch {
      // 拉不到最新就保持现状；本机版本仍在 localDraft，不会丢
    }
  }

  /** 用回自己的版本继续写：带着对方的最新 rev 再提交一次。 */
  async function useLocalDraft(): Promise<void> {
    const draft = localDraft.value;
    if (!draft) return;
    localDraft.value = null;
    blocks.value = docToBlocks(draft);
    dirty.value = true;
    saveState.value = 'pending';
    await save();
  }

  function discardLocalDraft(): void {
    localDraft.value = null;
    saveErrorKey.value = null;
    saveState.value = 'idle';
    dirty.value = false;
  }

  function viewDraft(): NoteDoc | null {
    return localDraft.value;
  }

  /** 附件块：先落一个带稳定 id 的占位块，再由本地核心回填摘要与名称。 */
  async function attachFile(role: 'inline' | 'file'): Promise<EditorBlock | null> {
    const targetId = noteId.value;
    if (!targetId || writeBlocked.value) return null;
    const type = role === 'inline' ? 'image' : 'attachment';
    const block: EditorBlock = {
      id: newBlockId(),
      type,
      shape: role === 'inline' ? 'image' : 'attachment',
      attrs: { role, pending: true },
      content: [],
      rest: {},
      raw: null,
    };
    const index = Math.max(0, blocks.value.length - 1);
    const next = [...blocks.value];
    next.splice(index + 1, 0, block);
    blocks.value = next;
    dirty.value = true;
    saveState.value = 'pending';
    try {
      const attachment = await callCommand<Attachment>(Commands.attachFile, { noteId: targetId, blockId: block.id, role });
      const merged: EditorBlock = {
        ...block,
        attrs: {
          ...block.attrs,
          pending: false,
          ...(attachment?.sha256 ? { sha256: attachment.sha256, ref: attachment.id ?? attachment.sha256 } : {}),
          ...(attachment?.name ? { name: attachment.name } : {}),
          ...(typeof attachment?.size === 'number' ? { size: attachment.size } : {}),
          ...(attachment?.mediaType ? { mediaType: attachment.mediaType } : {}),
        },
      };
      blocks.value = blocks.value.map((item) => (item.id === block.id ? merged : item));
      if (attachment?.id) pendingAttachments.value = { ...pendingAttachments.value, [attachment.id]: attachment };
      debouncedSave();
      return merged;
    } catch (error) {
      const bridge = asBridgeError(error);
      blocks.value = blocks.value.filter((item) => item.id !== block.id);
      if (bridge.code !== 'cancelled' && bridge.code !== 'aborted') toasts.push(bridge.messageKey, 'warn');
      return null;
    }
  }

  function attachmentState(id: string | undefined): string | undefined {
    if (!id) return undefined;
    return pendingAttachments.value[id]?.state;
  }

  function reset(): void {
    debouncedSave.cancel();
    noteId.value = null;
    blocks.value = [];
    dirty.value = false;
    saveState.value = 'idle';
    localDraft.value = null;
  }

  return {
    noteId,
    blocks,
    rev,
    docVersion,
    dirty,
    saveState,
    saveLabel,
    saveErrorKey,
    loading,
    inTrash,
    versionTooNew,
    writeBlocked,
    readOnlyReason,
    hasDraftConflict,
    isEmpty,
    ready,
    lastSavedAt,
    localDraft,
    open,
    hydrate,
    currentDoc,
    replaceBlocks,
    updateBlock,
    commitStructural,
    insertBlockAfter,
    appendBlock,
    consumeFocusHint,
    flush,
    save,
    reloadRemote,
    useLocalDraft,
    discardLocalDraft,
    viewDraft,
    attachFile,
    attachmentState,
    schedule: () => debouncedSave(),
    cancelScheduled: () => debouncedSave.cancel(),
    reset,
    textBlock,
  };
});
