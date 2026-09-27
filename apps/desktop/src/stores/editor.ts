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
  attachmentAttrs,
  AttachmentEmpty,
  AttachmentTooLarge,
  toAttachPayload,
  toDataUrl,
  type AttachmentData,
} from '../editor/attachmentWire';
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
    // 本地优先的一条硬规则：**同一份笔记、本地还有没保存的输入、而这次回读的 rev
    // 并不比本地更新** —— 那它拿到的就是"保存之前的旧快照"（新建笔记后立刻打字时，
    // 一次 notes-changed 回读就会走到这里）。此时保留本地正文与 dirty，让待发中的
    // autosave 自己把内容写进去；盖掉的话表现就是"屏幕上有字、库里是空正文、
    // 右下角写着已保存"（纯黑盒 UAT 三轮里两轮红就是这么来的）。
    // rev 更新的情况照旧应用 —— 那是真的远端改动，不能拿本地挡住同步。
    if (noteId.value === note.id && dirty.value && (note.rev ?? 0) <= rev.value) {
      inTrash.value = note.deletedAt !== null && note.deletedAt !== undefined;
      return;
    }
    noteId.value = note.id;
    rev.value = typeof note.rev === 'number' ? note.rev : 0;
    docVersion.value = typeof note.doc?.v === 'number' ? note.doc.v : SUPPORTED_DOC_VERSION;
    inTrash.value = note.deletedAt !== null && note.deletedAt !== undefined;
    blocks.value = docToBlocks(note.doc);
    // 图片块里的 `sha256` 只是内容键 —— 显示要另外把字节取回来变成 data URL。
    // 取不到就显示占位（INV：附件不阻塞正文），所以这里不 await、不抛错。
    for (const block of blocks.value) {
      const sha = typeof block.attrs?.sha256 === 'string' ? block.attrs.sha256 : null;
      if (sha && block.shape === 'image') void ensureAttachmentUrl(sha);
    }
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
    if (writeBlocked.value) {
      // 挡下写入可以（笔记还没就绪、只读上下文），但**绝不静默**：静默的失败形态是
      // "屏幕上明明有字、库里是空正文，而右下角写着已保存"。主干上这是偶发的
      // （纯黑盒 UAT 三轮里红两轮，见 CHANGELOG 已知限制），所以先把谎报堵住。
      saveState.value = 'error';
      saveErrorKey.value = 'save_dropped';
      return;
    }
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
    // 这一版发出去之后，用户完全可能又打了字。回包带来的是**过去**的那一版，
    // 拿它当"当前状态"就会把后打的字抹掉（实测：库里只剩第一条换行，屏幕上是
    // 整句，右下角还写着"已保存"）。所以先把发出去的签名记下来，回包时对照。
    const sentSignature = JSON.stringify(doc);
    const targetId = noteId.value;
    saveState.value = 'saving';
    try {
      const note = await callCommand<Note>(Commands.editNote, { id: targetId, doc, expectedRev: rev.value });
      if (noteId.value !== targetId) return;
      if (typeof note?.rev === 'number') rev.value = note.rev;
      if (JSON.stringify(currentDoc()) !== sentSignature) {
        // 在飞的这段时间里正文又变了：这次回包不代表当前状态。保持 dirty、
        // 不应用回包、不写 lastSavedAt，另起一轮把新版本存进去。
        dirty.value = true;
        saveState.value = 'pending';
        saveErrorKey.value = null;
        debouncedSave();
        return;
      }
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

  /**
   * 附件块：字节经过前端，但 sha256、落盘、写库、入上传队列全在核心
   * （形状见 `editor/attachmentWire.ts`）。
   *
   * 顺序不是可有可无：核心的 `attach_blob` 首次挂载会翻转派生列 `has_attachment`，
   * 那是**在同一事务里推进笔记 rev** 的。所以先把编辑器待保存的内容落完（占位块还不进
   * 正文，免得留一个假的"处理中"），再让核心写附件并接住它回的新 rev，最后才把带 sha256
   * 的块写进正文。反过来的话就是我端到端里抓到的那条 `stale_edit（expected 7, actual 8）`：
   * 用户插一张图，得到的却是"这条笔记在别处被改动了"并把界面切走。
   */
  async function attachFile(role: 'inline' | 'file', file: File | null): Promise<EditorBlock | null> {
    const targetId = noteId.value;
    if (!targetId || !file || writeBlocked.value) return null;
    debouncedSave.cancel();
    await save();
    if (noteId.value !== targetId) return null;
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
    try {
      const attachment = await callCommand<Attachment>(Commands.attachFile, { ...(await toAttachPayload(targetId, block.id, role, file)) });
      if (typeof attachment?.rev === 'number') rev.value = attachment.rev;
      const merged: EditorBlock = { ...block, attrs: { ...attachmentAttrs(role, file.name, attachment) } };
      blocks.value = blocks.value.map((item) => (item.id === block.id ? merged : item));
      if (attachment?.sha256) void ensureAttachmentUrl(attachment.sha256);
      if (attachment?.id) pendingAttachments.value = { ...pendingAttachments.value, [attachment.id]: attachment };
      dirty.value = true;
      saveState.value = 'pending';
      debouncedSave();
      return merged;
    } catch (error) {
      // 占位块撤干净就完事：正文从没写过它，所以这里不该再多存一版（白推进一次 rev）。
      blocks.value = blocks.value.filter((item) => item.id !== block.id);
      if (error instanceof AttachmentTooLarge) {
        toasts.push('attach.tooLarge', 'warn');
        return null;
      }
      if (error instanceof AttachmentEmpty) {
        toasts.push('attach.empty', 'warn');
        return null;
      }
      const bridge = asBridgeError(error);
      if (bridge.code !== 'cancelled' && bridge.code !== 'aborted') toasts.push(bridge.messageKey, 'warn');
      return null;
    }
  }

  /**
   * 显示用的 data URL 只活在内存里：写进块属性就等于写进 doc，而 doc 会随同步走 ——
   * 那是把每个附件在正文里再存一份 base64（体积 +4/3，且每次编辑都拖着它）。
   */
  const attachmentUrls = ref<Record<string, string>>({});
  const urlFetches = new Map<string, Promise<void>>();

  function attachmentUrl(sha256: string | undefined): string | null {
    if (!sha256) return null;
    return attachmentUrls.value[sha256] ?? null;
  }

  /** 同一个 sha 只取一次；取不到就保持没有 URL，让 UI 显示占位而不是错误。 */
  function ensureAttachmentUrl(sha256: string): Promise<void> {
    if (attachmentUrls.value[sha256]) return Promise.resolve();
    const inflight = urlFetches.get(sha256);
    if (inflight) return inflight;
    const p = (async () => {
      try {
        const url = toDataUrl(await callCommand<AttachmentData>(Commands.attachmentData, { sha256 }));
        if (url) attachmentUrls.value = { ...attachmentUrls.value, [sha256]: url };
      } catch {
        // 附件缺就缺着显示占位：它不该把正文变成错误堆栈（INV：附件不阻塞文本）
      } finally {
        urlFetches.delete(sha256);
      }
    })();
    urlFetches.set(sha256, p);
    return p;
  }

  /**
   * 全局快捷键（Shift+F）与工具条/命令面板共用同一个取文件入口：这里只递一个意图，
   * 真正开选择器的是 RichEditor 里那颗隐藏的 `<input type=file>`。两处各开各的
   * 选择器就会出现"快捷键那条测不到、也没带 accept 过滤"的分叉。
   */
  const attachRequest = ref<'inline' | 'file' | null>(null);
  function requestAttach(role: 'inline' | 'file'): void {
    attachRequest.value = role;
  }
  function clearAttachRequest(): void {
    attachRequest.value = null;
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
    attachRequest,
    requestAttach,
    clearAttachRequest,
    attachmentState,
    attachmentUrl,
    ensureAttachmentUrl,
    schedule: () => debouncedSave(),
    cancelScheduled: () => debouncedSave.cancel(),
    reset,
    textBlock,
  };
});
