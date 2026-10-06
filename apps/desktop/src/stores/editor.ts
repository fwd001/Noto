/**
 * 编辑器状态：Document ↔ 块模型、自动保存（乐观并发）、分歧保护。
 * 铁律：保存失败为 stale_edit 时绝不静默覆盖 —— 本机版本完整留在 localDraft，
 * 界面切到对方版本并给出可操作的取舍入口。
 */
import { defineStore } from 'pinia';
import { computed, ref } from 'vue';
import { callCommand } from '../api/bridge';
import { Commands, type Attachment, type AttachmentLedgerState, type Note, type NoteDoc } from '../api/types';
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
  // `loading` 也在这一串里，为的是切篇那个往返窗口：`open()` 先把 noteId 换成下一篇、blocks 清空，
  // 而回读还在飞 —— 这一刻补上来的写会以 `{id: 下一篇, expectedRev: 上一篇的 rev}` 出门
  // （2026-10-02 在 store 层复现出来，正是 lane 里那对 `actual 5 / expected 2`）。
  // 窗口里**拒绝**写入（updateBlock 把原因落到 saveErrorKey，不静默），比把串了的那一支发出去好。
  const writeBlocked = computed(() => loading.value || versionTooNew.value || inTrash.value || noteId.value === null);
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
    // 同时一次问清这篇引用到的每个对象在本机的账：文件附件那颗芯片不取字节（一颗芯片
    // 的显示判据不该触发 32 MiB 读盘），它只能靠这本账说话。
    const referenced: string[] = [];
    for (const block of blocks.value) {
      const sha = typeof block.attrs?.sha256 === 'string' ? block.attrs.sha256 : null;
      if (!sha) continue;
      if (block.shape === 'image') void ensureAttachmentUrl(sha);
      if (block.shape === 'image' || block.shape === 'attachment') referenced.push(sha);
    }
    void fetchAttachmentLedger(referenced);
    dirty.value = false;
    saveState.value = 'idle';
    saveErrorKey.value = null;
  }

  async function open(id: string | null, force = false): Promise<void> {
    // `force` 只给"同一篇被别处改了状态"的场合用（现在只有一处：从回收站恢复回来）。
    // 默认那句早退是对的 —— 点列表里已经开着的那一行不该把用户的输入刷掉；
    // 但 `inTrash` 与 `rev` 只在 hydrate 里被赋值，恢复之后不重读，编辑器就一直停在
    // "只读 + 旧 rev"那一格（2026-10-02 真机读数：恢复之后打字，屏幕上没字、桥那边也没收到任何写）。
    if (!force && id === noteId.value) return;
    await flush();
    debouncedSave.cancel();
    localDraft.value = null;
    saveErrorKey.value = null;
    if (id === null) {
      noteId.value = null;
      blocks.value = [];
      // `inTrash` 是"这一篇在回收站里"的状态，没有"这一篇"就不该留着它 —— 否则空编辑器会把
      // 只读原因说成 `inTrash`（用户读到的是"这条在最近删除里"，而屏幕上什么都没有）。
      inTrash.value = false;
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
    const queued = debouncedSave.pending();
    debouncedSave.cancel();
    if (queued) await save();
    // 还要等掉**已经在飞**的那一支（原来的 `if (!pending) return` 就是漏在这里）：
    // `open()` 先 flush 再读回这条笔记，如果读发生在写落地之前，拿到的就是"这次写之前"的快照，
    // 于是刚保存的内容（实测是一次加粗）被当成旧数据盖掉；而那支保存的回包发现"正文又变了"，
    // 会照着被盖掉的版本再存一次 —— 用户的编辑就此消失。表现是"屏幕上明明有粗体，刷新后没了"。
    await saveChain;
  }

  /**
   * 认领"同一行已经被推进"的 rev —— 只前进、只对得上当前打开的那一篇、**不动正文**。
   *
   * 为什么需要它：置顶与"移到文件夹"在核心里走的是同一条 `commit_edit`（pinned/folder 要能同步出去，
   * 就必须占一格 rev），而 `rev.value` 平时只跟着自己的 `edit_note` 回包前进。不交接的读数（2026-10-02 真机）：
   * 三次置顶把那一行推到 5，下一支自动保存仍报 `expectedRev=2` ⇒ 核心按 `stale_edit` 拒，
   * 用户刚打的字被 `reloadRemote` 换掉、哪儿也没落，屏幕上却写着"这条笔记在别处被改动了"。
   *
   * 只在这里改 rev：认领的是一次**本地已成功的写**，不是远端内容 —— 远端改动仍走 `enterStale` 的草稿冲突那一条路。
   */
  function adoptRev(id: string, nextRev: number): void {
    if (typeof id !== 'string' || typeof nextRev !== 'number') return;
    if (noteId.value !== id) return;
    if (nextRev <= rev.value) return;
    rev.value = nextRev;
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

  /**
   * 这台设备"有没有这份可用字节"的那本账（sha -> 核心回的那对状态位）。
   *
   * 为什么要有它：占位那句话的判据必须来自**真实存在的账**，而不是"这次会话上传过没有"。
   * 只有后者时，从别的设备同步来、本机还没下载的附件画起来跟完好的一样 —— 既不说明缺，
   * 也不给那两颗已经写好的动作（2026-10-06 记为 G74）。
   */
  const ledger = ref<Record<string, AttachmentLedgerState>>({});

  /** 没问过 / 问不到都回 `null`。`null` 不等于"缺" —— 那是"未知"，界面此刻不该下结论。 */
  function attachmentLedger(sha256: string | undefined): AttachmentLedgerState | null {
    if (!sha256) return null;
    return ledger.value[sha256] ?? null;
  }

  function rememberLedger(row: AttachmentLedgerState | null | undefined): void {
    if (!row?.sha256) return;
    ledger.value = { ...ledger.value, [row.sha256]: row };
  }

  /** 一次问一批（打开一篇笔记就一次）。去重放在这里，免得核心为同一个 sha 跑两遍。 */
  async function fetchAttachmentLedger(shas: string[]): Promise<void> {
    const uniq = [...new Set(shas.filter((sha) => sha.length > 0))];
    if (uniq.length === 0) return;
    try {
      const rows = await callCommand<AttachmentLedgerState[]>(Commands.attachmentStates, { shas: uniq });
      for (const row of rows ?? []) rememberLedger(row);
    } catch {
      // 问不到账就继续"未知"：拿一次桥的失败去告诉用户"你的附件没了"，比不说更坏。
    }
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
   * 坏图/坏附件占位上的**「重试取回」**：让核心去撤那个否定结论（`absent`/`error`），
   * 这一条命令本身不下载任何东西 —— 下载还是由常驻附件轮去做（同一套队列、同一套预算）。
   *
   * 界面上为什么不自己判断"该不该重试"：那种判据会在前端长出一台第二状态机（§39），
   * 而本仓已经在 kind 词汇、stats 键名这两类边上漂过两次。这里只把意图递过去、
   * 把核心说的话（成没成、为什么没成）显示出来。
   */
  async function retryAttachmentFetch(sha256: string | undefined): Promise<AttachmentLedgerState | null> {
    if (!sha256) return null;
    try {
      const st = await callCommand<AttachmentLedgerState>(Commands.attachmentRetry, { sha256 });
      // 核心刚把否定结论撤掉 ⇒ 那本账必须跟着改口，否则占位会继续说"缺"到下次打开为止。
      rememberLedger(st);
      toasts.pushText(t('editor.attachmentRetryDone'), 'info');
      // 这一次意图会落成一条待办，徽标该立刻反映"还有事在做"
      void sync.syncNow();
      return st;
    } catch (e) {
      pushAttachmentActionError(e);
      return null;
    }
  }

  /**
   * **「重新上传本机这份」**：本机有好字节、而服务器那一份被证明坏了（或干脆没有）时，
   * 由用户授权一次覆盖式重传。同样地，覆盖与复读校验都在核心那侧；这里只显示结果。
   * 核心拒绝的三种情况（没登记 / 本机没有那份 / 那份内容与 sha 不符）各有各的文案，
   * 因为用户下一步该做的事不一样。
   */
  async function reuploadAttachment(sha256: string | undefined): Promise<AttachmentLedgerState | null> {
    if (!sha256) return null;
    try {
      const st = await callCommand<AttachmentLedgerState>(Commands.attachmentReupload, { sha256 });
      rememberLedger(st);
      toasts.pushText(t('editor.attachmentReuploadDone'), 'info');
      void sync.syncNow();
      return st;
    } catch (e) {
      pushAttachmentActionError(e);
      return null;
    }
  }

  /** 恢复动作失败的显示：错误码与 messageKey 都是核心给的，这里只把它显示出来（不自己判语义）。 */
  function pushAttachmentActionError(e: unknown): void {
    const bridge = asBridgeError(e);
    toasts.push(bridge.messageKey, 'error');
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
    adoptRev,
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
    attachmentLedger,
    ensureAttachmentUrl,
    // 占位上的两个用户动作（「重试取回」/「重新上传本机这份」）
    retryAttachmentFetch,
    reuploadAttachment,
    schedule: () => debouncedSave(),
    cancelScheduled: () => debouncedSave.cancel(),
    reset,
    textBlock,
  };
});
