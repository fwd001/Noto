/**
 * 「这一篇是哪台设备改的」（规范 §6 第 12 格）。
 *
 * 为什么要一个纯函数：这一格只有三种答案 —— 没有来源（不画）、本机改的、另一台设备改的 ——
 * 而"另一台"那个短 id 怎么截、大小写怎么归一，是能被单测钉住的东西；
 * 把它留在组件里就只剩"看截图对不对"。
 *
 * **缺位不兜底**：`deviceId` 是空/缺失时回 `null`，界面那一格整条不出现 ——
 * 编一个"本机"或空串都是在替核心说话（同 `deviceIdentity.spec.ts` 那条口径）。
 */
export type DeviceTag = { kind: 'this' } | { kind: 'other'; short: string; full: string };

export function deviceTag(
  deviceId: string | null | undefined,
  myDeviceId: string | null | undefined,
): DeviceTag | null {
  const id = (deviceId ?? '').trim();
  if (id.length === 0) return null;
  const mine = (myDeviceId ?? '').trim();
  if (mine.length > 0 && id.toLowerCase() === mine.toLowerCase()) return { kind: 'this' };
  // 短 id：去掉连字符取前 8 位 —— 足够在"同一台服务器上的几台设备"之间区分，又不占地方。
  // 完整 id 仍带在 `full` 里（挂 `title`），免得"短"变成"看不清是谁"。
  const flat = id.replace(/-/g, '');
  return { kind: 'other', short: flat.slice(0, 8), full: id };
}
