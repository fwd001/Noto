import { describe, expect, it } from 'vitest';
import tokensCss from './tokens.css?raw';
import baseCss from './base.css?raw';
import editorCss from './editor.css?raw';

/**
 * design token 契约测试：
 *  1. 深浅两套正文对比度都 ≥ 7:1（WCAG AAA）；
 *  2. 组件样式里不得出现硬编码颜色，只能引用变量；
 *  3. 点击目标 ≥ 44px。
 * 直接从 tokens.css 取真实值算，不用截图。
 */

type Vars = Record<string, string>;

function blocks(css: string): Array<{ selector: string; vars: Vars }> {
  const stripped = css.replace(/\/\*[\s\S]*?\*\//g, '');
  const out: Array<{ selector: string; vars: Vars }> = [];
  for (const match of stripped.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
    const selector = (match[1] ?? '').trim();
    const body = match[2] ?? '';
    if (!selector.includes(':root') && !selector.includes('[data-theme')) continue;
    const vars: Vars = {};
    for (const declaration of body.matchAll(/(--[\w-]+)\s*:\s*([^;]+);/g)) {
      const name = declaration[1];
      const value = declaration[2];
      if (name && value) vars[name] = value.trim();
    }
    out.push({ selector, vars });
  }
  return out;
}

const parsed = blocks(tokensCss);

function themeVars(matcher: (selector: string) => boolean): Vars {
  const merged: Vars = {};
  for (const block of parsed) {
    if (matcher(block.selector)) Object.assign(merged, block.vars);
  }
  return merged;
}

const light = themeVars((selector) => selector.includes('[data-theme=\'light\']'));
const dark = themeVars((selector) => selector.includes('[data-theme=\'dark\']'));
const shared = themeVars((selector) => selector === ':root');

function channel(value: number): number {
  const scaled = value / 255;
  return scaled <= 0.03928 ? scaled / 12.92 : ((scaled + 0.055) / 1.055) ** 2.4;
}

function luminance(hex: string): number {
  const digits = hex.replace('#', '');
  const expanded = digits.length === 3 ? digits.split('').map((c) => c + c).join('') : digits.slice(0, 6);
  const r = channel(Number.parseInt(expanded.slice(0, 2), 16));
  const g = channel(Number.parseInt(expanded.slice(2, 4), 16));
  const b = channel(Number.parseInt(expanded.slice(4, 6), 16));
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function contrast(foreground: string, background: string): number {
  const first = luminance(foreground);
  const second = luminance(background);
  return (Math.max(first, second) + 0.05) / (Math.min(first, second) + 0.05);
}

function ratio(vars: Vars, foregroundKey: string, backgroundKey: string): number {
  const foreground = vars[foregroundKey];
  const background = vars[backgroundKey];
  expect(foreground, `${foregroundKey} 未定义`).toBeDefined();
  expect(background, `${backgroundKey} 未定义`).toBeDefined();
  return contrast(foreground ?? '#000000', background ?? '#ffffff');
}

describe.each([
  ['浅色', light],
  ['深色', dark],
])('%s 主题对比度', (_label, vars) => {
  it('正文 ≥ 7:1（AAA）', () => {
    expect(ratio(vars, '--text-primary', '--bg-pane')).toBeGreaterThanOrEqual(7);
    expect(ratio(vars, '--text-primary', '--bg-canvas')).toBeGreaterThanOrEqual(7);
    expect(ratio(vars, '--text-primary', '--bg-sunken')).toBeGreaterThanOrEqual(7);
    expect(ratio(vars, '--text-primary', '--bg-raised')).toBeGreaterThanOrEqual(7);
  });

  it('次级文字也 ≥ 7:1，弱文字 ≥ 4.5:1', () => {
    expect(ratio(vars, '--text-secondary', '--bg-pane')).toBeGreaterThanOrEqual(7);
    expect(ratio(vars, '--text-muted', '--bg-pane')).toBeGreaterThanOrEqual(4.5);
  });

  it('徽标色与按钮前景色 ≥ 4.5:1，链接可读', () => {
    expect(ratio(vars, '--text-on-accent', '--accent')).toBeGreaterThanOrEqual(4.5);
    expect(ratio(vars, '--text-link', '--bg-pane')).toBeGreaterThanOrEqual(4.5);
    expect(ratio(vars, '--text-primary', '--bg-highlight')).toBeGreaterThanOrEqual(7);
  });
});

describe('token 结构约束', () => {
  it('两套主题都定义了同一批变量', () => {
    const keys = Object.keys(light);
    expect(keys.length).toBeGreaterThan(12);
    for (const key of keys) expect(dark[key], `${key} 在深色里缺失`).toBeDefined();
  });

  it('字号缩放范围与共享 token 一致', () => {
    expect(shared['--touch-min']).toBe('44px');
    expect(shared['--editor-font-scale']).toBe('1');
  });

  it('组件样式里不出现硬编码颜色，只允许引用变量', () => {
    const combined = `${baseCss}\n${editorCss}`;
    const hex = combined.match(/#[0-9a-fA-F]{3,8}\b/g) ?? [];
    expect(hex).toEqual([]);
    const functional = combined.match(/rgba?\(/g) ?? [];
    expect(functional).toEqual([]);
    expect(combined).toContain('var(--');
  });

  it('按钮/输入/徽标的最小命中尺寸来自 token', () => {
    expect(baseCss).toMatch(/\.btn\s*\{[\s\S]*?min-height:\s*var\(--touch-min\)/);
    expect(baseCss).toMatch(/\.input[\s\S]*?min-height:\s*var\(--touch-min\)/);
    expect(baseCss).toMatch(/\.badge\s*\{[\s\S]*?min-height:\s*var\(--touch-min\)/);
  });

  it('尊重系统"减少动效"', () => {
    expect(tokensCss).toContain('prefers-reduced-motion');
    expect(tokensCss).toMatch(/--dur-fast:\s*0ms/);
  });

  it('字体只用系统字体栈', () => {
    expect(tokensCss).toMatch(/--font-ui:[^;]*-apple-system/);
    expect(tokensCss).toMatch(/--font-ui:[^;]*Segoe UI/);
    expect(tokensCss).not.toMatch(/@font-face|url\(/);
  });
});
