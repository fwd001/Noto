/**
 * §6「附件管理器」那一格的**逐份账**（缺口 G99 的后一半）。
 *
 * 为什么这些判断放在一个纯模块里而不是组件里：这一格的排序规则与"列了几行"都是要
 * 对用户说的话（「有问题的排在前面」「还有 N 份没列」），说错就是假话；放进组件
 * 就只能靠浏览器量，而浏览器一次只看得到真账那一两行 —— 这里每一条都能拿夹具直问。
 *
 * 一条底线：**引用数只从 `refs` 来，名字只从 `name` 来**。两者在核心是两个来源
 * （`COUNT(DISTINCT note_id)` 与 `filename` 列），界面把它们混用就会出现
 * "列了两个名字便说两篇"这种对不上账的话。
 */
import type { AttachmentInventoryRow } from '../api/types';

/** 这一格一次最多列几行。整库的账压成一屏可读的清单，剩下的**必须说有多少没列** ——
 *  静默截断与"这台设备的附件就这些"在界面上长一个样（§5 那条"不许静默裁掉"同一族）。 */
export const ROW_LIMIT = 20;

/**
 * 这一行落在哪一档（数字越小越该在前面）。
 *
 * · `0` 字节不在这台设备上**而笔记还在引用它** —— 用户此刻点开的是缺的那张图；
 * · `1` 在隔离区 —— 没人在引用它，但它还占着磁盘，界面上那句倒计时说的是这一档；
 * · `2` 其余（本机有字节、也没进隔离区）。
 *
 * 为什么 0 压在 1 前面：这两档不会重叠（隔离只认"零引用"的那一行，核心那条 SQL 拒掉
 * 仍被引用的），真要重叠了也是"此刻看不见的图"更要紧。判据量的是这条顺序本身。
 */
export function rowBand(row: AttachmentInventoryRow): 0 | 1 | 2 {
  if (row.localState !== 'available' && row.refs > 0) return 0;
  if (row.quarantinedUntil !== null) return 1;
  return 2;
}

/**
 * 越靠前的行越该被看到：先按档位，再按体积（占磁盘多的在前，这正是"管理器"的用途），
 * 最后按 sha 收口 —— 体积相同的两份**必须**每次排同一个顺序，否则这一格在两次刷新之间
 * 会自己换队，而界面上没有任何东西解释为什么换。
 */
export function sortLedgerRows(rows: readonly AttachmentInventoryRow[]): AttachmentInventoryRow[] {
  return [...rows].sort((a, b) => {
    const band = rowBand(a) - rowBand(b);
    if (band !== 0) return band;
    if (a.bytes !== b.bytes) return b.bytes - a.bytes;
    if (a.sha256 === b.sha256) return 0;
    return a.sha256 < b.sha256 ? -1 : 1;
  });
}

export interface LedgerView {
  shown: AttachmentInventoryRow[];
  /** 有多少份**没**列出来（0 才是"这就是全部"）。 */
  hidden: number;
}

/** 排好序并切出上界。`hidden` 由"总数 − 上界"算，不由调用方数 —— 数错一次就是一句假话。 */
export function ledgerView(rows: readonly AttachmentInventoryRow[] | null | undefined): LedgerView {
  const sorted = sortLedgerRows(rows ?? []);
  return {
    shown: sorted.slice(0, ROW_LIMIT),
    hidden: Math.max(0, sorted.length - ROW_LIMIT),
  };
}

/**
 * 本机那一侧的话（DATA-MODEL §8 的四个值，各有一句）。四个键都写成字面量而不是拼出来：
 * 界面上的键要能被"每个键都有登记、每个键都有人用"那两条检查扫到。
 */
export function localStateKey(row: AttachmentInventoryRow): string {
  switch (row.localState) {
    case 'available':
      return 'settings.attLocalHere';
    case 'partial':
      return 'settings.attLocalPart';
    case 'missing':
      return 'settings.attLocalAbsent';
    default:
      return 'settings.attLocalBroken';
  }
}

/** 服务器那一侧的话（未知 / 没有 / 有 / 坏）。与本机那一列**各自一句**，§4.4 要的就是这两列分得开。 */
export function remoteStateKey(row: AttachmentInventoryRow): string {
  switch (row.remoteState) {
    case 'present':
      return 'settings.attRemoteHave';
    case 'absent':
      return 'settings.attRemoteNone';
    case 'error':
      return 'settings.attRemoteBroken';
    default:
      return 'settings.attRemoteUnknown';
  }
}

/** 那一行的名字：账上有名字就返回 `null`（调用方直接用那个名字），没有才需要一个兜底的词。 */
export function rowNameKey(row: AttachmentInventoryRow): string | null {
  if (attachmentCleanName(row.name)) return null;
  return attachmentNameKey(row.isImage);
}

/**
 * "这张图 / 这份文件"那一对词的唯一出处（§6-9 那一列与正文那颗附件芯片共用）。
 *
 * 为什么要抽出来：正文的芯片以前在块上没有名字时拿 **sha 的前 12 位**当名字
 * （`RichEditor.attachmentName`，缺口 G103）。§4.5 那句「绝不能用一串哈希代替内容」
 * 管的就是这一族 —— 而这一族最容易在两个地方各写一遍：一处修好了，另一处继续念哈希。
 */
export function attachmentNameKey(isImage: boolean): string {
  return isImage ? 'settings.attNameImage' : 'settings.attNameFile';
}

/** 名字那一列的真值判断：空白不算名字（与核心把空白折成 `null` 同一口径）。 */
export function attachmentCleanName(value: string | null | undefined): string | null {
  const clean = value?.trim();
  return clean ? clean : null;
}

/**
 * 这一行该不该给§4.4 那两颗自救动作。条件照 §4.4 的口径逐条对：
 *
 * · **重试取回**：本机没有字节（缺 / 半 / 坏），**或**这一份躺在隔离区里 ——
 *   核心那条路径先查隔离区，命中就本地补回且**一次网络都不打**（`App::retry_attachment`）。
 *   零引用又没进隔离区的行不给：那没人在等它，画一颗按钮只会让人以为点了有什么用。
 * · **重新上传本机这份**：本机有好字节、**仍有笔记在引用**，而服务器那一侧被证明坏了或压根没有。
 *   `remoteState === 'unknown'`（还没查过）**不给** —— 那条的语义是"覆盖服务器那一份"，
 *   在不知道对面是什么的时候就给不可逆的入口，是这一格最不该有的那种大方。
 */
export function rowActions(row: AttachmentInventoryRow): { retry: boolean; reupload: boolean } {
  const localAbsent = row.localState !== 'available';
  const retry = (localAbsent && row.refs > 0) || row.quarantinedUntil !== null;
  const reupload = row.localState === 'available' && row.refs > 0
    && (row.remoteState === 'absent' || row.remoteState === 'error');
  return { retry, reupload };
}

/**
 * 「重试取回」点成之后要说的那一句，**由核心回包里的 localState 决定**（缺口 G104）。
 *
 * 为什么不让调用方自己说：核心有两条都算成功的路径 —— 排队等下载（本机还是没字节），
 * 以及**在隔离区本地命中**（字节立刻回来了，`未发一次请求`）。以前这里恒说
 * 「已重新排进下载队列，下一次同步会再去问服务器一次」，于是本地命中那一发说的是一句
 * 没发生过的话：用户会一直等一次根本不会来的下载。
 */
export function retryOutcomeKey(localState: string | undefined): string {
  return localState === 'available' ? 'settings.attRetryLocal' : 'editor.attachmentRetryDone';
}
