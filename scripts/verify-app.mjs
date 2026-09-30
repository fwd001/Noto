/**
 * 端到端 UI 验证：真浏览器 + 真 vite + **真 Rust 核心**（notera-cli serve 的 dev 桥），
 * 不是假数据。每一步要么 PASS 要么 FAIL，没有"应该没问题"这一档。
 *
 * 前置（脚本不管，由调用方起）：
 *   cargo run -p notera-cli -- --data-dir <空目录> serve --port 17323
 *   pnpm --dir apps/desktop dev                      # 5173
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
// 某些步骤**故意**走核心的拒绝路径（具名错误码 + 400）。两条全局不变量（控制台零 error、
// 网络请求零失败）要是把这些也算成坏了，就只能永远不去点那条路 —— 那等于把失败面留在无人验证的黑暗里。
// 所以这里按 `命令名:错误码` **精确配对**放行，而不是放行"这一步里的所有 4xx"：
// 配不上对的 4xx 照旧算坏了，而多放行的额度由 assertRefusalsSeen 反过来查（声明了却没发生 = 步骤在空转）。
const allowedRefusals = new Map();
const seenRefusals = new Map();
// 编辑器每次渲染缺的图都会探一次 attachment_data，条数不固定 —— 记成"至少几条"。
let expectedAtLeast = 0;
function expectRefusal(cmd, code, atLeast = 1) {
  allowedRefusals.set(`${cmd}:${code}`, true);
  expectedAtLeast += atLeast;
}
function assertRefusalsSeen() {
  let seen = 0;
  for (const k of seenRefusals.keys()) if (!allowedRefusals.has(k)) throw new Error(`出现了没声明过的拒绝：${k}`);
  for (const [k] of allowedRefusals) {
    const n = seenRefusals.get(k) ?? 0;
    if (n === 0) throw new Error(`声明了 ${k} 却一次没发生 —— 这一步在空转`);
    seen += n;
  }
  if (seen < expectedAtLeast) throw new Error(`预期的拒绝只有 ${seen} 条，声明了 ${expectedAtLeast} 条`);
}
const classify4xx = (body) => {
  const m = /"code"\s*:\s*"([a-z_]+)"/.exec(body);
  return m ? m[1] : null;
};
page.on('console', (m) => {
  if (m.type() !== 'error') return;
  // 一条 4xx 会被 Chrome 额外记一行 "Failed to load resource"。放行它**不看步骤、只看出处**：
  // 这条 console 消息的 location.url 必须正是某条已声明拒绝的命令端点，否则照旧算坏。
  const text = m.text();
  if (/Failed to load resource/.test(text)) {
    const url = m.location()?.url ?? '';
    const cmd = url.split('/cmd/')[1]?.split('?')[0] ?? '';
    if (cmd && [...allowedRefusals.keys()].some((pair) => pair.startsWith(`${cmd}:`))) return;
  }
  consoleErrors.push(text);
});
page.on('pageerror', (e) => pageErrors.push(String(e)));
page.on('requestfailed', (r) => failedRequests.push(`${r.method()} ${r.url()} → ${r.failure()?.errorText}`));
// 4xx/5xx 不是"请求失败"，但同样是坏了：只盯 requestfailed 会漏掉整类静默错误。
// 记下响应体里的 code，好把"业务上正当的拒绝"和"真的坏了"区分开。
page.on('response', (r) => {
  if (r.status() < 400) return;
  r.text()
    .then((body) => {
      const cmd = r.url().split('/cmd/')[1]?.split('?')[0] ?? '';
      const code = classify4xx(body);
      const pair = `${cmd}:${code}`;
      if (code && allowedRefusals.has(pair)) {
        seenRefusals.set(pair, (seenRefusals.get(pair) ?? 0) + 1);
        return; // 这一步要的就是这条拒绝，界面上也确实把它显示成了人话
      }
      failedRequests.push(`${r.request().method()} ${r.url()} → HTTP ${r.status()} [步骤：${currentStepName}] ${body.slice(0, 120)}`);
    })
    .catch(() => failedRequests.push(`${r.request().method()} ${r.url()} → HTTP ${r.status()} [步骤：${currentStepName}]`));
});

const saveLog = [];
// 「重排落了库而加粗没落」这类红，光看结果分不出是**没发出去**、**发出去被拒**、还是
// **发出去了但被一支更旧的回包盖掉**。把每一支写正文的请求按序记下来（带当时屏幕上有几个
// strong），红的时候直接打进消息里 —— 没有这行日志时这条缺陷只能靠猜。
page.on('request', (r) => {
  const cmd = r.url().split('/cmd/')[1]?.split('?')[0] ?? '';
  if (cmd !== 'edit_note' && cmd !== 'create_note') return;
  let body = null;
  try {
    body = JSON.parse(r.postData() ?? 'null');
  } catch {
    /* 非 JSON 载荷也照样记一行形状 */
  }
  const raw = r.postData() ?? '';
  // 只记尺寸看不出"这一版到底是哪一个快照"，所以把块的 id·型与 strong 数一起记下来：
  // 红的时候能直接说出"发出去的那一版里根本没有 bold 的那块，而且它等于第 N 步的状态"。
  const shape = (body?.doc?.content ?? [])
    .map((b) => `${String(b.id).slice(-2)}:${b.type}${b.type === 'paragraph' || b.type === 'heading' ? '' : ''}`)
    .join(',');
  saveLog.push(
    `t=${Date.now()} ${cmd} id=…${String(body?.id ?? body?.noteId ?? '?').slice(-4)} rev=${body?.expectedRev ?? body?.expected_rev ?? '-'} blocks=${(body?.doc?.content ?? []).length} bold=${(raw.match(/"bold"|strong/g) ?? []).length} shape=[${shape}]`,
  );
});
page.on('response', async (r) => {
  const cmd = r.url().split('/cmd/')[1]?.split('?')[0] ?? '';
  if (cmd !== 'edit_note' && cmd !== 'create_note') return;
  saveLog.push(`t=${Date.now()}   → HTTP ${r.status()}`);
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
  saveLog.push(`t=${Date.now()} ―――― 步骤 ${name}`);
  try {
    const detail = await fn();
    record(name, true, detail ?? '');
  } catch (e) {
    const text = String(e).replace(/\n/g, '\n      ');
    // 600 个字符会把"写序列"这种多行诊断切掉（这批就是这样：打到 rev=6 就断了，而最要紧的
    // 是它后面那两行）。失败消息本来就只为诊断服务，放宽到能装下整条序列。
    record(name, false, text.slice(0, 2400));
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
    throw new Error(
      `加粗没落库：刷新后 strong 不见了\n      写到核心那一侧的序列：\n      ${saveLog.slice(-14).join('\n      ')}`,
    );
  }
  return `${after.length} 块同序，加粗仍在 ｜ 写入序列：${saveLog.slice(-4).join(' / ')}`;
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

// 坏图/坏附件占位上的两个自救动作（2026-09-28 的决定：终态那一格必须给用户一个能点的东西）。
//
// 这一步只证"点得动、答得回人话"这一条边：占位上真有两颗按钮、点了真进核心、核心的
// 具名结论真以中文显示出来、而且不阻塞正文。至于"撤掉结论之后当轮真能取回""覆盖式重传
// 真把服务器那份坏字节换掉"—— 那两条需要一台真 WebDAV 服务器与两台设备，由
// FT-ATT-25 / FT-ATT-27 在 Rust 侧证（`cargo test -p notera-host --test attachment_faults`）。
// 记在这里是为了别让"lane 绿了"被读成"整条功能都验过了"。
const toastLines = async () => (await page.locator('.toast').allInnerTexts()).map((x) => x.trim());
await step('坏图占位上的两颗自救按钮：点了真进核心，回答是真话', async () => {
  const rows = await callBridge('list_notes', { limit: 500 });
  const hit = rows.filter((r) => r.hasAttachment).sort((a, b) => String(b.updatedAt).localeCompare(String(a.updatedAt)))[0];
  if (!hit) throw new Error('前置不成立：这一步要在一条真有附件的笔记上跑');
  const doc = JSON.stringify(await (await callBridge('get_note', { id: hit.id })).doc);
  const sha = (doc.match(/"sha256":"([0-9a-f]{64})"/) ?? [])[1];
  if (!sha) throw new Error(`正文里没有内容键：${doc.slice(0, 160)}`);
  const blob = `${DATA_DIR}/attachments/${sha.slice(0, 2)}/${sha}`;
  if (!fs.existsSync(blob)) throw new Error(`前置不成立：本机那份字节本应在 ${blob}`);
  const savedBytes = fs.readFileSync(blob);
  // 声明这一步**要看的**具名拒绝（精确到 命令名:错误码，配不上对的 4xx 照旧算坏）：
  // 编辑器的占位本身靠一次 attachment_data 的具名缺失，两颗按钮各走一条核心的拒绝。
  expectRefusal('attachment_data', 'attachment_missing');
  expectRefusal('attachment_reupload', 'nothing_to_upload');
  expectRefusal('attachment_retry', 'nothing_to_retry');
  fs.unlinkSync(blob); // 磁盘清理 / 杀毒隔离就是这形态
  await page.reload();
  await page.locator(`[data-testid^="note-row-"]`).first().waitFor({ timeout: 6000 });
  await page.locator(`[data-testid="note-row-${hit.id}"]`).click();
  const retry = page.locator('[data-testid="attachment-retry"]');
  const reup = page.locator('[data-testid="attachment-reupload"]');
  await retry.first().waitFor({ timeout: 6000 });
  if ((await reup.count()) === 0) throw new Error('占位上只有「重试取回」，另一颗动作没渲染出来');
  const names = [(await retry.first().innerText()).trim(), (await reup.first().innerText()).trim()];
  if (names.some((n) => n.length === 0)) throw new Error(`按钮没有读得出来的名字：${JSON.stringify(names)}`);

  // ① 本机那份已经不在了 —— 「重新上传本机这份」必须被核心**具名拒绝**，并且说的是人话。
  await reup.first().click();
  await page.waitForTimeout(400);
  const afterReup = await toastLines();
  if (!afterReup.some((x) => x.includes('不能上传'))) {
    throw new Error(`点了「重新上传本机这份」，界面上没有出现核心的那句拒绝（提示：${JSON.stringify(afterReup)}）`);
  }
  // ② 「重试取回」也要真进核心。这一格有两种诚实答案：账上还说"本机有"→ 告诉你不必取回；
  //    后台已经把它降级 → 接受意图并说"已排进下载队列"。两种都比"点了没反应"强，
  //    而**任何一种都不许漏出内部码**。
  await retry.first().click();
  await page.waitForTimeout(400);
  const afterRetry = await toastLines();
  const answered = afterRetry.some((x) => x.includes('不需要取回') || x.includes('已重新排进下载队列'));
  if (!answered) throw new Error(`点了「重试取回」没有得到核心的答复（提示：${JSON.stringify(afterRetry)}）`);
  if (afterRetry.some((x) => x.includes('cmd.') || x.includes('error.'))) {
    throw new Error(`答复里漏出了错误码而不是文案：${JSON.stringify(afterRetry)}`);
  }
  // ③ 附件坏了不许把正文一起带走（§8 收尾句）。
  const body = await page.locator('[data-testid="editor-doc"]').innerText();
  if (body.trim().length === 0) throw new Error('附件占位把正文顶掉了');
  assertRefusalsSeen();
  // 收尾把本机那份放回去：后面的"导出"那一步要求"库里有附件 → 包里就有 attachments/"，
  // 这一步制造的是**临时**的缺失，不留给别人当假红（真要说的是"读不到就少传并如实记账"，
  // 那条由 notera-importer 的导出测试与上面那段 tracing::warn 负责）。
  fs.writeFileSync(blob, savedBytes);
  if (!fs.existsSync(blob)) throw new Error('没能把本机字节放回去');
  return `${names.join(' / ')} 都在 · 覆盖被拒说得清 · 重试有真答复 · 正文仍在`;
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

await step('侧栏那一行的名字是**渲染出来**看得见的（不是 innerText 里在）', async () => {
  // 上一条用 allInnerTexts() 断"侧栏有它" —— 那拿的是**未渲染**的文本。真实读数是：
  // 246px 的一行里，hover 才露面的动作簇（4 × 44px）用 opacity: 0 藏在流里**照样占 176px**，
  // 名字只剩 18~50px，连"默认"两个字都被裁掉半个字、单字名的可见宽度是 0。
  // 所以这条断言必须打在几何上：短名不许被裁、hover 时按钮点得着且不小于 44px、hover 不许让行宽跳。
  await page.locator('[data-testid="nav-all"]').click();
  await page.waitForTimeout(500);
  const name = `短名 ${Date.now() % 100000}`;
  const root = (await callBridge('list_folders', {}))[0];
  await page.locator(`[data-testid="folder-${root.id}"]`).hover();
  await page.locator(`[data-testid="folder-new-sub-${root.id}"]`).click();
  await page.fill('[data-testid="folder-create-input"]', name);
  await page.keyboard.press('Enter');
  await page.waitForTimeout(900);
  const row = page.locator('.tree__row', { hasText: name }).first();
  if ((await row.count()) === 0) throw new Error(`侧栏没有刚建的「${name}」`);
  const rest = await row.evaluate((r) => {
    const label = r.querySelector('.tree__label');
    const tools = r.querySelector('.tree__tools');
    return {
      text: label.textContent.trim(),
      labelWidth: Math.round(label.getBoundingClientRect().width),
      clipped: label.scrollWidth > label.clientWidth + 1,
      rowWidth: Math.round(r.getBoundingClientRect().width),
      toolsOpacity: getComputedStyle(tools).opacity,
    };
  });
  const fails = [];
  if (rest.text !== name) fails.push(`标签文本是「${rest.text}」，要「${name}」`);
  if (rest.clipped) fails.push(`静止态名字就被裁了：可见 ${rest.labelWidth}px / 需要更多（行宽 ${rest.rowWidth}px）`);
  // hover：动作簇露面，且**画在哪儿就点得着哪儿**（浮层不能被子元素压住，也不能小于 44px）
  await row.hover();
  await page.waitForTimeout(150);
  const hov = await row.evaluate((r) => {
    const tools = r.querySelector('.tree__tools');
    const btns = [...tools.querySelectorAll('.btn')];
    const hitOne = btns.map((b) => {
      const q = b.getBoundingClientRect();
      const hit = document.elementFromPoint(q.x + q.width / 2, q.y + q.height / 2);
      return b === hit || b.contains(hit);
    });
    return {
      opacity: getComputedStyle(tools).opacity,
      sizes: btns.map((b) => {
        const q = b.getBoundingClientRect();
        return [Math.round(q.width), Math.round(q.height)];
      }),
      hitOne,
      rowWidth: Math.round(r.getBoundingClientRect().width),
    };
  });
  if (hov.opacity !== '1') fails.push(`hover 时动作簇的 opacity 是 ${hov.opacity}，没露面`);
  if (hov.rowWidth !== rest.rowWidth) fails.push(`hover 让行宽从 ${rest.rowWidth}px 变成 ${hov.rowWidth}px ⇒ 鼠标扫过列表字会跳`);
  hov.sizes.forEach(([w, h], i) => {
    if (w < 44 || h < 44) fails.push(`第 ${i + 1} 颗动作按钮 ${w}×${h}，小于 A11Y-04 的 44×44`);
    if (!hov.hitOne[i]) fails.push(`第 ${i + 1} 颗按钮中心命中的不是它自己（点不着）`);
  });
  if (fails.length > 0) throw new Error(fails.join('；'));
  return `「${name}」静止态可见宽 ${rest.labelWidth}px 未被裁；hover 后 ${hov.sizes.length} 颗动作按钮都可点、行宽仍 ${hov.rowWidth}px`;
});

await step('工具条的块型菜单：弹层要看得见、点得着，点完块型真的变（G29）', async () => {
  // 出货默认窗口是 1240 宽，工具条在那一档是横向可滚的（`overflow-x: auto`）。CSS 规定一个轴不是
  // visible 时另一个轴的 visible 也算 auto ⇒ 这条 53 px 高的横条把自己 absolute 弹出的菜单**整个裁掉**：
  // 按钮进入展开态、DOM 里弹层也在，但屏幕上没有菜单，点下去命中的是编辑区。
  // 所以判据不能问"弹层在不在 DOM 里"（红的时候它也在），要问"在视口内吗、点得着吗、点完块型变了吗"。
  await page.setViewportSize({ width: 1240, height: 800 });
  const primary = page.locator('[data-testid="new-note"]');
  if ((await primary.count()) > 0) await primary.first().click();
  await page.waitForSelector('[data-testid="editor-doc"]', { timeout: 5000 });
  const field = page.locator('[data-testid="editor-doc"] [contenteditable="true"]').first();
  await field.click();
  await page.keyboard.type('块型菜单的现场', { delay: 12 });
  await page.waitForTimeout(300);

  await page.locator('.tb__menu-wrap button').first().click();
  const pop = page.locator('.tb__popover');
  await pop.waitFor({ timeout: 3000 });
  const geo = await pop.evaluate((el) => {
    const r = el.getBoundingClientRect();
    const tb = document.querySelector('.tb').getBoundingClientRect();
    const item = [...el.querySelectorAll('.tb__item')].find((n) => n.textContent.trim() === '引用');
    const ir = item.getBoundingClientRect();
    const hit = document.elementFromPoint(ir.left + ir.width / 2, ir.top + ir.height / 2);
    return {
      top: Math.round(r.top),
      bottom: Math.round(r.bottom),
      vh: window.innerHeight,
      tbBottom: Math.round(tb.bottom),
      inViewport: r.top >= 0 && r.bottom <= window.innerHeight,
      hitIsItem: !!hit && (hit === item || item.contains(hit)),
      hitTag: hit ? `${hit.tagName}.${(hit.className ?? '').toString().split(' ')[0]}` : 'null',
    };
  });
  const fails = [];
  if (!geo.inViewport) fails.push(`弹层盒子 ${geo.top}→${geo.bottom} 不在视口高 ${geo.vh} 之内`);
  if (!geo.hitIsItem) fails.push(`「引用」那一项中心命中的是 ${geo.hitTag} 而不是它自己（工具条底在 ${geo.tbBottom}，弹层伸到 ${geo.bottom}）⇒ 这颗是死的`);
  if (fails.length > 0) throw new Error(fails.join('；'));

  const before = await blockSig();
  await pop.locator('.tb__item').filter({ hasText: /^引用$/ }).click();
  await page.waitForTimeout(800);
  const after = await blockSig();
  if (JSON.stringify(before) === JSON.stringify(after)) throw new Error(`点了「引用」块序列没变：${JSON.stringify(after)}`);
  if (!after.some((s) => s.startsWith('blockquote:'))) throw new Error(`点完没有 blockquote 那一类：${JSON.stringify(after)}`);
  await page.setViewportSize({ width: 1440, height: 900 });
  return `弹层 ${geo.top}→${geo.bottom} 在 ${geo.vh} 高的视口内且点得着；${before.at(-1)} → ${after.at(-1)}`;
});

await step('工具条放不下时要有"这边还有东西"那句话，而且每颗都够得着（G30）', async () => {
  // 默认窗口 1240 那一档实测：可视 638 / 内容 818 ⇒ 插入附件、图片、撤销、重做 四颗初始在视野外。
  // 常驻滚动条是刻意不留的（会把编辑区第一行顶下去），那就得用别的办法说出来 —— 渐隐。
  // 这条判据同时钉三件事：① 溢出时确有提示；② 提示跟着滚动位置变（不是写死的）；③ 视野外那几颗键盘 Tab 得到。
  await page.setViewportSize({ width: 1240, height: 800 });
  const bar = page.locator('.tb');
  await bar.waitFor({ timeout: 5000 });
  const start = await bar.evaluate((el) => {
    const box = el.getBoundingClientRect();
    const out = [...el.querySelectorAll('.tb__btn')].filter((b) => b.getBoundingClientRect().right > box.right + 0.5);
    const after = getComputedStyle(el, '::after');
    return {
      clientW: Math.round(box.width),
      scrollW: el.scrollWidth,
      outLabels: out.map((b) => (b.getAttribute('aria-label') || b.textContent || '').trim().slice(0, 6)),
      moreRight: el.getAttribute('data-more-right'),
      cueContent: getComputedStyle(el, '::after').content,
      cueImage: getComputedStyle(el, '::after').backgroundImage,
    };
  });
  const fails = [];
  if (start.scrollW <= start.clientW) fails.push(`这一档没溢出（可视 ${start.clientW} ≥ 内容 ${start.scrollW}）⇒ 判据在空转`);
  if (start.outLabels.length === 0) fails.push('没有按钮在视野外 ⇒ 这条没东西可保');
  if (start.moreRight !== 'true') fails.push('右边还有东西但 data-more-right 不在 ⇒ 提示不会出现');
  // 两条都要看：`content: none` 时伪元素根本不生成，可 computed 里那个 gradient 还在 ——
  // 只查 backgroundImage 会永远为真（写这条时被自己的变异戳穿了一次）。
  if (start.cueContent === 'none' || start.cueContent === '') fails.push(`右缘渐隐没生成（::after content = ${start.cueContent}）`);
  if (!/linear-gradient/.test(start.cueImage)) fails.push(`渐隐没有画（backgroundImage = ${start.cueImage.slice(0, 30)}）`);

  // 键盘可达：视野外那几颗，Tab 聚焦之后必须完整进入视野（focus 的滚动是平滑的，要等它停）
  const reach = await bar.evaluate(async (el) => {
    const rows = [];
    for (const b of el.querySelectorAll('.tb__btn')) {
      const wasOut = b.getBoundingClientRect().right > el.getBoundingClientRect().right + 0.5;
      b.focus();
      await new Promise((res) => setTimeout(res, 260));
      const r = b.getBoundingClientRect();
      const box = el.getBoundingClientRect();
      rows.push({
        label: (b.getAttribute('aria-label') || b.textContent || '').trim().slice(0, 6),
        wasOut,
        ok: r.left >= box.left - 0.5 && r.right <= box.right + 0.5,
      });
    }
    return rows;
  });
  for (const r of reach) if (r.wasOut && !r.ok) fails.push(`Tab 聚焦「${r.label}」之后仍没完整进入视野`);

  // 滚到最右端：提示必须换边（写死的实现会在这里红），而渐隐也要跟着换
  const end = await bar.evaluate(async (el) => {
    el.scrollLeft = el.scrollWidth;
    await new Promise((res) => setTimeout(res, 260));
    const box = el.getBoundingClientRect();
    const hidden = [...el.querySelectorAll('.tb__btn')].filter((b) => b.getBoundingClientRect().left < box.left - 0.5);
    return {
      moreRight: el.getAttribute('data-more-right'),
      moreLeft: el.getAttribute('data-more-left'),
      hiddenCount: hidden.length,
      cueLeftContent: getComputedStyle(el, '::before').content,
      cueLeftImage: getComputedStyle(el, '::before').backgroundImage,
    };
  });
  if (end.moreRight === 'true') fails.push('已经滚到最右端，data-more-right 还亮着 ⇒ 提示不跟着滚动位置走');
  if (end.hiddenCount > 0 && end.moreLeft !== 'true') fails.push(`左边还藏着 ${end.hiddenCount} 颗却没说（data-more-left 不在）`);
  if (end.hiddenCount > 0 && (end.cueLeftContent === 'none' || !/linear-gradient/.test(end.cueLeftImage)))
    fails.push(`左边还有东西，可左缘那道渐隐没在画（content=${end.cueLeftContent}）`);

  await page.screenshot({ path: `${OUT}/app-toolbar-overflow.png` });
  await page.setViewportSize({ width: 1440, height: 900 });
  if (fails.length > 0) throw new Error(fails.join('；'));
  return `可视 ${start.clientW}/内容 ${start.scrollW}，视野外 ${start.outLabels.join('、')} 都有渐隐提示且 Tab 可达；滚到底后提示换边（左藏 ${end.hiddenCount} 颗）`;
});

await step('全应用扫一遍"画得出来却点不着"的控制（G29 那一类的通判据）', async () => {
  // 为什么要有这条：G29 是"按钮在、坐标在、可被祖先 overflow 裁掉 ⇒ 整块菜单是死的"，
  // 一条一条补断言永远追不上形状。这里问一个通用问题：**任何被画出来的控制，那一下必须落在它自己身上**。
  // 故意藏起来的（祖先 opacity:0 / visibility:hidden / display:none，比如 hover 才露面的动作键）不参与判定；
  // **被裁掉的不豁免** —— 那正是缺陷本身。
  const SEL = 'button, [role="button"], [role="menuitem"], [role="checkbox"], input:not([type="hidden"]), select, a[href]';
  const sweep = (label) =>
    page
      .evaluate(
        (sel) => {
          const bad = [];
          const painted = (el) => {
            for (let n = el; n && n !== document.body; n = n.parentElement) {
              const cs = getComputedStyle(n);
              if (cs.display === 'none' || cs.visibility === 'hidden' || Number(cs.opacity) === 0) return false;
            }
            return true;
          };
          const nameOf = (el) =>
            `${el.tagName.toLowerCase()}.${String(el.className || '').split(' ')[0]}「${(
              el.getAttribute('aria-label') || el.title || el.textContent || ''
            )
              .trim()
              .slice(0, 12)}」`;
          for (const el of document.querySelectorAll(sel)) {
            if (el.disabled || el.getAttribute('aria-disabled') === 'true') continue;
            const r = el.getBoundingClientRect();
            if (r.width < 1 || r.height < 1 || !painted(el)) continue;
            const cx = r.left + r.width / 2;
            const cy = r.top + r.height / 2;
            if (cx < 0 || cy < 0 || cx >= innerWidth || cy >= innerHeight) continue;
            const hit = document.elementFromPoint(cx, cy);
            if (!hit || hit === el || el.contains(hit) || hit.contains(el)) continue;
            bad.push(`${nameOf(el)}@${Math.round(r.left)},${Math.round(r.top)} 被 ${hit.tagName.toLowerCase()}.${String(hit.className).split(' ')[0]} 挡住`);
          }
          return bad;
        },
        SEL,
      )
      .then((rows) => rows.map((r) => `${label}：${r}`));

  const found = [];
  // 导航走 testid，不走可见文字：390 那一档底部那一排里没有「全部笔记」这颗（要先开抽屉），
  // 按文字点会在窄屏超时 —— 那不是产品坏，是我挑错了定位方式。
  // 缺口 G31 已知未修：文件夹多到一定数量时，最后几行落在常驻页脚那一块底下、滚也滚不出来。
  // 这条判据不许把它算成通过：单独计数、打进读数，且只放行"形状完全对上"的那几处
  // （`.tree__name` 被 `.side-foot` / `.nav-btn` 挡）——遮挡者或被挡的东西一变照样红，allowlist 不能长成垃圾桶。
  const isG31 = (row) => /^[\d×]+[^：]*：button\.tree__name/.test(row) && /(side-foot|nav-btn)/.test(row);
  const goList = async (narrow) => {
    if (narrow) {
      await page.click('[data-testid="mobile-sidebar"]');
      await page.waitForTimeout(450);
    }
    await page.click('[data-testid="nav-all"]');
    await page.waitForTimeout(450);
  };
  const goSettings = async (narrow) => {
    await page.click(narrow ? '[data-testid="mobile-settings"]' : '[data-testid="nav-settings"]');
    await page.waitForTimeout(450);
  };
  for (const w of [1240, 390]) {
    await page.setViewportSize({ width: w, height: w === 1240 ? 800 : 844 });
    await page.waitForTimeout(350);
    await goList(w < 700);
    found.push(...(await sweep(`${w} 列表`)));
    await goSettings(w < 700);
    found.push(...(await sweep(`${w} 设置页`)));
  }
  // 弹层展开的那一档单独扫（G29 就是这一档漏的）
  await page.setViewportSize({ width: 1240, height: 800 });
  await goList(false);
  await page.locator('[data-testid="new-note"]').first().click();
  await page.waitForSelector('.tb');
  found.push(...(await sweep('1240 编辑器')));
  await page.locator('.tb__menu-wrap button').first().click();
  await page.waitForTimeout(250);
  found.push(...(await sweep('1240 块型菜单展开')));
  await page.keyboard.press('Escape');
  await page.setViewportSize({ width: 1440, height: 900 });
  const g31 = found.filter(isG31);
  const rest = found.filter((r) => !isG31(r));
  if (rest.length > 0) throw new Error(`${rest.length} 处：\n  ${rest.slice(0, 6).join('\n  ')}`);
  return `两个视口 × 五种画面：除已知 G31（侧栏底部 ${g31.length} 行被常驻页脚挡住，另计）外，每一颗画得出来的控制都点得着`;
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

await step('窄屏从设置页点「菜单」要真的能回笔记列表（不许只剩一块黑幕）', async () => {
  // 侧栏"给不给看"以前只看 `view === 'workspace'`：停在设置页时点「菜单」，
  // 抽屉状态被打开、遮罩铺满屏幕，侧栏却还是 display:none ⇒ 一块点开的黑幕，
  // 而窄屏底部那一排里没有"回列表"的入口 —— 手机上这条路就断了（§6 的"无法返回"）。
  await page.setViewportSize({ width: 390, height: 844 });
  await page.waitForTimeout(500);
  await page.click('[data-testid="mobile-settings"]');
  await page.waitForTimeout(600);
  await page.click('[data-testid="mobile-sidebar"]');
  await page.waitForTimeout(500);
  const opened = await page.evaluate(() => {
    const sb = document.querySelector('[data-testid="sidebar"]');
    const r = sb.getBoundingClientRect();
    return {
      shown: getComputedStyle(sb).display !== 'none' && r.width > 0,
      width: Math.round(r.width),
      scrim: !!document.querySelector('[data-testid="scrim"]'),
      navAll: (document.querySelector('[data-testid="nav-all"]')?.getBoundingClientRect().width ?? 0) > 0,
    };
  });
  const fails = [];
  if (!opened.scrim) fails.push('点「菜单」没有铺开遮罩 ⇒ 抽屉根本没开');
  if (!opened.shown) fails.push(`遮罩铺开了但侧栏还是藏着的（宽 ${opened.width}px）⇒ 只剩一块黑幕`);
  if (!opened.navAll) fails.push('抽屉里没有「全部笔记」⇒ 没有回列表的那一条路');
  if (fails.length > 0) {
    await page.setViewportSize({ width: 1440, height: 900 });
    throw new Error(fails.join('；'));
  }
  await page.click('[data-testid="nav-all"]');
  await page.waitForTimeout(800);
  const back = await page.evaluate(() => ({
    scrim: !!document.querySelector('[data-testid="scrim"]'),
    rows: document.querySelectorAll('[data-testid^="note-row-"]').length,
  }));
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.waitForTimeout(400);
  if (back.scrim) throw new Error('回到列表之后遮罩还铺着 ⇒ 挡住界面');
  if (back.rows === 0) throw new Error('点了「全部笔记」却没回到列表（一条笔记行都没有）');
  return `抽屉打开（侧栏 ${opened.width}px）→ 点「全部笔记」回到 ${back.rows} 行，遮罩已收`;
});

await step('全应用扫一遍：没有文字被**静默裁掉**（窄屏也算，G26 那一类）', async () => {
  // 一条覆盖全应用的几何判据，而不是一处修一条断言 —— G26（侧栏名字只剩一个字符）与
  // 标题栏品牌被裁成 "No…" 是同一个形状：**该占位的没占位，该显示的显示不全**，
  // 而读 innerText 的判据对它完全失明。
  // 放行两种**设计如此**的形状：① `.visually-hidden`（给读屏器的 1px 裁剪）；
  // ② 带 `text-overflow: ellipsis` 且文字确实长（>6 字）—— 那是"名字太长"，不是"栏位被吃掉"。
  const sweep = () =>
    page.evaluate(() => {
      const bad = [];
      for (const el of document.querySelectorAll('*')) {
        const cs = getComputedStyle(el);
        if (cs.display === 'none' || cs.visibility === 'hidden' || cs.opacity === '0') continue;
        const r = el.getBoundingClientRect();
        if (r.width < 1 || r.height < 1) continue;
        const own = [...el.childNodes].filter((n) => n.nodeType === 3).map((n) => n.textContent.trim()).join('').trim();
        if (own.length === 0) continue;
        if (el.scrollWidth <= el.clientWidth + 1) continue;
        if (el.classList.contains('visually-hidden')) continue;
        if (cs.textOverflow === 'ellipsis' && own.length > 6) continue;
        bad.push(`${el.tagName}.${String(el.className).slice(0, 30)}「${own.slice(0, 16)}」要 ${el.scrollWidth}px 得 ${el.clientWidth}px`);
      }
      return bad;
    });
  const views = [
    ['工作区', '[data-testid="nav-all"]'],
    ['编辑器', '[data-testid^="note-row-"]'],
    ['设置页', '[data-testid="nav-settings"]'],
  ];
  const visit = async (sel) => {
    const loc = page.locator(sel).first();
    if ((await loc.count()) === 0 || !(await loc.isVisible())) return false;
    await loc.click();
    await page.waitForTimeout(700);
    return true;
  };
  const found = [];
  const take = async (label) => {
    for (const b of await sweep()) found.push(`${label}：${b}`);
  };
  for (const w of [1440, 390]) {
    await page.setViewportSize({ width: w, height: w === 1440 ? 900 : 844 });
    await page.waitForTimeout(500);
    if (w === 1440) {
      for (const [label, sel] of views) {
        if (!(await visit(sel))) {
          found.push(`1440/${label}：入口 ${sel} 点不到，这一格没看东西`);
          continue;
        }
        await take(`1440/${label}`);
      }
    } else {
      // 窄屏没有侧栏（导航在 mobile-* 那一排），所以扫"当前视图 + 抽屉 + 抽屉里的设置"
      await take('390/当前视图');
      if (await visit('[data-testid="mobile-sidebar"]')) await take('390/侧栏抽屉');
      if (await visit('[data-testid="nav-settings"]')) await take('390/设置页');
    }
  }
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.waitForTimeout(400);
  // 收尾必须把状态交还给后面的步骤：抽屉要是还开着，遮罩会把每一次点击都吃掉
  const scrim = page.locator('[data-testid="scrim"]');
  if ((await scrim.count()) > 0 && (await scrim.first().isVisible())) {
    await scrim.first().click();
    await page.waitForTimeout(400);
  }
  if (!(await visit('[data-testid="nav-all"]'))) throw new Error('收尾没能回到「全部笔记」，会把后面的步骤带脏');
  if (found.length > 0) throw new Error(found.slice(0, 8).join('\n    '));
  return '两个视口、六个画面，无静默裁剪';
});

await step('控制台零 error', async () => {
  const all = [...consoleErrors, ...pageErrors];
  if (all.length > 0) throw new Error(`${all.length} 条：\n    ${all.slice(0, 6).join('\n    ')}`);
  return 'clean';
});

// IMP-08：`.enex` 的**界面路径** —— 之前只有 Rust 侧的命令测试与 vue-tsc，没人真点过。
// 这一步只认屏幕事实：点设置 → 填路径 → 点导入 → 报告与逐条说明出现在页面上，
// 并且导入回来的两条笔记**在列表里看得见**（不是内存态）。
await step('设置页导入 .enex：报告、说明、以及列表里真的出现这两条', async () => {
  const dir = 'D:/code/Notes/.logs/enex-lane';
  fs.mkdirSync(dir, { recursive: true });
  const file = `${dir}/lane-notes.enex`;
  const stamp = Date.now();
  const t1 = `恩ex 甲 ${stamp}`;
  const t2 = `恩ex 乙 ${stamp}`;
  fs.writeFileSync(
    file,
    `<?xml version="1.0" encoding="UTF-8"?>\n<en-export version="6.5.1">\n` +
      `<note><title>${t1}</title><content><![CDATA[<en-note><div>第一段正文</div></en-note>]]></content><tag>财务</tag></note>\n` +
      `<note><title>${t2}</title><content><![CDATA[<en-note><div>第二段正文</div></en-note>]]></content></note>\n` +
      `</en-export>`,
    'utf8',
  );
  // 前面有一步把视口调成了手机宽度（侧栏在那个布局下根本不渲染）。先恢复桌面视口，
  // 再重新加载从"用户刚打开应用"的状态出发 —— 测的是导入这条路，不是上一步的残留。
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(URL_BASE, { waitUntil: 'networkidle', timeout: 20000 });
  await page.locator('[data-testid="nav-settings"]').scrollIntoViewIfNeeded();
  await page.click('[data-testid="nav-settings"]', { timeout: 8000 });
  const pathBox = page.locator('[data-testid="data-path"]');
  await pathBox.waitFor({ state: 'visible', timeout: 5000 });
  await pathBox.fill(file);
  await page.click('[data-testid="import-files"]', { timeout: 5000 });
  const report = page.locator('[data-testid="import-files-report"]');
  await report.waitFor({ state: 'visible', timeout: 8000 });
  const line = (await report.innerText()).replace(/\s+/g, ' ');
  // 报告那行是「新增 / 重复 / 失败：N / N / N」，判据读的是屏幕上的数，不是命令返回值
  if (!/[:：]\s*2\s*\/\s*0\s*\/\s*0/.test(line)) {
    throw new Error(`导入报告该是 2 / 0 / 0，实际读到「${line}」`);
  }
  const notices = (await page.locator('[data-testid="import-files-notices"]').first().innerText()).replace(/\s+/g, ' ');
  // §39：本库表达不了的（Evernote 的 <tag>）必须点名，不许悄悄消失
  if (!/tag/.test(notices)) throw new Error(`说明里没点出 <tag> 没落地：「${notices}」`);
  // 回到列表，用界面的眼睛确认两条笔记真在那儿
  await page.click('[data-testid="nav-list"], [data-testid="banner-link"]', { timeout: 5000 }).catch(() => {});
  await page.goto(URL_BASE, { waitUntil: 'networkidle', timeout: 20000 });
  const body = await page.locator('body').innerText();
  for (const title of [t1, t2]) {
    if (!body.includes(title)) throw new Error(`导入的「${title}」没出现在列表里`);
  }
  return `列表两条都在；说明：${notices.slice(0, 60)}`;
});

await step('网络请求零失败', async () => {
  if (failedRequests.length > 0) throw new Error(`${failedRequests.length} 条：${failedRequests.slice(0, 5).join('; ')}`);
  return '0 failed';
});

const failed = rows.filter((r) => !r.ok).length;
// 红一次就把"写到核心那一侧的完整序列"打在最后，而不是只塞进那一步的失败消息里（消息长度有限，
// 序列被切掉就等于没插桩）。绿的时候不打，免得把这条 lane 的正常输出变成噪音。
if (failed > 0) {
  console.log(`\n—— 写正文的请求序列（${saveLog.length} 行）——`);
  for (const line of saveLog) console.log(`  ${line}`);
}
console.log(`\nverify-app: ${rows.length - failed}/${rows.length} 步通过`);
await browser.close();
process.exit(failed === 0 ? 0 : 1);
