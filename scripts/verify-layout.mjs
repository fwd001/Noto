/**
 * 布局门禁：量**渲染后的几何**，不量 DOM 里有没有那个类。
 *
 * 钉三件用户实际看得见的性质（都是本轮从真读数里提出来的）：
 *  ① 设置页一栏到底：所有卡片左缘与宽度**同一个值**，且在 900→1800 之间不随视口跳列
 *     （旧写法 `auto-fit minmax(360px,1fr)` 实测 900/1100 出 2 列、1440 出 3 列、1800 出 **4 列**，
 *      六张卡片底边落在六个不同的地方 —— 用户那句「2 列 3 列长度对不齐不好看」）。
 *  ② 侧栏在任何视口都**够得着**：不在版面上时，侧栏外面必须有一颗 ☰，
 *     点它侧栏要真的进视口（旧形状下 820–1179 那一档侧栏在屏幕外，
 *     而唯一的 ☰ 长在侧栏内部、列表栏那颗又只在 `isCompact` 才出现 ⇒ 文件夹/同步/设置整档够不着）。
 *  ③ 全程零 console error。
 *
 * 前置（脚本不管，由调用方起）：
 *   cargo run -p notera-cli -- --data-dir <空目录> serve --port 17323
 *   pnpm --dir apps/desktop dev                       # 5173
 *   node scripts/verify-layout.mjs
 */
const PW = process.env.PW_CORE || 'file:///C:/Users/lhcz-fu/node_modules/playwright-core/index.js';
const CHROME = process.env.CHROME || 'C:/Users/lhcz-fu/AppData/Local/ms-playwright/chromium-1243/chrome-win64/chrome.exe';
const URL_BASE = process.env.APP_URL || 'http://127.0.0.1:5173';
const WIDTHS = [900, 1100, 1440, 1800];
const OUT = process.env.OUT_DIR || 'D:/code/Notes/docs/evidence/uat';

const fs = await import('node:fs').then((m) => m.default);
const { chromium } = await (await import(PW)).default;
const browser = await chromium.launch({ executablePath: CHROME });
const failures = [];
const notes = [];

function check(name, ok, detail) {
  (ok ? notes : failures).push(`${ok ? 'PASS' : 'FAIL'} ${name}${detail ? ` —— ${detail}` : ''}`);
}

for (const width of WIDTHS) {
  const errors = [];
  const page = await browser.newPage({ viewport: { width, height: 950 } });
  page.on('console', (m) => { if (m.type() === 'error') errors.push(m.text().slice(0, 160)); });
  page.on('pageerror', (e) => errors.push(String(e).slice(0, 160)));
  await page.goto(URL_BASE, { waitUntil: 'networkidle' });
  await page.waitForSelector('[data-testid="sidebar"]', { timeout: 15000 });
  await page.waitForTimeout(2200);

  // ② 侧栏够得着
  const reach = await page.evaluate(() => {
    const side = document.querySelector('[data-testid="sidebar"]');
    const r = side.getBoundingClientRect();
    const inline = r.x >= 0 && r.width > 0 && r.right <= window.innerWidth;
    const handle = document.querySelector('[data-testid="open-sidebar"]');
    let handleInView = false;
    if (handle) {
      const h = handle.getBoundingClientRect();
      handleInView = h.x >= 0 && h.width > 0 && h.right <= window.innerWidth;
    }
    return { inline, hasHandle: Boolean(handle), handleInView };
  });
  if (!reach.inline) {
    check(`宽 ${width}：侧栏不在版面上时，侧栏外必须有一颗 ☰`, reach.hasHandle && reach.handleInView, JSON.stringify(reach));
    await page.evaluate(() => document.querySelector('[data-testid="open-sidebar"]').click());
    await page.waitForTimeout(600);
    const opened = await page.evaluate(() => {
      const r = document.querySelector('[data-testid="sidebar"]').getBoundingClientRect();
      const nav = document.querySelector('[data-testid="nav-settings"]').getBoundingClientRect();
      return { sideInView: r.x >= 0 && r.right <= window.innerWidth && r.width > 0, navInView: nav.x >= 0 && nav.right <= window.innerWidth };
    });
    check(`宽 ${width}：点 ☰ 侧栏真的进来、且「设置」那一行可点`, opened.sideInView && opened.navInView, JSON.stringify(opened));
    await page.screenshot({ path: `${OUT}/21-drawer-${width}.png` });
  } else {
    check(`宽 ${width}：侧栏inline（无需外部 ☰）`, true, JSON.stringify(reach));
  }

  // ① 设置页一栏到底
  await page.evaluate(() => document.querySelector('[data-testid="nav-settings"]').click());
  await page.waitForTimeout(1200);
  const cards = await page.evaluate(() =>
    Array.from(document.querySelectorAll('.settings__grid > .card')).map((el) => {
      const r = el.getBoundingClientRect();
      return { x: Math.round(r.x), w: Math.round(r.width) };
    }),
  );
  check(`宽 ${width}：设置页卡片数 = 6`, cards.length === 6, `实到 ${cards.length}`);
  const xs = [...new Set(cards.map((c) => c.x))];
  const ws = [...new Set(cards.map((c) => c.w))];
  check(`宽 ${width}：所有卡片左缘同一个值`, xs.length === 1, `x ∈ ${JSON.stringify(xs)}`);
  check(`宽 ${width}：所有卡片宽度同一个值`, ws.length === 1, `w ∈ ${JSON.stringify(ws)}`);
  await page.screenshot({ path: `${OUT}/21-settings-${width}.png`, fullPage: true });
  notes.push(`     宽 ${width} 卡片几何 = ${JSON.stringify(cards[0])} ×${cards.length}`);

  check(`宽 ${width}：console error 为零`, errors.length === 0, errors.slice(0, 3).join(' | '));
  await page.close();
}

// ① 的另一半：**跨视口**不许换列（同一份内容在 900 与 1800 下卡片宽度差不能是"多塞一列"的量级）
await browser.close();
console.log(notes.join('\n'));
console.log(failures.length ? `\n${failures.join('\n')}\n>>> 布局门禁 FAIL` : '\n>>> 布局门禁 PASS');
process.exit(failures.length ? 1 : 0);
