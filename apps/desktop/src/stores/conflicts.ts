/** 分歧收件箱：并排看两份内容，三个动作（保留两份 / 用这个替换 / 我来合并）。 */
import { defineStore } from 'pinia';
import { computed, ref } from 'vue';
import { callCommand } from '../api/bridge';
import { Commands, type ConflictCard } from '../api/types';
import { asBridgeError } from '../util/errors';
import { useToastStore } from './toasts';

export type ConflictAction = 'keepBoth' | 'replaceWithLocal' | 'replaceWithRemote' | 'manualMerge';

function ensureCard(value: unknown): ConflictCard | null {
  if (typeof value !== 'object' || value === null) return null;
  const raw = value as Record<string, unknown>;
  const conflictId = typeof raw.conflictId === 'number' ? raw.conflictId : typeof raw.id === 'number' ? raw.id : Number.NaN;
  if (!Number.isFinite(conflictId)) return null;
  return {
    conflictId,
    ...(typeof raw.noteId === 'string' ? { noteId: raw.noteId } : {}),
    ...(typeof raw.copyNoteId === 'string' ? { copyNoteId: raw.copyNoteId } : {}),
    ...(typeof raw.noteTitle === 'string' ? { noteTitle: raw.noteTitle } : {}),
    ...(typeof raw.title === 'string' ? { title: raw.title } : {}),
    ...(typeof raw.localRev === 'number' ? { localRev: raw.localRev } : {}),
    ...(typeof raw.remoteRev === 'number' ? { remoteRev: raw.remoteRev } : {}),
    ...(typeof raw.localPreview === 'string' ? { localPreview: raw.localPreview } : {}),
    ...(typeof raw.remotePreview === 'string' ? { remotePreview: raw.remotePreview } : {}),
    ...(typeof raw.createdAt === 'string' ? { createdAt: raw.createdAt } : {}),
    ...(Array.isArray(raw.blockIds) ? { blockIds: raw.blockIds as string[] } : {}),
  };
}

export const useConflictStore = defineStore('conflicts', () => {
  const toasts = useToastStore();
  const cards = ref<ConflictCard[]>([]);
  const loading = ref(false);
  const errorKey = ref<string | null>(null);
  const selectedId = ref<number | null>(null);
  const previews = ref<Record<string, string>>({});
  const count = computed(() => cards.value.length);
  const selected = computed(() => cards.value.find((card) => card.conflictId === selectedId.value) ?? cards.value[0] ?? null);

  async function load(): Promise<void> {
    loading.value = true;
    errorKey.value = null;
    try {
      const result = await callCommand<unknown>(Commands.openConflicts, {});
      const list = Array.isArray(result) ? result : [];
      cards.value = list.map(ensureCard).filter((card): card is ConflictCard => card !== null);
      if (cards.value.length > 0 && selectedId.value === null) selectedId.value = cards.value[0]?.conflictId ?? null;
      for (const card of cards.value) void loadPreview(card);
    } catch (error) {
      errorKey.value = asBridgeError(error).messageKey;
    } finally {
      loading.value = false;
    }
  }

  function previewKey(card: ConflictCard, side: 'local' | 'remote'): string {
    return `${card.conflictId}:${side}`;
  }

  /** 并排预览：优先用 preview_text，缺失时退回卡片自带的预览文本。 */
  async function loadPreview(card: ConflictCard): Promise<void> {
    const sides: Array<{ side: 'local' | 'remote'; id: string | undefined; rev: number | undefined; fallback: string | undefined }> = [
      { side: 'local', id: card.noteId ?? card.copyNoteId ?? undefined, rev: card.localRev, fallback: card.localPreview },
      { side: 'remote', id: card.copyNoteId ?? card.noteId ?? undefined, rev: card.remoteRev, fallback: card.remotePreview },
    ];
    for (const entry of sides) {
      if (entry.fallback !== undefined && previews.value[previewKey(card, entry.side)] === undefined) {
        previews.value = { ...previews.value, [previewKey(card, entry.side)]: entry.fallback };
      }
      if (!entry.id || typeof entry.rev !== 'number') continue;
      try {
        const text = await callCommand<string>(Commands.previewText, { id: entry.id, rev: entry.rev });
        if (typeof text === 'string') previews.value = { ...previews.value, [previewKey(card, entry.side)]: text };
      } catch {
        // 拿不到就保留已有预览，不让面板空白
      }
    }
  }

  function previewFor(card: ConflictCard, side: 'local' | 'remote'): string {
    return previews.value[previewKey(card, side)] ?? '';
  }

  async function resolve(card: ConflictCard, action: ConflictAction): Promise<boolean> {
    errorKey.value = null;
    try {
      await callCommand<unknown>(Commands.resolveConflict, { id: card.conflictId, action });
      cards.value = cards.value.filter((item) => item.conflictId !== card.conflictId);
      selectedId.value = cards.value[0]?.conflictId ?? null;
      toasts.push('conflict.resolvedNow', 'info');
      return true;
    } catch (error) {
      errorKey.value = asBridgeError(error).messageKey;
      return false;
    }
  }

  function noteNewConflict(conflictId: number): void {
    if (cards.value.some((card) => card.conflictId === conflictId)) return;
    cards.value = [...cards.value, { conflictId }];
  }

  return { cards, loading, errorKey, selectedId, selected, count, previews, previewFor, load, resolve, noteNewConflict };
});
