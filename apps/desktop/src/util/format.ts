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

/**
 * 备份产物的时间戳是核心从文件 mtime 造的紧凑 UTC 串（`20261007T091530Z`），
 * `new Date()` 不认它 —— 直接丢给 `formatWhen` 会**静默返回空串**，界面上那一格就没了时间。
 * 所以先转成 ISO；转不动就原样返回，宁可看见生串也不看见空白。
 */
export function stampToIso(value: string): string {
  const m = /^(\d{4})(\d{2})(\d{2})T(\d{2})(\d{2})(\d{2})Z$/.exec(value);
  if (!m) return value;
  return `${m[1]}-${m[2]}-${m[3]}T${m[4]}:${m[5]}:${m[6]}Z`;
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

/**
 * 编辑器角上那一格「改于 {时间}」：要的是**墙上时刻**，不是会自己老掉的相对时间。
 *
 * 与 `formatWhen` 分开是有原因的：那一格（§4.3 的"上一次：3 分钟前"）说的是"距今多久"，
 * 而这一格说的是一篇笔记最后一次落笔是几点 —— 用相对时间写它，屏幕上的字会自己变旧，
 * 而它下面那句「已存在本机」说的却是此刻的状态，两行对不上。
 * 同一天 `HH:MM`、同年 `M月D日`、跨年带年份；取不到时间给**空串**，调用方据此不画那一格
 * （不许替它编一个"刚刚"，那是 §4.3 那条口径的同一个道理）。
 */
export function formatModified(value: string | null | undefined, now: number = Date.now()): string {
  const date = toDate(value);
  if (!date) return '';
  const today = new Date(now);
  const md = `${date.getMonth() + 1}月${date.getDate()}日`;
  if (date.getFullYear() !== today.getFullYear()) return `${date.getFullYear()}年${md}`;
  if (date.getMonth() === today.getMonth() && date.getDate() === today.getDate()) {
    return `${pad(date.getHours())}:${pad(date.getMinutes())}`;
  }
  return md;
}

/**
 * 隔离区那一格要的是"还剩几天"，不是绝对时刻：字节要几十天之后才**有资格**离开磁盘，
 * 人对「3 月 7 日」没有紧迫感、也不容易发现自己看的是去年的日历。
 * 取不到或时间源读不出来 ⇒ **null**，调用方据此不画倒计时（宁可不报，也不猜一个数）。
 */
export function daysUntil(value: string | null | undefined, now: number = Date.now()): number | null {
  const date = toDate(value);
  if (!date) return null;
  return Math.max(0, Math.ceil((date.getTime() - now) / DAY));
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
