import type { MessageKey } from '../i18n';

/**
 * 一颗附件在这台设备上"此刻能不能用"，以及**该说哪一句**（UI-REDESIGN-BRIEF §2.5、§3 附件那两行）。
 *
 * 存在的理由：读侧此前只有图片那一支能表达"这份字节不在这台设备上"（靠"取字节失败 ⇒ 没有 URL
 * ⇒ 画占位"这个副作用），而文件附件那颗芯片只认识**本次会话刚上传过**的对象 ——
 * 从别的设备同步来、本机还没字节的附件于是画起来跟完好的一样，既不说明缺、也不给那两颗已存在的动作。
 *
 * 这里只做**读账 → 句子**这一格映射，并且刻意只分两档：
 *  - 「正在等待下载」这句话只有在"本机没有、服务器上有"时才是真话 —— 队列确实会去要它；
 *  - 其余所有"本机不可用"（没有 / 只有一半 / 校验不过 / 连账都没有）合用一句中性说法。
 *    为什么不各写一句：`error`（本机有字节但哈希对不上）与 `missing` 的**用户动作**不同，
 *    但那颗动作按钮的判据属于核心（`attachment_retry` / `attachment_reupload` 各自会拒并说明理由），
 *    在前端复制一台状态机就是 §39 禁的那件事。句子先说实话，细节交给按钮的拒绝语。
 */
export type LocalState = 'missing' | 'partial' | 'available' | 'error';
export type RemoteState = 'unknown' | 'absent' | 'present' | 'error';

/** 账上那一对的读侧形状（核心 `attachment_states` 的那一个 DTO）。 */
export interface LedgerPair {
  localState: string;
  remoteState: string;
}

/** 本机有没有这份可用的字节。`available` 是唯一说"有"的取值；未知（还没问过）不算有。 */
export function localUsable(localState: string | undefined): boolean {
  return localState === 'available';
}

/**
 * 该说的那一句（文案键），或 `null` 表示没什么要说。
 *
 * **未知必须回 null**：笔记刚打开、那次读账还在飞的时候，如果按"没有"处理，
 * 每一颗附件都会先闪一下"缺"再收回去 —— 那是把没发生的事报给用户。
 */
export function attachmentNotice(pair: LedgerPair | undefined | null): MessageKey | null {
  if (!pair) return null;
  if (localUsable(pair.localState)) return null;
  if (pair.localState === 'missing' && pair.remoteState === 'present') return 'editor.attachmentMissing';
  return 'editor.attachmentNotOnDevice';
}
