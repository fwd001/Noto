/**
 * 布局门禁：量**渲染后的几何**，不量 DOM 里有没有那个类。
 *
 * 钉五件用户实际看得见的性质（都是本轮从真读数里提出来的）：
 *  ① 设置页一栏到底：所有卡片左缘与宽度**同一个值**，且在 900→1800 之间不随视口跳列
 *     （旧写法 `auto-fit minmax(360px,1fr)` 实测 900/1100 出 2 列、1440 出 3 列、1800 出 **4 列**，
 *      六张卡片底边落在六个不同的地方 —— 用户那句「2 列 3 列长度对不齐不好看」）。
 *  ② 侧栏在任何视口都**够得着**：不在版面上时，侧栏外面必须有一颗 ☰，
 *     点它侧栏要真的进视口（旧形状下 820–1179 那一档侧栏在屏幕外，
 *     而唯一的 ☰ 长在侧栏内部、列表栏那颗又只在 `isCompact` 才出现 ⇒ 文件夹/同步/设置整档够不着）。
 *  ③ 全程零 console error。
 *  ④ **一个视图只有一层滚动条**：文档本身不许滚（外层滚上去底下是空白，用户那句），
 *     而内容必须全部落在内层那一栏的滚动范围里（滚到底能看见最后一张卡 = 不是靠遮丑）。
 *     修前 1280×900 设置页实测两层：文档 177px + `.settings__body` 2002px。
 *  ④′ ④ 那句是「各个界面」，不是「设置页」：逐个视图（全部/废纸篓/搜索/编辑器/冲突/设置）扫，
 *     并且判据从"比 height 差"换成"**真的去滚它**"（`scrollTop = 99999` 再读回来）—— 老写法量不到
 *     `overflow:hidden` 的根被焦点滚走的那种隐形滚动区（实测 950/800/700 三档分别有 167/317/417px）。
 *  ⑤ 置顶那颗点**不悬停也在**：算出来的 opacity 必须是 1，而同行的删除那颗必须是 0
 *     （后者是正对照 —— 否则"量到 1"可能只是因为整套 hover 规则没生效）。
 *  ⑥–⑨（本轮 UX 批加的四条，判据都打在渲染后的几何或真调用上）：
 *     ⑥ 宽屏只有一颗把手且收起后回得来；⑦ 页面上没有原生 `<select>`、面板是我们画的；
 *     ⑧ 删除确认是**悬浮层** —— 打开它不许把下面任何一行顶走；⑧′ 而那颗"确认"必须**真的删掉**：
 *     界面上那一行少一行、核心也不再把它交给界面（缺口 G63 就是从这一格空档里漏出去的）；
 *     ⑨ 文件夹平铺成一层，
 *     历史子层不许因为"不渲染层级"就找不到，且"移动到父级"/"在这下面新建"两颗要真的没了。
 *  ⑩ 快捷新建的模板：真点一次，核心里只多一篇、落库载荷第一块是空段落，编辑器真画出三行待办。
 *  ⑬ 「文字大小 / 文字颜色」那两层菜单在**手机宽**（390）下也要完整可见、点得着，且点了真画出来
 *     （⑫ 只在 1440 量过，而工具条本身横向可滚 ⇒ 少一次钳位就是 G29 那一族）。
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

const BRIDGE = process.env.BRIDGE || 'http://127.0.0.1:17323';

async function cmd(name, args = {}) {
  const res = await fetch(`${BRIDGE}/cmd/${name}`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(args),
  });
  return res.json();
}

/** 核心给界面的 `list_folders` 是一棵树（子层嵌在 `children`），要按 id 找东西就得先摊平。 */
function flattenFolders(nodes, out = []) {
  for (const n of Array.isArray(nodes) ? nodes : []) {
    out.push(n);
    flattenFolders(n.children, out);
  }
  return out;
}

/** 每次跑的当批戳：待删夹具的名字带上它，免得和上一轮留下的墓碑看起来是同一格。 */
const stamp = new Date().toISOString().slice(11, 19).replace(/:/g, '');

/**
 * ⑤ 那一腿要列表里**真的有一篇**，否则"那颗点可见"会退化成"什么都没量到"。
 * 夹具直接经真核心写入（不是往页面里塞 DOM）—— 列表那一行是核心给的。
 */
async function ensureSeededNote() {
  const list = await cmd('list_notes', { folderId: null, trash: false });
  if (!Array.isArray(list)) throw new Error(`list_notes 没回数：${JSON.stringify(list).slice(0, 120)}`);
  const id = list[0]?.id ?? (await cmd('create_note', {
    folderId: null,
    doc: { v: 1, content: [{ id: 'seedblk01', type: 'paragraph', content: [{ text: '布局门禁夹具：这一篇必须出现在列表里' }] }] },
  }))?.id;
  if (!id) throw new Error('拿不到列表里的笔记 id：⑤ 那条判据无从量起');
  const pinned = await cmd('set_note_pinned', { id, pinned: true });
  if (!pinned || pinned.id !== id) throw new Error(`置顶夹具失败：${JSON.stringify(pinned).slice(0, 120)}`);

  // ⑧ 那一腿要一个"可以删"的文件夹 —— 默认本上没有删除入口（那是 P0 那条修复本身）。
  // ⚠ 核心给的 `list_folders` 是**一棵树**（子层嵌在 `children` 里），不是一排。
  //   早先这里按顶层数组找子层，于是每跑一次门禁都以为"子层不存在"而新建一个 ——
  //   开发库里就这么堆出了 9 个同名 `布局夹具子`，而判据照样全绿（拍平之后它们确实"都还在"）。
  //   夹具自己也得有"复用而不是再造"的判据，否则它既是判据又是污染源。
  const folders = flattenFolders(await cmd('list_folders', {}));
  const plain = folders.find((f) => f.systemKind == null && f.parentId == null);
  const folderId = plain?.id ?? (await cmd('create_folder', { parentId: null, name: '布局夹具本' }))?.id;
  if (!folderId) throw new Error('造不出一个普通文件夹：⑧ 那条判据无从量起');

  // ⑨ 那一腿要一个**历史嵌套**的子层：拍平之后它必须还在（不能因为不渲染层级就消失），
  //    而且与父级同一左缘。先查再造，免得每次跑门禁都多堆一个文件夹。
  const existingChild = folders.find((f) => f.parentId === folderId);
  const childId = existingChild?.id ?? (await cmd('create_folder', { parentId: folderId, name: '布局夹具子' }))?.id;
  if (!childId) throw new Error('造不出历史子层：⑨ 那条"拍平不许丢文件夹"的判据无从量起');

  // ⑧′ 那一腿要一颗**可以被真删掉**的本：每次跑现造现删，不许复用（复用过的就在回收站里了）。
  //    代价是每次留下一条文件夹墓碑 —— 核心目前没有 purge_folder / restore_folder 命令。
  const sacrifice = await cmd('create_folder', { parentId: null, name: `布局夹具删 ${stamp}` });
  if (!sacrifice?.id) throw new Error(`造不出待删的文件夹：${JSON.stringify(sacrifice).slice(0, 120)}`);
  return { noteId: id, folderId, childId, sacrificeId: sacrifice.id };
}

/**
 * ④′ 的夹具：把每一栏都**真的压到要滚**。
 * 扫到"这一栏没有第二层滚动条"可能是两件事：量过了且对，或者压根没内容可滚 ——
 * 后者不算检查过。所以这里造 30 篇（列表溢出）、删 14 篇（废纸篓溢出）、
 * 再造一篇 60 段的长文（编辑区溢出），搜索用同一批命中。
 * 先按标题前缀清掉上一轮的：这条门禁会反复跑，不许每次多堆 30 篇。
 */
async function ensureScrollFixtures() {
  const MARK = '滚动夹具';
  for (const trash of [false, true]) {
    const rows = await cmd('list_notes', { folderId: null, trash });
    for (const n of Array.isArray(rows) ? rows : []) {
      if ((n.title ?? '').startsWith(MARK)) await cmd('purge_note', { id: n.id });
    }
  }
  const para = (text) => ({ type: 'paragraph', content: [{ text }] });
  const doc = (lines) => ({ v: 1, content: lines.map((text, i) => ({ id: `fx${String(i).padStart(3, '0')}${String(Date.now()).slice(-4)}`, ...para(text) })) });
  const ids = [];
  for (let i = 0; i < 30; i += 1) {
    const r = await cmd('create_note', { folderId: null, doc: doc([`${MARK}：这一行要够长，把列表那一栏撑到必须滚 ${i}`]) });
    if (r?.id) ids.push(r.id);
  }
  const long = await cmd('create_note', {
    folderId: null,
    doc: doc(Array.from({ length: 60 }, (_, i) => `${MARK} 长文第 ${i} 段：编辑区这一栏要真的能滚起来，否则"只有一层"是空判据`)),
  });
  for (const id of ids.slice(0, 14)) await cmd('delete_note', { id });
  if (ids.length < 30 || !long?.id) throw new Error(`④′ 的夹具造不出来：列表 ${ids.length}/30，长文 ${long?.id ?? '无'}`);
  return { ids, longId: long.id };
}

let seed;
let fx;
try {
  fx = await ensureScrollFixtures();
  seed = await ensureSeededNote();
} catch (e) {
  console.error(`布局门禁的前置不满足：${e.message}\n要先起：cargo run -p notera-cli -- --data-dir <目录> serve --port 17323`);
  process.exit(1);
}
const seedNoteId = seed.noteId;

const browser = await chromium.launch({ executablePath: CHROME });
const failures = [];
const notes = [];

function check(name, ok, detail) {
  (ok ? notes : failures).push(`${ok ? 'PASS' : 'FAIL'} ${name}${detail ? ` —— ${detail}` : ''}`);
}

/**
 * ④′ 的探针。四件事一次量完，全部打在渲染后的几何上：
 *  · **根那一格能不能被滚**（`scrollTop = 99999` 再读回来）—— 不许只比 scrollHeight/clientHeight：
 *    上一版就是这么算出"外层 0px"却量不到 body 被焦点滚走的 167px（`overflow:hidden` 的格子
 *    照样能被焦点/scrollIntoView 滚，这才是用户那句"滚上去底下是空白"剩下的那一半）。
 *  · 版心的盒子必须正好等于视口（多出来的那一截就是没人能看见、也没人能滚回来的死区）。
 *  · 同一栏里不许有两层**在流**的滚动区互相套着（悬浮层按 P1 那条不参与排版，故排除 fixed/absolute）。
 *  · 每一层滚到底时，它的最后一格必须真的进视口 —— 挡住"靠遮丑过关"。
 */
const LAYER_SCAN = async () => {
  const b = document.body;
  const de = document.scrollingElement ?? document.documentElement;
  b.scrollTop = 99999;
  const bodyShift = b.scrollTop;
  b.scrollTop = 0;
  de.scrollTop = 99999;
  const docShift = de.scrollTop;
  de.scrollTop = 0;
  const shell = document.querySelector('.app-shell');
  const hits = [];
  for (const el of document.querySelectorAll('.app-shell *')) {
    const cs = getComputedStyle(el);
    if (!/(auto|scroll)/.test(cs.overflowY)) continue;
    if (cs.position === 'fixed' || cs.position === 'absolute') continue;
    const over = el.scrollHeight - el.clientHeight;
    if (over <= 1) continue;
    const pane = el.closest('.pane');
    hits.push({ el, col: pane ? pane.className.replace('pane ', '').slice(0, 18) : '(根)', cls: String(el.className).split(' ').slice(0, 2).join('.') || el.tagName.toLowerCase(), over });
  }
  const layers = [];
  for (const h of hits) {
    h.el.scrollTop = h.over;
    await new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)));
    const last = h.el.lastElementChild;
    const lr = last?.getBoundingClientRect();
    layers.push({ col: h.col, cls: h.cls, over: h.over, lastCls: last ? (String(last.className).split(' ')[0] || last.tagName.toLowerCase()) : null, lastBottom: lr ? Math.round(lr.bottom) : null, vh: window.innerHeight });
    h.el.scrollTop = 0;
  }
  const nested = [];
  for (const a of hits) {
    for (const c of hits) {
      if (a !== c && a.el.contains(c.el)) nested.push(`${a.col}/${a.cls} ⊃ ${c.col}/${c.cls}`);
    }
  }
  return {
    bodyShift,
    docShift,
    shellH: shell ? Math.round(shell.getBoundingClientRect().height) : null,
    innerH: window.innerHeight,
    layers,
    nested,
  };
};

for (const width of WIDTHS) {
  const errors = [];
  const page = await browser.newPage({ viewport: { width, height: 950 } });
  page.on('console', (m) => { if (m.type() === 'error') errors.push(m.text().slice(0, 160)); });
  page.on('pageerror', (e) => errors.push(String(e).slice(0, 160)));
  await page.goto(URL_BASE, { waitUntil: 'networkidle' });
  await page.waitForSelector('[data-testid="sidebar"]', { timeout: 15000 });
  await page.waitForTimeout(2200);

  // ② 侧栏够得着 + ⑥ 全屏幕只有一颗把手
  // 把手的两个合法位置：自绘标题栏那一颗（sidebar-handle），或没有那一行时主区左上那颗
  // （open-sidebar）。侧栏内部的 sidebar-collapse 不算"外面的把手"。
  const OUTSIDE = '[data-testid="sidebar-handle"], [data-testid="open-sidebar"]';
  const ALL_HANDLES = '[data-testid="sidebar-handle"], [data-testid="open-sidebar"], [data-testid="sidebar-collapse"]';
  const reach = await page.evaluate((OUT) => {
    const side = document.querySelector('[data-testid="sidebar"]');
    const r = side.getBoundingClientRect();
    const inline = r.x >= 0 && r.width > 0 && r.right <= window.innerWidth;
    const handle = document.querySelector(OUT);
    let handleInView = false;
    if (handle) {
      const h = handle.getBoundingClientRect();
      handleInView = h.x >= 0 && h.width > 0 && h.right <= window.innerWidth;
    }
    return { inline, hasHandle: Boolean(handle), handleInView };
  }, OUTSIDE);
  if (!reach.inline) {
    check(`宽 ${width}：侧栏不在版面上时，侧栏外必须有一颗 ☰`, reach.hasHandle && reach.handleInView, JSON.stringify(reach));
    await page.evaluate(() => document.querySelector('[data-testid="sidebar-handle"], [data-testid="open-sidebar"]').click());
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

  // ⑥ 把手数量：侧栏开着 = 1 颗，收着 = 1 颗（用户那句"折起之后下面一层还有一个折起"）
  const handlesOpen = await page.evaluate((sel) => {
    const v = (s) => Array.from(document.querySelectorAll(s)).filter((el) => {
      const r = el.getBoundingClientRect();
      return r.width > 0 && r.height > 0;
    }).map((el) => el.getAttribute('data-testid'));
    return { all: v(sel), outside: v('[data-testid="sidebar-handle"], [data-testid="open-sidebar"]') };
  }, ALL_HANDLES);
  check(`宽 ${width}：侧栏开着时全屏幕只有一颗把手`, handlesOpen.all.length === 1, JSON.stringify(handlesOpen));
  await page.evaluate(() => document.querySelector('[data-testid="sidebar-handle"], [data-testid="open-sidebar"]').click());
  await page.waitForTimeout(500);
  const handlesClosed = await page.evaluate((sel) => {
    const v = (s) => Array.from(document.querySelectorAll(s)).filter((el) => {
      const r = el.getBoundingClientRect();
      return r.width > 0 && r.height > 0;
    }).map((el) => el.getAttribute('data-testid'));
    return { all: v(sel), outside: v('[data-testid="sidebar-handle"], [data-testid="open-sidebar"]') };
  }, ALL_HANDLES);
  check(`宽 ${width}：侧栏收起后仍然只有一颗把手（且它在侧栏外，回得来）`, handlesClosed.all.length === 1 && handlesClosed.outside.length === 1, JSON.stringify(handlesClosed));
  notes.push(`     宽 ${width} 把手：开着 ${JSON.stringify(handlesOpen.all)} → 收起 ${JSON.stringify(handlesClosed.all)}`);
  // 收回"开着"，让后面几条腿看到同一份起点
  await page.evaluate(() => document.querySelector('[data-testid="sidebar-handle"], [data-testid="open-sidebar"]').click());
  await page.waitForTimeout(500);

  // ⑤ 置顶那颗点：不悬停也在（鼠标停在 (0,0)，不在任何行上）
  const pin = await page.evaluate((noteId) => {
    const row = document.querySelector(`[data-testid="note-row-${noteId}"]`);
    if (!row) return { missing: 'row' };
    const dot = row.querySelector('[data-testid="note-pin-toggle"]');
    if (!dot) return { missing: 'dot' };
    const actions = row.querySelector('.row-item__actions');
    const r = dot.getBoundingClientRect();
    return {
      dotOpacity: getComputedStyle(dot).opacity,
      dotText: dot.textContent.trim(),
      dotPressed: dot.getAttribute('aria-pressed'),
      dotInActions: Boolean(dot.closest('.row-item__actions')),
      actionsOpacity: actions ? getComputedStyle(actions).opacity : null,
      dotInView: r.width > 0 && r.height > 0 && r.top >= 0 && r.bottom <= window.innerHeight,
      dotSize: [Math.round(r.width), Math.round(r.height)],
    };
  }, seedNoteId);
  check(
    `宽 ${width}：置顶那颗点常显（opacity=1、不在 hover 层里、在视口内），同行 hover 层 opacity=0`,
    pin.dotOpacity === '1' && pin.dotInActions === false && pin.dotInView === true && pin.actionsOpacity === '0',
    JSON.stringify(pin),
  );
  check(`宽 ${width}：已置顶那颗读得出"已置顶"（● + aria-pressed=true）`, pin.dotText === '●' && pin.dotPressed === 'true', JSON.stringify(pin));
  check(`宽 ${width}：那颗点 ≥44×44（§6 触摸目标下限）`, Number(pin.dotSize?.[0]) >= 44 && Number(pin.dotSize?.[1]) >= 44, JSON.stringify(pin.dotSize));

  // ⑧ 确认层是悬浮的：打开它不许把下面任何一行顶走（用户那句"而不是底下占了一个"）
  // 量**相对**位移：`.banners` 那一格会在两次采样之间自己冒出来（离线横幅，与弹层无关），
  // 拿视口绝对坐标比就会把它的 44px 算到弹层头上 —— 第一版就是这么假红的。
  const treeShapeSrc = `(() => {
    const body = document.querySelector('.pane--sidebar .pane-body');
    const tree = document.querySelector('.pane--sidebar .tree');
    const bt = body.getBoundingClientRect().top;
    return {
      treeH: Math.round(tree.getBoundingClientRect().height),
      relTops: Array.from(document.querySelectorAll('.tree__row')).map((r) => Math.round(r.getBoundingClientRect().top - bt)),
    };
  })()`;
  const popBefore = await page.evaluate(treeShapeSrc);
  const hasDelete = await page.evaluate(() => Boolean(document.querySelector('[data-testid="folder-delete"]')));
  if (!hasDelete) {
    check(`宽 ${width}：夹具里该有一个可删的普通文件夹`, false, '页面上找不到 folder-delete');
  } else {
    // 那三颗是 hover 才掀开的（工具层浮在行上）—— 不先把指针放到行上，
    // Playwright 会报"被 .tree__name 拦截"。这不是将就，是这条路径本来的样子。
    const row = page.locator('.tree__row', { has: page.locator(`[data-testid="folder-${seed.folderId}"]`) });
    await row.hover();
    await page.waitForTimeout(150);
    await row.locator('[data-testid="folder-delete"]').click();
    await page.waitForTimeout(300);
    const popAfter = await page.evaluate(`
      (() => {
        const body = document.querySelector('.pane--sidebar .pane-body');
        const tree = document.querySelector('.pane--sidebar .tree');
        const bt = body.getBoundingClientRect().top;
        const panel = document.querySelector('[data-testid="app-popover-panel"]');
        const r = panel?.getBoundingClientRect();
        return {
          treeH: Math.round(tree.getBoundingClientRect().height),
          relTops: Array.from(document.querySelectorAll('.tree__row')).map((x) => Math.round(x.getBoundingClientRect().top - bt)),
          panel: panel && r ? { position: getComputedStyle(panel).position, h: Math.round(r.height), inView: r.top >= 0 && r.bottom <= window.innerHeight, hasConfirm: Boolean(panel.querySelector('[data-testid="folder-delete-confirm"]')) } : null,
        };
      })()
    `);
    const shifted = popAfter.relTops.filter((t, i) => t !== popBefore.relTops[i]).length;
    check(
      `宽 ${width}：删除确认是悬浮层（面板 absolute、可见、带确认按钮，且树的高度与各行相对位置一格未动）`,
      popAfter.panel?.position === 'absolute' && popAfter.panel.h > 0 && popAfter.panel.hasConfirm && popAfter.panel.inView && shifted === 0 && popAfter.treeH === popBefore.treeH,
      JSON.stringify({ panel: popAfter.panel, shiftedRows: shifted, treeH: [popBefore.treeH, popAfter.treeH], relTops: [popBefore.relTops, popAfter.relTops] }),
    );
    await page.keyboard.press('Escape');
    await page.waitForTimeout(200);
    const closed = await page.evaluate(() => document.querySelectorAll('[data-testid="app-popover-panel"]').length);
    check(`宽 ${width}：Esc 关得掉那层确认`, closed === 0, `残留 ${closed} 个面板`);

    // ⑧′ 决定类按钮要验**效果**，光验"悬浮层画对了"不够。这一格以前是空的，
    //     而它底下正藏着缺口 G63：软删掉的文件夹仍然留在 `list_folders` 交给界面那一排里 ——
    //     用户点完"确认删除"，那一行还在原地。所以这里两边都量：界面上那一行、以及核心还认不认它。
    //     只在 1440 跑一遍：它是行为判据，那颗待删的本一轮只有一份。
    if (width === 1440) {
      const before = await page.evaluate(() => document.querySelectorAll('[data-testid="folder-row"]').length);
      const victim = page.locator('.tree__row', { has: page.locator(`[data-testid="folder-${seed.sacrificeId}"]`) });
      check(`宽 ${width}：待删那一行本来在界面上`, (await victim.count()) === 1, `实到 ${await victim.count()} 行`);
      await victim.hover();
      await page.waitForTimeout(150);
      await victim.locator('[data-testid="folder-delete"]').click();
      await page.waitForTimeout(300);
      await page.locator('[data-testid="app-popover-panel"] [data-testid="folder-delete-confirm"]').click();
      await page.waitForTimeout(900);
      const after = await page.evaluate((id) => ({
        row: Boolean(document.querySelector(`[data-testid="folder-${id}"]`)),
        rows: document.querySelectorAll('[data-testid="folder-row"]').length,
      }), seed.sacrificeId);
      const coreIds = flattenFolders(await cmd('list_folders', {})).map((f) => f.id);
      check(
        `宽 ${width}：确认删除之后那一行真的从界面上没了（核心也不再把它交给界面）`,
        !after.row && after.rows === before - 1 && !coreIds.includes(seed.sacrificeId),
        JSON.stringify({ ...after, before, 核心还认它: coreIds.includes(seed.sacrificeId) }),
      );
      notes.push(`     确认删除实测：界面 ${before} → ${after.rows} 行，那一行还在=${after.row}，核心列表含它=${coreIds.includes(seed.sacrificeId)}`);
    }
  }

  // ⑨ 文件夹只剩一层：所有行同一左缘，且层级 UI（"移动到父级"/"在这下面新建"）确实没了。
  //    历史子层必须**还在这一排里**（拍平不等于藏起来 —— 不然它里面的笔记就找不回了）。
  const flat = await page.evaluate((ids) => {
    const rows = Array.from(document.querySelectorAll('[data-testid="folder-row"]'));
    const leftOf = (id) => {
      const el = document.querySelector(`[data-testid="folder-${id}"]`);
      return el ? Math.round(el.closest('.tree__row').getBoundingClientRect().left) : null;
    };
    return {
      xs: [...new Set(rows.map((r) => Math.round(r.getBoundingClientRect().left)))],
      rowCount: rows.length,
      parentX: leftOf(ids[0]),
      childX: leftOf(ids[1]),
      move: document.querySelectorAll('[data-testid="folder-move"]').length,
      sub: document.querySelectorAll('[data-testid^="folder-new-sub-"]').length,
    };
  }, [seed.folderId, seed.childId]);
  check(
    `宽 ${width}：文件夹平铺一层（历史子层还在、与父级同一左缘，且没有"移动到父级"/"在这下面新建"）`,
    flat.rowCount > 0 && flat.xs.length === 1 && flat.parentX !== null && flat.parentX === flat.childX && flat.move === 0 && flat.sub === 0,
    JSON.stringify(flat),
  );

  // ⑩ 快捷新建的模板那颗 ▾。单测能证明 build() 造的载荷对，证明不了**点下去有没有 dispatch**、
  //    也证明不了编辑器按那份载荷画没画出来 —— 而"控件存在但没有效果"正是本项目踩过的那一族。
  //    只在 1440 跑一遍：这是行为判据，不是几何判据，四个视口跑四遍只会往夹具库里多塞三篇。
  if (width === 1440) {
    const before = await cmd('list_notes', { folderId: null, trash: false });
    await page.click('[data-testid="new-note-templates"]');
    await page.waitForTimeout(300);
    const options = await page.evaluate(() =>
      Array.from(document.querySelectorAll('[data-testid="app-popover-panel"] [data-testid^="template-"]')).map((el) => el.dataset.testid),
    );
    check(
      `宽 ${width}：模板那层列出候选（待办 / 会议），且不再列"空白"`,
      options.length === 2 && options.includes('template-todo') && options.includes('template-meeting') && !options.includes('template-blank'),
      JSON.stringify(options),
    );
    await page.click('[data-testid="template-todo"]');
    await page.waitForTimeout(1500);

    const after = await cmd('list_notes', { folderId: null, trash: false });
    const fresh = after.filter((n) => !before.some((b) => b.id === n.id));
    check(`宽 ${width}：点一次模板核心里只多一篇`, fresh.length === 1, `实到 ${fresh.length} 篇：${JSON.stringify(fresh.map((n) => n.id))}`);

    let wire = null;
    let painted = null;
    if (fresh.length === 1) {
      const full = await cmd('get_note', { id: fresh[0].id });
      const blocks = full?.doc?.content ?? [];
      wire = {
        firstType: blocks[0]?.type ?? null,
        firstText: blocks[0]?.content?.length ?? -1,
        checks: blocks.filter((b) => b.type === 'checklistItem').length,
        title: full?.title ?? null,
      };
      check(
        `宽 ${width}：落库的那份载荷第一块是空段落 + 三行待办`,
        wire.firstType === 'paragraph' && wire.firstText === 0 && wire.checks === 3,
        JSON.stringify(wire),
      );
      check(
        `宽 ${width}：标题没被模板文字顶掉（第一行留给用户）`,
        wire.title === '' || wire.title === null,
        JSON.stringify({ title: wire.title }),
      );
      painted = await page.evaluate(() => {
        const first = document.querySelector('.nb-block .nb-content');
        return {
          checks: document.querySelectorAll('.nb-check').length,
          placeholder: first?.dataset.placeholder ?? null,
          firstText: (first?.innerText ?? '').trim(),
          visible: first ? getComputedStyle(first, ':before').content : null,
        };
      });
      check(
        `宽 ${width}：编辑器真画出三行待办，第一行空着且挂着「请输入标题和正文」`,
        painted.checks === 3 && painted.placeholder === '请输入标题和正文' && painted.firstText === '',
        JSON.stringify(painted),
      );
      await cmd('purge_note', { id: fresh[0].id });
      notes.push(`     宽 ${width} 模板实测 落库=${JSON.stringify(wire)} 画出=${JSON.stringify({ checks: painted.checks, ph: painted.placeholder, text: painted.firstText })}`);
    }
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

  // ④ 一个视图只有一层滚动条：文档不许滚，且内容全在内层那一栏的滚动范围里
  const layers = await page.evaluate(() => {
    const de = document.scrollingElement ?? document.documentElement;
    const inner = Array.from(document.querySelectorAll('*')).filter((el) => {
      const cs = getComputedStyle(el);
      return /(auto|scroll)/.test(cs.overflowY) && el.scrollHeight > el.clientHeight + 1;
    });
    const byCol = {};
    for (const el of inner) {
      const col = el.closest('.pane')?.className.replace('pane ', '').slice(0, 18) ?? '(根)';
      (byCol[col] ??= []).push({ cls: String(el.className).slice(0, 40), over: el.scrollHeight - el.clientHeight });
    }
    return {
      docOverflow: de.scrollHeight - de.clientHeight,
      byCol,
      worst: Math.max(0, ...Object.values(byCol).map((v) => v.length)),
    };
  });
  check(`宽 ${width}：文档不滚（外层那一条没了）`, layers.docOverflow === 0, JSON.stringify(layers));
  // 「只有一层」的主语是**一栏**，不是整屏。侧栏排到 11 个文件夹时它自己那一栏本来就该能滚 ——
  // 老写法 `innerCount === 1` 把这两件事混在一起，④′ 逐个视图扫的第一跑就是这样撞红的：
  // 红的是判据写错了，不是界面错了（那条 55px 是侧栏的一层，不是第二层）。
  check(`宽 ${width}：设置那一栏恰好一层滚动条`, (layers.byCol.settings ?? []).length === 1, JSON.stringify(layers.byCol));
  check(`宽 ${width}：任何一栏都不许多于一层滚动条`, layers.worst <= 1, JSON.stringify(layers.byCol));

  const lastCard = await page.evaluate(async () => {
    const el = document.querySelector('.settings__body');
    el.scrollTop = el.scrollHeight;
    await new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)));
    const all = document.querySelectorAll('.settings__grid > .card');
    const last = all[all.length - 1].getBoundingClientRect();
    const de = document.scrollingElement ?? document.documentElement;
    return {
      scrolledTo: Math.round(el.scrollTop),
      maxScroll: Math.round(el.scrollHeight - el.clientHeight),
      lastBottom: Math.round(last.bottom),
      vh: window.innerHeight,
      docOverflow: de.scrollHeight - de.clientHeight,
    };
  });
  check(
    `宽 ${width}：滚到底能完整看见最后一张卡（内层那栏真装得下，不是靠遮丑）`,
    lastCard.lastBottom <= lastCard.vh + 2 && lastCard.docOverflow === 0 && lastCard.scrolledTo > 0,
    JSON.stringify(lastCard),
  );
  notes.push(`     宽 ${width} 内层滚动 ${lastCard.scrolledTo}/${lastCard.maxScroll}px · 末卡底 ${lastCard.lastBottom} vs 视口 ${lastCard.vh}`);
  await page.evaluate(() => { document.querySelector('.settings__body').scrollTop = 0; });

  // ④′ 用户那句是「各个界面有滚动条的应该只有一层」，不是一句"设置页"。
  //     上一版这条判据只在设置页量了一次，而且只看 `scrollingElement` 的 height 差 ——
  //     于是"根那一格其实还能被焦点滚走 167px"这一整族都从判据底下滑过去了。
  //     这里逐个视图扫，且把最后一格留在设置页，好让下面的 ⑦ 从同一份起点开始。
  const VIEWS = [
    ['全部笔记', async () => { await page.evaluate(() => document.querySelector('[data-testid="nav-all"]').click()); }, true, null],
    ['废纸篓', async () => { await page.evaluate(() => document.querySelector('[data-testid="nav-trash"]').click()); }, true, null],
    ['搜索结果', async () => {
      await page.evaluate(() => document.querySelector('[data-testid="nav-all"]').click());
      await page.waitForTimeout(500);
      const si = page.locator('[data-testid="search-input"]');
      if (await si.count() > 0 && (await si.isVisible())) await si.fill('滚动夹具');
      await page.waitForTimeout(1000);
    }, true, null],
    ['编辑器', async () => {
      await page.evaluate(() => document.querySelector('[data-testid="nav-all"]').click());
      await page.waitForTimeout(500);
      const si = page.locator('[data-testid="search-input"]');
      if (await si.count() > 0 && (await si.isVisible())) await si.fill('');
      await page.waitForTimeout(600);
      await page.evaluate((id) => {
        const row = document.querySelector(`[data-testid="note-row-${id}"]`);
        row?.scrollIntoView({ block: 'center' });
        row?.click();
      }, fx.longId);
      await page.waitForTimeout(1200);
    }, true, 'editor'],
    // 冲突那一格没有对端就造不出条目，所以这里只要求"根不许滚 + 不许嵌套"，
    // 不许把它当成"检查过了"：读数是 0 层，判据在这一格确实是空的，如实写出来。
    ['冲突', async () => { await page.evaluate(() => document.querySelector('[data-testid="nav-conflicts"]').click()); }, false, null],
    ['设置', async () => { await page.evaluate(() => document.querySelector('[data-testid="nav-settings"]').click()); }, true, 'settings'],
  ];
  for (const [i, entry] of VIEWS.entries()) {
    const [label, go, expectOverflow, expectCol] = entry;
    await go();
    const s = await page.evaluate(LAYER_SCAN);
    const nm = `宽 ${width} · ${label}`;
    check(`${nm}：根那一格不许留隐形滚动区（body 与文档都滚不动）`, s.bodyShift === 0 && s.docShift === 0, JSON.stringify({ bodyShift: s.bodyShift, docShift: s.docShift }));
    check(`${nm}：版心盒子等于视口（多出来那一截谁也看不见、也滚不回来）`, s.shellH === s.innerH, JSON.stringify({ shellH: s.shellH, innerH: s.innerH }));
    check(`${nm}：同一栏不许两层在流的滚动区互相套着`, s.nested.length === 0, JSON.stringify(s.nested));
    const blind = s.layers.filter((l) => l.lastBottom !== null && l.lastBottom > l.vh + 2);
    check(`${nm}：每层滚到底时最后一格都进视口（不是靠遮丑）`, blind.length === 0, JSON.stringify(blind));
    if (expectOverflow) {
      check(`${nm}：夹具确实把这一栏压到溢出（否则上面四条是"没量到"）`, s.layers.length >= 1, JSON.stringify(s.layers));
    } else {
      notes.push(`     ${nm}：这一格本批造不出溢出内容（要两台设备/对端才有条目），四条判据里只有前两条在这里有牙`);
    }
    if (expectCol) {
      check(`${nm}：溢出的是 ${expectCol} 那一栏（夹具没白造）`, s.layers.some((l) => l.col.includes(expectCol)), JSON.stringify(s.layers.map((l) => l.col)));
    }
    notes.push(`     ${nm} 在流的滚动层 = ${JSON.stringify(s.layers.map((l) => `${l.col}/${l.cls} ${l.over}px，滚到底末格底 ${l.lastBottom}/视口 ${l.vh}`))}`);
    await page.screenshot({ path: `${OUT}/24-layers-${width}-${['all', 'trash', 'search', 'editor', 'conflicts', 'settings'][i]}.png` });
  }
  await page.evaluate(() => document.querySelector('[data-testid="nav-settings"]').click());
  await page.waitForTimeout(900);

  // ⑦ 下拉：不许再有原生 <select>（它的面板由操作系统画 ⇒ 两端不可能一致），
  //    换成无头库之后面板是我们 DOM 里的节点，于是"画得对不对、在不在视口里"可量。
  const native = await page.evaluate(() => document.querySelectorAll('select').length);
  check(`宽 ${width}：页面上没有原生 <select>`, native === 0, `实到 ${native}`);
  await page.click('[data-testid="account-tls"]');
  await page.waitForTimeout(300);
  const panel = await page.evaluate(() => {
    const el = document.querySelector('[data-testid="app-select-panel"]');
    if (!el) return { missing: true };
    const r = el.getBoundingClientRect();
    const opts = el.querySelectorAll('[role="option"]');
    return {
      top: Math.round(r.top),
      bottom: Math.round(r.bottom),
      right: Math.round(r.right),
      bg: getComputedStyle(el).backgroundColor,
      optionCount: opts.length,
      labels: Array.from(opts).map((o) => o.textContent.trim()),
    };
  });
  check(
    `宽 ${width}：TLS 面板是我们画的、4 项、完整在视口内`,
    !panel.missing && panel.optionCount === 4 && panel.top >= 0 && panel.bottom <= 950 && panel.right <= width && panel.bg !== 'rgba(0, 0, 0, 0)',
    JSON.stringify(panel),
  );
  notes.push(`     宽 ${width} 下拉面板 ${JSON.stringify({ top: panel.top, bottom: panel.bottom, right: panel.right, bg: panel.bg, n: panel.optionCount })}`);
  await page.keyboard.press('Escape');
  await page.waitForTimeout(200);

  check(`宽 ${width}：console error 为零`, errors.length === 0, errors.slice(0, 3).join(' | '));
  await page.close();
}

/**
 * ⑪ 移动端键盘（用户那句「要考虑好移动端的这种体验，就是输入框弹起带来的这种体验」）。
 *
 * 桌面 Chromium 没有软键盘，所以这里在应用挂载**之前**把 `visualViewport` 换成一个可控的假对象，
 * 然后把它的 height 缩 300px 并 dispatch resize —— 这正是 iOS 上键盘弹起时的形状：
 * **`innerHeight` 一动不动，只有可视视口变**。判据打在渲染后的几何上（版心的 rect 高度），
 * 不打在"变量设了没有"上：设了而样式没读，是这个项目反复踩过的第二种空控件。
 *
 * ⚠ 这一腿验的是"链路接对了"，不等于 iOS 真机验收（真键盘的时序、offsetTop 的行为要设备）。
 */
{
  const KB = 300;
  const ctx = await browser.newContext({ viewport: { width: 390, height: 844 } });
  const kbPage = await ctx.newPage();
  const kbErrors = [];
  kbPage.on('pageerror', (e) => kbErrors.push(String(e).slice(0, 160)));
  kbPage.on('console', (m) => { if (m.type() === 'error') kbErrors.push(m.text().slice(0, 160)); });
  await kbPage.addInitScript(() => {
    const fake = Object.assign(new EventTarget(), { height: window.innerHeight, offsetTop: 0 });
    Object.defineProperty(window, 'visualViewport', { value: fake, configurable: true, writable: true });
  });
  await kbPage.goto(URL_BASE, { waitUntil: 'networkidle' });
  await kbPage.waitForSelector('.app-shell', { timeout: 15000 });
  await kbPage.waitForTimeout(1800);

  const geom = () => kbPage.evaluate((kb) => {
    const shell = document.querySelector('.app-shell').getBoundingClientRect();
    const toast = document.querySelector('[data-testid="toast-host"]');
    const cs = getComputedStyle(document.documentElement);
    return {
      shellH: Math.round(shell.height),
      inner: window.innerHeight,
      vvH: Math.round(window.visualViewport.height),
      vhVar: cs.getPropertyValue('--app-vh').trim(),
      kbVar: cs.getPropertyValue('--app-kb').trim(),
      toastBottom: toast ? Math.round(toast.getBoundingClientRect().bottom) : null,
      kb: kb,
    };
  }, KB);

  const before = await geom();
  check('移动端键盘：没键盘时版心就是整屏', before.shellH === 844 && before.kbVar === '0px', JSON.stringify(before));

  await kbPage.evaluate((kb) => {
    // 只有可视视口缩了 —— 故意不碰 innerHeight，那是 iOS 的真形状
    window.visualViewport.height = window.innerHeight - kb;
    window.visualViewport.dispatchEvent(new Event('resize'));
  }, KB);
  await kbPage.waitForTimeout(400);
  const during = await geom();
  check(
    '移动端键盘：可视视口一缩，版心必须真的跟着缩（而 innerHeight 依旧不动）',
    during.shellH === 544 && during.inner === 844 && during.kbVar === '300px',
    JSON.stringify({ before: before.shellH, during }),
  );
  check(
    '移动端键盘：浮层那一条也要抬到键盘之上',
    during.toastBottom !== null && during.toastBottom <= during.shellH + 2,
    JSON.stringify({ toastBottom: during.toastBottom, shellH: during.shellH }),
  );
  await kbPage.screenshot({ path: `${OUT}/22-keyboard-390.png` });

  await kbPage.evaluate(() => {
    window.visualViewport.height = window.innerHeight;
    window.visualViewport.dispatchEvent(new Event('scroll')); // iOS 上也常只发 scroll
  });
  await kbPage.waitForTimeout(400);
  const after = await geom();
  check('移动端键盘：收起之后版心与浮层都复原', after.shellH === 844 && after.kbVar === '0px', JSON.stringify(after));
  check('移动端键盘：这一腿 console error 为零', kbErrors.length === 0, kbErrors.slice(0, 3).join(' | '));
  notes.push(`     移动端键盘实测 390×844：静止 ${before.shellH} → 弹起 ${during.shellH}（inner 恒 ${during.inner}）→ 收起 ${after.shellH}；toast 底 ${before.toastBottom} → ${during.toastBottom}`);
  await ctx.close();
}

// ① 的另一半：**跨视口**不许换列（同一份内容在 900 与 1800 下卡片宽度差不能是"多塞一列"的量级）
/**
 * ⑫ 「文字大小」「文字颜色」两颗控件（用户第 ③ 条）。
 *
 * 为什么不能只靠单测：这条链有三段 —— 控件发事件 → 模型存进 doc → 渲染层把 doc 画成样式。
 * 单测各管一段，**中间断掉的那一段谁都不红**（本项目最常见的形状就是"控件在、点了没用"）。
 * 所以这里用真浏览器走一遍：双击选一个词 → 点控件 → 量**渲染后的 computed style**。
 */
{
  const kb = await browser.newPage({ viewport: { width: 1440, height: 950 } });
  const kbErrors = [];
  kb.on('pageerror', (e) => kbErrors.push(String(e).slice(0, 160)));
  kb.on('console', (m) => { if (m.type() === 'error') kbErrors.push(m.text().slice(0, 160)); });
  await kb.goto(URL_BASE, { waitUntil: 'networkidle' });
  await kb.waitForSelector('[data-testid^="note-row-"]', { timeout: 15000 });
  await kb.click('[data-testid^="note-row-"]');
  await kb.waitForSelector('.nb-block .nb-content', { timeout: 15000 });
  await kb.waitForTimeout(800);

  const baseFont = await kb.evaluate(() => parseFloat(getComputedStyle(document.querySelector('.nb-block .nb-content')).fontSize));

  // 双击选一个词（不靠程序化 Range：那会绕过浏览器自己的选区，量的就不是同一条路了）
  await kb.dblclick('.nb-block .nb-content', { position: { x: 14, y: 8 } });
  await kb.waitForTimeout(250);
  await kb.click('[data-testid="tb-size"]');
  await kb.waitForTimeout(250);
  const sizeMenu = await kb.evaluate(() => {
    const el = document.querySelector('[data-testid="tb-size-menu"]');
    if (!el) return { missing: true };
    const r = el.getBoundingClientRect();
    return { top: Math.round(r.top), bottom: Math.round(r.bottom), left: Math.round(r.left), right: Math.round(r.right), n: el.querySelectorAll('[role="menuitemradio"]').length };
  });
  check(
    '字号菜单是我们画的浮层、4 档、完整在视口内（工具条那条 overflow 容器不许把它裁掉）',
    !sizeMenu.missing && sizeMenu.n === 4 && sizeMenu.top >= 0 && sizeMenu.bottom <= 950 && sizeMenu.right <= 1440,
    JSON.stringify(sizeMenu),
  );
  await kb.click('[data-testid="tb-size-xl"]');
  await kb.waitForTimeout(500);
  const sized = await kb.evaluate((base) => {
    const el = document.querySelector('.nb-content span[data-mark="fontSize"]');
    if (!el) return { missing: true, base };
    const cs = getComputedStyle(el);
    return { base, rendered: parseFloat(cs.fontSize), step: el.getAttribute('data-mark-attrs') };
  }, baseFont);
  check(
    `点「特大」之后那一档真的画出来了（computed font-size = 1.7 × ${baseFont}px）`,
    !sized.missing && Math.abs(sized.rendered - baseFont * 1.7) < 1,
    JSON.stringify(sized),
  );

  await kb.dblclick('.nb-block .nb-content', { position: { x: 14, y: 8 } });
  await kb.waitForTimeout(250);
  await kb.click('[data-testid="tb-color"]');
  await kb.waitForTimeout(250);
  await kb.click('[data-testid="tb-color-red"]');
  await kb.waitForTimeout(500);
  const colored = await kb.evaluate(() => {
    const el = document.querySelector('.nb-content span[data-mark="color"]');
    if (!el) return { missing: true };
    return { color: getComputedStyle(el).color, attrs: el.getAttribute('data-mark-attrs') };
  });
  check(
    '点「红」之后渲染出来的是浅色主题那颗 --ink-red（走 token，不是写死的十六进制）',
    !colored.missing && colored.color === 'rgb(179, 32, 47)',
    JSON.stringify(colored),
  );
  await kb.screenshot({ path: `${OUT}/23-size-color-1440.png` });

  // 正对照：菜单里"标准"发的是**移除**，画面上必须回到基准字号
  await kb.dblclick('.nb-block .nb-content', { position: { x: 14, y: 8 } });
  await kb.waitForTimeout(250);
  await kb.click('[data-testid="tb-size"]');
  await kb.waitForTimeout(250);
  await kb.click('[data-testid="tb-size-m"]');
  await kb.waitForTimeout(500);
  const cleared = await kb.evaluate((base) => {
    const marked = document.querySelector('.nb-content span[data-mark="fontSize"]');
    const el = document.querySelector('.nb-block .nb-content');
    return { stillMarked: Boolean(marked), rendered: parseFloat(getComputedStyle(el).fontSize), base };
  }, baseFont);
  check(
    '选「标准」是摘掉这一档、字号回到基准（不许留一个没有视觉效果的标记）',
    !cleared.stillMarked && Math.abs(cleared.rendered - baseFont) < 0.6,
    JSON.stringify(cleared),
  );
  check('⑫ 这一腿 console error 为零', kbErrors.length === 0, kbErrors.slice(0, 3).join(' | '));
  notes.push(`     字号/颜色实测：基准 ${baseFont}px → 特大 ${sized.rendered}px；红 = ${colored.color}；摘掉后回到 ${cleared.rendered}px`);
  await kb.close();
}

/**
 * ⑬ 那两层菜单在**手机宽**下也要画得下、点得着（③ 的两颗控件 × ⑨ 的移动端 × ⑧ 的两端一致）。
 * ⑫ 只在 1440 量过；工具条自己是横向可滚的（390 档实测 scrollWidth 797 / clientWidth 380），
 * 菜单靠 fixed + 钳位/翻转落位 —— 少一次钳位就是 G29 那一族："按钮进入展开态，屏幕上却没有任何菜单"。
 */
{
  const narrow = await browser.newPage({ viewport: { width: 390, height: 844 } });
  const nErrors = [];
  narrow.on('pageerror', (e) => nErrors.push(String(e).slice(0, 160)));
  narrow.on('console', (m) => { if (m.type() === 'error') nErrors.push(m.text().slice(0, 160)); });
  await narrow.goto(URL_BASE, { waitUntil: 'networkidle' });
  await narrow.waitForSelector('[data-testid^="note-row-"]', { timeout: 15000 });
  await narrow.click('[data-testid^="note-row-"]');
  await narrow.waitForSelector('.nb-block .nb-content', { timeout: 15000 });
  await narrow.waitForTimeout(900);
  const nBase = await narrow.evaluate(() => parseFloat(getComputedStyle(document.querySelector('.nb-block .nb-content')).fontSize));

  // 两种落点都要量：`rest` = 工具条不动（手机宽下这两颗本来就停在右缘附近，是用户第一次点到的形状），
  // `end` = 把触发器滚到容器右缘（最坏情况）。缺口 G64 就是从 `rest` 这一档量出来的：
  // 390 宽时字号菜单落在 301..481，出界 91px，菜单项中心点 elementFromPoint 直接是 null。
  for (const [trigger, menu, label] of [['tb-size', 'tb-size-menu', '字号'], ['tb-color', 'tb-color-menu', '颜色']]) {
    for (const place of ['rest', 'end']) {
      await narrow.evaluate(([id, p]) => {
        const bar = document.querySelector('.tb');
        if (bar && p === 'rest') bar.scrollLeft = 0;
        document.querySelector(`[data-testid="${id}"]`)?.scrollIntoView({ inline: p === 'rest' ? 'nearest' : 'end', block: 'nearest' });
      }, [trigger, place]);
      await narrow.waitForTimeout(300);
      await narrow.dblclick('.nb-block .nb-content', { position: { x: 12, y: 8 } });
      await narrow.waitForTimeout(250);
      await narrow.click(`[data-testid="${trigger}"]`);
      await narrow.waitForTimeout(400);
      const m = await narrow.evaluate((id) => {
        const el = document.querySelector(`[data-testid="${id}"]`);
        if (!el) return { missing: true };
        const r = el.getBoundingClientRect();
        const it = el.querySelector('[role="menuitemradio"]');
        const b = it.getBoundingClientRect();
        const hit = document.elementFromPoint(Math.round(b.left + b.width / 2), Math.round(b.top + b.height / 2));
        return {
          top: Math.round(r.top), bottom: Math.round(r.bottom), left: Math.round(r.left), right: Math.round(r.right),
          vw: window.innerWidth, overflowRight: Math.round(r.right - window.innerWidth),
          n: el.querySelectorAll('[role="menuitemradio"]').length,
          inView: r.left >= 0 && r.right <= window.innerWidth && r.top >= 0 && r.bottom <= window.innerHeight,
          itemHitsItself: Boolean(hit && el.contains(hit)),
          hit: hit ? `${hit.tagName.toLowerCase()}.${String(hit.className).split(' ')[0]}` : null,
        };
      }, menu);
      check(
        `${label}菜单 · 触发器在${place === 'rest' ? '静止位' : '容器右缘'}：完整在视口内且菜单项点得着`,
        !m.missing && m.inView === true && m.itemHitsItself === true && m.n >= 4,
        JSON.stringify(m),
      );
      notes.push(`     手机宽菜单落点 ${label}/${place}：${m.left}..${m.right}（视口 ${m.vw}，出界 ${m.overflowRight}px），首项命中 ${m.hit}`);
      await narrow.click(`[data-testid="${trigger}"]`); // 再点一次收起，别把菜单留着影响下一档
      await narrow.waitForTimeout(250);
    }
  }

  await narrow.dblclick('.nb-block .nb-content', { position: { x: 12, y: 8 } });
  await narrow.waitForTimeout(250);
  await narrow.click('[data-testid="tb-size"]');
  await narrow.waitForTimeout(300);
  await narrow.click('[data-testid="tb-size-xl"]');
  await narrow.waitForTimeout(500);
  await narrow.dblclick('.nb-block .nb-content', { position: { x: 12, y: 8 } });
  await narrow.waitForTimeout(250);
  await narrow.click('[data-testid="tb-color"]');
  await narrow.waitForTimeout(300);
  await narrow.click('[data-testid="tb-color-red"]');
  await narrow.waitForTimeout(600);

  const painted = await narrow.evaluate((base) => {
    const s = document.querySelector('.nb-content span[data-mark="fontSize"]');
    const c = document.querySelector('.nb-content span[data-mark="color"]');
    return { base, size: s ? parseFloat(getComputedStyle(s).fontSize) : null, color: c ? getComputedStyle(c).color : null };
  }, nBase);
  check(
    `手机宽上点这两档也真的画出来了（基准 ${nBase}px → 特大 ${painted.size}px；红 = ${painted.color}）`,
    painted.size !== null && Math.abs(painted.size - nBase * 1.7) < 1 && painted.color === 'rgb(179, 32, 47)',
    JSON.stringify(painted),
  );
  await narrow.screenshot({ path: `${OUT}/26-narrow-menus-390.png` });
  check('⑬ 这一腿 console error 为零', nErrors.length === 0, nErrors.slice(0, 3).join(' | '));
  notes.push(`     手机宽（390×844）字号/颜色实测 ${JSON.stringify(painted)}`);
  await narrow.close();
}

await browser.close();
// 夹具清干净：这条门禁反复跑，不许每次往开发库里多堆 31 篇。
for (const id of [...fx.ids, fx.longId]) await cmd('purge_note', { id });
console.log(notes.join('\n'));
console.log(failures.length ? `\n${failures.join('\n')}\n>>> 布局门禁 FAIL` : '\n>>> 布局门禁 PASS');
process.exit(failures.length ? 1 : 0);
