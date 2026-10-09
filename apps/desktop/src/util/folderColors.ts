/**
 * 侧栏文件夹的那一小块颜色（§6「颜色」，2026-10-09 用户拍板：**只做侧栏小色点**）。
 *
 * 为什么是一张**固定色板**而不是自由取色器：那颗点是用户给自己打的标记，
 * 一旦允许任意颜色，就会出现"在浅色主题上几乎看不见的那一支"，
 * 而要给它兜底就得上对比度判断 —— 一把只有 8 个、每个都核过的色板，
 * 是这件事最小的可验证形状。渲染上另有一圈 `--line-strong` 描边兜着，
 * 所以低饱和那几支也不会因为主题而消失。
 *
 * 核心那边只校字面形状（`#rrggbb` 或清空），**不认这张表** —— 对面设备同步来的颜色
 * 照样存得下、照样画（用 `dotStyle`），不在板上的值不当成坏数据。
 */
export interface FolderSwatch {
  /** 存在库里的值（`folders.color`）。统一小写：哈希与对比都只认一种形状。 */
  hex: string;
  /** 那颗点的可读名字（读屏与 `title` 都用它；漏登记会在屏幕上印出键名）。 */
  labelKey: string;
}

export const FOLDER_SWATCHES: readonly FolderSwatch[] = [
  { hex: '#c2410c', labelKey: 'sidebar.colorRed' },
  { hex: '#b45309', labelKey: 'sidebar.colorAmber' },
  { hex: '#a16207', labelKey: 'sidebar.colorYellow' },
  { hex: '#3f6212', labelKey: 'sidebar.colorGreen' },
  { hex: '#155e75', labelKey: 'sidebar.colorCyan' },
  { hex: '#1e40af', labelKey: 'sidebar.colorBlue' },
  { hex: '#6b21a8', labelKey: 'sidebar.colorPurple' },
  { hex: '#9d174d', labelKey: 'sidebar.colorPink' },
];

/** 那颗点怎么画：颜色照库里的值给（对面同步来的、不在这张板上的也要画得出来）。 */
export function dotStyle(hex: string | null | undefined): Record<string, string> | null {
  return hex ? { 'background-color': hex } : null;
}
