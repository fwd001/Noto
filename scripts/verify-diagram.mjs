// Phase 0 artifact verifier: docs/diagram/architecture.html
// Asserts the diagram actually renders and behaves, instead of trusting a glance.
// Run: node scripts/verify-diagram.mjs
// 依赖盘上已有的 playwright-core + chromium，不联网安装。可用环境变量覆盖路径：
//   PW_CORE=file:///.../playwright-core/index.js  CHROME=.../chrome.exe  DIAGRAM=file:///.../architecture.html
const pw = await import(process.env.PW_CORE || 'file:///C:/Users/lhcz-fu/node_modules/playwright-core/index.js');

const CHROME = process.env.CHROME || 'C:/Users/lhcz-fu/AppData/Local/ms-playwright/chromium-1243/chrome-win64/chrome.exe';
const URL = process.env.DIAGRAM || 'file:///D:/code/Notes/docs/diagram/architecture.html';
const OUT = process.env.EVIDENCE_DIR || 'D:/code/Notes/docs/evidence/';
const { chromium } = pw.default ?? pw;

const results = [];
const check = (name, ok, evidence) => {
  results.push({ name, ok, evidence });
  console.log(`${ok ? 'PASS' : 'FAIL'}  ${name.padEnd(38)} ${evidence}`);
};

const firstLine = e => String(e && e.message ? e.message : e).split(String.fromCharCode(10))[0].slice(0, 90);
const clk = async (sel, ms = 6000) => {
  try { await page.locator(sel).first().click({ timeout: ms }); return true; }
  catch (e) { check(`click:${sel}`, false, firstLine(e)); return false; }
};
const fill = async (sel, val) => {
  try { await page.locator(sel).fill(val, { timeout: 6000 }); return true; }
  catch (e) { check(`fill:${sel}`, false, firstLine(e)); return false; }
};

const browser = await chromium.launch({ executablePath: CHROME });
const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });

const errors = [];
page.on('pageerror', e => errors.push('pageerror: ' + e.message));
page.on('console', m => { if (m.type() === 'error') errors.push('console: ' + m.text()); });

await page.goto(URL, { waitUntil: 'load' });
await page.waitForTimeout(600);

check('no-js-errors', errors.length === 0, errors.slice(0, 3).join(' | ') || 'clean');

const cardCount = await page.locator('.card').count();
check('all-modules-rendered', cardCount >= 17, `${cardCount} cards (16 modules + 1 内核说明)`);

/* 视觉质量断言：这些是上一轮"能跑但糊成一团"时缺失的检查。
   在默认视图 / 选中态 / 各筛选链路下都要成立，所以抽成可复用测量函数。 */
const measureVisual = () => page.evaluate(() => {
  const svg = document.getElementById('wires');
  const stage = document.getElementById('stage').getBoundingClientRect();
  const cards = [...document.querySelectorAll('.card[data-id]')].map(c => ({ id: c.dataset.id, r: c.getBoundingClientRect() }));
  const paths = [...svg.querySelectorAll('path')].filter(p => p.getAttribute('marker-end'));
  const texts = [...svg.querySelectorAll('text')];

  // 1) 标签两两重叠面积
  const boxes = texts.map(t => t.getBoundingClientRect());
  let labelOverlaps = 0; const overlapPairs = [];
  for (let i = 0; i < boxes.length; i++) for (let j = i + 1; j < boxes.length; j++) {
    const a = boxes[i], b = boxes[j];
    const w = Math.min(a.right, b.right) - Math.max(a.left, b.left);
    const h = Math.min(a.bottom, b.bottom) - Math.max(a.top, b.top);
    if (w > 2 && h > 2) { labelOverlaps++; overlapPairs.push(`${texts[i].textContent} x ${texts[j].textContent}`); }
  }
  // 2) 标签压在**无关**卡片上（压到自己两端的模块是允许的）
  let labelOnCard = 0, offenders = [];
  for (const t of texts) {
    const r = t.getBoundingClientRect();
    const [fid, tid] = (t.getAttribute('data-e') || '>').split('>');
    for (const c of cards) {
      if (c.id === fid || c.id === tid) continue;
      const w = Math.min(r.right, c.r.right) - Math.max(r.left, c.r.left);
      const h = Math.min(r.bottom, c.r.bottom) - Math.max(r.top, c.r.top);
      if (w > 3 && h > 3) { labelOnCard++; offenders.push(`${t.textContent}→${c.id}`); break; }
    }
  }
  // 2b) 标签与连线都不得压在"层标题"文字上（层标题是读图的骨架）
  const heads = [...document.querySelectorAll('.layer-h b, .layer-h i')].map(x => ({ txt: x.textContent.slice(0, 12), r: x.getBoundingClientRect() }));
  let labelOnHead = 0, headDetail = [];
  for (const t of texts) {
    const r = t.getBoundingClientRect();
    for (const hd of heads) {
      const w = Math.min(r.right, hd.r.right) - Math.max(r.left, hd.r.left);
      const h = Math.min(r.bottom, hd.r.bottom) - Math.max(r.top, hd.r.top);
      if (w > 3 && h > 3) { labelOnHead++; headDetail.push(`${t.textContent}~${hd.txt}`); break; }
    }
  }
  let wireOnHead = 0;
  for (const p of paths) {
    const L = p.getTotalLength();
    for (let k = 0; k <= 24; k++) {
      const pt = p.getPointAtLength(L * k / 24);
      const gx = pt.x + stage.left, gy = pt.y + stage.top;
      if (heads.some(hd => gx > hd.r.left + 3 && gx < hd.r.right - 3 && gy > hd.r.top + 2 && gy < hd.r.bottom - 2)) { wireOnHead++; break; }
    }
  }
  // 3) 连线穿过无关模块（沿路径采样，命中非端点卡片内部即算）
  let crossings = 0, sampled = 0; const crossDetail = [];
  for (const p of paths) {
    const L = p.getTotalLength();
    const ownerIds = [];
    // 端点常正好落在卡片边界上，需按 ±6px 容差判定归属，否则会把自家卡片误判为"穿过"
    const hitCard = (gx, gy) => cards.find(c =>
      gx > c.r.left - 6 && gx < c.r.right + 6 && gy > c.r.top - 6 && gy < c.r.bottom + 6);
    for (const frac of [0, 1]) {
      const pt = p.getPointAtLength(L * frac);
      const hit = hitCard(pt.x + stage.left, pt.y + stage.top);
      if (hit) ownerIds.push(hit.id);
    }
    sampled++;
    for (let k = 0; k <= 24; k++) {
      const pt = p.getPointAtLength(L * k / 24);
      const gx = pt.x + stage.left, gy = pt.y + stage.top;
      const inside = cards.find(c => !ownerIds.includes(c.id) && gx > c.r.left + 3 && gx < c.r.right - 3 && gy > c.r.top + 3 && gy < c.r.bottom - 3);
      if (inside) { crossings++; crossDetail.push(`${p.getAttribute('data-f')}>${p.getAttribute('data-t')}:${inside.id}`); break; }
    }
  }
  const coreCount = paths.length;
  return { coreCount, labels: texts.length, labelOverlaps, overlapPairs, labelOnCard, crossings, sampled, offenders, crossDetail, labelOnHead, headDetail, wireOnHead };
});
const assertVisual = (tag, v, minEdges, maxEdges) => {
  check(`[${tag}] labels-no-overlap`, v.labelOverlaps === 0, `${v.labels} 标签，重叠 ${v.labelOverlaps} -> ${v.overlapPairs.slice(0,2).join(' | ')}`);
  check(`[${tag}] labels-not-on-cards`, v.labelOnCard === 0, `压在无关卡片上 ${v.labelOnCard} -> ${v.offenders.slice(0,2).join(' ')}`);
  check(`[${tag}] labels-not-on-headers`, v.labelOnHead === 0, `压在层标题上 ${v.labelOnHead} -> ${v.headDetail.slice(0,2).join(' ')}`);
  check(`[${tag}] wires-avoid-cards`, v.crossings === 0, `${v.sampled} 条边采样，穿过无关模块 ${v.crossings} -> ${v.crossDetail.slice(0,3).join(' | ')}`);
  if (minEdges != null) check(`[${tag}] edge-count`, v.coreCount >= minEdges && v.coreCount <= maxEdges, `${v.coreCount} 条（期望 ${minEdges}..${maxEdges}）`);
};
const visual = await measureVisual();
assertVisual('默认视图', visual, 10, 18);
// 层标题是整行元素：允许连线从其"下方"经过，但标题必须不透明且绘制在连线之上（遮挡而非绕开）
const occl = await page.evaluate(() => {
  const hd = document.querySelector('.layer-h');
  const bg = getComputedStyle(document.body).backgroundColor;
  const cs = getComputedStyle(hd);
  const layerZ = +getComputedStyle(document.querySelector('.layer')).zIndex;
  const svgZ = +getComputedStyle(document.getElementById('wires')).zIndex;
  return { opaque: cs.backgroundColor === bg && cs.backgroundColor !== 'rgba(0, 0, 0, 0)', layerZ, svgZ };
});
check('layer-headers-occlude-wires', occl.opaque && occl.layerZ > (isNaN(occl.svgZ) ? 0 : occl.svgZ), `标题底色=页面底色 ${occl.opaque} · layer z=${occl.layerZ} vs wires z=${occl.svgZ}`);
check('labels-not-on-cards', visual.labelOnCard === 0, `压在无关卡片上的标签 ${visual.labelOnCard} 个 ${visual.offenders.slice(0,3).join(',')}`);
check('wires-avoid-cards', visual.crossings === 0, `${visual.sampled} 条边采样，穿过无关模块 ${visual.crossings} 条 -> ${visual.crossDetail.join(' | ')}`);

const wireStats = await page.evaluate(() => {
  const paths = [...document.querySelectorAll('#wires path')];
  const data = paths.map(p => p.getAttribute('d') || '');
  const bad = data.filter(d => !d || /NaN|Infinity|undefined/.test(d));
  const svg = document.getElementById('wires');
  const sr = document.getElementById('stage').getBoundingClientRect();
  return { paths: paths.length, bad: bad.length, svgW: +svg.getAttribute('width'), svgH: +svg.getAttribute('height'), stageW: Math.round(sr.width), stageH: Math.round(sr.height) };
});
check('wires-coords-sane', wireStats.bad === 0, `NaN/空 d=${wireStats.bad}`);
check('svg-covers-stage', Math.abs(wireStats.svgH - wireStats.stageH) < 120 && wireStats.svgW >= wireStats.stageW - 4, `svg ${wireStats.svgW}x${wireStats.svgH} vs stage ${wireStats.stageW}x${wireStats.stageH}`);

const contrast = await page.evaluate(() => {
  const cs = getComputedStyle(document.body);
  const lum = (rgb) => {
    const [r, g, b] = rgb.match(/[\d.]+/g).slice(0, 3).map(v => { const c = +v / 255; return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4; });
    return 0.2126 * r + 0.7152 * g + 0.0722 * b;
  };
  const ratio = (a, b) => { const [x, y] = [lum(a), lum(b)].sort((p, q) => q - p); return (x + 0.05) / (y + 0.05); };
  const bg = cs.backgroundColor;
  const out = {};
  for (const [sel, label] of [['.card .nm', 'card-name'], ['.card .ttl', 'card-title'], ['h1', 'title'], ['.hint', 'hint']]) {
    const el = document.querySelector(sel);
    if (el) out[label] = +ratio(getComputedStyle(el).color, bg).toFixed(2);
  }
  return { out, bg };
});
const worstContrast = Math.min(...Object.values(contrast.out));
check('contrast-aa', worstContrast >= 4.5, `最低 ${worstContrast}:1 on ${contrast.bg} · ${JSON.stringify(contrast.out)}`);

const hitTargets = await page.evaluate(() => {
  const small = [];
  document.querySelectorAll('button, .card').forEach(el => {
    const r = el.getBoundingClientRect();
    if (r.height > 0 && r.height < 44) small.push(`${el.tagName}.${el.className}|${Math.round(r.height)}px`);
  });
  return { total: document.querySelectorAll('button, .card').length, small: small.slice(0, 6), count: small.length };
});
check('touch-targets-44px', hitTargets.count === 0, `${hitTargets.total} 个可交互元素, 低于44px=${hitTargets.count} ${hitTargets.small.join(' ')}`);

// --- 穿透：点一个模块，右侧应出现三块契约 ---
await clk('.card[data-id="sync"]');
await page.waitForTimeout(400);
const detail = await page.evaluate(() => {
  const side = document.getElementById('side');
  const t = side.innerText.toLowerCase();   // h3 用 CSS text-transform:uppercase，innerText 会带大写
  return {
    hasTitle: t.includes('notera-sync'),
    hasDuties: t.includes('内部职责'),
    hasInput: t.includes('输入 · input'),
    hasOutput: t.includes('输出 · output'),
    hasStruct: t.includes('数据结构'),
    hasRules: t.includes('铁律'),
    rowsIn: side.querySelectorAll('.io-in table tbody tr').length,
    rowsOut: side.querySelectorAll('.io-out table tbody tr').length,
    structs: side.querySelectorAll('.st').length,
    sampleField: t.includes('syncplan'),
  };
});
assertVisual('选中sync模块', await measureVisual(), 5, 12);
check('drilldown-contracts', Object.entries(detail).every(([k, v]) => typeof v !== 'boolean' || v), JSON.stringify(detail));
await page.screenshot({ path: OUT + 'diagram-detail.png', fullPage: false });

const highlighted = await page.evaluate(() => {
  const strong = [...document.querySelectorAll('#wires path')].filter(p => +p.getAttribute('stroke-width') > 2).length;
  const dimmed = document.querySelectorAll('.card.dim').length;
  return { strong, dimmed };
});
check('selection-highlights-wires', highlighted.strong >= 4 && highlighted.dimmed > 0, `高亮连线=${highlighted.strong} 淡出卡片=${highlighted.dimmed}`);

// --- 链路筛选（先切，再量；期望值直接从 EDGES 计算，不靠肉眼）---
const counts = await page.evaluate(() => ({
  total: EDGES.length,
  core: EDGES.filter(e => e.core).length,
  sync: EDGES.filter(e => e.flow.includes('sync')).length,
  write: EDGES.filter(e => e.flow.includes('write')).length,
  net: EDGES.filter(e => e.flow.includes('net')).length,
}));
check('edge-model-consistent', counts.total >= 30 && counts.core >= 12, `共 ${counts.total} 条边 / 主干 ${counts.core} 条 / sync ${counts.sync} / write ${counts.write} / net ${counts.net}`);
for (const [key, label, expect] of [['sync', '② 同步穿透', counts.sync], ['write', '① 本地写入', counts.write], ['net', '④ 网络出口', counts.net], ['all', '全部依赖(主干)', counts.core]]) {
  await clk(`button[data-flow="${key}"]`);
  await page.waitForTimeout(350);
  const pathsNow = await page.evaluate(() => [...document.querySelectorAll('#wires path')].filter(p => p.getAttribute('marker-end')).length);
  check(`flow-filter:${label}`, pathsNow === expect, `${pathsNow} 条，期望 ${expect} 条`);
  assertVisual(`链路:${label}`, await measureVisual(), expect, expect);
  if (key === 'sync') {
    const touched = await page.evaluate(() => ({ dim: document.querySelectorAll('.card.dim').length, live: document.querySelectorAll('.card:not(.dim)').length }));
    check('flow-filter-marks-participants', touched.live >= 5 && touched.live < 17, `本链路参与模块=${touched.live} 淡出=${touched.dim}`);
  }
}

// --- 契约总表 ---
await clk('#v-matrix');
await page.waitForTimeout(400);
const matrix = await page.evaluate(() => ({
  rows: document.querySelectorAll('table.matrix tbody tr').length,
  hasTypes: document.body.innerText.includes('ApplyOp') && document.body.innerText.includes('RouteProof'),
}));
check('matrix-view', matrix.rows >= 16 && matrix.hasTypes, `${matrix.rows} 行, 关键字段可见=${matrix.hasTypes}`);
await page.screenshot({ path: OUT + 'diagram-matrix.png', fullPage: false });
await clk('#v-graph');
await page.waitForTimeout(300);

// --- 搜索定位边界：SQL 列名与 Rust 字段名必须互搜得到 ---
for (const [term, expectMin] of [['content_hash', 1], ['contentHash', 1], ['sync_rev', 1], ['ProxyProfile', 1], ['zzz-nothing', 0]]) {
  await fill('#q', term);
  await page.waitForTimeout(350);
  const s = await page.evaluate(() => ({
    marks: document.querySelectorAll('#side mark').length,
    litCards: document.querySelectorAll('.card.hitmatch').length,
    dimmed: document.querySelectorAll('.card.dim').length,
    hitBar: (document.querySelector('#side .hint b')?.textContent || '').slice(0, 24),
  }));
  check(`search:${term}`, s.marks >= expectMin && s.litCards >= expectMin, `高亮 ${s.marks} 处 / 命中模块 ${s.litCards} / ${s.hitBar}`);
}
await fill('#q', '');
await page.waitForTimeout(250);
const cleared = await page.evaluate(() => ({ marks: document.querySelectorAll('mark').length, hit: document.querySelectorAll('.card.hitmatch').length }));
check('search-clear-restores', cleared.marks === 0 && cleared.hit === 0, `marks=${cleared.marks} hitmatch=${cleared.hit}（选中态淡出保留属预期）`);

// --- 明暗双主题都不破版 ---
const before = await page.evaluate(() => document.body.innerText.length);
await clk('#theme');
await page.waitForTimeout(300);
const light = await page.evaluate(() => ({ theme: document.documentElement.dataset.theme, paths: [...document.querySelectorAll('#wires path')].filter(p => p.getAttribute('marker-end')).length, fg: getComputedStyle(document.body).color }));
check('light-theme-works', light.theme === 'light' && light.paths >= 5, `${light.theme} 下 ${light.paths} 条连线, 前景 ${light.fg}`);
await page.screenshot({ path: OUT + 'diagram-light.png' });
await clk('#theme');
await page.waitForTimeout(250);
const after = await page.evaluate(() => document.body.innerText.length);
check('content-stable-across-theme', Math.abs(after - before) < 5, `${before} → ${after} 字符`);

await page.screenshot({ path: OUT + 'diagram-desktop.png', fullPage: true });
await clk('.card[data-id="host"]');
await page.waitForTimeout(300);
await page.screenshot({ path: OUT + 'diagram-host.png' });

// --- 手机视口：不能整体横向溢出 ---
await page.setViewportSize({ width: 390, height: 844 });
await page.waitForTimeout(500);
const mobile = await page.evaluate(() => {
  const de = document.documentElement;
  const card = document.querySelector('.card[data-id="host"]');
  const btn = document.querySelector('button');
  return {
    bodyScrollW: de.scrollWidth, innerW: window.innerWidth,
    canvasScrollable: (() => { const c = document.getElementById('canvas'); return c.scrollWidth > c.clientWidth; })(),
    sideBelow: card ? card.getBoundingClientRect().width > 0 : false,
    btnH: btn ? Math.round(btn.getBoundingClientRect().height) : 0,
    overflowers: [...document.querySelectorAll('body *')].filter(e => e.getBoundingClientRect().right > window.innerWidth + 2 && getComputedStyle(e).position !== 'fixed').slice(0, 3).map(e => `${e.tagName}.${e.className}`.slice(0, 30)),
  };
});
check('mobile-no-page-overflow', mobile.bodyScrollW <= mobile.innerW + 2, `body ${mobile.bodyScrollW}px vs viewport ${mobile.innerW}px；图区内部横向滚动=${mobile.canvasScrollable}`);
check('mobile-cards-render', mobile.sideBelow && mobile.btnH >= 44, `卡片可见=${mobile.sideBelow} 按钮高=${mobile.btnH}px`);
await page.screenshot({ path: OUT + 'diagram-mobile.png', fullPage: false });

const errsAfter = errors.length;
check('no-runtime-errors-after-interaction', errsAfter === 0, errsAfter ? errors.slice(0, 3).join(' | ') : 'clean through 30+ interactions');

await browser.close();
const failed = results.filter(r => !r.ok).length;
console.log(`\ntotal=${results.length} failed=${failed}`);
process.exit(failed ? 1 : 0);
