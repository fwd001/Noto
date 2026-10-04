/**
 * 同步静止态门禁（缺口 G46 的复跑器）：空库、没配账户时，
 * 点徽标 / 按 F5 **都不许**让徽标进入"正在同步"，也不许发出一次 `sync_now`，
 * 而这一次点击要有去处（设置页）。全程零 console error。
 *
 * 前置（脚本不管，由调用方起）：
 *   cargo run -p notera-cli -- --data-dir <空目录> serve --port 17323
 *   pnpm --dir apps/desktop dev                       # 5173
 *   node scripts/verify-sync-idle.mjs
 *
 * 为什么要有这一条而不是只留单测：根因在**调用边**上
 * （`sync_now` 从不回错 + 没配账户时没有调度器 ⇒ 没有任何事件来纠正 `syncing`），
 * 这一形只有真浏览器 + 真 HTTP 桥才量得到"点了之后 12 秒它到底停在哪一格"。
 */
const PW = process.env.PW_CORE || 'file:///C:/Users/lhcz-fu/node_modules/playwright-core/index.js';
const CHROME = process.env.CHROME || 'C:/Users/lhcz-fu/AppData/Local/ms-playwright/chromium-1243/chrome-win64/chrome.exe';
const URL_BASE = process.env.APP_URL || 'http://127.0.0.1:5173';
const fs = await import('node:fs').then((m) => m.default);
const { chromium } = await (await import(PW)).default;

const errors = [];
const syncReqs = [];
const browser = await chromium.launch({ executablePath: CHROME });
const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
page.on('console', (m) => { if (m.type() === 'error') errors.push(m.text().slice(0, 200)); });
page.on('pageerror', (e) => errors.push(String(e).slice(0, 200)));
page.on('request', (r) => { if (r.url().includes('/cmd/sync_now')) syncReqs.push(r.url()); });

await page.goto(URL_BASE, { waitUntil: 'networkidle' });
await page.waitForSelector('[data-testid="sync-badge"]', { timeout: 15000 });
await page.waitForTimeout(4000);

const read = () => page.getAttribute('[data-testid="sync-badge"]', 'data-badge');
const text = () => page.innerText('[data-testid="sync-badge"]');
const before = await read();
const syncAtBoot = syncReqs.length;

await page.click('[data-testid="sync-badge"]');
const series = [];
for (let i = 0; i < 24; i++) { await page.waitForTimeout(500); series.push(await read()); }
const after = series[series.length - 1];
const settingsOpen = await page.locator('[data-testid="account-save"]').isVisible().catch(() => false);

console.log(`点击前 = ${before} 文案=${(await text()).replace(/\s+/g, ' ').trim()}`);
console.log(`点击后 12 秒 = ${[...new Set(series)].join(',')}（唯一值）`);
console.log(`sync_now 请求数：启动=${syncAtBoot} 点击后累计=${syncReqs.length}`);
console.log(`设置页是否打开 = ${settingsOpen}`);
await page.screenshot({ path: 'D:/code/Notes/docs/evidence/uat/22-sync-idle.png', clip: { x: 0, y: 0, width: 340, height: 640 } });

// F5 也要静止
await page.goto(URL_BASE, { waitUntil: 'networkidle' });
await page.waitForTimeout(3000);
const n0 = syncReqs.length;
await page.keyboard.press('F5');
await page.waitForTimeout(3000);
console.log(`F5 后：徽标=${await read()} 新增 sync_now=${syncReqs.length - n0}`);
fs.writeFileSync('D:/code/Notes/docs/evidence/uat/22-sync-idle.json', JSON.stringify({ before, after, series: [...new Set(series)], syncAtBoot, syncAfterClick: syncReqs.length, settingsOpen, errors }, null, 2));

const pass = before === 'idle' && after === 'idle' && syncAtBoot === 0 && syncReqs.length === 0 && settingsOpen === true && errors.length === 0;
console.log(pass ? '>>> PASS：没配同步 = 静止、不发请求、点了有去处、零 error' : '>>> FAIL 见上面读数');
await browser.close();
process.exit(pass ? 0 : 1);
