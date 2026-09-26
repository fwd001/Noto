/**
 * §5 探测结果的界面表达（SYNC-PROTOCOL §5 末行要求：S3 账户必须被标成
 * "服务器不支持并发保护，建议多设备串行编辑"）。
 *
 * 这里只做"把位图翻译成人话"，不做任何判定：策略是核心算好后随账户视图发下来的，
 * 前端不许自己从位图反推策略 —— 那等于把 §5 的表抄第二份，两份迟早漂移。
 */
import type { Account } from '../api/types';

/** 位值与 `notera_webdav::Caps` 逐一对齐（那才是写库的一侧）。 */
export const CAP_BITS = {
  strongEtag: 1 << 0,
  conditionalPut: 1 << 1,
  overwriteFMove: 1 << 2,
  depthInfinity: 1 << 3,
  range: 1 << 4,
  chunked: 1 << 5,
} as const;

export type CapName = keyof typeof CAP_BITS;

/** 芯片顺序 = 对用户的重要程度：并发保护在前，性能优化在后。 */
const CAP_ORDER: readonly CapName[] = ['conditionalPut', 'overwriteFMove', 'strongEtag', 'depthInfinity', 'range'];

export interface CapChip {
  name: CapName;
  /** 文案键，须在 i18n 表里登记（源码扫描的门禁会查）。 */
  labelKey: string;
  on: boolean;
}

/** 未知位一律忽略：核心以后加位时，界面少显示一项而不是显示错的。 */
export function capChips(mask: number | null | undefined): CapChip[] {
  if (typeof mask !== 'number') return [];
  return CAP_ORDER.map((name) => ({
    name,
    labelKey: `sync.cap.${name}`,
    on: (mask & CAP_BITS[name]) === CAP_BITS[name],
  }));
}

export type CapsState = 'unknown' | 'protected' | 'unprotected';

/**
 * `unknown` = 还没探过（与"探过了但不支持"是两句话，不能混着说）。
 * `unprotected` 只由核心下发的 `writeStrategy === 'S3'` 决定。
 */
export function capsState(account: Pick<Account, 'capMask' | 'writeStrategy'> | null | undefined): CapsState {
  if (!account || typeof account.capMask !== 'number') return 'unknown';
  return account.writeStrategy === 'S3' ? 'unprotected' : 'protected';
}
