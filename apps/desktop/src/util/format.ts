/** 时间与体积的人类可读形式。 */

const MINUTE = 60 * 1000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

function toDate(value: string | null | undefined): Date | null {
  if (!value) return null;
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? null : date;
}

function pad(value: number): string {
  return value < 10 ? `0${value}` : String(value);
}

/** 显示用：只按"何时"排给人看，绝不参与任何新旧判定（判定在本地核心）。 */
export function formatWhen(value: string | null | undefined, now: number = Date.now()): string {
  const date = toDate(value);
  if (!date) return '';
  const diff = now - date.getTime();
  if (diff < 0) return date.toLocaleString('zh-CN', { hour12: false });
  if (diff < MINUTE) return '刚刚';
  if (diff < HOUR) return `${Math.floor(diff / MINUTE)} 分钟前`;
  if (diff < DAY) return `${Math.floor(diff / HOUR)} 小时前`;
  if (diff < 7 * DAY) return `${Math.floor(diff / DAY)} 天前`;
  const sameYear = date.getFullYear() === new Date(now).getFullYear();
  const md = `${date.getMonth() + 1}月${date.getDate()}日`;
  return sameYear ? `${md} ${pad(date.getHours())}:${pad(date.getMinutes())}` : `${date.getFullYear()}-${md}`;
}

export function formatBytes(value: number | null | undefined): string {
  if (typeof value !== 'number' || !Number.isFinite(value) || value < 0) return '—';
  if (value < 1024) return `${value} B`;
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KB`;
  if (value < 1024 * 1024 * 1024) return `${(value / (1024 * 1024)).toFixed(1)} MB`;
  return `${(value / (1024 * 1024 * 1024)).toFixed(2)} GB`;
}

export function formatNumber(value: number | null | undefined): string {
  if (typeof value !== 'number' || !Number.isFinite(value)) return '—';
  return value.toLocaleString('zh-CN');
}
