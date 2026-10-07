/**
 * 图标系统 v2 的**唯一来源**（设计稿 §2）。
 *
 * 为什么禁 Unicode 字形（§2 的理由，这里落成机器可查的东西）：☰ ↻  ⇤  ☑ ▾ ✎ 这类字形
 * 依赖各平台字体回退 —— Windows 的 Segoe UI 与 macOS 的 SF Pro 形状必然不同，
 * 而本项目是"一份前端服务四端"。所以一律换成画出来的路径。
 *
 * 统一规格：20×20 画布、1.75px 描边（≈1:11.4，接近 Lucide/Feather）、round caps/joins、
 * 颜色继承 `currentColor`。**同屏图标必须同粗细** —— 混用 1.5 与 1.75 是"看着不专业"最常见的来源。
 *
 * ⚠️ 这些形状是按 §2 规格自绘的**占位实现**：Ardot 画布（WebGL）驱动不了缩放，拿不到设计师的
 * SVG 源文件。等 `docs/design/` 里有原图，替换这里的 `d` 即可，调用点一行都不用动。
 */

export interface IconSpec {
  /** 描边路径（fill=none）。实心整形或点阵图标可以完全没有它。 */
  d?: string[];
  /** 虚线描边路径（§4.1「有改动未落地」那一枚要的就是**虚线圆**，用 dasharray 画，不是猜四个弧）。 */
  dashed?: string[];
  /** 实心小圆（拖拽把手那种点阵）。 */
  dots?: Array<[number, number]>;
  /** 需要填充的整形（已置顶的星标）。 */
  solid?: string[];
}

export type IconName =
  | 'sync-synced' | 'sync-syncing' | 'sync-offline' | 'sync-failed' | 'sync-idle'
  | 'pin-on' | 'pin-off'
  | 'attach-file' | 'attach-image'
  | 'check' | 'drag-handle' | 'plus' | 'pencil' | 'chevron-down' | 'chevron-left' | 'chevron-right' | 'arrow-back'
  | 'indent-in' | 'indent-out' | 'trash' | 'warn' | 'rule'
  | 'list' | 'alert' | 'question' | 'menu' | 'settings' | 'undo' | 'redo'
  | 'save-saving' | 'save-dirty' | 'save-saved' | 'save-failed'
  | 'win-min' | 'win-max' | 'win-restore' | 'close';

/** 同步五格共用同一片云，只换内部徽标 —— 这样一眼看出是同步家族（§2.3）。 */
const CLOUD = 'M6.4 15h7.3a3.1 3.1 0 0 0 .6-6.1A4.3 4.3 0 0 0 6 8.3a3.1 3.1 0 0 0 .4 6.7Z';

/**
 * 保存四格共用同一张纸（§2.2 的载体规则：**保存 → 文档**），只换纸里那枚徽标 ——
 * 与云那一族同一个道理，一眼看出是"存盘"这一家四格，而不是四个互不相干的东西。
 */
const DOC = ['M5.8 3.4h4.6l3.8 3.8v9.4H5.8Z', 'M10.4 3.4v3.8h3.8'];

export const ICONS: Record<IconName, IconSpec> = {
  'sync-synced': { d: [CLOUD, 'M7.9 11.1l1.5 1.5 2.8-2.9'] },
  'sync-syncing': { d: [CLOUD, 'M8.1 10.6a2.3 2.3 0 1 1 .8 2.2', 'M8.1 8.9v1.8h1.8'] },
  'sync-offline': { d: [CLOUD, 'M7.6 11.4l1.3 1.2 1.3-1.2 1.3 1.2'] },
  'sync-failed': { d: [CLOUD, 'M10 8.6v3', 'M10 13.4h.01'] },
  'sync-idle': { d: [CLOUD, 'M8 11.4h4'] },

  // §4.1 保存四格：环形指示 / 虚线圆 / 实心勾 / 感叹号，都压在同一张纸上。
  'save-saving': { d: [...DOC, 'M11.2 11.5a2.1 2.1 0 1 0-2.1-2.1'] },
  'save-dirty': { d: DOC, dashed: ['M9.7 8.3a2.2 2.2 0 1 1 0 4.4 2.2 2.2 0 0 1 0-4.4'] },
  'save-saved': { d: [...DOC, 'M7.7 11.1l1.4 1.4 2.7-2.8'] },
  'save-failed': { d: [...DOC, 'M9.7 9.1v2.4', 'M9.7 13.3h.01'] },

  'pin-on': { solid: ['M10 3.6l2 4.1 4.5.6-3.3 3.1.8 4.5L10 13.8l-4 2.1.8-4.5-3.3-3.1 4.5-.6Z'] },
  'pin-off': { d: ['M10 3.9l1.9 3.9 4.3.6-3.1 3 .7 4.3-3.8-2-3.8 2 .7-4.3-3.1-3 4.3-.6Z'] },

  'attach-file': { d: ['M6 3.6h5.2L14.4 7v9.4H6Z', 'M11.2 3.6V7h3.2'] },
  'attach-image': { d: ['M3.8 5.4h12.4v9.2H3.8Z', 'M3.8 12.1l3.3-2.9 2.8 2.4 2.4-1.9 4.9 4', 'M7.3 8.5h.01'] },

  check: { d: ['M4.6 10.4l3.4 3.4 7.4-7.6'] },
  'drag-handle': { dots: [[7.6, 5.4], [12.4, 5.4], [7.6, 10], [12.4, 10], [7.6, 14.6], [12.4, 14.6]] },
  plus: { d: ['M10 4.6v10.8', 'M4.6 10h10.8'] },
  pencil: { d: ['M4.8 15.2l.9-3 7.5-7.5 2.1 2.1-7.5 7.5Z', 'M12.2 5.6l2.1 2.1'] },
  'chevron-down': { d: ['M5.6 8.2L10 12.6l4.4-4.4'] },
  // 侧栏那颗收/开把手以前是 `⟨`（U+27E8，数学括号）—— 字体回退下形状与粗细都不受控。
  'chevron-left': { d: ['M12.4 5.6L8 10l4.4 4.4'] },
  'chevron-right': { d: ['M7.6 5.6L12 10l-4.4 4.4'] },
  'arrow-back': { d: ['M12.4 4.8L6.6 10l5.8 5.2'] },
  'indent-in': { d: ['M10.4 6h5', 'M10.4 14h5', 'M4.6 10h3.2', 'M6.2 8l1.6 2-1.6 2'] },
  'indent-out': { d: ['M10.4 6h5', 'M10.4 14h5', 'M4.6 10h3.2', 'M7.8 8L6.2 10l1.6 2'] },
  trash: { d: ['M5.4 6.8h9.2', 'M7 6.8V4.9h6v1.9', 'M6.4 6.8l.7 8.3h5.8l.7-8.3'] },
  warn: { d: ['M10 4.2l6.3 11.2H3.7Z', 'M10 8.3v3.1', 'M10 13.6h.01'] },
  rule: { d: ['M4 10h12'] },
  list: { d: ['M4.4 6h11.2', 'M4.4 10h11.2', 'M4.4 14h7.6'] },
  alert: { d: ['M10 4.6v6.2', 'M10 14.6h.01'] },
  question: { d: ['M7.7 7.7a2.4 2.4 0 1 1 3.2 2.3c-.7.3-1 .8-1 1.6', 'M10 14.8h.01'] },
  menu: { d: ['M4.2 6.2h11.6', 'M4.2 10h11.6', 'M4.2 13.8h11.6'] },
  // 撤销 / 重做：§2.4 清单里那两枚（以前是 ↶ ↷，U+21B6/21B7 —— 图标门禁的黑名单没列到它们）。
  undo: { d: ['M8.4 5.6L4.8 9.2l3.6 3.6', 'M4.8 9.2h8a3.8 3.8 0 0 1 0 7.6H9.4'] },
  redo: { d: ['M11.6 5.6l3.6 3.6-3.6 3.6', 'M15.2 9.2h-8a3.8 3.8 0 0 0 0 7.6h3.4'] },
  settings: { d: ['M10 7.4a2.6 2.6 0 1 0 0 5.2 2.6 2.6 0 0 0 0-5.2Z', 'M10 3.4v2', 'M10 14.6v2', 'M3.4 10h2', 'M14.6 10h2', 'M5.3 5.3l1.4 1.4', 'M13.3 13.3l1.4 1.4', 'M14.7 5.3l-1.4 1.4', 'M6.7 13.3l-1.4 1.4'] },
  'win-min': { d: ['M4.6 13.4h10.8'] },
  'win-max': { d: ['M5.2 5.2h9.6v9.6H5.2Z'] },
  'win-restore': { d: ['M7.2 4.6h8.2v8.2', 'M4.6 7.2h8.2v8.2H4.6Z'] },
  close: { d: ['M5.6 5.6l8.8 8.8', 'M14.4 5.6L5.6 14.4'] },
};

/** 载体分组（§2.2）：不同语义必须用不同载体，绝不同组混用。这条表是给门禁用的清单。 */
export const CARRIERS: Record<string, IconName[]> = {
  cloud: ['sync-synced', 'sync-syncing', 'sync-offline', 'sync-failed', 'sync-idle'],
  star: ['pin-on', 'pin-off'],
  document: ['save-saving', 'save-dirty', 'save-saved', 'save-failed'],
  image: ['attach-image'],
  // ⚠️ 附件那两颗也是"纸"，与保存那一族同载体 —— §2.2 的撞脸规则在这里其实是被破了的。
  // 现在靠"纸里有没有徽标"分开（保存四格内部都有一枚状态徽标，附件这两颗没有），
  // 真分开要等设计师的 SVG。单列一组是为了不让它混进 `document` 那条基形判据里。
  attachment: ['attach-file'],
};

/**
 * 同步五格 → 图标（§2.3）。**唯一一份**：侧栏那条状态条与移动端底栏那颗 tab 读的是同一张表，
 * 两处各写一遍迟早会分叉（分叉的后果是"第五格在某一处看起来像在忙"，那是 §2.3 明令禁止的）。
 */
export const SYNC_ICONS: Record<string, IconName> = {
  synced: 'sync-synced',
  syncing: 'sync-syncing',
  offline: 'sync-offline',
  failed: 'sync-failed',
  // 未配置账户：静止那格。转不停的圈会被读成"正在忙"，而这里的事实是"没在同步"。
  idle: 'sync-idle',
};

/**
 * 保存四格 → 图标。键是 `stores/editor.ts` 算出来的 `saveKind`（**不是** `saveState`）：
 * 文案与图形必须读同一个派生值，否则会出现"写着正在保存、画着虚线圆"那种自相矛盾的读数。
 */
export const SAVE_ICONS: Record<string, IconName> = {
  saving: 'save-saving',
  dirty: 'save-dirty',
  saved: 'save-saved',
  error: 'save-failed',
};
