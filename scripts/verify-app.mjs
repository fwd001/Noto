/**
 * 端到端 UI 验证：真浏览器 + 真 vite + **真 Rust 核心**（notera-cli serve 的 dev 桥），
 * 不是假数据。每一步要么 PASS 要么 FAIL，没有"应该没问题"这一档。
 *
 * 前置（脚本不管，由调用方起）：
 *   cargo run -p notera-cli -- --data-dir <空目录> serve --port 17323
 *   npm --prefix apps/desktop run dev                      # 5173
 *
 *   node scripts/verify-app.mjs
 */
const PW = process.env.PW_CORE || 'file:///C:/Users/lhcz-fu/node_modules/playwright-core/index.js';
const CHROME = process.env.CHROME || 'C:/Users/lhcz-fu/AppData/Local/ms-playwright/chromium-1243/chrome-win64/chrome.exe';
const URL_BASE = process.env.APP_URL || 'http://127.0.0.1:5173';
const OUT = 'D:/code/Notes/docs/evidence';
// 备份/恢复两步要看盘上的真实产物，所以得知道本地核心用的是哪个数据目录
const DATA_DIR = process.env.DATA_DIR || 'D:/code/Notes/.logs/e2e-data';
const fs = await import('node:fs').then((m) => m.default);

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
// 4xx/5xx 不是"请求失败"，但同样是坏了：只盯 requestfailed 会漏掉整类静默错误。
// 记下响应体里的 code，好把"业务上正当的拒绝"和"真的坏了"区分开。
page.on('response', (r) => {
  if (r.status() < 400) return;
  r.text()
    .then((body) => failedRequests.push(`${r.request().method()} ${r.url()} → HTTP ${r.status()} [步骤：${currentStepName}] ${body.slice(0, 120)}`))
    .catch(() => failedRequests.push(`${r.request().method()} ${r.url()} → HTTP ${r.status()} [步骤：${currentStepName}]`));
});

const rows = [];
// 失败请求要能归位到"哪一步在做" —— 偶发的 4xx 只报 URL 与状态码，下次复现时仍然无从下手。
let currentStepName = '(未进入任何步骤)';
function record(step, ok, detail) {
  rows.push({ step, ok, detail });
  console.log(`${ok ? 'PASS' : 'FAIL'}  ${step}${detail ? `  —— ${detail}` : ''}`);
}
async function step(name, fn) {
  currentStepName = name;
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

// 桥侧读数：放在所有步骤之前定义，步骤里才不必在意声明顺序（TDZ）。
const callBridge = async (name, args = {}) => {
  const r = await fetch(`http://127.0.0.1:17323/cmd/${name}`, {
    method: 'POST',
    headers: { 'content-type': 'application/json', origin: URL_BASE },
    body: JSON.stringify(args),
  });
  return r.json();
};
const liveNotes = async () => {
  const rows = await callBridge('list_notes');
  if (!Array.isArray(rows)) throw new Error(`list_notes 没返回数组：${JSON.stringify(rows).slice(0, 120)}`);
  return rows;
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

await step('选中文字 → 浮动工具条出现、位置不压工具条、能加粗', async () => {
  const field = page.locator('[data-testid="editor-doc"] [data-type="heading"] [contenteditable="true"]').first();
  await field.click();
  await page.keyboard.press('Home');
  await page.keyboard.down('Shift');
  for (let i = 0; i < 3; i += 1) await page.keyboard.press('ArrowRight');
  await page.keyboard.up('Shift');
  await page.waitForTimeout(300);
  const bar = page.locator('[data-testid="selection-bar"]');
  const shown = await bar.waitFor({ timeout: 4000 }).then(() => true).catch(() => false);
  if (!shown) {
    const why = await page.evaluate(() => {
      const sel = window.getSelection();
      return {
        inDom: document.querySelectorAll('[data-testid="selection-bar"]').length,
        rangeCount: sel ? sel.rangeCount : -1,
        collapsed: sel ? sel.isCollapsed : null,
        text: sel ? String(sel).slice(0, 20) : null,
        anchorInContent: sel && sel.rangeCount
          ? !!sel.getRangeAt(0).commonAncestorContainer.parentElement?.closest('.nb-content')
          : null,
      };
    });
    throw new Error(`浮动条没出现：${JSON.stringify(why)}`);
  }
  const box = await bar.boundingBox();
  const doc = await page.locator('[data-testid="editor-doc"]').boundingBox();
  if (!box || !doc) throw new Error('量不到浮动条或编辑区');
  if (box.y < doc.y) throw new Error(`浮动条跑到了编辑区上方（y=${Math.round(box.y)} < ${Math.round(doc.y)}）`);
  await page.screenshot({ path: `${OUT}/app-selection-bar.png` });
  await page.locator('[data-testid="sel-bold"]').click();
  await page.waitForTimeout(700);
  if ((await page.locator('[data-testid="editor-doc"] strong').count()) === 0) throw new Error('点加粗没有生效');
  if ((await page.locator('[data-testid="selection-bar"]').count()) > 0) throw new Error('动作后浮动条应收起');
  return '出现 → 加粗生效 → 收起';
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
  if ((await page.locator('[data-testid="editor-doc"] strong').count()) === 0) {
    throw new Error('加粗没落库：刷新后 strong 不见了');
  }
  return `${after.length} 块同序，加粗仍在`;
});

await step('插入图片：真选一个文件 → 显示出来，而正文里只留内容键', async () => {
  // 这条边整条都是新的：曾经前端少发两个必填字段，点"插入图片/附件"必然 bad_args，
  // 而没有任何测试走过它。选文件用隐藏的 <input type=file>（WebView 里就是原生选择器），
  // 于是浏览器 dev 桥与真窗口是同一条代码路径 —— 这一步才谈得上"验过"。
  const png = Buffer.from(
    'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==',
    'base64',
  );
  const file = `${DATA_DIR}/e2e-attach.png`;
  fs.writeFileSync(file, png);
  await page.setInputFiles('[data-testid="attach-input"]', file);
  const img = page.locator('[data-testid="editor-doc"] img.nb-image');
  await img.last().waitFor({ timeout: 6000 });
  const src = (await img.last().getAttribute('src')) ?? '';
  if (!src.startsWith('data:image/png;base64,')) throw new Error(`图片没按附件的字节显示：${src.slice(0, 48)}`);
  await page.waitForTimeout(1600); // 等自动保存落库
  // 认笔记不能用标题：前面的步骤拖过块、转过代码块，标题是从"第一个文本块"派生的，
  // 到这一步早就不是当初那句 E2E 笔记了。附件位 + 最近更新时间才是确定的判据。
  const rows = await callBridge('list_notes', { limit: 500 });
  const withFile = rows.filter((r) => r.hasAttachment);
  const hit = withFile.sort((a, b) => String(b.updatedAt).localeCompare(String(a.updatedAt)))[0];
  if (!hit) throw new Error(`没有任何一条笔记带附件位（共 ${rows.length} 条）：${JSON.stringify(rows.slice(0, 3))}`);
  const doc = JSON.stringify(await (await callBridge('get_note', { id: hit.id })).doc);
  if (!doc.includes('sha256')) throw new Error(`正文里没有内容键，图就是凭空的：${doc.slice(0, 160)}`);
  // base64 进了 doc = 每个附件在正文里再存一份（+4/3 体积），还会跟着每次编辑同步走
  if (doc.includes('base64')) throw new Error('附件的 base64 被写进了正文');
  return `图片显示 ✓ · 正文只存 sha256 ✓ · ${png.length} 字节落盘`;
});

await step('界面上没有漏出文案键名（编辑器 + 工具条）', async () => {
  const leaked = await leakedKeys();
  if (leaked.length > 0) throw new Error(`漏出键名：${leaked.join(', ')}`);
  return 'clean';
});

await step('工具条不靠滚动条腾地方（窄栏下也不吃掉一行）', async () => {
  // 就在编辑器已经打开的地方量：设置页会藏掉侧栏，1100 以下也藏
  await page.setViewportSize({ width: 1100, height: 800 });
  await page.waitForTimeout(500);
  const m = await page.locator('.tb').first().evaluate((el) => ({
    lost: el.offsetHeight - el.clientHeight,
    scrollable: el.scrollWidth > el.clientWidth,
    w: el.clientWidth,
  }));
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.waitForTimeout(300);
  if (m.lost > 2) throw new Error(`工具条被横向滚动条吃掉 ${m.lost}px（应 0–2px 边框），栏宽 ${m.w}`);
  return m.scrollable ? `栏宽 ${m.w}px：溢出但滚动条不占布局（仍可滚）` : `栏宽 ${m.w}px：无需滚动`;
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

await step('回收站这条边：删除 → 回收站看得到 → 恢复 → 再删 → 彻底删除', async () => {
  // §9 的两级删除是数据安全的地基，而它在 UI 层从来没有端到端跑过：删除走工具栏、
  // 回收站走侧栏、恢复/永久删除走行内按钮，中间任何一条边的参数名或视图模式错了，
  // 用户看到的就是"删不掉"或者"恢复回来是空的"。用一条专门的笔记，不动前面步骤依赖的那条。
  const doomed = `回收站验证 ${Date.now()}`;
  const made = await callBridge('create_note', { doc: { v: 1, content: [{ id: 'blk000001', type: 'paragraph', content: [{ text: doomed }] }] } });
  const row = (name) => page.locator(`[data-testid^="note-row-"]:has-text(${JSON.stringify(name)})`);
  await page.locator('[data-testid="nav-all"]').click();
  await page.waitForTimeout(700);
  await row(doomed).first().waitFor({ timeout: 5000 });

  await row(doomed).first().click();
  await page.locator('[data-testid="trash-note"]').click();
  await page.waitForTimeout(800);
  if ((await row(doomed).count()) > 0) throw new Error('点了删除，它还留在列表里');

  await page.locator('[data-testid="nav-trash"]').click();
  await page.waitForTimeout(800);
  if ((await row(doomed).count()) === 0) throw new Error('回收站里没有它 —— list_notes(trash) 这条边没通');
  await page.locator('[data-testid="restore-note"]').first().click();
  await page.waitForTimeout(800);
  if ((await row(doomed).count()) > 0) throw new Error('按了恢复，它还留在回收站里');
  const live = await callBridge('list_notes', { limit: 500 });
  if (!live.some((r) => r.id === made.id)) throw new Error('界面说恢复了，库里却没有');

  await page.locator('[data-testid="nav-all"]').click();
  await page.waitForTimeout(700);
  await row(doomed).first().click();
  await page.locator('[data-testid="trash-note"]').click();
  await page.waitForTimeout(800);
  await page.locator('[data-testid="nav-trash"]').click();
  await page.waitForTimeout(800);
  await page.locator('[data-testid="purge-note"]').first().click();
  await page.waitForTimeout(300);
  await page.locator('[data-testid="purge-confirm"]').click();
  await page.waitForTimeout(900);
  if ((await row(doomed).count()) > 0) throw new Error('按了彻底删除，它还挂在回收站里');
  const trash = await callBridge('list_notes', { limit: 500, trash: true });
  if (trash.some((r) => r.id === made.id)) throw new Error('库里的回收站视图还留着它');
  let revived = null;
  try {
    revived = await callBridge('get_note', { id: made.id });
  } catch {
    /* not_found 正是永久删除该有的样子 */
  }
  if (revived) throw new Error(`永久删除之后 get_note 还回得来：${JSON.stringify(revived).slice(0, 120)}`);
  await page.locator('[data-testid="nav-all"]').click();
  await page.waitForTimeout(500);
  return '删除 · 回收站 · 恢复 · 永久删除 全通，删的这条已彻底消失';
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

await step('库统计真的接上了：五行都是数字，不是占位符', async () => {
  // 这条边曾经把存储层的 StoreStats 原样发出去（snake_case：notes_trash / outbox_pending），
  // 而界面读的是 notesInTrash / inflightOps。TS 类型是断言不是校验，于是这几行
  // 全成了 formatNumber(undefined) 的「—」，前端单测喂 camelCase 假数据恰好把它盖住。
  const card = page.locator('.stats');
  await card.waitFor({ timeout: 4000 });
  const text = await card.innerText();
  if (text.includes('—')) throw new Error(`统计里有没接上的字段（显示成占位符）：${text.replace(/\s+/g, ' ')}`);
  const numbered = text.split('\n').map((l) => l.trim()).filter((l) => /[0-9]/.test(l));
  if (numbered.length < 5) throw new Error(`期望 5 行带数字的统计，实际只有 ${numbered.length} 行：${text.replace(/\s+/g, ' ')}`);
  return numbered.join(' / ');
});

await step('界面上没有漏出文案键名（设置页）', async () => {
  const leaked = await leakedKeys();
  if (leaked.length > 0) throw new Error(`漏出键名：${leaked.join(', ')}`);
  return 'clean';
});

await step('备份：真产出一个可校验的快照文件', async () => {
  await page.locator('[data-testid="backup-db"]').click();
  await page.waitForTimeout(1500);
  const filled = await page.locator('[data-testid="data-path"]').inputValue();
  if (!filled.endsWith('.sqlite')) throw new Error(`备份后路径框没回填快照路径：${JSON.stringify(filled)}`);
  if (!fs.existsSync(filled)) throw new Error(`界面说备份在 ${filled}，但盘上没有`);
  const size = fs.statSync(filled).size;
  if (size < 4096) throw new Error(`备份只有 ${size} 字节，不像一个库`);
  const report = await page.locator('[data-testid="data-report"]').innerText();
  if (!/[0-9a-f]{8}/.test(report)) throw new Error(`报告里没有自证信息：${report}`);
  return `${size} 字节 · ${filled.split(/[\\/]/).pop()}`;
});

await step('恢复：只排期并留下可核对的标记，不静默改库', async () => {
  const before = fs.readFileSync(`${DATA_DIR}/notera.sqlite`);
  await page.locator('[data-testid="restore-db"]').click();
  await page.waitForTimeout(1200);
  const markerPath = `${DATA_DIR}/restore-pending.json`;
  if (!fs.existsSync(markerPath)) throw new Error('点了恢复却没有写下待恢复标记');
  const marker = JSON.parse(fs.readFileSync(markerPath, 'utf8'));
  if (!/^[0-9a-f]{64}$/.test(marker.sha256)) throw new Error(`标记里的 sha256 不合法：${marker.sha256}`);
  if (!fs.existsSync(marker.path)) throw new Error(`标记指向的备份不存在：${marker.path}`);
  if (Buffer.compare(fs.readFileSync(`${DATA_DIR}/notera.sqlite`), before) !== 0) {
    throw new Error('恢复排期阶段就改了当前库 —— 应当等到下次启动才落地');
  }
  fs.rmSync(markerPath);
  const toast = await page.locator('[data-testid="data-report"], .toast').allInnerTexts();
  return `已排期且现库未动 ${toast.join(' ').slice(0, 40)}`;
});

/** 直接问本地核心要活笔记列表：比"数界面上的行"稳，不受当前停在哪个视图影响。 */

await step('服务器能力块：刚配上时说的是"还没探过"，不是"不支持"', async () => {
  // 走 UI 填表保存：顺带覆盖"空 id 的草案要落对 sync_accounts 那一行"（此前会静默不出站）
  await page.fill('[data-testid="account-baseUrl"]', 'https://127.0.0.1:9/dav');
  await page.fill('[data-testid="account-username"]', 'notera-e2e');
  await page.fill('[data-testid="account-password"]', 'e2e-secret');
  await page.click('[data-testid="account-save"]');
  await page.waitForTimeout(1000);
  const box = page.locator('[data-testid="server-caps"]');
  if ((await box.count()) === 0) throw new Error('配好账户后没出现"服务器能力"这一块');
  const verdict = (await page.locator('[data-testid="server-caps-verdict"]').innerText()).trim();
  if (!verdict.includes('还没有')) throw new Error(`还没探测就被说成有结论了：「${verdict}」`);
  const acct = await callBridge('account');
  if (!acct || !acct.id) throw new Error(`保存后 /cmd/account 读不到账户：${JSON.stringify(acct).slice(0, 140)}`);
  // 核心的 Option<u32> 在 JSON 里是 null：下发非 null 的位图就等于编造探测结论
  if (acct.capMask !== null && acct.capMask !== undefined) throw new Error(`还没探测就不该下发 capMask：${JSON.stringify(acct).slice(0, 160)}`);
  if ((await page.locator('.caps__chip').count()) !== 0) throw new Error('还没探测就摆出了能力芯片');
  const leaked = await leakedKeys();
  await callBridge('remove_account', { id: acct.id });
  if (leaked.length > 0) throw new Error(`能力块漏出键名：${leaked.join(', ')}`);
  return `verdict=「${verdict.slice(0, 18)}…」 id=${String(acct.id).slice(0, 8)}（填表→保存→读回→清理）`;
});

await step('导出：真产出一个能读回来的 ZIP，且不盖掉刚才的备份', async () => {
  const backup = (await page.locator('[data-testid="data-path"]').inputValue()).trim();
  const before = (await liveNotes()).length;
  await page.locator('[data-testid="export-data"]').click();
  await page.waitForTimeout(1800);
  const report = await page.locator('[data-testid="data-report"]').innerText();
  const zip = report.match(/[A-Za-z]:[^\s]*\.zip/)?.[0];
  if (!zip) throw new Error(`导出报告里没有给出 ZIP 路径：${report}`);
  if (!fs.existsSync(zip)) throw new Error(`界面说导出到 ${zip}，盘上没有`);
  if (fs.readFileSync(zip).subarray(0, 2).toString('latin1') !== 'PK') throw new Error('导出的不是真 ZIP');
  if (fs.statSync(zip).size < 1024) throw new Error('包太小，不像装了整库');
  // 附件必须真的在包里。设置页那条边写死了 `includeAttachments: true`，而这里曾经
  // 只看"包能不能打开"，于是"一个附件都没带"的导出照样算绿（实测踩过：blob 落在
  // `<attachments>/<2hex>/<sha>` 两层目录里，按一层目录名筛 64hex 永远筛不到）。
  // 库里的附件数直接从刚才那张统计卡读 —— 顺带也证明那张卡接的是真数。
  const statsText = (await page.locator('.stats').innerText()).replace(/\s+/g, ' ');
  const wantAtt = Number((statsText.match(/(\d+) 个附件/) || [])[1] ?? 0);
  const hasAtt = fs.readFileSync(zip).subarray(0, 200000).includes('attachments/');
  if (wantAtt > 0 && !hasAtt) throw new Error(`库里有 ${wantAtt} 个附件，导出的包里却没有 attachments/ 条目（${statsText}）`);
  // 这条钉住一个真实事故：备份路径被回填到"输出位置"后，导出会正好盖掉那份备份
  if (!fs.existsSync(backup)) throw new Error(`导出把备份文件弄没了：${backup}`);
  if (fs.readFileSync(backup).subarray(0, 15).toString('latin1') !== 'SQLite format 3') throw new Error('备份文件被导出覆盖了');

  await page.locator('[data-testid="data-path"]').fill(zip);
  await page.locator('[data-testid="import-data"]').click();
  await page.waitForTimeout(2000);
  const after = (await liveNotes()).length;
  if (after !== before) throw new Error(`把同一个库导回自己，笔记数从 ${before} 变成 ${after}（应幂等）`);
  const ids = new Set((await liveNotes()).map((r) => r.id));
  if (ids.size !== after) throw new Error('id 集合与条数不符，说明有副本被造出来');
  return `${zip.split(/[\\/]/).pop()} · ${fs.statSync(zip).size} 字节 · 导入后仍 ${after} 条`;
});

await step('按文件夹导出：勾一个文件夹，包就只有那一棵子树', async () => {
  // 核心会算子树 + 祖先链，但"能不能用到"取决于界面有没有入口。这一步真的点一遍：
  // 打开开关 → 勾那个新文件夹 → 导出 → 报告必须自己说是子树包，而且内容确实只有那一棵。
  const name = `子树验证 ${Date.now()}`;
  const roots = await callBridge('list_folders', {});
  const sub = await callBridge('create_folder', { parentId: roots[0].id, name });
  await callBridge('create_note', {
    folderId: sub.id,
    doc: { v: 1, content: [{ id: 'blk000001', type: 'paragraph', content: [{ text: '只属于这棵子树' }] }] },
  });
  await page.locator('[data-testid="export-scoped"]').check();
  await page.locator(`[data-testid="export-folder-${sub.id}"]`).check();
  await page.locator('[data-testid="export-data"]').click();
  await page.waitForTimeout(1800);
  const report = await page.locator('[data-testid="data-report"]').innerText();
  if (!report.includes('子树')) throw new Error(`报告没说自己导的是子树包：${report}`);
  const direct = await callBridge('export_data', { folderIds: [sub.id], includeAttachments: true, path: `${DATA_DIR}/scoped-${Date.now()}.zip` });
  if (direct.scope !== 'folders') throw new Error(`core 报的范围不对：${JSON.stringify(direct)}`);
  if (direct.counts.notes !== 1) throw new Error(`子树包里就该只有那一篇：${JSON.stringify(direct.counts)}`);
  // 祖先链带着走，否则这份包导回干净库会因外键缺失整体失败
  if (direct.counts.folders < 2) throw new Error(`子树 + 祖先链至少两个文件夹：${JSON.stringify(direct.counts)}`);
  await page.locator('[data-testid="export-scoped"]').uncheck();
  return `子树 · ${direct.counts.folders} 个文件夹 · ${direct.counts.notes} 篇笔记`;
});

await step('子文件夹在界面上是看得见的：侧栏有它，"移动到"也选得到', async () => {
  // 后端把文件夹作为**嵌套树**下发，前端要规范化成自己的树。这一步走真 UI：
  // 在默认本下建一个子层 → 侧栏必须出现它 → 笔记的"移动到"下拉必须选得到它。
  // 曾经规范化只认平铺输入、把树里的 children 抹掉：单元测试全绿，产品里子文件夹整个隐形。
  await page.locator('[data-testid="nav-all"]').click();
  await page.waitForTimeout(500);
  const root = (await callBridge('list_folders', {}))[0];
  const name = `深层子夹 ${Date.now()}`;
  await page.locator(`[data-testid="folder-${root.id}"]`).hover();
  await page.locator(`[data-testid="folder-new-sub-${root.id}"]`).click();
  await page.fill('[data-testid="folder-create-input"]', name);
  await page.keyboard.press('Enter');
  await page.waitForTimeout(900);
  const labels = (await page.locator('.tree__label').allInnerTexts()).map((l) => l.trim());
  if (!labels.includes(name)) throw new Error(`侧栏没有刚建的子文件夹，只有：${labels.join(' / ')}`);
  // 建完子夹后当前视图就落到那个空文件夹上了，要验"移动到"得先回"全部"
  await page.locator('[data-testid="nav-all"]').click();
  await page.waitForTimeout(700);
  const listText = async () => (await page.locator('[data-testid="note-list"]').innerText().catch(() => '(没有列表区)')).replace(/\s+/g, ' ').slice(0, 160);
  let rows = page.locator('[data-testid^="note-row-"]');
  if ((await rows.count()) === 0) {
    // 前序步骤会删笔记、导包，列表空不空不该由本步赌运气：自己造一条
    await page.locator('[data-testid="new-note"]').click();
    await page.waitForTimeout(900);
    rows = page.locator('[data-testid^="note-row-"]');
  }
  if ((await rows.count()) === 0) throw new Error(`点了「新建笔记」列表还是空：${await listText()}`);
  await rows.first().click();
  await page.waitForTimeout(700);
  const options = (await page.locator('[data-testid="move-folder"] option').allInnerTexts()).map((o) => o.trim());
  if (options.length === 0) throw new Error(`打开笔记后没有「移动到」下拉：${await listText()}`);
  if (!options.some((o) => o.includes(name))) throw new Error(`"移动到"下拉里找不到这个子文件夹：${options.join(' | ')}`);
  return `侧栏与下拉都认得「${name}」（路径 ${options.find((o) => o.includes(name))}）`;
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
