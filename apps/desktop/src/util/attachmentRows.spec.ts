/**
 * 附件逐份清单的判据（§6「附件管理器」的后一半，缺口 G99 剩下的那一格）。
 *
 * 这里每一条都对应界面上**一句要给人看的话**：谁排在前面、列了几行、还有多少没列、
 * 那一行的名字从哪来。所以每条都配一个"只触发它自己"的输入 —— 特别是排序那条：
 * 三份夹具若体积各不相同，删掉 sha 那个收口也照样绿（第 31 刀"两行样本里最后一条恰好就是最早那条"
 * 就是同一个形状），所以这里**故意放了两份体积相同**的行，且它们在输入里的顺序与期望相反。
 */
import { describe, expect, it } from 'vitest';
import type { AttachmentInventoryRow } from '../api/types';
import {
  ROW_LIMIT,
  attachmentCleanName,
  attachmentNameKey,
  ledgerView,
  localStateKey,
  remoteStateKey,
  retryOutcomeKey,
  rowActions,
  rowBand,
  rowNameKey,
  sortLedgerRows,
} from './attachmentRows';

const row = (over: Partial<AttachmentInventoryRow> & { sha256: string }): AttachmentInventoryRow => ({
  bytes: 10,
  localState: 'available',
  remoteState: 'present',
  refs: 1,
  quarantinedUntil: null,
  name: 'x.png',
  isImage: true,
  ...over,
});

describe('附件逐份清单的档位与排序', () => {
  it('三档各由自己的条件决定，两两不重叠', () => {
    // 缺字节 + 仍被引用 ⇒ 最前面那一档（用户此刻点开的是缺的那张图）。
    expect(rowBand(row({ sha256: 'a', localState: 'missing', refs: 2 }))).toBe(0);
    expect(rowBand(row({ sha256: 'b', localState: 'partial', refs: 1 }))).toBe(0);
    expect(rowBand(row({ sha256: 'c', localState: 'error', refs: 1 }))).toBe(0);
    // 隔离区那一档看的是"有没有到期时刻"，与本机那一列无关 —— available 也能在隔离区里。
    expect(rowBand(row({ sha256: 'd', quarantinedUntil: '2026-11-01T00:00:00Z' }))).toBe(1);
    expect(
      rowBand(row({ sha256: 'e', localState: 'missing', refs: 0, quarantinedUntil: '2026-11-01T00:00:00Z' })),
    ).toBe(1);
    // 缺字节但**没人引用、也没进隔离区** ⇒ 既不是"看不见的图"也不是"倒计时"，落在最后。
    expect(rowBand(row({ sha256: 'f', localState: 'missing', refs: 0 }))).toBe(2);
    expect(rowBand(row({ sha256: 'g' }))).toBe(2);
  });

  it('先按档位、再按体积从大到小、体积相同按 sha —— 且不许改动传进来的那一份', () => {
    const input = [
      row({ sha256: 'a'.repeat(64), bytes: 100 }),
      row({ sha256: 'e'.repeat(64), bytes: 5, localState: 'missing', refs: 3 }),
      row({ sha256: 'c'.repeat(64), bytes: 1000, localState: 'missing', refs: 0, quarantinedUntil: '2026-11-01T00:00:00Z' }),
      // 与 'a' 同体积，但在输入里排在它后面：删掉 sha 那一步收口时，这两行的相对顺序
      // 就退回输入顺序 —— 期望顺序是反的，所以那条变异必红。
      row({ sha256: '9'.repeat(64), bytes: 100 }),
      row({ sha256: 'b'.repeat(64), bytes: 500, localState: 'missing', refs: 1 }),
    ];
    const before = input.map((r) => r.sha256);
    expect(sortLedgerRows(input).map((r) => r.sha256)).toEqual([
      'b'.repeat(64),
      'e'.repeat(64),
      'c'.repeat(64),
      '9'.repeat(64),
      'a'.repeat(64),
    ]);
    expect(input.map((r) => r.sha256)).toEqual(before);
  });

  /**
   * 体积那一步要**单独**有牙。上面那份夹具不够：档位 0 那两行（'b' 500 与 'e' 5）
   * 恰好"体积从大到小"与"按 sha 升序"给出同一个顺序，所以摘掉体积那一步它照旧绿
   * （MS3 实测就是这样）。这一份把同档两行的 sha 顺序与体积顺序**拧反**，
   * 于是"按体积排"这件事只有它自己能证明。
   */
  it('同一档里，体积大的在前 —— 这个顺序与 sha 升序相反（少了体积那一步就红）', () => {
    const input = [
      row({ sha256: 'z'.repeat(64), bytes: 900 }),
      row({ sha256: '2'.repeat(64), bytes: 10 }),
    ];
    expect(sortLedgerRows(input).map((r) => r.sha256)).toEqual(['z'.repeat(64), '2'.repeat(64)]);
  });

  /**
   * 上界那个数是**写死在判据里的字面量**，不是从被测常量算出来的。
   * 上一版这里写的是 `ROW_LIMIT + 3` 行、`slice(0, ROW_LIMIT)` 份 —— 于是把 20 改成 21
   * 之后两边一起跟着动，测试照样绿（MS4 实测就是这样：屏幕多画一行而单测什么都没看见）。
   * 期望值与读数同源就分辨不出上游坏，所以这里三处都是字面量。
   */
  it('上界是 20：23 份 ⇒ 画 20 行、说"还有 3 份"', () => {
    expect(ROW_LIMIT).toBe(20);
    const many = Array.from({ length: 23 }, (_, i) => row({ sha256: String(i).padStart(64, '0') }));
    const view = ledgerView(many);
    expect(view.shown.length).toBe(20);
    expect(view.hidden).toBe(3);
    expect(view.shown.map((r) => r.sha256)).toEqual(many.slice(0, 20).map((r) => r.sha256));
  });

  it('上界之内一份都不少，越过上界要说得出漏了几份', () => {
    // 恰好压在上界：那句"还有 N 份"不许出现（把 <= 写成 < 就是在这里红）。
    const exact = Array.from({ length: 20 }, (_, i) => row({ sha256: String(i).padStart(64, '0') }));
    expect(ledgerView(exact).hidden).toBe(0);
    expect(ledgerView(exact).shown.length).toBe(20);
    // 不到上界时 hidden 不许变成负数（少了 Math.max 的那一档在这里红 —— MS5 实测就是单测抓到的）。
    expect(ledgerView([row({ sha256: 'z'.repeat(64) })])).toEqual({
      shown: [expect.objectContaining({ sha256: 'z'.repeat(64) })],
      hidden: 0,
    });
    // 读不到 / 空账：这一格不许把自己弄成"有一行空行"。
    expect(ledgerView(null)).toEqual({ shown: [], hidden: 0 });
    expect(ledgerView([])).toEqual({ shown: [], hidden: 0 });
  });
});

describe('附件那一行的措辞来源', () => {
  it('本机四个值各有各的一句，互不共用', () => {
    const keys = ['available', 'partial', 'missing', 'error'].map((state) =>
      localStateKey(row({ sha256: 'a', localState: state })),
    );
    expect(new Set(keys).size).toBe(4);
    expect(keys).toEqual([
      'settings.attLocalHere',
      'settings.attLocalPart',
      'settings.attLocalAbsent',
      'settings.attLocalBroken',
    ]);
    // 账上出现没见过的值 ⇒ 说"读不出来"那一类，不许说"有"。
    expect(localStateKey(row({ sha256: 'a', localState: 'weird' }))).toBe('settings.attLocalBroken');
  });

  it('服务器四个值各有各的一句，且与本机那一列不共用键', () => {
    const keys = ['present', 'absent', 'error', 'unknown'].map((state) =>
      remoteStateKey(row({ sha256: 'a', remoteState: state })),
    );
    expect(new Set(keys).size).toBe(4);
    expect(keys).toEqual([
      'settings.attRemoteHave',
      'settings.attRemoteNone',
      'settings.attRemoteBroken',
      'settings.attRemoteUnknown',
    ]);
    expect(remoteStateKey(row({ sha256: 'a', remoteState: 'weird' }))).toBe('settings.attRemoteUnknown');
    // 两列共键 = 界面上"本机有 / 服务器也有"其实是同一句，§4.4 要的分不开就没了。
    const localKeys = ['available', 'partial', 'missing', 'error'].map((state) =>
      localStateKey(row({ sha256: 'a', localState: state })),
    );
    expect(keys.some((k) => localKeys.includes(k))).toBe(false);
  });

  it('有名字就用名字；没名字按"这张图 / 这份文件"，两种都没有名字的说法', () => {
    expect(rowNameKey(row({ sha256: 'a', name: '报告.pdf', isImage: false }))).toBeNull();
    expect(rowNameKey(row({ sha256: 'a', name: null, isImage: true }))).toBe('settings.attNameImage');
    expect(rowNameKey(row({ sha256: 'a', name: null, isImage: false }))).toBe('settings.attNameFile');
    // 空串与"这个键压根没发过来"（旧核心的载荷 / 桥改了形状）也是"没名字" ——
    // 界面那一行不许因此画出一个空白名字。类型上说 `name` 一定有，所以这一发要绕过类型。
    expect(rowNameKey(row({ sha256: 'a', name: '', isImage: true }))).toBe('settings.attNameImage');
    expect(rowNameKey({ ...row({ sha256: 'a' }), name: undefined } as unknown as AttachmentInventoryRow)).toBe(
      'settings.attNameImage',
    );
  });
});

describe('那一行该不该给两颗自救动作（§4.4）', () => {
  it('缺字节**且仍有人在等**才给重试取回；零引用又没进隔离区的不给', () => {
    expect(rowActions(row({ sha256: 'a', localState: 'missing', refs: 2 }))).toEqual({ retry: true, reupload: false });
    expect(rowActions(row({ sha256: 'a', localState: 'partial', refs: 1 }))).toEqual({ retry: true, reupload: false });
    expect(rowActions(row({ sha256: 'a', localState: 'error', refs: 1 }))).toEqual({ retry: true, reupload: false });
    // 没人引用、也没进隔离区 ⇒ 没有"看不见的图"在等，画一颗按钮只会让人以为点了有什么用。
    expect(rowActions(row({ sha256: 'a', localState: 'missing', refs: 0 }))).toEqual({ retry: false, reupload: false });
  });

  it('隔离区里的那一份给重试取回 —— 核心那条路径先查本地隔离区，点下去是撤销隔离', () => {
    expect(rowActions(row({ sha256: 'a', refs: 0, quarantinedUntil: '2026-11-01T00:00:00Z' }))).toEqual({
      retry: true,
      reupload: false,
    });
    // 本机有字节 + 在隔离区：仍然给（这一发点的是"把这份拿回来"，不是"再去下载一次"）。
    expect(rowActions(row({ sha256: 'a', refs: 0, localState: 'available', quarantinedUntil: '2026-11-01T00:00:00Z' }))).toEqual({
      retry: true,
      reupload: false,
    });
  });

  it('重新上传本机这份只在"本机是好的、服务器那侧被证明坏了或没有"时给', () => {
    expect(rowActions(row({ sha256: 'a', localState: 'available', refs: 1, remoteState: 'absent' }))).toEqual({
      retry: false,
      reupload: true,
    });
    expect(rowActions(row({ sha256: 'a', localState: 'available', refs: 1, remoteState: 'error' }))).toEqual({
      retry: false,
      reupload: true,
    });
    // 「还没查过」不给覆盖入口：不知道对面是什么就递一颗"把服务器那份换掉"，是这一格最不该有的大方。
    expect(rowActions(row({ sha256: 'a', localState: 'available', refs: 1, remoteState: 'unknown' }))).toEqual({
      retry: false,
      reupload: false,
    });
    expect(rowActions(row({ sha256: 'a', localState: 'available', refs: 1, remoteState: 'present' }))).toEqual({
      retry: false,
      reupload: false,
    });
    // 本机有、服务器没有、但**没人在引用** ⇒ 传上去也没人用：不给（零引用的那档归隔离区那条路径）。
    expect(rowActions(row({ sha256: 'a', localState: 'available', refs: 0, remoteState: 'absent' }))).toEqual({
      retry: false,
      reupload: false,
    });
  });

  it('两列都不好的那一行只给重试取回（本机坏的时候"上传本机这份"是不可用的动作）', () => {
    expect(rowActions(row({ sha256: 'a', localState: 'error', refs: 1, remoteState: 'error' }))).toEqual({
      retry: true,
      reupload: false,
    });
  });
});

describe('「这张图 / 这份文件」这一对词只有一个出处（G103）', () => {
  it('图片与非图片各一个词，两个都是给人看的话', () => {
    expect(attachmentNameKey(true)).toBe('settings.attNameImage');
    expect(attachmentNameKey(false)).toBe('settings.attNameFile');
    expect(attachmentNameKey(true)).not.toBe(attachmentNameKey(false));
  });

  it('空白与缺失都算"没有名字"，有名字才用名字', () => {
    expect(attachmentCleanName('报告.pdf')).toBe('报告.pdf');
    expect(attachmentCleanName('  两侧有空白.png  ')).toBe('两侧有空白.png');
    expect(attachmentCleanName('   ')).toBeNull();
    expect(attachmentCleanName('')).toBeNull();
    expect(attachmentCleanName(null)).toBeNull();
    expect(attachmentCleanName(undefined)).toBeNull();
  });

  it('那一行的名字与芯片的名字走同一套判断：空白名不再被当成名字画上屏', () => {
    // 第一版 `if (row.name)` 把 "   " 当成有名字 ⇒ 屏幕上是一个空白加一个分隔符。
    expect(rowNameKey(row({ sha256: 'a', name: '   ', isImage: true }))).toBe('settings.attNameImage');
    expect(rowNameKey(row({ sha256: 'a', name: ' 真有名字.png ', isImage: false }))).toBeNull();
  });
});

describe('「重试取回」的文案由核心回了什么决定（G104）', () => {
  it('本机立刻有字节 ⇒ 说的是本地补回来了；还在等 ⇒ 才说排进下载队列', () => {
    expect(retryOutcomeKey('available')).toBe('settings.attRetryLocal');
    expect(retryOutcomeKey('missing')).toBe('editor.attachmentRetryDone');
    expect(retryOutcomeKey('partial')).toBe('editor.attachmentRetryDone');
    expect(retryOutcomeKey('error')).toBe('editor.attachmentRetryDone');
    // 读不到那一格也不许说成本地命中（说反的那一句是"没有打网络"，最容易被信以为真）。
    expect(retryOutcomeKey(undefined)).toBe('editor.attachmentRetryDone');
  });
});
