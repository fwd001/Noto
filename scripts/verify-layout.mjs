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
 *  ⑤ 置顶那颗点**不悬停也在**：算出来的 opacity 必须是 1，而同行的删除那颗必须是 0
 *     （后者是正对照 —— 否则"量到 1"可能只是因为整套 hover 规则没生效）。
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
  const folders = await cmd('list_folders', {});
  const plain = Array.isArray(folders) ? folders.find((f) => f.systemKind == null && f.parentId == null) : null;
  const folderId = plain?.id ?? (await cmd('create_folder', { parentId: null, name: '布局夹具本' }))?.id;
  if (!folderId) throw new Error('造不出一个普通文件夹：⑧ 那条判据无从量起');

  // ⑨ 那一腿要一个**历史嵌套**的子层：拍平之后它必须还在（不能因为不渲染层级就消失），
  // 而且与父级同一左缘。先查再造，免得每次跑门禁都多堆一个文件夹。
  const hasChild = Array.isArray(folders) && folders.some((f) => f.parentId === folderId);
  const childId = hasChild
    ? folders.find((f) => f.parentId === folderId).id
    : (await cmd('create_folder', { parentId: folderId, name: '布局夹具子' }))?.id;
  if (!childId) throw new Error('造不出历史子层：⑨ 那条"拍平不许丢文件夹"的判据无从量起');
  return { noteId: id, folderId, childId };
}

let seed;
try {
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
    return {
      docOverflow: de.scrollHeight - de.clientHeight,
      innerCount: inner.length,
      inner: inner.map((el) => ({ cls: String(el.className).slice(0, 40), over: el.scrollHeight - el.clientHeight })),
    };
  });
  check(`宽 ${width}：文档不滚（外层那一条没了）`, layers.docOverflow === 0, JSON.stringify(layers));
  check(`宽 ${width}：设置页恰好一层滚动条`, layers.innerCount === 1, JSON.stringify(layers.inner));

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

// ① 的另一半：**跨视口**不许换列（同一份内容在 900 与 1800 下卡片宽度差不能是"多塞一列"的量级）
await browser.close();
console.log(notes.join('\n'));
console.log(failures.length ? `\n${failures.join('\n')}\n>>> 布局门禁 FAIL` : '\n>>> 布局门禁 PASS');
process.exit(failures.length ? 1 : 0);
