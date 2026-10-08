/**
 * §6「版本历史浏览」：这一篇的历史（缺口 G100 的读侧 + 回滚）。
 *
 * 三条口径是从核心那边搬过来的，改这里之前先看 `crates/notera-host/tests/note_revisions.rs`：
 *  · 列表**只有元信息**，正文按版另读一次（保留窗口每篇 200 行，一次全发就是把这篇乘 200 端上桥）；
 *  · "这一版跟现在一样吗"是核心算完再发的（`sameAsNow`），这里不拿哈希自己比；
 *  · **回滚走 `edit_note` 那唯一一条写出口** —— 不绕过 CAS，也不另开一条写路径。
 *    所以覆盖之后屏幕上是一次普通的"本机改动"，历史里两版都还在。
 */
import { defineStore } from 'pinia';
import { computed, ref } from 'vue';
import { callCommand } from '../api/bridge';
import {
  Commands,
  type NoteDoc,
  type NoteRevisionRow,
  type NoteRevisions,
  type RevisionDoc,
} from '../api/types';

/** 那一版的正文，摊成给人读的几行（块与块之间空一行，与编辑器里的分段一致）。 */
export function docToLines(doc: NoteDoc | null | undefined): string[] {
  const blocks = (doc as { content?: unknown } | null | undefined)?.content;
  if (!Array.isArray(blocks)) return [];
  const lines: string[] = [];
  for (const block of blocks) {
    const inline = (block as { content?: unknown } | null)?.content;
    if (!Array.isArray(inline)) continue;
    const text = inline
      .map((part) => (part as { text?: unknown })?.text)
      .filter((t): t is string => typeof t === 'string')
      .join('');
    if (text.trim() !== '') lines.push(text);
  }
  return lines;
}

export const useVersionsStore = defineStore('versions', () => {
  const noteId = ref<string | null>(null);
  const rows = ref<NoteRevisionRow[]>([]);
  const currentRev = ref(0);
  const syncRev = ref(0);
  const truncated = ref(false);
  const failed = ref(false);
  const loading = ref(false);
  const preview = ref<RevisionDoc | null>(null);
  const previewFailed = ref(false);

  /** 有没有"还没公告出去"的版（`rev != syncRev`）—— 界面上那句"最新一版还没传上去"读的就是这个。 */
  const hasUnpublished = computed(() => currentRev.value > syncRev.value);
  const previewLines = computed(() => docToLines(preview.value?.doc));

  function reset(): void {
    noteId.value = null;
    rows.value = [];
    currentRev.value = 0;
    syncRev.value = 0;
    truncated.value = false;
    failed.value = false;
    preview.value = null;
    previewFailed.value = false;
  }

  async function load(id: string): Promise<void> {
    if (noteId.value !== id) {
      // 换篇笔记：先把上一份的现场清掉。留着它，用户会在这一篇的格子里看到那一篇的版本。
      reset();
      noteId.value = id;
    }
    loading.value = true;
    try {
      const got = await callCommand<NoteRevisions>(Commands.noteRevisions, { id });
      const usable = got && Array.isArray(got.rows);
      rows.value = usable ? got.rows : [];
      currentRev.value = usable ? Number(got.currentRev) || 0 : 0;
      syncRev.value = usable ? Number(got.syncRev) || 0 : 0;
      truncated.value = usable ? got.truncated === true : false;
      failed.value = !usable;
    } catch {
      rows.value = [];
      currentRev.value = 0;
      syncRev.value = 0;
      truncated.value = false;
      failed.value = true;
    } finally {
      loading.value = false;
    }
  }

  /** 点开列表里那一行才读正文。读失败要单独说 —— 不许画成"这一版什么都没写"。 */
  async function openPreview(id: string, rev: number): Promise<void> {
    previewFailed.value = false;
    try {
      const got = await callCommand<RevisionDoc>(Commands.noteRevision, { id, rev });
      preview.value = got && typeof got.rev === 'number' && got.doc ? got : null;
      if (!preview.value) previewFailed.value = true;
    } catch {
      preview.value = null;
      previewFailed.value = true;
    }
  }

  function closePreview(): void {
    preview.value = null;
    previewFailed.value = false;
  }

  /**
   * 用某一版覆盖现在。返回的是核心新落的那一版的 rev（失败 ⇒ null）。
   *
   * `expectedRev` 用的是**列表里那个 currentRev**：这正是两个窗口都开着时的护栏 ——
   * 别处改过 ⇒ 核心回 `stale_edit`，这里不重试也不静默刷新，让界面说"这一版没覆盖成功"。
   */
  async function restore(id: string, rev: number): Promise<number | null> {
    const got = await callCommand<RevisionDoc>(Commands.noteRevision, { id, rev }).catch(() => null);
    if (!got || !got.doc) return null;
    try {
      const note = await callCommand<{ rev?: number }>(Commands.editNote, {
        id,
        doc: got.doc,
        expectedRev: currentRev.value,
      });
      return typeof note?.rev === 'number' ? note.rev : null;
    } catch {
      return null;
    }
  }

  return {
    noteId,
    rows,
    currentRev,
    syncRev,
    truncated,
    failed,
    loading,
    preview,
    previewFailed,
    hasUnpublished,
    previewLines,
    reset,
    load,
    openPreview,
    closePreview,
    restore,
  };
});
