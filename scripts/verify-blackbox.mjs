/**
 * 纯黑盒 UAT（总指令 §23 / TEST-PLAN L5）。
 *
 * 与 `verify-app.mjs` 的分工：那一步会用本地桥复核**库里的真实状态**（界面说成功
 * 而库里没有就算失败），因此它不是黑盒。这一步恰恰相反 —— **只允许**点击、输入、
 * 键盘、刷新，断言只看屏幕上显示出来的文字与几何：
 *
 *   - 全脚本没有一次 `fetch('/cmd/...')`，也没有任何数据库读取；
 *   - 任何"内部值"都不许参与判定。
 *
 * 为什么值得单独有一条：用户每天面对的就是"屏幕上看得见什么"。只在有内部状态
 * 兜底的测试里成立的性质（比如"待同步计数掉了"），不等于用户看得见它掉了。
 *
 * 前置（与 verify-app 相同）：
 *   notera-cli --data-dir <空目录> serve --port 17323
 *   pnpm --dir apps/desktop dev
 *
 *   node scripts/verify-blackbox.mjs
 */
const PW = process.env.PW_CORE || 'file:///C:/Users/lhcz-fu/node_modules/playwright-core/index.js';
const CHROME = process.env.CHROME || 'C:/Users/lhcz-fu/AppData/Local/ms-playwright/chromium-1243/chrome-win64/chrome.exe';
const URL_BASE = process.env.APP_URL || 'http://127.0.0.1:5173';
const SHOT = 'D:/code/Notes/docs/evidence/blackbox.png';
import fs from 'node:fs';
import os from 'node:os';
const pw = await (await import(PW)).default;
const { chromium } = pw;

const browser = await chromium.launch({ executablePath: CHROME });
const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
const consoleErrors = [];
const badResponses = [];
page.on('console', (m) => m.type() === 'error' && consoleErrors.push(m.text()));
page.on('pageerror', (e) => consoleErrors.push(String(e)));
page.on('response', (r) => {
  if (r.status() < 400) return;
  r.text()
    .then((b) => badResponses.push(`${r.request().method()} ${r.url()} → ${r.status()} [步骤：${stepName}] ${b.slice(0, 120)}`))
    .catch(() => badResponses.push(`${r.request().method()} ${r.url()} → ${r.status()} [步骤：${stepName}]`));
});

const rows = [];
let stepName = '(尚未开始)';
function record(ok, detail) {
  rows.push({ stepName, ok, detail });
  console.log(`${ok ? 'PASS' : 'FAIL'}  ${stepName}${detail ? `  —— ${detail}` : ''}`);
}
async function step(name, fn) {
  stepName = name;
  try {
    record(true, await fn());
  } catch (e) {
    record(false, String(e).split('\n').slice(0, 2).join(' / '));
  }
}
/** 屏幕上看得见的文字 —— 黑盒判定只允许读它。 */
const visible = async (sel) => (await page.locator(sel).first().innerText()).replace(/\s+/g, ' ').trim();
const count = (sel) => page.locator(sel).count();

await step('打开就能用：首帧是笔记列表，不是白屏也不是加载圈', async () => {
  await page.goto(URL_BASE);
  await page.locator('[data-testid="note-list"]').waitFor({ timeout: 8000 });
  const body = (await visible('body'));
  if (body.length < 20) throw new Error(`首帧几乎没有可见文字（${body.length} 字），像白屏`);
  if ((await count('[data-testid="nav-all"]')) === 0) throw new Error('侧栏「全部笔记」不在');
  // 这条门禁的后续判定（"搜索恰好 1 行"、"列表里只有它"）都建立在**空库起步**上。
  // 复用别人跑过的数据目录会让它时红时绿 —— 那种绿不是证据。
  const leftovers = await page.locator('[data-testid^="note-row-"]').count();
  if (leftovers > 0) {
    throw new Error(`需要一座空库（发现 ${leftovers} 条遗留笔记）。请用独立数据目录起 dev 桥：notera-cli --data-dir <空目录> serve`);
  }
  return `可见文字 ${body.length} 字，起步库内 0 条`;
});

const title = `黑盒笔记 ${Date.now()}`;
await step('点「新建笔记」→ 敲正文 → 列表里立刻看得见这条', async () => {
  await page.click('[data-testid="new-note"]');
  await page.waitForTimeout(600);
  await page.locator('[data-testid="editor-doc"]').click();
  await page.keyboard.type(title);
  await page.waitForTimeout(1400);
  await page.locator('[data-testid="nav-all"]').click();
  await page.waitForTimeout(800);
  const list = await visible('[data-testid="note-list"]');
  if (!list.includes(title)) throw new Error(`列表里没有刚敲的那条：${list.slice(0, 120)}`);
  return title;
});

const folder = `黑盒子层 ${Date.now()}`;
await step('侧栏点「＋」建子文件夹 → 屏幕上立刻出现它', async () => {
  const rootRow = page.locator('[data-testid^="folder-"]').first();
  const rootId = (await rootRow.getAttribute('data-testid')).replace('folder-', '');
  await rootRow.hover();
  await page.click(`[data-testid="folder-new-sub-${rootId}"]`);
  await page.fill('[data-testid="folder-create-input"]', folder);
  await page.keyboard.press('Enter');
  await page.waitForTimeout(900);
  const labels = (await page.locator('.tree__label').allInnerTexts()).map((s) => s.trim());
  if (!labels.includes(folder)) throw new Error(`侧栏没出现「${folder}」，只有：${labels.join(' / ')}`);
  return folder;
});

await step('打开笔记 → 「移动到」选那个子文件夹 → 点进子文件夹看得见它', async () => {
  await page.locator('[data-testid^="note-row-"]').filter({ hasText: title }).first().click();
  await page.waitForTimeout(600);
  const pick = page.locator('[data-testid="move-folder"]');
  const labels = (await pick.locator('option').allInnerTexts()).map((s) => s.trim());
  const wanted = labels.find((l) => l.endsWith(folder));
  if (!wanted) throw new Error(`「移动到」下拉里没有那个子文件夹，选项是：${labels.join(' | ')}`);
  // 收起状态的 `<select>` 上点 option 元素不会触发 change（Playwright 要显式选）
  await pick.selectOption({ label: wanted });
  await page.waitForTimeout(1200);
  await page.locator('[data-testid="nav-all"]').click();
  await page.waitForTimeout(500);
  // 点侧栏那颗文件夹按钮（`text=` 会同时命中"移动到"下拉里那个隐藏 option，
  // 于是 click 一直等一个不可见的目标）
  await page.locator('button.tree__name', { hasText: folder }).first().click();
  await page.waitForTimeout(900);
  const list = await visible('[data-testid="note-list"]');
  if (!list.includes(title)) throw new Error(`移动后进那个子文件夹看不到它：${list.slice(0, 140)}`);
  return '移动 + 子文件夹内可见';
});

await step('输入即搜：关键词只留下这一条，清空后恢复', async () => {
  await page.fill('[data-testid="search-input"]', '黑盒笔记');
  await page.waitForTimeout(900);
  const hit = await count('[data-testid^="note-row-"]');
  if (hit !== 1) throw new Error(`搜索后应只剩 1 行，实际 ${hit} 行`);
  await page.fill('[data-testid="search-input"]', '');
  await page.waitForTimeout(900);
  if ((await count('[data-testid^="note-row-"]')) < 1) throw new Error('清空关键词后列表没恢复');
  return '1 行命中 → 清空恢复';
});

await step('删除 → 回收站看得见 → 恢复 → 回到列表', async () => {
  await page.locator('[data-testid="nav-all"]').click();
  await page.waitForTimeout(600);
  const row = page.locator('[data-testid^="note-row-"]').filter({ hasText: title }).first();
  await row.hover();
  await row.locator('[aria-label="移到最近删除"], [title="移到最近删除"]').first().click();
  await page.waitForTimeout(1000);
  await page.click('[data-testid="nav-trash"]');
  await page.waitForTimeout(900);
  const trashText = await visible('[data-testid="note-list"]');
  if (!trashText.includes(title)) throw new Error(`回收站里没有它：${trashText.slice(0, 140)}`);
  const trow = page.locator('[data-testid^="note-row-"]').filter({ hasText: title }).first();
  await trow.hover();
  await trow.locator('[data-testid="restore-note"]').click();
  await page.waitForTimeout(1000);
  await page.click('[data-testid="nav-all"]');
  await page.waitForTimeout(900);
  const back = await visible('[data-testid="note-list"]');
  if (!back.includes(title)) throw new Error(`恢复后列表里没有它：${back.slice(0, 140)}`);
  return '回收站 → 恢复，全程只看屏幕';
});

await step('刷新（等价于重启 App）之后：列表和正文都还在', async () => {
  await page.reload();
  await page.locator('[data-testid="note-list"]').waitFor({ timeout: 8000 });
  await page.waitForTimeout(1200);
  const list = await visible('[data-testid="note-list"]');
  if (!list.includes(title)) throw new Error(`重启后列表没有它：${list.slice(0, 140)}`);
  await page.locator('[data-testid^="note-row-"]').filter({ hasText: title }).first().click();
  await page.waitForTimeout(900);
  const doc = await visible('[data-testid="editor-doc"]');
  if (!doc.includes(title)) throw new Error(`点开之后正文是空的或对不上：${doc.slice(0, 140)}`);
  return doc.slice(0, 24);
});

await step('插入图片：走那个文件选择器 → 屏幕上真解码出这张图 → 刷新之后还在', async () => {
  // 这一条整个黑盒里最值得：用户只看得到"图在不在"，看不到 `hasAttachment`。
  // 判据只有两个屏幕事实 —— `<img>` 真的**解出像素**（naturalWidth>0，光有 src 属性
  // 不算），以及刷新重开之后还解得出来。全程不读库、不调命令。
  const png = Buffer.from(
    'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==',
    'base64',
  );
  const file = `${os.tmpdir()}\\notera-blackbox-${Date.now()}.png`;
  fs.writeFileSync(file, png);
  await page.locator('[data-testid="editor-doc"]').click();
  await page.locator('[data-testid="attach-input"]').setInputFiles(file);
  const img = page.locator('[data-testid="editor-doc"] img.nb-image').last();
  await img.waitFor({ timeout: 8000 });
  const shown = await img.evaluate((el) => ({ w: el.naturalWidth, ok: el.complete && el.naturalWidth > 0 }));
  if (!shown.ok) throw new Error(`那张图没解出像素（naturalWidth=${shown.w}）`);
  await page.waitForTimeout(1800); // 等自动保存
  await page.reload();
  await page.locator('[data-testid="note-list"]').waitFor({ timeout: 8000 });
  await page.waitForTimeout(1000);
  await page.locator('[data-testid^="note-row-"]').filter({ hasText: title }).first().click();
  await page.waitForTimeout(1300);
  const again = page.locator('[data-testid="editor-doc"] img.nb-image').last();
  await again.waitFor({ timeout: 8000 });
  const kept = await again.evaluate((el) => el.complete && el.naturalWidth > 0);
  fs.rmSync(file, { force: true });
  if (!kept) throw new Error('刷新后那张图解码不出来了 —— 附件没跟着笔记活下来');
  return `naturalWidth=${shown.w}，刷新后仍在`;
});

await step('键盘可达：Esc 与 Tab 不失控，焦点始终在界面里', async () => {
  await page.keyboard.press('Escape');
  await page.waitForTimeout(300);
  await page.keyboard.press('Tab');
  await page.keyboard.press('Tab');
  const focused = await page.evaluate(() => {
    const el = document.activeElement;
    return el ? `${el.tagName.toLowerCase()}${el.getAttribute('data-testid') ? `[${el.getAttribute('data-testid')}]` : ''}` : '(none)';
  });
  if (focused === '(none)' || focused === 'body') throw new Error('Tab 之后焦点跑没了');
  return `焦点在 ${focused}`;
});

await page.screenshot({ path: SHOT });
await step('全程控制台零 error、无 4xx/5xx', async () => {
  if (consoleErrors.length > 0) throw new Error(consoleErrors.slice(0, 3).join(' | '));
  if (badResponses.length > 0) throw new Error(badResponses.slice(0, 3).join(' | '));
  return `截图 ${SHOT.split('/').pop()}`;
});

const failed = rows.filter((r) => !r.ok).length;
console.log(`\nverify-blackbox: ${rows.length - failed}/${rows.length} 步通过（无一处读库或调命令）`);
await browser.close();
process.exit(failed > 0 ? 1 : 0);
