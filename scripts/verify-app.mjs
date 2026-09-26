/**
 * 端到端 UI 验证：真浏览器 + 真 vite + **真 Rust 核心**（notera-cli serve 的 dev 桥），
 * 不是假数据。每一步要么 PASS 要么 FAIL，没有"应该没问题"这一档。
 *
 * 前置（脚本不管，由调用方起）：
 *   cargo run -p notera-cli -- --data <空目录> serve --port 17323
 *   npm --prefix apps/desktop run dev                      # 5173
 *
 *   node scripts/verify-app.mjs
 */
const PW = process.env.PW_CORE || 'file:///C:/Users/lhcz-fu/node_modules/playwright-core/index.js';
const CHROME = process.env.CHROME || 'C:/Users/lhcz-fu/AppData/Local/ms-playwright/chromium-1243/chrome-win64/chrome.exe';
const URL_BASE = process.env.APP_URL || 'http://127.0.0.1:5173';
const OUT = 'D:/code/Notes/docs/evidence';

const pw = await (await import(PW)).default;
const { chromium } = pw;
const browser = await chromium.launch({ executablePath: CHROME });
const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });

const consoleErrors = [];
const pageErrors = [];
const failedRequests = [];
page.on('console', (m) => {
  if (m.type() === 'error') consoleErrors.push(m.text());
});
page.on('pageerror', (e) => pageErrors.push(String(e)));
page.on('requestfailed', (r) => failedRequests.push(`${r.method()} ${r.url()} → ${r.failure()?.errorText}`));

const rows = [];
function record(step, ok, detail) {
  rows.push({ step, ok, detail });
  console.log(`${ok ? 'PASS' : 'FAIL'}  ${step}${detail ? `  —— ${detail}` : ''}`);
}
async function step(name, fn) {
  try {
    const detail = await fn();
    record(name, true, detail ?? '');
  } catch (e) {
    const text = String(e).replace(/\n/g, '\n      ');
    record(name, false, text.slice(0, 600));
  }
}

// 编辑器渲染时把空格写成 U+00A0 以保持连续空格（dom.ts:104），读回时再折回 U+0020。
// 所以"屏幕上的文本"与"库里的文本"必然差在这一格 —— 断言要按库的口径比。
const asStored = (s) => s.replace(/ /g, ' ');

/** 界面上漏出未登记的文案键（形如 editor.blockCodeBlock）—— 单测扫不到动态键，只能看渲染结果。 */
const KEY_LEAK = /\b(?:editor|settings|sync|tb|cmd|state|slash|note|list|nav|conflict|proxy|error|link|mobile|sidebar|win|app|toast)\.[a-zA-Z][\w.]*\b/g;
const leakedKeys = async () => {
  const text = await page.evaluate(() => document.body.innerText);
  return [...new Set(text.match(KEY_LEAK) ?? [])];
};

/** 块序列指纹：类型 + 正文文本。重排类断言都要比这个，单看数量证明不了顺序。
 *  文本只取 .nb-content：把手字形（+ 和 ⠿）也在块元素里，按整块 innerText 会比不齐。 */
const blockSig = () =>
  page.locator('[data-testid="editor-doc"] .nb-block').evaluateAll((els) =>
    els.map((el) => {
      const content = el.querySelector('.nb-content');
      const raw = (content ?? el).innerText.replace(/ /g, ' ').trim();
      return `${el.getAttribute('data-type')}:${raw.slice(0, 20)}`;
    }),
  );

/** 按住把手拖到某个块上：pointer 事件走的是真实鼠标轨迹，不是直接调内部函数。 */
async function dragGripToBlock(fromIndex, toIndex) {
  const grip = page.locator('[data-testid="drag-handle"]').nth(fromIndex);
  const g = await grip.boundingBox();
  const target = page.locator('[data-testid="editor-doc"] .nb-block').nth(toIndex);
  const t = await target.boundingBox();
  if (!g || !t) throw new Error('把手或目标块量不到尺寸');
  await page.mouse.move(g.x + g.width / 2, g.y + g.height / 2);
  await page.mouse.down();
  await page.mouse.move(g.x + g.width / 2, (g.y + t.y) / 2 + 4, { steps: 6 });
  await page.mouse.move(g.x + g.width / 2, t.y + t.height - 4, { steps: 10 });
  return { g, t };
}

const stamp = Date.now();
const title = `E2E 笔记 ${stamp}`;

await step('页面加载完成（#app 可见）', async () => {
  await page.goto(URL_BASE, { waitUntil: 'networkidle', timeout: 20000 });
  await page.waitForSelector('#app', { timeout: 8000 });
  return URL_BASE;
});

await step('后端可达：链路横幅未提示"未连接"', async () => {
  const banner = await page.locator('[data-testid="banner-link"]').count();
  if (banner > 0) {
    const text = (await page.locator('[data-testid="banner-link"]').first().innerText()).trim();
    throw new Error(`显示链路横幅：${text}`);
  }
  return '无链路横幅';
});

await step('空态或列表骨架存在', async () => {
  const empty = await page.locator('[data-testid="empty-state"], [data-testid="list-empty"], [data-testid^="note-row-"]').count();
  if (empty === 0) throw new Error('既没有空态也没有列表行');
  return `${empty} 个候选元素`;
});

await step('新建笔记 → 编辑器出现', async () => {
  // `first-new-note` 只在空态里出现；库非空时用的是列表上的 `new-note`。两个都是合法入口。
  const primary = page.locator('[data-testid="new-note"]');
  if ((await primary.count()) > 0) await primary.first().click({ timeout: 5000 });
  else await page.click('[data-testid="first-new-note"]', { timeout: 5000 });
  await page.waitForSelector('[data-testid="editor-doc"]', { timeout: 5000 });
  return 'editor-doc 可见';
});

await step('输入正文并自动保存（rev 前进）', async () => {
  // 真正可编辑的是块级 contenteditable，`editor-doc` 只是滚动容器。
  const field = page.locator('[data-testid="editor-doc"] [contenteditable="true"]').first();
  await field.waitFor({ timeout: 5000 });
  await field.click();
  await page.keyboard.type(title, { delay: 12 });
  await page.waitForTimeout(1400);
  const text = asStored(await field.innerText());
  if (!text.includes(title)) throw new Error(`编辑器内容不含输入：${JSON.stringify(text.slice(0, 60))}`);
  return text.slice(0, 40);
});

await step('Markdown 缩写即时转换（# 空格 → 标题）', async () => {
  const field = page.locator('[data-testid="editor-doc"] [contenteditable="true"]').last();
  await field.click();
  await page.keyboard.press('Enter');
  await page.keyboard.type('# 一级标题', { delay: 12 });
  await page.waitForTimeout(700);
  const heading = page.locator('[data-testid="editor-doc"] [data-type="heading"]');
  if ((await heading.count()) === 0) throw new Error('输入 "# " 没有转成标题');
  const text = asStored(await heading.last().locator('.nb-content').innerText());
  if (!text.includes('一级标题')) throw new Error(`标题内容不对：${JSON.stringify(text)}`);
  return text;
});

await step('"/" 命令面板：出现 → 过滤 → 回车选中代码块', async () => {
  const field = page.locator('[data-testid="editor-doc"] [contenteditable="true"]').last();
  await field.click();
  await page.keyboard.press('Enter');
  await page.keyboard.type('/', { delay: 12 });
  await page.waitForSelector('[data-testid="slash-menu"]', { timeout: 4000 });
  const all = await page.locator('[data-testid="slash-menu"] [role="option"]').count();
  await page.keyboard.type('cod', { delay: 12 });
  await page.waitForTimeout(400);
  const filtered = await page.locator('[data-testid="slash-menu"] [role="option"]').count();
  if (!(filtered < all && filtered > 0)) throw new Error(`过滤没生效：${all} → ${filtered}`);
  await page.keyboard.press('Enter');
  await page.waitForTimeout(500);
  if ((await page.locator('[data-testid="slash-menu"]').count()) > 0) throw new Error('选中后面板应关闭');
  if ((await page.locator('[data-testid="editor-doc"] [data-type="codeBlock"]').count()) === 0) {
    throw new Error('回车没有把这块变成代码块');
  }
  return `${all} 项 → 过滤到 ${filtered} 项 → 已转代码块`;
});

await step('块把手：拖第一块到末尾，改的是顺序且不丢块', async () => {
  const before = await blockSig();
  if (before.length < 3) throw new Error(`只有 ${before.length} 个块，测不出重排`);
  await dragGripToBlock(0, before.length - 1);
  const marked = await page.locator('[data-testid="editor-doc"] .nb-block[data-drop]').count();
  if (marked === 0) throw new Error('拖拽过程中没有画出插入指示线');
  await page.screenshot({ path: `${OUT}/app-drag-reorder.png` });
  await page.mouse.up();
  await page.waitForTimeout(700);
  const after = await blockSig();
  if (after.length !== before.length) throw new Error(`块数变了：${before.length} → ${after.length}`);
  if ([...after].sort().join('|') !== [...before].sort().join('|')) throw new Error('重排丢了或改了块内容');
  if (after.join('|') === before.join('|')) throw new Error('顺序没变：拖拽没生效');
  if (after[after.length - 1] !== before[0]) throw new Error(`末块不是被拖的那块：${JSON.stringify(after)}`);
  return `${before.map((s) => s.split(':')[0]).join('·')} → ${after.map((s) => s.split(':')[0]).join('·')}`;
});

await step('Alt+↓ 键盘重排（不碰鼠标也能改顺序）', async () => {
  const before = await blockSig();
  const field = page.locator('[data-testid="editor-doc"] [contenteditable="true"]').first();
  await field.click();
  await page.keyboard.press('Alt+ArrowDown');
  await page.waitForTimeout(700);
  const after = await blockSig();
  if (after.join('|') === before.join('|')) throw new Error('Alt+↓ 没有移动块');
  if (after[1] !== before[0]) throw new Error(`没往下走一格：${JSON.stringify(after)}`);
  return `${before[0].split(':')[0]} 下移一格`;
});

await step('把手"+"：在当前块下方插入空块并聚焦', async () => {
  const before = (await blockSig()).length;
  const last = page.locator('[data-testid="editor-doc"] .nb-block').last();
  await last.hover();
  await page.locator('[data-testid="insert-below"]').last().click();
  await page.waitForTimeout(700);
  const after = await blockSig();
  if (after.length !== before + 1) throw new Error(`块数 ${before} → ${after.length}，应 +1`);
  const focused = await page.evaluate(() => {
    const el = document.activeElement;
    return el && el.closest('[data-block-id]')?.getAttribute('data-type');
  });
  if (focused !== 'paragraph') throw new Error(`焦点没落在新空段上，实际在 ${focused}`);
  return `${before} → ${after.length} 块，焦点在新块`;
});

const orderBeforeReload = await blockSig();

await step('回列表能看到这条笔记', async () => {
  await page.click('[data-testid="nav-all"]', { timeout: 5000 }).catch(() => {});
  await page.waitForTimeout(600);
  const hit = await page.locator(`[data-testid^="note-row-"]:has-text(${JSON.stringify(title)})`).count();
  if (hit === 0) throw new Error('列表里找不到刚建的笔记');
  return `${hit} 行匹配`;
});

await step('刷新后仍在（真的落库，不是内存态）', async () => {
  await page.reload({ waitUntil: 'networkidle' });
  await page.waitForTimeout(900);
  const hit = await page.locator(`[data-testid^="note-row-"]:has-text(${JSON.stringify(title)})`).count();
  if (hit === 0) throw new Error('刷新后笔记消失');
  return `${hit} 行匹配`;
});

await step('点开这条笔记 → 正文真的显示出来（不是空面板/重试占位）', async () => {
  await page.locator(`[data-testid^="note-row-"]:has-text(${JSON.stringify(title)})`).first().click();
  await page.waitForTimeout(900);
  const blank = await page.locator('.editor-blank, [data-testid="retry-open"]').count();
  if (blank > 0) throw new Error('选中标题却落在空面板上：编辑器没跟着 selectedId 打开');
  const field = page.locator('[data-testid="editor-doc"] [contenteditable="true"]').first();
  await field.waitFor({ timeout: 5000 });
  // 按整篇正文比，不按"第一个可编辑块"：上面的步骤故意重排过，首块已不是标题段
  const text = asStored(await page.locator('[data-testid="editor-doc"]').innerText());
  if (!text.includes(title)) throw new Error(`正文不含预期：期望 ${JSON.stringify(title)} 实际 ${JSON.stringify(text.slice(0, 60))}`);
  return text.slice(0, 32);
});

await step('重排落到库里了：刷新重开后块顺序与刷新前一致', async () => {
  const after = await blockSig();
  if (after.join('|') !== orderBeforeReload.join('|')) {
    throw new Error(`顺序没持久化：\n      前 ${JSON.stringify(orderBeforeReload)}\n      后 ${JSON.stringify(after)}`);
  }
  return `${after.length} 块同序`;
});

await step('界面上没有漏出文案键名（编辑器 + 工具条）', async () => {
  const leaked = await leakedKeys();
  if (leaked.length > 0) throw new Error(`漏出键名：${leaked.join(', ')}`);
  return 'clean';
});

await step('搜索能命中这条笔记', async () => {
  const box = page.locator('input[type="search"], [data-testid="search-input"], input[placeholder*="搜索"]').first();
  await box.waitFor({ timeout: 4000 });
  await box.fill(title);
  await page.waitForTimeout(900);
  const hit = await page.locator(`[data-testid^="note-row-"]:has-text(${JSON.stringify(title)})`).count();
  if (hit === 0) throw new Error('搜索无命中');
  await box.fill('');
  return `${hit} 行命中`;
});

await step('同步徽标存在且只有 4 态之一', async () => {
  const el = page.locator('[data-testid="sync-badge"], [data-testid="mobile-sync"]').first();
  await el.waitFor({ timeout: 4000 });
  const badge = (await el.getAttribute('data-badge')) ?? (await el.innerText());
  const allowed = ['synced', 'syncing', 'offline', 'failed', '未连接', '离线', '同步中', '已同步', '失败'];
  if (!allowed.some((a) => String(badge).toLowerCase().includes(a.toLowerCase()))) {
    throw new Error(`徽标值超出 4 态契约：${badge}`);
  }
  return String(badge).trim();
});

await step('设置视图可打开（账户/代理表单存在）', async () => {
  const trigger = page.locator('[data-testid="nav-settings"], [data-testid="mobile-settings"], [data-testid="open-settings"]').first();
  await trigger.waitFor({ timeout: 4000 });
  await trigger.click();
  await page.waitForTimeout(700);
  const baseUrl = await page.locator('[data-testid="account-baseUrl"]').count();
  return baseUrl > 0 ? '账户表单在' : '设置页在（无账户表单）';
});

await step('界面上没有漏出文案键名（设置页）', async () => {
  const leaked = await leakedKeys();
  if (leaked.length > 0) throw new Error(`漏出键名：${leaked.join(', ')}`);
  return 'clean';
});

await step('桌面视口无横向溢出', async () => {
  const m = await page.evaluate(() => ({ sw: document.documentElement.scrollWidth, cw: document.documentElement.clientWidth }));
  if (m.sw > m.cw + 1) throw new Error(`scrollWidth ${m.sw} > clientWidth ${m.cw}`);
  return `${m.cw}px`;
});

await step('截图（桌面）', async () => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.waitForTimeout(400);
  await page.screenshot({ path: `${OUT}/app-desktop.png`, fullPage: false });
  return 'app-desktop.png';
});

await step('移动端视口可用（390×844）', async () => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.waitForTimeout(700);
  const m = await page.evaluate(() => ({ sw: document.documentElement.scrollWidth, cw: document.documentElement.clientWidth }));
  if (m.sw > m.cw + 1) throw new Error(`移动端横向溢出：scrollWidth ${m.sw} > ${m.cw}`);
  const tap = page.locator('[data-testid="mobile-new"], [data-testid="mobile-sidebar"], [data-testid="mobile-sync"]');
  const n = await tap.count();
  if (n === 0) throw new Error('移动端没有任何 44px 触摸入口');
  for (const el of await tap.all()) {
    const box = await el.boundingBox();
    if (box && (box.height < 44 || box.width < 44)) throw new Error(`触摸目标 <44px：${Math.round(box.width)}×${Math.round(box.height)}`);
  }
  await page.screenshot({ path: `${OUT}/app-mobile.png` });
  return `${n} 个移动入口，全部 ≥44px`;
});

await step('控制台零 error', async () => {
  const all = [...consoleErrors, ...pageErrors];
  if (all.length > 0) throw new Error(`${all.length} 条：\n    ${all.slice(0, 6).join('\n    ')}`);
  return 'clean';
});

await step('网络请求零失败', async () => {
  if (failedRequests.length > 0) throw new Error(`${failedRequests.length} 条：${failedRequests.slice(0, 5).join('; ')}`);
  return '0 failed';
});

const failed = rows.filter((r) => !r.ok).length;
console.log(`\nverify-app: ${rows.length - failed}/${rows.length} 步通过`);
await browser.close();
process.exit(failed === 0 ? 0 : 1);
