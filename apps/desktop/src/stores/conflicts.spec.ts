/**
 * 冲突面板的并排预览：左右两栏到底各读哪一份文档。
 *
 * 这块之前一个测试都没有，于是"两栏指向同一份内容"这种错法可以一直活着 ——
 * 用户看着两段一模一样的文字决定"用哪一版"，选完发现丢的是自己那段。
 * 落地形状见 CONFLICT-RESOLUTION §6.1：正文是**服务器那一版**（采纳过来的），
 * 本机那一版活在副本笔记里，所以
 *   右（服务器）= (noteId, remoteRev)   左（你的）= (copyNoteId, copyRev)
 * 两侧 `localRev`/`remoteRev` 常常是同一个数（各自从同一确认点推到同一个 rev），
 * 因此"两栏都用 noteId"必然读出同一份内容 —— 下面三条盯的就是这个。
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { stubLocalService } from '../testing/http';
import { useConflictStore } from './conflicts';

const NOTE = '01920000-0000-7000-8000-000000000001';
const COPY = '01920000-0000-7000-8000-000000000002';

/** load() 里的预览是 `void loadPreview(card)`，不等它 —— 排空一轮任务队列。 */
function flush(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

beforeEach(() => {
  setActivePinia(createPinia());
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('冲突卡片的并排预览', () => {
  it('左栏读副本那一版、右栏读正文那一版，两栏不指向同一份文档', async () => {
    const service = stubLocalService({
      open_conflicts: () => [
        { id: 7, noteId: NOTE, copyNoteId: COPY, copyRev: 1, noteTitle: '吵架的笔记', localRev: 2, remoteRev: 2, createdAt: '2026-09-26T00:00:00Z' },
      ],
      preview_text: () => '甲设备加的段落',
    });
    const conflicts = useConflictStore();

    await conflicts.load();
    await flush();

    expect(service.callsOf('preview_text').map((call) => call.args)).toEqual([
      { id: COPY, rev: 1 },
      { id: NOTE, rev: 2 },
    ]);
  });

  it('卡片带回了 copyRev：核心不发这个字段，左栏就没有可信的取法', async () => {
    const conflicts = useConflictStore();
    stubLocalService({
      open_conflicts: () => [
        { id: 71, noteId: NOTE, copyNoteId: COPY, copyRev: 3, noteTitle: 'x', localRev: 2, remoteRev: 2 },
      ],
      preview_text: () => '',
    });

    await conflicts.load();

    expect(conflicts.cards[0].copyRev).toBe(3);
  });

  it('没有副本可指时不拿正文顶替左栏', async () => {
    const service = stubLocalService({
      open_conflicts: () => [{ id: 8, noteId: NOTE, copyNoteId: null, noteTitle: '只有正文', localRev: 2, remoteRev: 2 }],
      preview_text: () => '乙设备加的段落',
    });
    const conflicts = useConflictStore();

    await conflicts.load();
    await flush();

    // 只该有右栏那一次。左栏宁可空着，也不能显示成正文 —— 那是把服务器那一版冒充成"你的那一版"。
    expect(service.callsOf('preview_text').map((call) => call.args)).toEqual([{ id: NOTE, rev: 2 }]);
  });

  it('核心没发 copyRev 时左栏不发请求（不是拿 localRev 去猜）', async () => {
    const service = stubLocalService({
      open_conflicts: () => [{ id: 9, noteId: NOTE, copyNoteId: COPY, noteTitle: '旧核心', localRev: 2, remoteRev: 2 }],
      preview_text: () => '乙设备加的段落',
    });
    const conflicts = useConflictStore();

    await conflicts.load();
    await flush();

    expect(service.callsOf('preview_text').map((call) => call.args)).toEqual([{ id: NOTE, rev: 2 }]);
  });
});

/**
 * P11（删除 vs 修改）里右栏那份"对面那一版"。
 *
 * 判据故意写得像事故本身：让 `preview_text` 返回**本机**那一版的文字。
 * 只要代码还按 `(noteId, remoteRev)` 去查本机历史（rev 是各设备自己的编号，同号常是
 * 另一份内容），右栏就会被本机内容冒充 —— 那正是第一版写错、被两台真设备测试抓红的地方。
 */
describe('P11：对面那一版只能由卡片携带', () => {
  it('卡片带 remotePreview 时右栏读它，且不被本机历史的内容冒充', async () => {
    stubLocalService({
      open_conflicts: () => [
        {
          id: 11,
          noteId: NOTE,
          noteTitle: '被另一台删掉的笔记',
          localRev: 4,
          remoteRev: 4,
          remotePreview: '对面那一版：删除前的正文',
          createdAt: '2026-09-27T00:00:00Z',
        },
      ],
      // 任何一次针对 (noteId, remoteRev) 的本机查询都会把右栏冲掉 → 断言能看见
      preview_text: () => '本机这一版的正文',
    });
    const conflicts = useConflictStore();
    await conflicts.load();
    await flush();
    const card = conflicts.cards[0];
    expect(card).toBeTruthy();
    expect(conflicts.previewFor(card, 'remote')).toBe('对面那一版：删除前的正文');
    expect(conflicts.remoteMissing(card)).toBe(false);
  });

  it('没取回来时右栏是"缺"，由界面说清而不是留一片空白', async () => {
    stubLocalService({
      open_conflicts: () => [
        { id: 12, noteId: NOTE, noteTitle: '取不到那一版', localRev: 5, remoteRev: 3, createdAt: '2026-09-27T00:00:00Z' },
      ],
      preview_text: () => {
        throw new Error('这一版本机历史上也没有');
      },
    });
    const conflicts = useConflictStore();
    await conflicts.load();
    await flush();
    const card = conflicts.cards[0];
    expect(conflicts.previewFor(card, 'remote')).toBe('');
    expect(conflicts.remoteMissing(card)).toBe(true);
    // 本机那一版仍然读得到：缺的只是对面那半，不是整个卡片失效
    expect(conflicts.previewFor(card, 'local')).not.toBe('对面那一版');
  });
});
