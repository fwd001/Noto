/**
 * token 交接核对器：把设计师给的 tokens.css 拿来，逐条量它与本应用**当前这份契约**的落差。
 *
 * 存在的理由：应用不是"引用了一些颜色"，而是整层踩在 design token 上 —— 组件里
 * 66 个 `var(--x)` 调用点、`tokens.spec.ts` 用真值算 WCAG 对比度、还禁止 webfont 与硬编码色值。
 * 换掉这个文件因此不是"覆盖一下"就完事，历史上这类替换静默丢过东西（prefers-reduced-motion
 * 那一块就写在 tokens.css 里，整文件替换会把它一起删掉而没人报警）。
 *
 * 六条判据各钉一件事：
 *  A 界面正在消费的 token 一个都不许缺（缺了 = 那一处静默回退成无值，画出来是"没样式"）；
 *  B 深浅两套的对比度仍要够（沿用 tokens.spec.ts 那 7 组配色对，不另立口径）；
 *  C 不许引 webfont（离线可用是硬要求，字体只能系统栈）；
 *  D `--touch` 不许低于 44px（底线约束）；
 *  E 现行 tokens.css 里那些**结构性规则**（减少动效、两套主题都有）不许在新文件里消失；
 *  F 顺手列出"多出来的"和"取值漂移的"，供人决定改调用点还是改映射 —— 只报事实，不替人拍板。
 */
import { readFileSync, readdirSync, statSync, existsSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const read = (p) => readFileSync(path.join(repoRoot, p), 'utf8');

const target = process.argv[2];
if (!target) {
  console.error('用法：node scripts/check-tokens-intake.mjs <候选 tokens.css 路径>');
  console.error('  正对照（拿现役文件量自己）：node scripts/check-tokens-intake.mjs apps/desktop/src/styles/tokens.css');
  process.exit(2);
}
const targetAbs = path.isAbsolute(target) ? target : path.join(repoRoot, target);
if (!existsSync(targetAbs)) {
  console.error(`读不到候选文件：${targetAbs}`);
  process.exit(2);
}

const CURRENT = 'apps/desktop/src/styles/tokens.css';
const SRC = 'apps/desktop/src';

function walk(dir, out = []) {
  for (const name of readdirSync(dir)) {
    const full = path.join(dir, name);
    if (statSync(full).isDirectory()) walk(full, out);
    else if (/\.(css|vue|ts)$/.test(name) && !/\.spec\.ts$/.test(name)) out.push(full);
  }
  return out;
}

/** 注释里的 `var(--x)` 不是消费点（`editor/dom.ts` 里那句散文就害过我一次）。 */
function srcTexts() {
  return walk(path.join(repoRoot, SRC)).map((file) => ({
    rel: path.relative(repoRoot, file).replace(/\\/g, '/'),
    abs: file,
    text: readFileSync(file, 'utf8').replace(/\/\*[\s\S]*?\*\//g, '').replace(/^[ \t]*\/\/.*$/gm, ''),
  }));
}

const SOURCES = srcTexts();

/** 有些 token 本来就不该由 tokens.css 提供，见下方 providedOutsideTokens 的注释。 */
function providedOutsideTokens(excludeAbs) {
  const out = new Set();
  for (const s of SOURCES) {
    if (path.resolve(s.abs) === excludeAbs) continue;
    for (const m of s.text.matchAll(/(--[\w-]+)\s*:/g)) out.add(m[1]);
    for (const m of s.text.matchAll(/['"](--[\w-]+)['"]/g)) out.add(m[1]);
  }
  return out;
}

/** 界面消费的 token 名 -> 调用点清单（tokens.css 自己不算消费者，它就是被换的那个文件）。 */
function consumedTokens() {
  const map = new Map();
  for (const s of SOURCES) {
    if (path.resolve(s.abs) === path.resolve(repoRoot, CURRENT)) continue;
    for (const m of s.text.matchAll(/var\((--[\w-]+)/g)) {
      const name = m[1];
      if (!map.has(name)) map.set(name, []);
      map.get(name).push(s.rel);
    }
  }
  return map;
}

/** 把 CSS 里所有 `选择器 { … }` 的自定义属性收下来（注释与嵌套花括号先抹平）。 */
function parseVars(css) {
  const stripped = css.replace(/\/\*[\s\S]*?\*\//g, '');
  const blocks = [];
  for (const m of stripped.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
    const selector = m[1].trim().replace(/^@media[^{]*$/i, '');
    const body = m[2];
    const vars = {};
    for (const d of body.matchAll(/(--[\w-]+)\s*:\s*([^;]+);/g)) vars[d[1]] = d[2].trim();
    if (Object.keys(vars).length) blocks.push({ selector, vars });
  }
  return blocks;
}

function themeBuckets(css) {
  const buckets = { shared: {}, light: {}, dark: {} };
  for (const block of parseVars(css)) {
    const s = block.selector;
    const key = /dark/.test(s) ? 'dark' : /light/.test(s) ? 'light' : /:root|html|\*/.test(s) ? 'shared' : null;
    if (!key) continue;
    Object.assign(buckets[key], block.vars);
  }
  return buckets;
}

/** 值里可能再引用别的 token（别名写法），要能解开才能算对比度。 */
function resolve(value, buckets) {
  let out = value;
  for (let i = 0; i < 8; i += 1) {
    const m = out.match(/^var\((--[\w-]+)(?:\s*,\s*([^()]*))?\)$/);
    if (!m) break;
    const found = buckets[m[1]] ?? m[2];
    if (found === undefined) break;
    out = found;
  }
  return out.trim();
}

function channel(v) {
  const s = v / 255;
  return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
}

function luminance(hex) {
  const digits = hex.replace('#', '');
  const expanded = digits.length === 3 ? digits.split('').map((c) => c + c).join('') : digits.slice(0, 6);
  const r = channel(Number.parseInt(expanded.slice(0, 2), 16));
  const g = channel(Number.parseInt(expanded.slice(2, 4), 16));
  const b = channel(Number.parseInt(expanded.slice(4, 6), 16));
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function contrast(a, b) {
  const la = luminance(a);
  const lb = luminance(b);
  return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05);
}

const HEX = /^#([0-9a-fA-F]{3}|[0-9a-fA-F]{6})$/;

const failures = [];
const notes = [];
/**
 * 配色对 = 界面真会画的组合（v2 词汇）。
 * 前三组带文档给的期望值（设计稿 §1.1 那张表），其余只钉阈值 —— 文档没列但界面会画。
 */
const PAIRS = [
  ['--ink', '--canvas', 7],
  ['--ink', '--surface', 7],
  ['--ink', '--sunken', 7],
  ['--body', '--canvas', 7],
  ['--mute', '--canvas', 4.5],
  ['--mute', '--sunken', 4.5],
  ['--on-accent', '--accent', 4.5],
  ['--accent', '--canvas', 4.5],
  ['--danger', '--canvas', 4.5],
  ['--warn', '--canvas', 4.5],
  ['--ok', '--canvas', 4.5],
  ['--ink', '--mark-bg', 7],
];

const candidateCss = readFileSync(targetAbs, 'utf8');
const candidate = themeBuckets(candidateCss);
const current = themeBuckets(read(CURRENT));
const consumed = consumedTokens();

// A —— 消费中的 token 不许缺
const outside = providedOutsideTokens(path.resolve(repoRoot, CURRENT));
const missing = [];
let fromElsewhere = 0;
for (const [name, sites] of consumed) {
  if (candidate.shared[name] !== undefined || candidate.light[name] !== undefined || candidate.dark[name] !== undefined) continue;
  if (outside.has(name)) {
    fromElsewhere += 1;
    continue;
  }
  missing.push({ name, count: sites.length, sample: [...new Set(sites)].slice(0, 3).join(', ') });
}
if (missing.length) {
  failures.push(`A 界面在消费、候选文件却没定义的 token 有 ${missing.length} 个：\n` +
    missing.map((m) => `    ${m.name}（${m.count} 处，如 ${m.sample}）`).join('\n'));
} else {
  notes.push(`A 界面消费的 ${consumed.size} 个 token 全部有出处` +
    `（候选文件提供 ${consumed.size - fromElsewhere} 个，别处提供 ${fromElsewhere} 个：base.css 默认值或运行时改写）`);
}

// B —— 对比度沿用 tokens.spec.ts 那组配色对
for (const theme of ['light', 'dark']) {
  const buckets = { ...candidate.shared, ...candidate[theme] };
  const line = [];
  for (const [fg, bg, min] of PAIRS) {
    const f = resolve(buckets[fg] ?? '', buckets);
    const b = resolve(buckets[bg] ?? '', buckets);
    if (!HEX.test(f) || !HEX.test(b)) {
      failures.push(`B ${theme} 配色对 ${fg} on ${bg} 算不出来（值不是十六进制色：${JSON.stringify({ f, b })}），门禁要求 ≥${min}`);
      continue;
    }
    const r = contrast(f, b);
    if (r < min) failures.push(`B ${theme} 配色对 ${fg} on ${bg} 只有 ${r.toFixed(2)}:1，要求 ≥${min}`);
    else line.push(`${fg}/${bg.replace('--bg-', '')}=${r.toFixed(1)}${r < min + 1 ? '⚠' : ''}`);
  }
  if (line.length) notes.push(`B ${theme} 对比度实测：${line.join(' ')}`);
}

// C —— 不许 webfont
if (/@font-face|url\(/.test(candidateCss)) failures.push('C 候选文件里有 @font-face 或 url( —— 离线可用是硬要求，字体只能用系统栈');
else notes.push('C 没有 webfont 引用（系统字体栈这条守住了）');

// D —— 触摸目标
const touch = candidate.shared['--touch'] ?? candidate.light['--touch'];
if (touch === undefined) failures.push(`D 候选文件没有 --touch（界面 ${consumed.get('--touch-min')?.length ?? 0} 处靠它撑 44px 命中区）`);
else if (Number.parseFloat(touch) < 44) failures.push(`D --touch 是 ${touch}，底线是 44px`);
else notes.push(`D --touch = ${touch}`);

// E —— 结构性规则不许随着整文件替换消失
const currentCss = read(CURRENT);
for (const [label, re] of [
  ['prefers-reduced-motion 那一块', /prefers-reduced-motion/],
  ['减少动效时把时长归零', /--dur-fast:\s*0ms/],
  ['深浅两套主题各自成块', /\[data-theme=['"]?dark/],
]) {
  if (re.test(currentCss) && !re.test(candidateCss)) failures.push(`E 现行 tokens.css 有「${label}」，候选文件里没有 —— 整文件替换会静默删掉它`);
}
notes.push('E 结构性规则（减少动效 / 两套主题）比对完');

// F —— 多出来与取值漂移
const candidateAll = new Set([...Object.keys(candidate.shared), ...Object.keys(candidate.light), ...Object.keys(candidate.dark)]);
const currentAll = new Set([...Object.keys(current.shared), ...Object.keys(current.light), ...Object.keys(current.dark)]);
const extra = [...candidateAll].filter((k) => !currentAll.has(k));
const drift = [];
for (const k of candidateAll) {
  if (!currentAll.has(k)) continue;
  for (const theme of ['shared', 'light', 'dark']) {
    if (candidate[theme][k] !== undefined && current[theme][k] !== undefined && candidate[theme][k] !== current[theme][k]) {
      drift.push(`${k}@${theme}: ${current[theme][k]} → ${candidate[theme][k]}`);
    }
  }
}
notes.push(`F 候选多出的 token ${extra.length} 个${extra.length ? `：${extra.slice(0, 12).join(' ')}${extra.length > 12 ? ' …' : ''}` : ''}`);
notes.push(`F 同名取值漂移 ${drift.length} 处${drift.length ? `，前 10：\n    ${drift.slice(0, 10).join('\n    ')}` : ''}`);

console.log(`候选：${path.relative(repoRoot, targetAbs).replace(/\\/g, '/')}｜现役消费：${consumed.size} 个 token`);
for (const n of notes) console.log(`  ${n}`);
if (failures.length) {
  console.log(`\n落差 ${failures.length} 条：`);
  for (const f of failures) console.log(`  ✗ ${f}`);
  console.log('\n>>> token 交接核对 FAIL');
  process.exit(1);
}
console.log('\n>>> token 交接核对 PASS');
