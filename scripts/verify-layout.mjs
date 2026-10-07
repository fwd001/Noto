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
 *  ⑭ 每一层浮层（模板 / 两处下拉 / 选区工具条）在 390 与 1440 两档都不许出界 —— G64 那一族的通判据。
 *  ⑮ 触屏那一档（`hover: none`）：只在 hover 露面的那两簇控件必须常驻、且真点得着（第 ④×⑨ 条）。
 *     修前实测 390×844 触屏：侧栏那簇 `opacity:0` + `pointer-events:none` ⇒ 手机上"改名/删除"根本不存在；
 *     列表行那簇 `opacity:0` 却仍吃点击 ⇒ 看不见却能点着，是隐形陷阱。桌面一侧不许变（⑤ 钉的是那个形状）。
 *  ⑯ 触屏**真拖一行**（第 ③×⑨ 条）：点过的那行把手要看得见（`opacity:1`）、≥44×44 且中心命中自己，
 *     然后用 CDP 真发一段 touch 序列把它拖到最上面 —— 判据打在**核心里存的那份顺序**上，不只打 DOM。
 *  ⑰ 触屏档通扫四个视图（列表 / 侧栏抽屉 / 四行编辑器 / 设置）：**不许有"看不见却接得住点击"的控件**。
 *     这是 G65/G66 那一族的通判据 —— 按"有效不透明度"（祖先链上乘积，opacity 不继承）+ 中心命中算，
 *     只在触屏档判。它第一跑就逮到编辑器那一格：四行的笔记里，非当前那 3 行各留一颗看不见的 44×44 拖拽靶。
 *  ⑱ 设置页不许再留**系统画的控件**（第 ⑪ 条）：每颗 checkbox / range 的 computed `appearance` 必须是 none、
 *     整行命中区 ≥44、滑杆填色走我们自己的 CSS 变量；1440 与 390 两档各量一次，触屏档再按一次 End 验效果。
 *  ⑲ 软键盘弹起之后，**正在编辑那一行必须还在编辑区可视范围里**（第 ⑨ 条）：焦点落在下缘那一行 →
 *     把可视视口缩 300 px → 行底要回到编辑区底之上，且 `scrollTop` 必须是应用自己动的（不许靠运气）。
 *  ⑳ 打开任何**就地输入 / 确认浮层**都不许改变其他行的 top（第 ④ 句"悬浮层的层级 / 在原来那一行里输出"）：
 *     三个面各量一次 —— 就地改名、文件夹删除确认、笔记永久删除确认，并各配一条样本量判据。
 *  ㉑ 置顶那颗点**点了要看得出变了**（第 ⑦ 条）：`pin-off` 与 `pin-on` 必须同时存在（正对照），真点一次走
 *     off→on→off，每步对图标名 / aria-pressed / 计算色，并回核心读 `pinned` 那一位。⑤ 只钉了"常显 + 已置顶读得出 on"，
 *     那颗点若永远画 on，⑤ 两条照样全绿 —— 这就是这腿存在的理由。
 *  ㉒ 「新建文件夹」那一格的正向一路（第 ④ 条）：弹窗是全局悬浮层（不许顶走任何一行）、打开就能打字、
 *     空名字不许确认、**取消真的什么都没建**、回车建的在核心里读得回来且侧栏看得见那一行。
 *  ㉔ §3.2 侧栏常驻：矮窗口（520 高）里同步状态条 / 设置入口 / 库读数都要还在，
 *     且侧栏那一栏自己不许有可滚溢出（能滚的只能是里面那段）。
 *  ㉓ 附件那颗芯片要说得清"这台设备此刻有没有这份可用的字节"（G74）：真账 available 时一句都不许说（正对照）、
 *     注入 missing+present 必须出现「正在等待下载」与两颗动作、注入 missing+unknown 必须换成中性那句。
 *  ㉕ §3.5 移动端底部胶囊栏：栏 64 / tab 56×52 / 主按钮 116×52 实心 --ink / 左右 12 底部 20 /
 *     图标 20 标签 10px / 栏不参与文档流而内容区自己让出 ≥84；再验三颗"决定类"按钮的效果
 *     （开抽屉、换页、核心里真多出一篇）与第四颗的反向那一格（没配账户点它绝不许转）。
 *  ㉖ §4.9 交互状态规格：按钮四变体 × 五状态（胶囊圆角、主按钮实心 --ink、按下 scale(0.97)、
 *     悬停 +6% 亮度、禁用 0.45、危险只换字与描边）、输入框"只读 ≠ 错误"两档各量各的、
 *     Toast 底部居中 ≤520 只染边框且带一颗点得着的「知道了」。
 *  ㉗ §4.1 保存四格（route 按住 edit_note 把"正在保存"停在屏幕上读；放成 400 读"原因上不上屏"，
 *     即缺口 G79 那一格），并回库里读一遍确认"已存在本机"说的是事实。
 *  ㉚ §4.2 第四格「整机只读（库过新）」：只拦首帧那次 `stats`，让它带 `libraryReadOnly`
 *     （这一位的新生产者 —— 旧形状里它挂在一个核心从不发出的同步事件上，缺口 G85），
 *     量全局横幅在不在、有没有给"请升级以编辑"这句下一步、是不是常驻（不是 4.5s 的 toast），
 *     读侧还读不读得到真数据，以及打了字之后**真核心**那一篇的 rev 与正文有没有动。
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
 * 开局先扫一遍**上一轮崩在中途留下的夹具**。
 * 每一腿自己收尾，但"崩在半路"那一轮收不了尾：实测残骸堆到 26 条笔记里 19 条是夹具，
 * 于是后面每一腿的"第几行 / 列表长度 / 最新那一篇"都被这些残骸改过 —— 制造的是与产品无关的红
 * （今天就是这样：保存四格那一腿找不到它自己刚建的那一篇，因为列表里挤了一堆同名前缀的旧货）。
 * `purgeByTitle` 是函数声明，会提升，所以这里能在它定义之前调用。
 */
const FIXTURE_MARKS = [
  '只读夹具', '常驻夹具', '拖排夹具', '滚动夹具', '状态夹具', '保存状态夹具',
  '置顶往返夹具', '通扫夹具', '键盘夹具', '附件账夹具', '弹窗夹具',
];
for (const mark of FIXTURE_MARKS) await purgeByTitle(mark);

/**
 * 文件夹那一族也一样要扫：⑧ 那一腿每跑一次建一颗带当批戳的"牺牲品"（`布局夹具删 HHMMSS`），
 * 崩在半路那一轮就永远留着它。残骸不是无害的 —— 编辑器那颗文件夹下拉的**选项数**变了，
 * 浮层就变高，390 那一档"选项点得着"那条腿的落点跟着挪（今天这条红就是这么来的：
 * 库里躺着 3 颗上一轮崩掉留下的牺牲品）。`布局夹具本/子` 是"复用而不是再造"的那两颗，不扫。
 */
{
  const stale = flattenFolders(await cmd('list_folders', {})).filter((f) => (f.name ?? '').startsWith('布局夹具删'));
  for (const f of stale) await cmd('delete_folder', { id: f.id });
  if (stale.length > 0) console.log(`开局清掉 ${stale.length} 颗上一轮留下的夹具文件夹：${stale.map((f) => f.name).join('、')}`);
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
      dotIcon: dot.querySelector('svg')?.getAttribute('data-icon') ?? '',
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
  check(`宽 ${width}：已置顶那颗读得出"已置顶"（pin-on + aria-pressed=true）`, pin.dotIcon === 'pin-on' && pin.dotPressed === 'true', JSON.stringify(pin));
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

  // §3.4 的溢出改造之后，390 这一档的这两颗不在条上，而在「更多 ›」面板里。
  // 先把它开起来 —— 面板里的行与条上的格子是同一个组件，钳位与"看得见的菜单"两档都要成立，
  // 这一腿因此比原来更硬：以前只量条上那一颗，现在量的是"换了位置的同一颗"。
  let openedMore = false;
  if ((await narrow.locator('[data-testid="tb-size"]').count()) === 0) {
    await narrow.click('[data-testid="tb-more"]');
    await narrow.waitForTimeout(500);
    openedMore = true;
  }
  check('㉝附 · 手机宽下这两颗确实要从「更多 ›」里进（开完面板才找得到）',
    openedMore === false || (await narrow.locator('[data-testid="tb-size-menu"], [data-testid="tb-size"]').count()) > 0,
    JSON.stringify({ openedMore }));

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
  // 选完一档，「更多 ›」会收起来（动作做完留着面板是让人以为还要再点一下）。
  // 所以后面每颗都要"找不到就先开面板"—— 这本身就是 §3.4 收纳之后的正常使用路径。
  const openTool = async (id) => {
    if ((await narrow.locator(`[data-testid="${id}"]`).count()) === 0) {
      await narrow.click('[data-testid="tb-more"]');
      await narrow.waitForTimeout(450);
    }
    await narrow.click(`[data-testid="${id}"]`);
    await narrow.waitForTimeout(300);
  };
  await openTool('tb-size');
  await narrow.click('[data-testid="tb-size-xl"]');
  await narrow.waitForTimeout(500);
  await narrow.dblclick('.nb-block .nb-content', { position: { x: 12, y: 8 } });
  await narrow.waitForTimeout(250);
  await openTool('tb-color');
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

/**
 * ⑭ 每一层**浮起来的东西**都不许出界（G64 那一族的通判据）。
 *
 * ⑬ 钉的是工具条那两层菜单，而同一族还有四格：快捷新建模板的 AppPopover、选中文字浮出的
 * 那条选区工具条、编辑器头那格文件夹下拉、设置页那四格下拉。它们的定位方式各不相同
 * （JS 算坐标 / absolute / Headless UI 自己排），所以"会不会有一格在窄屏上画到屏幕外"
 * 不能靠一处修好就推定全体没事 —— 每一格各自量一次，两档宽度各一次。
 * 判据打在 `right <= innerWidth && left >= 0` 与"选项中心点命中的是面板自己"，
 * 不打在"面板在 DOM 里"（那是 G29 已经犯过的错）。
 */
{
  const OVERLAYS = [
    { label: '快捷新建模板', trigger: '[data-testid="new-note-templates"]', panel: '[data-testid="app-popover-panel"]' },
    { label: '编辑器文件夹下拉', trigger: '[data-testid="editor-pane"] .app-select', panel: '[data-testid="app-select-panel"]' },
    { label: '设置页 TLS 下拉', trigger: '[data-testid="account-tls"]', panel: '[data-testid="app-select-panel"]', needSettings: true },
  ];
  for (const width of [390, 1440]) {
    const p = await browser.newPage({ viewport: { width, height: 844 } });
    const oErrors = [];
    p.on('pageerror', (e) => oErrors.push(String(e).slice(0, 140)));
    p.on('console', (m) => { if (m.type() === 'error') oErrors.push(m.text().slice(0, 140)); });
    await p.goto(URL_BASE, { waitUntil: 'networkidle' });
    await p.waitForSelector('[data-testid="sidebar"]', { timeout: 15000 });
    await p.waitForTimeout(1800);

    const readPanel = (sel) => p.evaluate((s) => {
      const el = document.querySelector(s);
      if (!el) return { missing: true };
      const r = el.getBoundingClientRect();
      const it = el.querySelector('button, [role="option"], .btn');
      let hits = false;
      if (it) {
        const b = it.getBoundingClientRect();
        const h = document.elementFromPoint(Math.round(b.left + b.width / 2), Math.round(b.top + b.height / 2));
        hits = Boolean(h && el.contains(h));
      }
      return { left: Math.round(r.left), right: Math.round(r.right), vw: window.innerWidth, inside: r.left >= 0 && r.right <= window.innerWidth, itemHits: hits };
    }, sel);

    for (const o of OVERLAYS) {
      if (o.needSettings) await p.evaluate(() => document.querySelector('[data-testid="nav-settings"]')?.click());
      else await p.evaluate(() => document.querySelector('[data-testid="nav-all"]')?.click());
      await p.waitForTimeout(900);
      if (o.trigger.includes('editor-pane')) {
        await p.click('[data-testid^="note-row-"]').catch(() => {});
        await p.waitForTimeout(1100);
      }
      await p.evaluate((t) => document.querySelector(t)?.click(), o.trigger);
      await p.waitForTimeout(450);
      const m = await readPanel(o.panel);
      check(`宽 ${width} · ${o.label}：浮层完整在视口内、选项点得着`, !m.missing && m.inside === true && m.itemHits === true, JSON.stringify(m));
      if (!m.missing) notes.push(`     浮层落点 宽 ${width} ${o.label}：${m.left}..${m.right}（视口 ${m.vw}）`);
      await p.keyboard.press('Escape');
      await p.waitForTimeout(250);
    }

    // 选区工具条：左边与靠右各选一次（它是跟着选区跑的，最坏情况在右缘）
    await p.evaluate(() => document.querySelector('[data-testid="nav-all"]')?.click());
    await p.waitForTimeout(700);
    await p.click('[data-testid^="note-row-"]').catch(() => {});
    await p.waitForTimeout(1100);
    for (const side of ['left', 'right']) {
      await p.evaluate(() => window.getSelection()?.removeAllRanges());
      const at = await p.evaluate((which) => {
        const blocks = Array.from(document.querySelectorAll('.nb-block .nb-content'));
        const el = which === 'left' ? blocks[0] : blocks[blocks.length - 1];
        if (!el) return null;
        // 要按**文字实际排到的右缘**取点：编辑区有 `--editor-measure` 那个宽度上限，
        // 拿块的盒子右缘去点会点到空白处（没有选区 ⇒ 那条工具条本来就不该出现）。
        const range = document.createRange();
        range.selectNodeContents(el);
        const tr = range.getBoundingClientRect();
        if (tr.width < 20) return null;
        return { x: which === 'left' ? Math.round(tr.left + 12) : Math.round(tr.right - 12), y: Math.round(tr.top + 8) };
      }, side);
      if (!at) break;
      await p.mouse.dblclick(at.x, at.y);
      await p.waitForTimeout(700);
      const m = await readPanel('[data-testid="selection-bar"]');
      check(`宽 ${width} · 选区工具条（在${side === 'left' ? '行首' : '行尾'}选）：完整在视口内、按钮点得着`, !m.missing && m.inside === true && m.itemHits === true, JSON.stringify({ at, ...m }));
      if (!m.missing) notes.push(`     浮层落点 宽 ${width} 选区工具条/${side}：${m.left}..${m.right}（视口 ${m.vw}）`);
    }
    check(`宽 ${width} · ⑭ 这一腿 console error 为零`, oErrors.length === 0, oErrors.slice(0, 3).join(' | '));
    await p.screenshot({ path: `${OUT}/28-overlays-${width}.png` });
    await p.close();
  }
}

/**
 * ⑮ 触屏那一档（`hover: none`）：只在 hover 露面的那两簇控件必须常驻、且真点得着（第 ④×⑨ 条）。
 *
 * 修前的读数：侧栏那簇 `opacity:0` 且 `pointer-events:none` ⇒ 手机上"改名/删除"这两颗**根本不存在**；
 * 列表行那簇更糟 —— `opacity:0` 却仍然吃点击 ⇒ 看不见却能点着，是隐形陷阱。
 * 桌面一侧不许变：那里仍是悬停才露面（⑤ 那条判据钉的就是这个形状）。
 */
{
  const tctx = await browser.newContext({ viewport: { width: 390, height: 844 }, hasTouch: true, isMobile: true });
  const tp = await tctx.newPage();
  const tErrors = [];
  tp.on('pageerror', (e) => tErrors.push(String(e).slice(0, 140)));
  tp.on('console', (m) => { if (m.type() === 'error') tErrors.push(m.text().slice(0, 140)); });
  await tp.goto(URL_BASE, { waitUntil: 'networkidle' });
  await tp.waitForSelector('[data-testid="sidebar"]', { timeout: 15000 });
  await tp.waitForTimeout(2000);
  // 390 档侧栏是关着的抽屉：不先拉开，量到的是"在屏幕外"而不是"看不见"
  await tp.evaluate(() => document.querySelector('[data-testid="sidebar-handle"], [data-testid="open-sidebar"]')?.click());
  await tp.waitForTimeout(700);

  const state = await tp.evaluate(() => {
    const pick = (sel) => {
      const el = document.querySelector(sel);
      if (!el) return null;
      const cs = getComputedStyle(el);
      return { opacity: cs.opacity, pe: cs.pointerEvents };
    };
    return {
      hoverNone: window.matchMedia('(hover: none)').matches,
      coarse: window.matchMedia('(pointer: coarse)').matches,
      tools: pick('.tree__tools'),
      actions: pick('.row-item__actions'),
    };
  });
  check('触屏模拟本身要成立（hover:none 与 pointer:coarse 都要为真，否则下面两条是空判据）', state.hoverNone === true && state.coarse === true, JSON.stringify(state));
  check('触屏：侧栏那簇工具常驻且可点（不常驻 = 手机上没有改名/删除）', state.tools?.opacity === '1' && state.tools?.pe === 'auto', JSON.stringify(state.tools));
  check('触屏：列表行的动作簇要露出来（opacity:0 却吃点击 = 隐形陷阱）', state.actions?.opacity === '1', JSON.stringify(state.actions));

  // "点得着"要打在命中测试上，不是打在"DOM 里有这颗按钮"上：不常驻时真正接住这一下的是
  // 名字那颗（`pointer-events: none` 的透明工具条让位）， mutate 一次就是下面这条先红。
  const hit = await tp.evaluate(() => {
    const b = document.querySelector('[data-testid="folder-rename"]');
    if (!b) return { missing: true };
    const r = b.getBoundingClientRect();
    const top = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
    // `contains` 而不是 `===`：那颗图标现在是一棵 SVG，中心命中的是它 —— 仍是这颗按钮接住的。
    // 判据没放宽：替它接住的那一颗（名字）不在这颗的子里，照样红。
    return { w: Math.round(r.width), hitEl: top?.tagName?.toLowerCase(), isBtn: Boolean(top && b.contains(top)) };
  });
  check('触屏：改名那颗的中心真的被它自己接住（不是名字按钮替它接 —— 那就是点不到）', !hit.missing && hit.isBtn === true, JSON.stringify(hit));

  // 点下去这一步要"红了就继续"：整个脚本挂在 tap 上会跳过后面的夹具清理，
  // 开发库里每次多堆 31 篇，下一轮的读数就不是它自己了。
  let tapFailed = '';
  try {
    await tp.tap('[data-testid="folder-rename"]', { timeout: 5000 });
  } catch (e) {
    tapFailed = String(e).split('\n')[0].slice(0, 120);
  }
  check('触屏：点「改名」这一下真发得出去（不许靠名字那颗代点）', tapFailed === '', tapFailed);
  await tp.waitForTimeout(700);
  const inline = await tp.evaluate(() => {
    const el = document.querySelector('[data-testid="folder-rename-input"]');
    if (!el) return { missing: true };
    const r = el.getBoundingClientRect();
    return { w: Math.round(r.width), focused: document.activeElement === el, value: el.value };
  });
  check('触屏点「改名」真长出就地输入框（带现名、拿到焦点 —— 用户那句"在原始的那个内容行里输出"）', !inline.missing && inline.focused === true && inline.w > 60 && (inline.value ?? '').length > 0, JSON.stringify(inline));
  await tp.screenshot({ path: `${OUT}/29-touch-tools-390.png` });
  check('⑮ 这一腿 console error 为零', tErrors.length === 0, tErrors.slice(0, 3).join(' | '));
  notes.push(`     触屏实测 390×844：侧栏工具 ${JSON.stringify(state.tools)}，行动作 ${JSON.stringify(state.actions)}，就地改名 ${JSON.stringify(inline)}`);
  await tctx.close();
}

/**
 * ⑯ 触屏**真的拖动一行**（第 ③ 条"每一行还能拖动上下行" × 第 ⑨ 条的移动端）。
 *
 * 为什么单独一条腿：⑮ 只量了"控件在不在、点得着吗"，而这一条要量的是**效果** ——
 * 把手看不见却能拖（修前实测 `opacity:0` + `hitIsGrip:true`）是一种"自动化全绿、人却找不到入口"的形状；
 * 反过来"看得见"也不等于"拖得动"：HTML5 DnD 在触摸端基本不触发，所以这里用 CDP 真发 touch 序列，
 * 再把**核心里存的那份顺序**读回来对账（不是只看 DOM 重排了）。
 */
{
  const MARK = '拖排夹具';
  await purgeByTitle(MARK); // 上一轮如果崩在中途，先把它自己的残留清掉再量
  const lines = [`${MARK} 甲`, `${MARK} 乙`, `${MARK} 丙`, `${MARK} 丁`];
  const made = await cmd('create_note', {
    folderId: null,
    doc: { v: 1, content: lines.map((text, i) => ({ id: `dg${i}${stamp}`, type: 'paragraph', content: [{ text }] })) },
  });
  const dragId = made?.id;
  const dctx = await browser.newContext({ viewport: { width: 390, height: 844 }, hasTouch: true, isMobile: true });
  const dp = await dctx.newPage();
  const dErrors = [];
  dp.on('pageerror', (e) => dErrors.push(String(e).slice(0, 140)));
  dp.on('console', (m) => { if (m.type() === 'error') dErrors.push(m.text().slice(0, 140)); });
  await dp.goto(URL_BASE, { waitUntil: 'networkidle' });
  await dp.waitForSelector('[data-testid^="note-row-"]', { timeout: 15000 });
  await dp.waitForTimeout(1200);
  for (const r of await dp.$$('[data-testid^="note-row-"]')) {
    if ((await r.innerText()).includes(MARK)) { await r.tap(); break; }
  }
  await dp.waitForSelector('.nb-block .nb-content', { timeout: 15000 });
  await dp.waitForTimeout(1000);

  // 点第 3 行：触屏没有 hover，"当前这一行"是唯一该露出把手的地方
  const hit3 = await dp.$$eval('.nb-block', (els) => {
    const r = els[2].getBoundingClientRect();
    return { x: Math.round(r.left + r.width * 0.5), y: Math.round(r.top + r.height / 2) };
  });
  await dp.touchscreen.tap(hit3.x, hit3.y);
  await dp.waitForTimeout(500);

  const grip = await dp.evaluate(() => {
    const on = document.querySelector('.nb-handles--on');
    const g = on?.querySelector('[data-testid="drag-handle"]');
    if (!g) return { missing: true };
    const cs = getComputedStyle(g.closest('.nb-handles'));
    const r = g.getBoundingClientRect();
    const top = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
    return {
      opacity: cs.opacity,
      w: Math.round(r.width),
      h: Math.round(r.height),
      gx: Math.round(r.left + r.width / 2),
      gy: Math.round(r.top + r.height / 2),
      hitIsGrip: Boolean(top && g.contains(top)),
      onRow: g.closest('.nb-block')?.innerText.trim().slice(0, 12),
    };
  });
  check('触屏：点过的那一行把手要**看得见**（看不见却能拖 = 自动化全绿、人找不到入口）', !grip.missing && grip.opacity === '1', JSON.stringify(grip));
  check('触屏：把手本身可命中且不小于 44×44（§6 的触摸下限）', !grip.missing && grip.hitIsGrip === true && grip.w >= 44 && grip.h >= 44, JSON.stringify(grip));

  // 只取正文那一格：`.nb-block` 的 innerText 会带上把手那颗 ⠿ 与行首符号，跟核心存的字符串对不上
  const texts = () => dp.$$eval('.nb-block', (els) => els.map((e) => (e.querySelector('.nb-content')?.innerText ?? '').replace(/\s+/g, ' ').trim()));
  const before = await texts();
  check('夹具真的给了四行（不然"顺序变了"会是空判据）', before.length === 4, JSON.stringify(before));

  // 真发一段触摸序列（不是 page.mouse：那是 pointerType=mouse，量的就不是同一条路了）
  const cdp = await dctx.newCDPSession(dp);
  const to = await dp.$$eval('.nb-block', (els) => {
    const r = els[0].getBoundingClientRect();
    return { x: Math.round(r.left + r.width * 0.5), y: Math.round(r.top + 2) };
  });
  const send = (type, x, y) => cdp.send('Input.dispatchTouchEvent', { type, touchPoints: type === 'touchEnd' ? [] : [{ x, y }] });
  let dragErr = '';
  try {
    await send('touchStart', grip.gx, grip.gy);
    for (let i = 1; i <= 8; i += 1) {
      await send('touchMove', Math.round(grip.gx + (to.x - grip.gx) * (i / 8)), Math.round(grip.gy + (to.y - grip.gy) * (i / 8)));
      await dp.waitForTimeout(50);
    }
    await send('touchEnd');
  } catch (e) {
    dragErr = String(e).split('\n')[0].slice(0, 120);
  }
  await dp.waitForTimeout(600);
  const after = await texts();
  check(`触屏拖一把：第 3 行真的排到最上面（${before[2] ?? '?'} → 首位）`, dragErr === '' && after[0] === before[2] && after.length === 4, JSON.stringify({ before, after, dragErr }));

  // 效果要落到核心，不是只重排了 DOM：轮询等自动保存那一程走完。
  // **这一条的牙要说准**：它是"屏幕与库一致"的守卫，不是"拖拽这一步自己会写库"的证明 ——
  // 两次变异（掐 `replaceBlocks` 里那次 `debouncedSave()`、把 `commitDrop` 换成只改本地状态）
  // 都没能让它红，因为编辑器还有别的持久化路径（失焦 flush 那一类）会补上这一笔。
  // 也就是说这条**测得到"拖完崩在半路"，测不到"拖这一步没发写"**，写台账时不许按后者吹。
  let coreOrder = [];
  for (let i = 0; i < 12; i += 1) {
    const got = await cmd('get_note', { id: dragId });
    coreOrder = (got?.doc?.content ?? []).map((b) => (b.content ?? []).map((s) => s.text).join(''));
    if (coreOrder[0] === after[0] && coreOrder.length === 4) break;
    await dp.waitForTimeout(400);
  }
  check('拖完的顺序**落进了核心**（读回存的那份，不是只看 DOM 变了）', coreOrder.join('|') === after.join('|'), JSON.stringify({ dom: after, core: coreOrder }));
  check('⑯ 这一腿 console error 为零', dErrors.length === 0, dErrors.slice(0, 3).join(' | '));
  await dp.screenshot({ path: `${OUT}/30-touch-drag-390.png` });
  notes.push(`     触屏拖动实测：把手 ${JSON.stringify(grip)}；顺序 ${before.join(' → ')} ⇒ ${after.join(' → ')}；核心 ${coreOrder.join(' → ')}`);
  await dctx.close();
  await purgeByTitle(MARK);
}

/**
 * ⑰ 触屏那一档的通扫：**页面上不许存在"看不见、中心却正好被它自己接住"的交互控件**。
 *
 * 这一条是 G65/G66 那一族的通判据（一次只修一簇 = 下一簇还会回来）：
 * `opacity:0` 只关掉绘制，**不关掉命中** —— 于是控件还在原地吃点击，只是人看不见。
 * 判据打在"有效不透明度"上而不是控件自己的 `opacity`：CSS 的 opacity **不继承**，
 * 一个 `opacity:0` 的容器把整块藏起来时，里面那颗按钮自己的 computed opacity 仍然是 1 ——
 * 只看自己那一层会把恰好那一格筛掉（第一版探针就是这么假绿的）。
 * 只在触屏档判：桌面那一档悬停会把它们露出来，"看不见却能点"在那一侧不成立。
 */
const TRAP_SCAN = () => {
  const eff = (el) => {
    let v = 1;
    for (let n = el; n && n !== document.documentElement; n = n.parentElement) {
      const cs = getComputedStyle(n);
      if (cs.display === 'none' || cs.visibility === 'hidden') return 1; // 根本不渲染，不算陷阱
      v *= parseFloat(cs.opacity);
      if (v < 0.05) return v;
    }
    return v;
  };
  const out = [];
  const sel = 'button, input, select, textarea, a[href], [role="button"], [tabindex]:not([tabindex="-1"])';
  for (const el of document.querySelectorAll(sel)) {
    const r = el.getBoundingClientRect();
    if (r.width < 8 || r.height < 8) continue;
    if (r.right < 0 || r.bottom < 0 || r.left > innerWidth || r.top > innerHeight) continue;
    if (eff(el) >= 0.05) continue;
    const top = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
    if (top && (top === el || el.contains(top))) {
      out.push(`${el.tagName.toLowerCase()}.${(el.className || '').toString().split(' ')[0]}[${el.getAttribute('data-testid') || ''}]`);
    }
  }
  return out;
};

{
  const MARK = '通扫夹具';
  await purgeByTitle(MARK);
  const made = await cmd('create_note', {
    folderId: null,
    doc: { v: 1, content: ['一', '二', '三', '四'].map((t, i) => ({ id: `ts${i}${stamp}`, type: 'paragraph', content: [{ text: `${MARK} ${t}` }] })) },
  });
  const sctx = await browser.newContext({ viewport: { width: 390, height: 844 }, hasTouch: true, isMobile: true });
  const sp = await sctx.newPage();
  const sErrors = [];
  sp.on('pageerror', (e) => sErrors.push(String(e).slice(0, 140)));
  sp.on('console', (m) => { if (m.type() === 'error') sErrors.push(m.text().slice(0, 140)); });
  await sp.goto(URL_BASE, { waitUntil: 'networkidle' });
  await sp.waitForSelector('[data-testid^="note-row-"]', { timeout: 15000 });
  await sp.waitForTimeout(1500);

  const drawer = (want) => sp.evaluate((w) => {
    const visible = (document.querySelector('[data-testid="sidebar"]')?.getBoundingClientRect().right ?? 0) > 40;
    if (visible !== w) document.querySelector('[data-testid="sidebar-handle"], [data-testid="open-sidebar"]')?.click();
  }, want);
  const views = [['列表', async () => {}]];
  views.push(['侧栏抽屉', async () => {
    await drawer(true);
    await sp.waitForTimeout(700);
  }]);
  views.push(['编辑器（四行）', async () => {
    await drawer(false);
    await sp.waitForTimeout(600);
    for (const r of await sp.$$('[data-testid^="note-row-"]')) {
      if ((await r.innerText()).includes(MARK)) { await r.tap(); break; }
    }
    await sp.waitForSelector('.nb-block .nb-content', { timeout: 15000 });
    await sp.waitForTimeout(900);
  }]);
  views.push(['设置', async () => {
    await drawer(true);
    await sp.waitForTimeout(600);
    await sp.evaluate(() => document.querySelector('[data-testid="nav-settings"]')?.click());
    await sp.waitForTimeout(900);
  }]);

  const seen = {};
  let editorBlocks = 0;
  for (const [name, go] of views) {
    await go();
    if (name.startsWith('编辑器')) editorBlocks = await sp.$$eval('.nb-block', (els) => els.length);
    seen[name] = await sp.evaluate(TRAP_SCAN);
    check(`触屏 · ${name}：没有"看不见却接得住点击"的控件（G65/G66 那一族的通判据）`, seen[name].length === 0, JSON.stringify(seen[name]));
  }
  // 扫到 0 个 ≠ 检查过：这一格判据的全部力气来自"非当前行"，只有一行的笔记会让它恒真
  check('触屏 · 编辑器那一格真的摊开四行（否则上面那条"陷阱 0 个"是空判据）', editorBlocks === 4, JSON.stringify({ editorBlocks }));
  check('⑰ 这一腿 console error 为零', sErrors.length === 0, sErrors.slice(0, 3).join(' | '));
  notes.push(`     触屏陷阱通扫：${JSON.stringify(seen)}`);
  await sp.screenshot({ path: `${OUT}/31-touch-traps-390.png` });
  await sctx.close();
  await purgeByTitle(MARK);
}

/**
 * ⑱ 设置页不许再留**系统画的控件**（第 ⑪ 条：「按钮 / Input / 弹窗这些都用统一样式的组件，
 * 两端 UI 要一致」）。
 *
 * 为什么必须量而不是读代码：`accent-color` 这类写法**看着像已经换肤了**，其实只改了颜色 ——
 * 方框的形状、圆角、尺寸仍是操作系统画的，Windows 与 macOS 上是两个不同的控件；
 * 而默认那一颗只有 13×13，摸都摸不准。判据因此打在**渲染后的 computed `appearance`** 与
 * 真实行高上，两档视口各量一次（1440 桌面 / 390 触屏）。
 */
{
  const scan = () => {
    const boxes = [...document.querySelectorAll('input[type="checkbox"]')];
    const ranges = [...document.querySelectorAll('input[type="range"]')];
    const all = [...boxes, ...ranges];
    return {
      n: all.length,
      native: all.filter((el) => getComputedStyle(el).appearance !== 'none').map((el) => `${el.type}:${el.getAttribute('data-testid') || '?'}`),
      rows: boxes.map((el) => Math.round(el.closest('label')?.getBoundingClientRect().height ?? 0)),
      rangeH: ranges.map((el) => Math.round(el.getBoundingClientRect().height)),
      fills: ranges.map((el) => getComputedStyle(el).getPropertyValue('--app-range-fill').trim()),
    };
  };

  for (const [width, touch] of [[1440, false], [390, true]]) {
    const sctx = await browser.newContext(
      touch
        ? { viewport: { width, height: 844 }, hasTouch: true, isMobile: true }
        : { viewport: { width, height: 950 } },
    );
    const sp = await sctx.newPage();
    const sErrors = [];
    sp.on('pageerror', (e) => sErrors.push(String(e).slice(0, 140)));
    sp.on('console', (m) => { if (m.type() === 'error') sErrors.push(m.text().slice(0, 140)); });
    await sp.goto(URL_BASE, { waitUntil: 'networkidle' });
    await sp.waitForSelector('[data-testid^="note-row-"]', { timeout: 15000 });
    await sp.waitForTimeout(1200);
    if (touch) {
      await sp.evaluate(() => document.querySelector('[data-testid="sidebar-handle"], [data-testid="open-sidebar"]')?.click());
      await sp.waitForTimeout(600);
    }
    await sp.click('[data-testid="nav-settings"]');
    await sp.waitForTimeout(900);
    // 导出那一格要先展开，否则清单里没有文件夹那一排，"全都不是系统控件"会少扫好几颗
    await sp.click('[data-testid="export-scoped"]');
    await sp.waitForTimeout(500);

    const got = await sp.evaluate(scan);
    check(`宽 ${width} · 设置页的清单不是空的（否则"没有系统控件"是空判据）`, got.n >= 4, JSON.stringify(got));
    check(`宽 ${width} · 设置页没有一颗还在用系统绘制（computed appearance 必须是 none）`, got.native.length === 0, JSON.stringify(got.native));
    check(`宽 ${width} · 每颗复选框的整行都是命中区（≥44，§6 触摸下限；点文案也要能切换）`, got.rows.length > 0 && got.rows.every((h) => h >= 44), JSON.stringify(got.rows));
    check(`宽 ${width} · 滑杆本体高度够按（触屏 ≥44，桌面 ≥24）`, got.rangeH.length === 1 && got.rangeH[0] >= (touch ? 44 : 24), JSON.stringify(got.rangeH));
    check(`宽 ${width} · 滑杆已走过的那一段由我们填色（CSS 变量在控件上，不是系统那套）`, got.fills.length === 1 && /%$/.test(got.fills[0]), JSON.stringify(got.fills));

    if (touch) {
      // 效果腿：真按一次键盘（不程序化塞 value —— 那会绕过浏览器自己的 input 事件）
      const before = await sp.evaluate(() => document.querySelector('input[type="range"]')?.value ?? '');
      await sp.focus('input[type="range"]');
      await sp.keyboard.press('End');
      await sp.waitForTimeout(600);
      const after = await sp.evaluate(() => {
        const el = document.querySelector('input[type="range"]');
        const readout = el?.closest('.field')?.querySelector('span')?.innerText ?? '';
        return { v: el?.value ?? '', max: el?.max ?? '', readout, fill: el ? getComputedStyle(el).getPropertyValue('--app-range-fill').trim() : '' };
      });
      check('触屏按 End：滑杆真的走到最大、读数与填色都跟着变（控件写的那一位就是被读的那一位）', after.v === after.max && after.v !== before && after.fill === '100%', JSON.stringify({ before, after }));
    }
    check(`宽 ${width} · ⑱ 这一腿 console error 为零`, sErrors.length === 0, sErrors.slice(0, 3).join(' | '));
    notes.push(`     设置页控件通扫 宽 ${width}：${JSON.stringify(got)}`);
    await sp.screenshot({ path: `${OUT}/32-native-controls-${width}.png` });
    await sctx.close();
  }
}

/**
 * ⑲ 软键盘弹起之后，**正在编辑那一行要还看得见**（第 ⑨ 条那句"输入框弹起带来的体验"）。
 *
 * 键盘那条腿（③ 那一格）只量了版心缩没缩、toast 抬没抬 —— 而用户在手机上做的事是**打字**：
 * 编辑区从底下被截掉 300 px 之后，焦点行如果本来就在下面那一段，它就留在键盘底下。
 * 修前实测（390×844、40 段的笔记、焦点落在编辑区下缘那一行）：行底 734 而编辑区底只有 425，
 * 且 `scrollTop` 一动没动（1005 → 1005）⇒ 字还在打，屏幕上看不见。
 *
 * 两处仪器自己的坑也记在这儿，因为它们都会把这条腿变成假绿：
 *  ① 移动仿真下 `visualViewport.height` 是原生 getter，**直接赋值会被静默丢掉** ⇒ 必须 `defineProperty`；
 *  ② 拿 `innerHeight` 找"最下面那一行"会找到移动端 tab bar 上（点上去命中的是「新建笔记」）
 *     ⇒ 只能在 `.editor-scroll` 自己的矩形里挑。
 */
{
  const KB = 300;
  const MARK = '键盘夹具';
  await purgeByTitle(MARK);
  const made = await cmd('create_note', {
    folderId: null,
    doc: {
      v: 1,
      content: Array.from({ length: 40 }, (_, i) => `${MARK} 第 ${String(i).padStart(2, '0')} 段`).map((text, i) => ({
        id: `kb${i}${stamp}`,
        type: 'paragraph',
        content: [{ text }],
      })),
    },
  });
  const kctx = await browser.newContext({ viewport: { width: 390, height: 844 }, hasTouch: true, isMobile: true });
  const kp = await kctx.newPage();
  const kErrors = [];
  kp.on('pageerror', (e) => kErrors.push(String(e).slice(0, 140)));
  kp.on('console', (m) => { if (m.type() === 'error') kErrors.push(m.text().slice(0, 140)); });
  await kp.goto(URL_BASE, { waitUntil: 'networkidle' });
  await kp.waitForSelector('[data-testid^="note-row-"]', { timeout: 15000 });
  await kp.waitForTimeout(1500);
  for (const r of await kp.$$('[data-testid^="note-row-"]')) {
    if ((await r.innerText()).includes(MARK)) { await r.tap(); break; }
  }
  await kp.waitForSelector('.nb-block .nb-content', { timeout: 15000 });
  await kp.waitForTimeout(1000);

  await kp.evaluate(() => [...document.querySelectorAll('.nb-block')][24].scrollIntoView({ block: 'start' }));
  await kp.waitForTimeout(500);
  const aim = await kp.evaluate(() => {
    const box = document.querySelector('.editor-scroll').getBoundingClientRect();
    const els = [...document.querySelectorAll('.nb-block')];
    let best = -1;
    let lowest = -1;
    els.forEach((el, i) => {
      const r = el.getBoundingClientRect();
      if (r.top > box.top && r.bottom < box.bottom - 4 && r.bottom > lowest) { lowest = r.bottom; best = i; }
    });
    const r = els[best].getBoundingClientRect();
    return { i: best, x: Math.round(r.left + 40), y: Math.round(r.top + r.height / 2) };
  });
  await kp.touchscreen.tap(aim.x, aim.y);
  await kp.waitForTimeout(500);

  const caret = () => kp.evaluate(() => {
    const scroller = document.querySelector('.editor-scroll');
    const focused = document.activeElement?.closest?.('.nb-block') ?? null;
    const r = focused?.getBoundingClientRect();
    return {
      idx: focused ? [...scroller.querySelectorAll('.nb-block')].indexOf(focused) : -1,
      rowBottom: r ? Math.round(r.bottom) : null,
      editorBottom: Math.round(scroller.getBoundingClientRect().bottom),
      scrollTop: Math.round(scroller.scrollTop),
      vv: Math.round(window.visualViewport.height),
      text: (focused?.querySelector('.nb-content')?.innerText ?? '').slice(0, 14),
    };
  });
  const before = await caret();
  check(`触屏：焦点真的落在挑中的那一行（${aim.i}）—— 否则下面全是空判据`, before.idx === aim.i && before.rowBottom !== null, JSON.stringify({ aim, before }));

  await kp.evaluate((kb) => {
    Object.defineProperty(window.visualViewport, 'height', { value: window.innerHeight - kb, configurable: true });
    window.visualViewport.dispatchEvent(new Event('resize'));
  }, KB);
  await kp.waitForTimeout(700);
  const during = await caret();
  check('仪器自检：可视视口那一格真的被改小了（没改到就是量了个假的）', during.vv === 844 - KB, JSON.stringify({ vv: during.vv }));
  check('键盘弹起：正在编辑那一行回到编辑区可视范围里（不许留在键盘底下）', during.rowBottom !== null && during.rowBottom <= during.editorBottom + 2, JSON.stringify({ before, during }));
  check('键盘弹起：是**应用自己滚的**（scrollTop 必须动，不许靠"那一行本来就在上面"混过去）', during.scrollTop !== before.scrollTop, JSON.stringify({ from: before.scrollTop, to: during.scrollTop }));

  await kp.evaluate(() => {
    Object.defineProperty(window.visualViewport, 'height', { value: window.innerHeight, configurable: true });
    window.visualViewport.dispatchEvent(new Event('resize'));
  });
  await kp.waitForTimeout(700);
  const after = await caret();
  check('键盘收起：焦点还在同一行，且那一行仍然看得见（不许跳走）', after.idx === before.idx && after.rowBottom !== null && after.rowBottom <= after.editorBottom + 2, JSON.stringify(after));
  check('⑲ 这一腿 console error 为零', kErrors.length === 0, kErrors.slice(0, 3).join(' | '));
  notes.push(`     键盘弹起实测 390×844：焦点行底 ${before.rowBottom}（编辑区底 ${before.editorBottom}）→ 弹起 ${during.rowBottom}（编辑区底 ${during.editorBottom}，scrollTop ${before.scrollTop}→${during.scrollTop}）→ 收起 ${after.rowBottom}`);
  await kp.screenshot({ path: `${OUT}/33-keyboard-caret-390.png` });
  await kctx.close();
  await cmd('delete_note', { id: made.id });
  await purgeByTitle(MARK);
}

/**
 * ⑳ **打开任何就地输入 / 确认浮层，都不许把别的行顶走**（第 ④ 条那句"悬浮窗的层级，
 * 而不是底下占了一个"，以及"在原始的那个内容行里输出，而不是底下突然补充一个新的行"）。
 *
 * 第 ⑧ 腿只量了文件夹那一格；这一条把同一句判据扫遍三个面：就地改名、文件夹删除确认、
 * 笔记永久删除确认。量的都是**其他行的 top 有没有动** —— 不是"浮层在不在 DOM 里"。
 */
{
  const octx = await browser.newContext({ viewport: { width: 1440, height: 950 } });
  const op = await octx.newPage();
  const oErrors = [];
  op.on('pageerror', (e) => oErrors.push(String(e).slice(0, 140)));
  op.on('console', (m) => { if (m.type() === 'error') oErrors.push(m.text().slice(0, 140)); });
  await op.goto(URL_BASE, { waitUntil: 'networkidle' });
  await op.waitForSelector('[data-testid="folder-row"]', { timeout: 15000 });
  await op.waitForTimeout(1500);

  // 判"有没有被顶走"要看**相邻行之间的间距**，不是行的绝对位置：
  // 绝对位置会被滚动混进来（点击时 Playwright 会把目标滚进视野，而 `overflow:hidden` 的容器
  // 也能被滚 —— 第一版就把"整体 +44 而间距没变"读成了缺陷，那是滚动不是布局）。
  // 间距只有"中间被插进东西"才会变 —— 那正是用户那句"底下突然补充一个新的行"的形状。
  const tops = (sel) => op.$$eval(sel, (els) => els.map((el) => Math.round(el.getBoundingClientRect().top)));
  const gaps = (t) => t.slice(1).map((v, i) => v - t[i]);
  const overlay = (sel) => op.evaluate((s) => {
    const el = document.querySelector(s);
    if (!el) return { missing: true };
    const r = el.getBoundingClientRect();
    const top = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
    return { h: Math.round(r.height), w: Math.round(r.width), focused: document.activeElement === el, hit: Boolean(top && el.contains(top)) };
  }, sel);
  const same = (a, b) => {
    const ga = gaps(a);
    const gb = gaps(b);
    return ga.length === gb.length && ga.every((v, i) => v === gb[i]);
  };

  const folders = await tops('[data-testid="folder-row"]');
  check('⑳ 样本量：侧栏至少三排文件夹（否则"没位移"可以是空判据）', folders.length >= 3, JSON.stringify(folders));

  // 桌面那一档工具是 hover 才露面的（静止态 `pointer-events: none`，⑤ 钉的就是这个形状），
  // 不先悬停就会点不到那颗 ✎（命中的是底下的名字按钮）—— 悬停本身就是用户的动作，不是绕路。
  await op.hover('[data-testid="folder-row"]');
  await op.waitForTimeout(300);
  await op.click('[data-testid="folder-rename"]');
  await op.waitForTimeout(500);
  const afterRename = await tops('[data-testid="folder-row"]');
  const renameBox = await overlay('[data-testid="folder-rename-input"]');
  check('就地改名不许把下面每一排顶下去（用户那句"而不是底下突然补充一个新的行"）', same(folders, afterRename), JSON.stringify({ before: folders, after: afterRename }));
  check('就地改名要长在原来那一行上（有高度、拿到焦点、中心命中它自己）', !renameBox.missing && renameBox.h > 12 && renameBox.focused === true && renameBox.hit === true, JSON.stringify(renameBox));
  await op.keyboard.press('Escape');
  await op.waitForTimeout(400);

  await op.hover('[data-testid="folder-row"]');
  await op.waitForTimeout(300);
  await op.click('[data-testid="folder-delete"]');
  await op.waitForTimeout(500);
  const afterDel = await tops('[data-testid="folder-row"]');
  const delBox = await overlay('[data-testid="folder-delete-confirm"]');
  check('文件夹"确认删除"是悬浮层：打开它不许改变任何一排的位置', same(folders, afterDel), JSON.stringify({ before: folders, after: afterDel }));
  check('文件夹"确认删除"那颗真的看得见、点得着', !delBox.missing && delBox.h > 12 && delBox.w > 12 && delBox.hit === true, JSON.stringify(delBox));
  await op.keyboard.press('Escape');
  await op.waitForTimeout(400);

  await op.click('[data-testid="nav-trash"]');
  await op.waitForTimeout(900);
  const trashTops = await tops('[data-testid^="note-row-"]');
  check('⑳ 样本量：回收站里至少三篇（同上，防空判据）', trashTops.length >= 3, JSON.stringify(trashTops.slice(0, 6)));
  await op.click('[data-testid="purge-note"]');
  await op.waitForTimeout(500);
  const afterPurge = await tops('[data-testid^="note-row-"]');
  const purgeBox = await overlay('[data-testid="purge-confirm"]');
  check('笔记"永久删除"确认也是悬浮层：不许把下面每一篇顶下去', same(trashTops, afterPurge), JSON.stringify({ before: trashTops.slice(0, 6), after: afterPurge.slice(0, 6) }));
  check('笔记"永久删除"那颗看得见、点得着', !purgeBox.missing && purgeBox.h > 12 && purgeBox.hit === true, JSON.stringify(purgeBox));
  check('⑳ 这一腿 console error 为零', oErrors.length === 0, oErrors.slice(0, 3).join(' | '));
  await op.screenshot({ path: `${OUT}/34-inplace-overlays-1440.png` });
  await octx.close();
}

/**
 * ㉑ 置顶那颗点**点了要看得出变了**（第 ⑦ 条原话："点击之后那个小圆点好像没有什么变化，
 * 它只有一个左上角只有一个对勾，这种不太好"）。
 *
 * 第 ⑤ 腿钉的是"那颗常显 + 已置顶读得出 `pin-on`"，但那两句**放在一起仍然可能被同一个形状满足**：
 * 如果那颗点永远画 `pin-on`，⑤ 的两条照样全绿。所以这里补的是正对照 —— 同一时刻必须存在
 * 一枚读得出"未置顶"的 `pin-off`，并且**真点一次**走完 off→on→off 一个来回，
 * 每步都对三样东西： `data-icon` / `aria-pressed` / 计算后的颜色，最后再回核心读 `pinned` 那一位
 * （界面写的那一位必须就是被读的那一位，见 [[verify-the-call-edge-not-just-the-callees-tests]]）。
 */
{
  const MARK = '置顶往返夹具';
  await purgeByTitle(MARK);
  const made = await cmd('create_note', {
    folderId: null,
    doc: { v: 1, content: [{ id: `pn${stamp}`, type: 'paragraph', content: [{ text: `${MARK} 正文` }] }] },
  });
  const pctx = await browser.newContext({ viewport: { width: 1440, height: 950 } });
  const pp = await pctx.newPage();
  const pErrors = [];
  pp.on('pageerror', (e) => pErrors.push(String(e).slice(0, 140)));
  pp.on('console', (m) => { if (m.type() === 'error') pErrors.push(m.text().slice(0, 140)); });
  await pp.goto(URL_BASE, { waitUntil: 'networkidle' });
  await pp.waitForSelector('[data-testid^="note-row-"]', { timeout: 15000 });
  await pp.waitForTimeout(1500);

  const readDot = () => pp.evaluate((id) => {
    const row = document.querySelector(`[data-testid="note-row-${id}"]`);
    if (!row) return { missing: 'row' };
    const dot = row.querySelector('[data-testid="note-pin-toggle"]');
    if (!dot) return { missing: 'dot' };
    return { glyph: dot.querySelector('svg')?.getAttribute('data-icon') ?? '', pressed: dot.getAttribute('aria-pressed'), color: getComputedStyle(dot).color, on: dot.classList.contains('row-item__pin--on') };
  }, made.id);
  const corePinned = async () => {
    const rows = await cmd('list_notes', { folderId: null, trash: false });
    const hit = (Array.isArray(rows) ? rows : []).find((n) => n.id === made.id);
    return hit ? hit.pinned === true : 'row-gone';
  };

  const off = await readDot();
  check('新笔记那一颗读得出"未置顶"（pin-off + aria-pressed=false —— 这是⑤缺的那枚正对照）', off.glyph === 'pin-off' && off.pressed === 'false' && off.on === false, JSON.stringify(off));
  const seedDot = await pp.evaluate((id) => {
    const d = document.querySelector(`[data-testid="note-row-${id}"] [data-testid="note-pin-toggle"]`);
    return d ? { glyph: d.querySelector('svg')?.getAttribute('data-icon') ?? '', color: getComputedStyle(d).color } : { missing: true };
  }, seedNoteId);
  check('同一时刻列表里两枚点长得不一样（on 与 off 并存，否则"点了没变化"还会回来）', seedDot.glyph === 'pin-on' && seedDot.glyph !== off.glyph, JSON.stringify({ seedDot, off }));

  /**
   * 点完之后**轮询到那颗点稳定**再判（最多 3s）。
   *
   * 为什么不是"等 900ms 读一次"：置顶会把这一行**换组**（置顶优先排序），
   * 换组在虚拟化列表里要重排窗口 —— 一次定长等待在行多时会读到换组前的那一帧，
   * 于是报出"点了没变"（本轮就红过一次：读数 off→off→on，而独立探针量同一颗是 off→on→off、
   * 核心 true→false 全程对得上 ⇒ **红的是仪器，不是产品**）。
   * 轮询不是放宽判据：最终仍要求 `pin-on` 与 `aria-pressed=true`，只是给它稳定下来的时间。
   */
  const settleDot = async (wanted) => {
    let last = null;
    for (let i = 0; i < 12; i += 1) {
      last = await readDot();
      if (last.glyph === wanted) return last;
      await pp.waitForTimeout(250);
    }
    return last;
  };

  await pp.click(`[data-testid="note-row-${made.id}"] [data-testid="note-pin-toggle"]`);
  const on = await settleDot('pin-on');
  check('点一次：同一颗变成"已置顶"，glyph 与颜色都跟着变（不是只换 aria）', on.glyph === 'pin-on' && on.pressed === 'true' && on.on === true && on.color !== off.color, JSON.stringify({ off, on }));
  check('点一次：置顶这一位**真的落进核心**（回读 list_notes 的 pinned）', (await corePinned()) === true, JSON.stringify(await corePinned()));
  check('置顶之后那一行不许从列表里消失（换组不是搬家搬没）', on.missing !== 'row', JSON.stringify(on));

  await pp.click(`[data-testid="note-row-${made.id}"] [data-testid="note-pin-toggle"]`);
  const back = await settleDot('pin-off');
  check('再点一次回到未置顶（一个来回不留半截状态）', back.glyph === 'pin-off' && back.pressed === 'false' && (await corePinned()) === false, JSON.stringify(back));
  check('㉑ 这一腿 console error 为零', pErrors.length === 0, pErrors.slice(0, 3).join(' | '));
  notes.push(`     置顶往返实测：${off.glyph}/${off.color} → ${on.glyph}/${on.color} → ${back.glyph}，核心 pinned true→false`);
  await pctx.close();
  await purgeByTitle(MARK);
}

/**
 * ㉒ 「新建文件夹」那颗 ＋ 走的是**弹窗输入**，而且这条决定要真的落到核心（第 ④ 条）。
 *
 * 第 ⑧ 腿钉的是"删除确认不许占排版"，第 ⑨ 腿钉的是"文件夹拍平成一层"，
 * 但"新建"这一路此前只在第 ⑨ 腿里被反向验过（`folder-new-sub-` 那两颗要没了）。
 * 这里补的是正向那一格：弹窗是悬浮层（不许顶走任何一行）、打开就能打字、
 * 空名字不许确认、**取消真的什么都没建**、回车建的能在核心里读回来。
 * 取消与确认都算"决定类按钮"——判据一律打在效果上（读核心那一排），不读弹窗关没关。
 */
{
  const NAME = `弹窗夹具 ${stamp}`;
  const xctx = await browser.newContext({ viewport: { width: 1440, height: 950 } });
  const xp = await xctx.newPage();
  const xErrors = [];
  xp.on('pageerror', (e) => xErrors.push(String(e).slice(0, 140)));
  xp.on('console', (m) => { if (m.type() === 'error') xErrors.push(m.text().slice(0, 140)); });
  await xp.goto(URL_BASE, { waitUntil: 'networkidle' });
  await xp.waitForSelector('[data-testid="folder-row"]', { timeout: 15000 });
  await xp.waitForTimeout(1500);

  const coreNames = async () => {
    const tree = await cmd('list_folders');
    return flattenFolders(tree).map((f) => f.name);
  };
  const rowGaps = () => xp.$$eval('[data-testid="folder-row"]', (els) => {
    const t = els.map((e) => Math.round(e.getBoundingClientRect().top));
    return t.slice(1).map((v, i) => v - t[i]);
  });
  const before = await rowGaps();
  const foldersBefore = await coreNames();

  await xp.click('[data-testid="new-folder"]');
  await xp.waitForTimeout(600);
  const dlg = await xp.evaluate(() => {
    const d = document.querySelector('[data-testid="new-folder-dialog"]');
    if (!d) return { missing: true };
    const r = d.getBoundingClientRect();
    const side = document.querySelector('[data-testid="sidebar"]').getBoundingClientRect();
    const scrim = document.querySelector('.app-dialog__scrim');
    const scs = scrim ? getComputedStyle(scrim) : null;
    const input = document.querySelector('[data-testid="new-folder-input"]');
    const confirmBtn = document.querySelector('[data-testid="app-dialog-confirm"]');
    const ir = input?.getBoundingClientRect();
    const cx = Math.round(r.left + r.width / 2);
    const top = document.elementFromPoint(cx, Math.round(r.top + r.height / 2));
    return {
      // 判"它是不是全局悬浮层"用三条设计无关的事实：面板中心落在侧栏之外、面板那一格最上面是它自己、
      // 遮罩是 fixed。（第一版我断的是"面板自己的 position 必须是 fixed"—— 那是量错了对象：
      // fixed 在 `.app-dialog__scrim` 与居中包层上，面板本来就是 static 的，红的是判据不是产品。）
      outsideSidebar: cx > Math.round(side.right),
      onTop: Boolean(top && d.contains(top)),
      scrimFixed: scs?.position === 'fixed',
      inputFocused: document.activeElement === input,
      inputHit: Boolean(ir && (() => { const t = document.elementFromPoint(ir.left + ir.width / 2, ir.top + ir.height / 2); return t === input || input?.contains(t); })()),
      confirmDisabled: confirmBtn?.disabled === true,
    };
  });
  const afterOpen = await rowGaps();
  check('新建文件夹是**全局悬浮弹窗**：面板在侧栏之外、盖在最上面、遮罩 fixed，且不许改变侧栏任何一行的间距', dlg.outsideSidebar === true && dlg.onTop === true && dlg.scrimFixed === true && JSON.stringify(before) === JSON.stringify(afterOpen), JSON.stringify({ dlg, before, afterOpen }));
  check('弹窗打开就能直接打字（输入框拿到焦点、中心命中它自己）', dlg.inputFocused === true && dlg.inputHit === true, JSON.stringify(dlg));
  check('空名字时「确认」是禁用的（不给建出一个空文件夹）', dlg.confirmDisabled === true, JSON.stringify(dlg));

  await xp.keyboard.press('Escape');
  await xp.waitForTimeout(600);
  const cancelled = await coreNames();
  check('取消这一路也要验效果：核心里一排文件夹不许多出一个', JSON.stringify(cancelled) === JSON.stringify(foldersBefore), JSON.stringify({ before: foldersBefore.length, after: cancelled.length, extra: cancelled.filter((n) => !foldersBefore.includes(n)) }));

  await xp.click('[data-testid="new-folder"]');
  await xp.waitForTimeout(500);
  await xp.keyboard.type(NAME);
  await xp.keyboard.press('Enter');
  await xp.waitForTimeout(1200);
  const created = await coreNames();
  check(`回车那一下真的建出来了（核心里读得到「${NAME}」）`, created.includes(NAME), JSON.stringify({ added: created.filter((n) => !foldersBefore.includes(n)) }));
  const rowShown = await xp.evaluate((name) => Array.from(document.querySelectorAll('[data-testid="folder-row"]')).some((r) => r.innerText.includes(name)), NAME);
  check('建出来的那一行在侧栏看得见（核心有了但界面没刷新是同一族的老形状）', rowShown === true, JSON.stringify({ rowShown }));

  // 清场：核心没有 purge_folder，软删之后读模型就不再看它（G63 那一格修的就是这条链）
  const tree = await cmd('list_folders');
  const hit = flattenFolders(tree).find((f) => f.name === NAME);
  if (hit?.id) await cmd('delete_folder', { id: hit.id });
  check('㉒ 这一腿 console error 为零', xErrors.length === 0, xErrors.slice(0, 3).join(' | '));
  notes.push(`     新建文件夹弹窗实测：${JSON.stringify(dlg)}；行数 ${foldersBefore.length} → 取消后 ${cancelled.length} → 回车后 ${created.length}（含「${NAME}」=${created.includes(NAME)}）`);
  await xp.screenshot({ path: `${OUT}/35-new-folder-dialog-1440.png` });
  await xctx.close();
}

/**
 * ㉓ 附件那颗芯片必须说得清"这台设备此刻有没有这份可用的字节"（§2.5 那条承诺 / G74）。
 *
 * 三格各钉一件事，缺一格就能被同一个形状糊过去：
 *  A **放行到真核心**（不注入）⇒ 计数证明这一次读账真的发了出去，且真账说 available 时芯片一句都不许说。
 *    第一版我把 A 写成"不注册路由、只看芯片没说话"⇒ 探针量到 `attachment_states` 被调 **0 次**，
 *    也就是"什么都没做"和"做对了"在那一版里长得一模一样（同 [[mutation-test-every-gate]] 的"扫到 0 项"那条）。
 *  B 注入 missing + present ⇒ 那句「正在等待下载」要真渲染出来，两颗动作按钮跟着在。
 *  C 注入 missing + unknown ⇒ 必须换成那句中性的 —— 服务器还没问过就说"正在等待下载"是许愿。
 *
 * 夹具必须先有一段文字：笔记标题由第一个非空文本块派生，只有附件块的这一篇**在列表里没有可读名字**，
 * 那一行永远点不中 ⇒ 整腿会退化成"编辑器根本没打开"的假绿（这一版就栽过）。
 */
{
  const MARK = '附件账夹具';
  await purgeByTitle(MARK);
  const paraId = `ap${stamp}`.slice(0, 12);
  const blockId = `aa${stamp}`.slice(0, 12);
  const made = await cmd('create_note', {
    folderId: null,
    doc: {
      v: 1,
      content: [
        { id: paraId, type: 'paragraph', content: [{ text: MARK }] },
        { id: blockId, type: 'attachment', attrs: { name: `${MARK}.txt` } },
      ],
    },
  });
  const attached = await cmd('attach_file', {
    noteId: made.id,
    blockId,
    role: 'file',
    filename: `${MARK}.txt`,
    mediaType: 'text/plain',
    bytesBase64: Buffer.from(`${MARK} 的字节内容，够长以避免被当成空文件拒绝`).toString('base64'),
  });
  const sha = attached?.sha256;
  check('㉓ 夹具要真有内容键（没附件就没法量这颗芯片）', typeof sha === 'string' && sha.length === 64, JSON.stringify(attached).slice(0, 160));
  // 真 UI 就是这么落库的：attach_file 把内容键合进块属性，再随一次 edit_note 进 doc。
  await cmd('edit_note', {
    id: made.id,
    expectedRev: attached?.rev ?? 1,
    doc: {
      v: 1,
      content: [
        { id: paraId, type: 'paragraph', content: [{ text: MARK }] },
        { id: blockId, type: 'attachment', attrs: { role: 'file', pending: false, sha256: sha, ref: sha, size: attached?.size, mediaType: 'text/plain', name: `${MARK}.txt` } },
      ],
    },
  });

  let states = { calls: 0, realAnswer: null };
  const openChip = async (injected) => {
    states = { calls: 0, realAnswer: null };
    const cctx = await browser.newContext({ viewport: { width: 1440, height: 950 } });
    await cctx.route('**/cmd/attachment_states', async (route) => {
      states.calls += 1;
      if (injected) {
        return route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(injected) });
      }
      const res = await route.fetch();
      const body = await res.text();
      states.realAnswer = body;
      return route.fulfill({ status: 200, contentType: 'application/json', body });
    });
    const cp = await cctx.newPage();
    const cErrors = [];
    cp.on('pageerror', (e) => cErrors.push(String(e).slice(0, 140)));
    cp.on('console', (m) => { if (m.type() === 'error') cErrors.push(m.text().slice(0, 140)); });
    await cp.goto(URL_BASE, { waitUntil: 'networkidle' });
    await cp.waitForSelector('[data-testid^="note-row-"]', { timeout: 15000 });
    await cp.waitForTimeout(1200);
    let opened = false;
    for (const r of await cp.$$('[data-testid^="note-row-"]')) {
      if ((await r.innerText()).includes(MARK)) { await r.click(); opened = true; break; }
    }
    await cp.waitForTimeout(1800);
    const chip = await cp.evaluate((mark) => {
      const chipEl = document.querySelector('.nb-chip');
      const notice = document.querySelector('[data-testid="attachment-notice"]');
      const box = (el) => { if (!el) return null; const b = el.getBoundingClientRect(); return { w: Math.round(b.width), h: Math.round(b.height), visible: b.width > 0 && b.height > 0 }; };
      return {
        opened: Boolean(chipEl),
        // 必须确认编辑器开的是**这一篇**：否则"芯片没说话"可能只是压根没打开。
        rightNote: chipEl ? (chipEl.innerText ?? '').includes(mark) : false,
        noticeText: notice ? (notice.innerText ?? '').trim() : null,
        retry: box(document.querySelector('[data-testid="attachment-retry"]')),
        reupload: box(document.querySelector('[data-testid="attachment-reupload"]')),
        markedMissing: Boolean(document.querySelector('.nb-chip--missing')),
      };
    }, MARK);
    return { chip, opened, cErrors, cp, cctx };
  }

  const a = await openChip(null);
  const aCalls = states.calls;
  check('A 打开这一篇要真的去问一次账（调用边，不是被调方的绿）', a.chip.opened && a.chip.rightNote && states.calls >= 1, JSON.stringify({ chip: a.chip, calls: states.calls }));
  check('A 真账说本机有好字节时，那颗芯片一句都不许说（正对照）', JSON.parse(states.realAnswer ?? '[]').some((r) => r.sha256 === sha && r.localState === 'available') && a.chip.noticeText === null && a.chip.retry === null && a.chip.markedMissing === false, JSON.stringify({ real: states.realAnswer, chip: a.chip }));
  await a.cp.screenshot({ path: `${OUT}/36-attach-ledger-available-1440.png` });
  await a.cctx.close();

  const b = await openChip([{ sha256: sha, localState: 'missing', remoteState: 'present' }]);
  check('B 本机缺 + 服务器有 ⇒ 必须真渲染出那句「正在等待下载」，两颗动作按钮跟着在且看得见', b.chip.noticeText === '附件不在这台设备上，正在等待下载' && b.chip.retry?.visible === true && b.chip.reupload?.visible === true && b.chip.markedMissing === true, JSON.stringify(b.chip));
  await b.cp.screenshot({ path: `${OUT}/37-attach-ledger-missing-1440.png` });
  await b.cctx.close();

  const c = await openChip([{ sha256: sha, localState: 'missing', remoteState: 'unknown' }]);
  check('C 服务器还没问过 ⇒ 换成那句中性说法，不许许愿"正在等待下载"（这一句里不许出现"下载"）', c.chip.noticeText === '这台设备上没有可用的这份附件' && !c.chip.noticeText.includes('下载'), JSON.stringify(c.chip));
  await c.cctx.close();

  check('㉓ 这一腿 console error 为零（注入回音不许把界面弄出报错）', [...a.cErrors, ...b.cErrors, ...c.cErrors].length === 0, [...a.cErrors, ...b.cErrors, ...c.cErrors].slice(0, 3).join(' | '));
  notes.push(`     附件账实测：A 真核心回音 ${JSON.stringify(a.chip.noticeText)}（问账 ${aCalls} 次）｜B(missing/present) ${JSON.stringify(b.chip.noticeText)}｜C(missing/unknown) ${JSON.stringify(c.chip.noticeText)}`);
  await cmd('purge_note', { id: made.id });
  await purgeByTitle(MARK);
}

/**
 * ㉔ §3.2 侧栏那三条"常驻"必须量在渲染后的几何上。
 *
 * 设计稿写的是：**导航与同步状态条常驻，只有文件夹列表内部滚动**，底部一行同时给设置入口和本地库读数。
 * 这句话以前只靠 CSS 结构成立，而结构最容易被一次改样式悄悄破掉（G62 那一族就是"看着挡住了其实还能滚"）。
 * 所以这一腿在**矮窗口**（520 高，文件夹多到放不下）里量四件事：
 *  ① 同步状态条 / 设置入口 / 库读数三块都真看得见（有尺寸、在视口内、中心命中自己）；
 *  ② 侧栏那一栏自己**不许**有可滚溢出（滚的只能是里面那段）；
 *  ③ 库读数要读出**真数字**（含"篇"与数字），不是空壳占位；
 *  ④ 夹具确实把文件夹塞多了（样本量判据，否则"没溢出"是恒真）。
 */
{
  const MARK = '常驻夹具';
  // 幂等：只补"缺的那几个"。第一版按 MARK 前缀算 before，于是每次跑都新造 12 个
  // （上一轮的墓碑还在库里，读模型滤掉了它们 ⇒ 看着像"没有"，又造一轮），开发库一轮厚一层。
  const liveFolders = flattenFolders(await cmd('list_folders'));
  const deficit = 12 - liveFolders.length;
  for (let i = 0; i < Math.max(0, deficit); i += 1) {
    await cmd('create_folder', { parentId: null, name: `${MARK} ${i}-${stamp}` });
  }
  const madeCount = flattenFolders(await cmd('list_folders')).length;

  const fctx = await browser.newContext({ viewport: { width: 1440, height: 520 } });
  const fp = await fctx.newPage();
  const fErrors = [];
  fp.on('pageerror', (e) => fErrors.push(String(e).slice(0, 140)));
  fp.on('console', (m) => { if (m.type() === 'error') fErrors.push(m.text().slice(0, 140)); });
  await fp.goto(URL_BASE, { waitUntil: 'networkidle' });
  await fp.waitForSelector('[data-testid="sidebar"]', { timeout: 15000 });
  await fp.waitForTimeout(1800);

  const shot = await fp.evaluate(() => {
    const side = document.querySelector('[data-testid="sidebar"]');
    /**
     * 只读**看得见**的那份文字。
     * 第一版这里用 `el.innerText`，读数出来是 `· 未配置同步 未配置同步` ——
     * 我据此以为界面把同一句话说了一遍又一遍，还去改了 store。
     * 实际是 `.visually-hidden` 用的是 `clip-path`（不是 `display:none`），
     * 那句给读屏器的 `aria-live` 文本照样进 `innerText` ⇒ **量的是仪器，不是界面**。
     * 所以这里显式把 `.visually-hidden` 与 0 尺寸节点剔掉。
     */
    const visibleText = (root) => Array.from(root.childNodes)
      .filter((n) => {
        // 注释节点必须剔掉：Vue 开发模式会把 falsy 的 `v-if` 留成 `<!--v-if-->` 占位，
        // 而我写在模板里的说明也是注释 —— 第一版把它们当文字读了，
        // 读数变成「· 未配置同步 v-if v-if v-if 那一句"为什么"必须看得见…」。
        if (n.nodeType === 8) return false;
        if (n.nodeType === 3) return true;
        if (n.nodeType !== 1) return false;
        const el = n;
        if (el.classList.contains('visually-hidden')) return false;
        const r = el.getBoundingClientRect();
        return r.width > 0 || r.height > 0;
      })
      .map((n) => (n.nodeType === 1 ? visibleText(n) : n.textContent ?? ''))
      .join(' ')
      .replace(/\s+/g, ' ')
      .trim();
    const view = (sel) => {
      const el = document.querySelector(sel);
      if (!el) return { missing: true };
      const r = el.getBoundingClientRect();
      const cx = Math.round(r.left + r.width / 2);
      const cy = Math.round(r.top + r.height / 2);
      const hit = document.elementFromPoint(cx, cy);
      return {
        h: Math.round(r.height),
        inViewport: r.top >= 0 && r.bottom <= window.innerHeight + 1,
        hitSelf: Boolean(hit && (hit === el || el.contains(hit) || hit.contains(el))),
        text: visibleText(el).slice(0, 60),
      };
    };
    const body = document.querySelector('[data-testid="sidebar"] .pane-body');
    // 状态 → 图标那一条**调用边**：五格表在 `ui/icons.ts`，但真正要成立的是"这一格渲染成了那枚、
    // 且静止那格没在转"。表绿而组件没接上，是这一族反复出现的形状。
    const sb = document.querySelector('[data-testid="syncbar"] svg');
    return {
      syncIcon: sb?.getAttribute('data-icon') ?? '',
      syncSpin: sb ? getComputedStyle(sb).animationName : '',
      syncbar: view('[data-testid="syncbar"]'),
      settings: view('[data-testid="nav-settings"]'),
      readout: view('[data-testid="library-readout"]'),
      sideOverflowY: side ? side.scrollHeight - side.clientHeight : -1,
      // §3.2 的正半句：**里面那一段必须真能滚**。少了这条，"侧栏自己不滚"可能只是因为没内容可滚。
      folderScrollY: body ? body.scrollHeight - body.clientHeight : -1,
      sideH: side ? Math.round(side.getBoundingClientRect().height) : -1,
      viewportH: window.innerHeight,
    };
  });

  check('㉔ 矮窗口（520 高）里同步状态条、设置入口、库读数三块都常驻可见',
    shot.syncbar.missing !== true && shot.settings.missing !== true && shot.readout.missing !== true
      && [shot.syncbar, shot.settings, shot.readout].every((b) => b.h > 0 && b.inViewport && b.hitSelf),
    JSON.stringify(shot));
  check('㉔ 同步状态条上那句话只说一遍（剔掉给读屏器的 aria-live 之后再比）',
    (shot.syncbar.text ?? '').split('未配置同步').length === 2, JSON.stringify(shot.syncbar));
  check('㉔ 侧栏那一栏自己不许有可滚溢出（滚的只能是里面那段）', shot.sideOverflowY <= 1, JSON.stringify(shot));
  check('㉔ 里面那段文件夹列表确实溢出可滚（样本量判据：否则上一条只是"没内容可滚"的恒真）', shot.folderScrollY > 0, JSON.stringify(shot));
  check('㉔ 库读数要读出真数字（含"篇"和数字），不是空壳', /篇/.test(shot.readout.text ?? '') && /\d/.test(shot.readout.text ?? ''), JSON.stringify(shot.readout));
  check('㉔ 样本量：文件夹确实塞到 12 个（否则"没溢出"是恒真）', madeCount >= 12, JSON.stringify({ madeCount }));
  check('㉔ 未配置那一格渲染成静止那枚云，且 animation-name 是 none（静止绝不像在忙，§2.3）',
    shot.syncIcon === 'sync-idle' && shot.syncSpin === 'none', JSON.stringify({ icon: shot.syncIcon, spin: shot.syncSpin }));
  check('㉔ 这一腿 console error 为零', fErrors.length === 0, fErrors.slice(0, 3).join(' | '));
  notes.push(`     侧栏常驻实测：同步条=${JSON.stringify(shot.syncbar.text)}｜读数=${JSON.stringify(shot.readout.text)}｜侧栏溢出=${shot.sideOverflowY}px（视口 ${shot.viewportH}）`);
  await fp.screenshot({ path: `${OUT}/38-sidebar-pinned-520.png` });
  await fctx.close();

  const after = flattenFolders(await cmd('list_folders'));
  for (const f of after.filter((x) => (x.name ?? '').startsWith(MARK))) await cmd('delete_folder', { id: f.id });
}

/** 把 `rgb(a, b, c)` 那串算成 WCAG 相对亮度，再算两个颜色的对比度（§5 要给数字，不给"看着还行"）。 */
function contrastRatio(fg, bg) {
  const lum = (s) => {
    const [r, g, b] = (s.match(/\d+(\.\d+)?/g) ?? ['0', '0', '0']).slice(0, 3).map((v) => {
      const c = Number(v) / 255;
      return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
    });
    return 0.2126 * r + 0.7152 * g + 0.0722 * b;
  };
  const [a, b] = [lum(fg), lum(bg)].sort((x, y) => y - x);
  return (a + 0.05) / (b + 0.05);
}

/**
 * ㉕ §3.5 移动端底部胶囊栏。这一格的规格**全是数字**（栏 64 / tab 56×52 / 主按钮 116×52 /
 * 左右 12 / 底部 20 / 图标 20 / 标签 10px / 内容区让出 ≥84），所以整条腿打在量数上 ——
 * 文字描述守不住这些数，只有量着才不会漂。
 *
 * 另外三条"决定类按钮要验效果"：菜单要真开抽屉、设置要真换页、新建要在**核心里**多出一篇
 * （点完界面自己变一下不算数）。第四颗（同步）验的是反向那一格：没配账户时点它**绝不许转**。
 */
{
  const bctx = await browser.newContext({ viewport: { width: 390, height: 844 }, hasTouch: true, isMobile: true });
  const bp = await bctx.newPage();
  const bErrors = [];
  bp.on('pageerror', (e) => bErrors.push(String(e).slice(0, 140)));
  bp.on('console', (m) => { if (m.type() === 'error') bErrors.push(m.text().slice(0, 140)); });
  await bp.goto(URL_BASE, { waitUntil: 'networkidle' });
  await bp.waitForSelector('[data-testid="dock"]', { timeout: 15000 });
  await bp.waitForTimeout(1800);

  const dock = await bp.evaluate(() => {
    const d = document.querySelector('[data-testid="dock"]');
    if (!d) return { missing: true };
    const cs = getComputedStyle(d);
    const r = d.getBoundingClientRect();
    const box = (el) => {
      const b = el.getBoundingClientRect();
      const hit = document.elementFromPoint(b.left + b.width / 2, b.top + b.height / 2);
      const svg = el.querySelector('svg');
      const label = el.querySelector('.dock__label');
      const ib = svg ? svg.getBoundingClientRect() : null;
      return {
        id: el.getAttribute('data-testid'),
        w: Math.round(b.width),
        h: Math.round(b.height),
        radius: getComputedStyle(el).borderRadius,
        hitSelf: Boolean(hit && el.contains(hit)),
        icon: svg?.getAttribute('data-icon') ?? '',
        iconW: ib ? Math.round(ib.width) : -1,
        iconH: ib ? Math.round(ib.height) : -1,
        labelPx: label ? getComputedStyle(label).fontSize : '',
        text: label?.textContent?.trim() ?? '',
      };
    };
    const body = document.querySelector('.app-body');
    const pane = document.querySelector('.pane--list') ?? document.querySelector('.app-body');
    const glyph = document.querySelector('[data-testid="mobile-sync"] svg');
    // 颜色一律跟**同一个页面里的 token 值**比，不写死三元组：token 换了色号，这条腿不该跟着改；
    // 而"这里根本没用 token"（写死一个色值）才是这条判据要抓的东西。
    const tok = (name) => {
      const p = document.createElement('span');
      p.style.color = `var(${name})`;
      document.body.appendChild(p);
      const v = getComputedStyle(p).color;
      p.remove();
      return v;
    };
    return {
      tokLine: tok('--line'),
      tokCanvas: tok('--canvas'),
      tokInk: tok('--ink'),
      position: cs.position,
      h: Math.round(r.height),
      radius: cs.borderRadius,
      borderW: cs.borderTopWidth,
      borderC: cs.borderTopColor,
      bg: cs.backgroundColor,
      left: Math.round(r.left),
      rightGap: Math.round(window.innerWidth - r.right),
      bottomGap: Math.round(window.innerHeight - r.bottom),
      dockTop: Math.round(r.top),
      bodyBottom: Math.round(body.getBoundingClientRect().bottom),
      bodyPadBottom: Math.round(parseFloat(getComputedStyle(body).paddingBottom)),
      paneBottom: Math.round(pane.getBoundingClientRect().bottom),
      viewportH: window.innerHeight,
      tabs: [...d.querySelectorAll('.dock__tab')].map(box),
      primaryBg: d.querySelector('.dock__tab--primary') ? getComputedStyle(d.querySelector('.dock__tab--primary')).backgroundColor : '',
      primaryColor: d.querySelector('.dock__tab--primary') ? getComputedStyle(d.querySelector('.dock__tab--primary')).color : '',
      syncIcon: glyph?.getAttribute('data-icon') ?? '',
      syncSpin: glyph ? getComputedStyle(glyph).animationName : '',
      syncBadge: document.querySelector('[data-testid="mobile-sync"]')?.getAttribute('data-badge') ?? '',
    };
  });

  check('㉕ 样本量：底栏正好四颗 tab（否则"每颗 56×52"可以是空判据）', dock.tabs?.length === 4, JSON.stringify(dock.tabs));
  check('㉕ 仪器自检：三个 token 探针互不相同（有一条是"未定义→退回继承色"，比较就失去意义）',
    new Set([dock.tokLine, dock.tokCanvas, dock.tokInk]).size === 3,
    JSON.stringify({ line: dock.tokLine, canvas: dock.tokCanvas, ink: dock.tokInk }));
  check('㉕ 栏：fixed、64 高、圆角 9999、1px --line 描边、--canvas 底（§3.5）',
    dock.position === 'fixed' && dock.h === 64 && dock.radius === '9999px' && dock.borderW === '1px'
      && dock.borderC === dock.tokLine && dock.bg === dock.tokCanvas,
    JSON.stringify(dock));
  check('㉕ 外层留白：左右各 12、底部 20（安全区内这一档就是 20）',
    dock.left === 12 && dock.rightGap === 12 && dock.bottomGap === 20,
    JSON.stringify({ left: dock.left, rightGap: dock.rightGap, bottomGap: dock.bottomGap }));
  check('㉕ 栏**不参与文档流**：内容区仍然铺到视口底（栏要是占了位，这一条就量不到"浮层"了）',
    dock.bodyBottom === dock.viewportH, JSON.stringify({ bodyBottom: dock.bodyBottom, viewportH: dock.viewportH }));
  check('㉕ 内容区自己让出 ≥84px，且列表那一栏的底真的在栏顶之上（最后一行不许被压住）',
    dock.bodyPadBottom >= 84 && dock.paneBottom <= dock.dockTop + 1,
    JSON.stringify({ pad: dock.bodyPadBottom, paneBottom: dock.paneBottom, dockTop: dock.dockTop }));

  const plain = (dock.tabs ?? []).filter((t) => t.id !== 'mobile-new');
  const primary = (dock.tabs ?? []).find((t) => t.id === 'mobile-new');
  check('㉕ 三颗普通 tab 各 56×52、胶囊圆角；主按钮 116×52（§3.5 的字面数）',
    plain.length === 3 && plain.every((t) => t.w === 56 && t.h === 52 && t.radius === '9999px')
      && primary?.w === 116 && primary?.h === 52,
    JSON.stringify({ plain, primary }));
  check('㉕ 每颗的中心被自己接住（不常驻、被栏的圆角裁掉、或被相邻那颗盖住都会红）',
    (dock.tabs ?? []).every((t) => t.hitSelf === true), JSON.stringify(dock.tabs.map((t) => [t.id, t.hitSelf])));
  // 注：会转的那枚，`getBoundingClientRect()` 量到的是**旋转后的外接框**（20 转 45° 是 28）。
  // 这一档同步是静止那格（没配账户），所以量到的是真实尺寸；哪天它在转的时候这条红了，
  // 先看的应该是"状态怎么变了"，不是"判据写错了"。
  check('㉕ 图标 20×20、标签 10px（§3.5 + §1.5 底栏标签）',
    (dock.tabs ?? []).every((t) => t.iconW === 20 && t.iconH === 20 && t.labelPx === '10px' && t.icon !== ''),
    JSON.stringify(dock.tabs.map((t) => [t.id, t.icon, t.iconW, t.labelPx])));
  check('㉕ 主按钮是实心 --ink，字色是 --canvas，且这一对的对比度够 AAA（§4.9 + §5）',
    dock.primaryBg === dock.tokInk && dock.primaryColor === dock.tokCanvas
      && contrastRatio(dock.primaryColor, dock.primaryBg) >= 7,
    JSON.stringify({ bg: dock.primaryBg, fg: dock.primaryColor, ratio: Number(contrastRatio(dock.primaryColor, dock.primaryBg).toFixed(2)) }));
  check('㉕ 没配账户时底栏那颗读的是"静止那格"，且**真的没在转**（§2.3 / §4.3 第五格）',
    dock.syncBadge === 'idle' && dock.syncIcon === 'sync-idle' && dock.syncSpin === 'none',
    JSON.stringify({ badge: dock.syncBadge, icon: dock.syncIcon, spin: dock.syncSpin }));

  // —— 三条"决定类按钮验效果" ——
  // 每一下都包一层：tap 不进去（被盖住 / pointer-events 关了 / 那颗根本不在）要**报成这一条红**，
  // 而不是把整条门禁吊死在半路 —— 后面还有三条判据和夹具清理。
  const tapSafe = async (sel) => {
    try {
      await bp.tap(sel, { timeout: 5000 });
      return '';
    } catch (e) {
      return String(e).split('\n')[0].slice(0, 90);
    }
  };

  const tapMenu = await tapSafe('[data-testid="mobile-sidebar"]');
  await bp.waitForTimeout(600);
  const drawerOpen = await bp.evaluate(() => document.querySelector('.app-shell')?.getAttribute('data-drawer'));
  check('点「菜单」：抽屉真的拉开（读 app-shell 的 data-drawer，不读按钮自己变没变）',
    tapMenu === '' && drawerOpen === 'sidebar', JSON.stringify({ tapMenu, drawerOpen }));
  // 关抽屉：不能 tap 遮罩的**中心** —— 那一格正被抽屉自己盖着（遮罩在 260 宽的抽屉之下）。
  // 打在最右边那条露出来的遮罩带上，才是用户真会点的那一下。
  await bp.touchscreen.tap(360, 400);
  await bp.waitForTimeout(500);
  const drawerClosed = await bp.evaluate(() => document.querySelector('.app-shell')?.getAttribute('data-drawer'));
  check('点抽屉外那条遮罩：真的收回去了（不收，后面几颗点的都是遮罩不是底栏）', drawerClosed === 'none', JSON.stringify({ drawerClosed }));

  const tapSettings = await tapSafe('[data-testid="mobile-settings"]');
  await bp.waitForTimeout(700);
  const onSettings = await bp.evaluate(() => Boolean(document.querySelector('[data-testid="settings-back"]')));
  check('点「设置」：真的换到了设置页（窄屏那页才有返回那颗，认它当路标）',
    tapSettings === '' && onSettings === true, JSON.stringify({ tapSettings, onSettings }));
  const tapBack = await tapSafe('[data-testid="settings-back"]');
  await bp.waitForTimeout(700);
  const leftSettings = await bp.evaluate(() => !document.querySelector('[data-testid="settings-back"]'));
  check('点「返回」：真的回了笔记那一栏（单栏形态下这是唯一退路，它不管用人就困在设置页）',
    tapBack === '' && leftSettings === true, JSON.stringify({ tapBack, leftSettings }));

  const idsBefore = new Set((await cmd('list_notes', { folderId: null, trash: false })).map((n) => n.id));
  const tapSync = await tapSafe('[data-testid="mobile-sync"]');
  await bp.waitForTimeout(900);
  const afterIdleTap = await bp.evaluate(() => {
    const g = document.querySelector('[data-testid="mobile-sync"] svg');
    return { icon: g?.getAttribute('data-icon') ?? '', spin: g ? getComputedStyle(g).animationName : '', badge: document.querySelector('[data-testid="mobile-sync"]')?.getAttribute('data-badge') ?? '' };
  });
  check('点「立即同步」但没配账户：那颗**不许**点亮成"正在同步"（用户原话那一格），也不许转',
    tapSync === '' && afterIdleTap.badge === 'idle' && afterIdleTap.icon === 'sync-idle' && afterIdleTap.spin === 'none',
    JSON.stringify({ tapSync, ...afterIdleTap }));

  const tapNew = await tapSafe('[data-testid="mobile-new"]');
  await bp.waitForTimeout(1200);
  const rows = await cmd('list_notes', { folderId: null, trash: false });
  const created = rows.filter((n) => !idsBefore.has(n.id));
  const paneAfterNew = await bp.evaluate(() => document.querySelector('.app-shell')?.getAttribute('data-pane'));
  check('点「新建」：核心里真的多出一篇，且窄屏切到了编辑器那一栏',
    tapNew === '' && created.length === 1 && paneAfterNew === 'editor', JSON.stringify({ tapNew, made: created.map((n) => n.id), paneAfterNew }));
  for (const n of created) await cmd('purge_note', { id: n.id });

  check('㉕ 这一腿 console error 为零', bErrors.length === 0, bErrors.slice(0, 3).join(' | '));
  notes.push(`     底栏实测：栏 ${dock.h} 高、留白 ${dock.left}/${dock.rightGap}/${dock.bottomGap}、tab ${JSON.stringify((dock.tabs ?? []).map((t) => `${t.w}×${t.h}`))}；内容让出 ${dock.bodyPadBottom}px，列表底 ${dock.paneBottom} vs 栏顶 ${dock.dockTop}`);
  await bp.screenshot({ path: `${OUT}/39-dock-390.png` });
  await bctx.close();
}

/**
 * ㉖ §4.9 交互状态规格：按钮四变体 × 五状态、输入框"只读 ≠ 错误"、Toast 只染边框且带「知道了」。
 *
 * 判据一律打在**渲染后的 computed style**上，不打在"CSS 文件里有没有那一行"上 ——
 * 状态样式最常见的糊法是"写着，但选择器没命中/被后一条盖掉"。
 * 颜色一律与同一页里的 token 值比（`tok()`），不写死三元组。
 */
{
  const MARK = '状态夹具';
  const sctx = await browser.newContext({ viewport: { width: 1440, height: 900 } });
  const sp = await sctx.newPage();
  const sErrors = [];
  sp.on('pageerror', (e) => sErrors.push(String(e).slice(0, 140)));
  sp.on('console', (m) => { if (m.type() === 'error') sErrors.push(m.text().slice(0, 140)); });
  await sp.goto(URL_BASE, { waitUntil: 'networkidle' });
  await sp.waitForSelector('[data-testid="nav-settings"]', { timeout: 15000 });
  await sp.click('[data-testid="nav-settings"]');
  await sp.waitForTimeout(900);

  const SNAP = () => sp.evaluate(() => {
    const tok = (n) => {
      const p = document.createElement('span');
      p.style.color = `var(${n})`;
      document.body.appendChild(p);
      const v = getComputedStyle(p).color;
      p.remove();
      return v;
    };
    const pick = (sel) => {
      const el = document.querySelector(sel);
      if (!el) return { missing: true };
      const cs = getComputedStyle(el);
      const r = el.getBoundingClientRect();
      const hit = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
      return {
        bg: cs.backgroundColor,
        fg: cs.color,
        radius: cs.borderRadius,
        borderC: cs.borderTopColor,
        opacity: cs.opacity,
        filter: cs.filter,
        transform: cs.transform,
        outline: `${cs.outlineWidth} ${cs.outlineStyle}`,
        outlineColor: cs.outlineColor,
        cursor: cs.cursor,
        h: Math.round(r.height),
        active: el.matches(':active'),
        centered: Math.abs((r.left + r.right) / 2 - window.innerWidth / 2) <= 1,
        hitSelf: Boolean(hit && el.contains(hit)),
        placeholder: el.matches('input, textarea') ? getComputedStyle(el, '::placeholder').color : '',
      };
    };
    const host = document.querySelector('[data-testid="toast-host"]');
    return {
      t: {
        ink: tok('--ink'), canvas: tok('--canvas'), danger: tok('--danger'), mute: tok('--mute'),
        surface: tok('--surface'), sunken: tok('--sunken'), accent: tok('--accent'), line: tok('--line'),
      },
      primary: pick('[data-testid="account-save"]'),
      press: pick('[data-testid="new-folder"]'),
      danger: pick('[data-testid="restore-db"]'),
      input: pick('[data-testid="account-baseUrl"]'),
      toastHost: host ? { w: Math.round(host.getBoundingClientRect().width), centered: Math.abs((host.getBoundingClientRect().left + host.getBoundingClientRect().right) / 2 - window.innerWidth / 2) <= 1 } : { missing: true },
    };
  });

  const base = await SNAP();
  check('㉖ 样本量：主按钮、危险按钮、输入框三颗都真在设置页上（否则下面全是空判据）',
    base.primary.missing !== true && base.danger.missing !== true && base.input.missing !== true, JSON.stringify(base));
  check('㉖ 主按钮：实心 --ink 底、--canvas 字、胶囊圆角，且这一对 ≥7:1（§4.9 + §5）',
    base.primary.bg === base.t.ink && base.primary.fg === base.t.canvas && base.primary.radius === '9999px'
      && contrastRatio(base.primary.fg, base.primary.bg) >= 7,
    JSON.stringify({ ...base.primary, ratio: Number(contrastRatio(base.primary.fg, base.primary.bg).toFixed(2)) }));
  check('㉖ 危险变体：文字与描边同转 --danger，**底是透明**（整块涂红会把"删这一条"画成"整页在报警"）',
    base.danger.fg === base.t.danger && base.danger.borderC === base.t.danger && base.danger.bg === 'rgba(0, 0, 0, 0)',
    JSON.stringify(base.danger));
  check('㉖ 输入框占位用 --mute（§4.9 默认那一行）', base.input.placeholder === base.t.mute, JSON.stringify({ ph: base.input.placeholder, mute: base.t.mute }));

  await sp.focus('[data-testid="account-baseUrl"]');
  await sp.waitForTimeout(250);
  const focused = await SNAP();
  check('㉖ 输入框聚焦：2px --accent 外描边，且**底色不动**（以前顺手把 sunken 换成 canvas，看着像换了状态）',
    focused.input.outline === '2px solid' && focused.input.outlineColor === focused.t.accent && focused.input.bg === focused.t.sunken,
    JSON.stringify(focused.input));

  // 两档**各量各的**：一条字段级校验不会同时是"只读"和"出错"，混在一起量等于没量。
  await sp.evaluate(() => { document.querySelector('[data-testid="account-baseUrl"]').readOnly = true; });
  const readOnly = await SNAP();
  await sp.evaluate(() => { document.querySelector('[data-testid="account-baseUrl"]').readOnly = false; });
  await sp.evaluate(() => { document.querySelector('[data-testid="account-baseUrl"]').setAttribute('aria-invalid', 'true'); });
  const invalid = await SNAP();
  await sp.evaluate(() => { document.querySelector('[data-testid="account-baseUrl"]').removeAttribute('aria-invalid'); });
  check('㉖ 只读那一档：--surface 底 + --line 描边 + --mute 字，描边**不带 --danger**（只读不是错误）',
    readOnly.input.bg === readOnly.t.surface && readOnly.input.fg === readOnly.t.mute
      && readOnly.input.borderC === readOnly.t.line,
    JSON.stringify({ bg: readOnly.input.bg, fg: readOnly.input.fg, border: readOnly.input.borderC }));
  check('㉖ 错误那一档：描边转 --danger、底**不跟着换**（换底就成"这一格被系统接管了"）',
    invalid.input.borderC === invalid.t.danger && invalid.input.bg === invalid.t.sunken,
    JSON.stringify({ border: invalid.input.borderC, bg: invalid.input.bg }));
  check('㉖ 只读与错误的计算结果必须**不是一套**（"两件不同的事"的形式化：两档读数逐字段比，全等就是没分开）',
    JSON.stringify([readOnly.input.bg, readOnly.input.fg, readOnly.input.borderC])
      !== JSON.stringify([invalid.input.bg, invalid.input.fg, invalid.input.borderC]),
    JSON.stringify({ ro: [readOnly.input.bg, readOnly.input.fg, readOnly.input.borderC], err: [invalid.input.bg, invalid.input.fg, invalid.input.borderC] }));

  // 按下这一档要**先把它滚进视野**：`mouse.move` 不像 `click` 会自动滚，
  // 设置页那一颗在折线以下时，第一次量到的是"页面别处被按下"（读数 transform: none）。
  // 也不能挑会提交的那颗：上一版挑了「保存账户」，mouse.down + mouse.up 就是一次真点击，
  // 于是量到的 transform 里混进了一次真提交（核心回 400，界面上多出一条错误 toast）。
  // 这里挑「新建文件夹」，并且**把手移到别处再抬起** —— click 只在同一元素上按下抬起才发。
  await sp.$eval('[data-testid="new-folder"]', (el) => el.scrollIntoView({ block: 'center' }));
  await sp.waitForTimeout(250);
  const box = await sp.$eval('[data-testid="new-folder"]', (el) => {
    const r = el.getBoundingClientRect();
    return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
  });
  await sp.mouse.move(box.x, box.y);
  await sp.mouse.down();
  await sp.waitForTimeout(200); // transform 是 120ms 的过渡，立刻读会读到"还没走到 0.97"
  const pressed = await SNAP();
  await sp.mouse.move(8, 8);
  await sp.mouse.up();
  // 抬起处与按下处不同，浏览器把 click 发给共同的祖先 ⇒ 这颗按钮的 @click 不该发。
  // 真开了弹窗就按原路关掉（**不用 Escape**：这条快捷键会把我们从设置页弹回笔记那一栏，
  // 下一档要 hover 的那颗当场消失，量到的是 30s 超时 —— 上一版就栽在这儿）。
  const dlgOpen = await sp.evaluate(() => Boolean(document.querySelector('[data-testid="new-folder-dialog"]')));
  if (dlgOpen) await sp.click('[data-testid="app-dialog-cancel"]');
  await sp.waitForTimeout(300);
  check('㉖ 按下那一档：transform 真的是 scale(0.97)，且这一下**没顺手把弹窗开出来**',
    pressed.press.active === true && pressed.press.transform === 'matrix(0.97, 0, 0, 0.97, 0, 0)' && dlgOpen === false,
    JSON.stringify({ active: pressed.press.active, transform: pressed.press.transform, dlgOpen }));

  await sp.hover('[data-testid="account-save"]');
  await sp.waitForTimeout(200);
  const hovered = await SNAP();
  check('㉖ 悬停那一档：主按钮走"亮度 +6%"，不是换一种底色',
    hovered.primary.filter === 'brightness(1.06)', JSON.stringify({ filter: hovered.primary.filter }));

  await sp.click('[data-testid="new-folder"]');
  await sp.waitForTimeout(600);
  const disabled = await sp.evaluate(() => {
    const el = document.querySelector('[data-testid="app-dialog-confirm"]');
    if (!el) return { missing: true };
    const cs = getComputedStyle(el);
    return { isDisabled: el.disabled, opacity: cs.opacity, cursor: cs.cursor };
  });
  check('㉖ 禁用那一档（空名字时弹窗那颗真的禁用）：opacity 0.45、cursor default',
    disabled.isDisabled === true && disabled.opacity === '0.45' && disabled.cursor === 'default', JSON.stringify(disabled));
  // 关弹窗走它自己的「取消」，不走 Escape —— Escape 在这条应用里还兼着"从设置页退回笔记那一栏"，
  // 一按就把后面几档要量的那颗按钮从 DOM 里拿掉了。
  await sp.click('[data-testid="app-dialog-cancel"]');
  await sp.waitForTimeout(400);

  const made = await cmd('create_note', {
    folderId: null,
    doc: { v: 1, content: [{ id: `st${stamp}`, type: 'paragraph', content: [{ text: `${MARK} ${stamp}` }] }] },
  });
  await sp.click('[data-testid="nav-all"]');
  await sp.waitForTimeout(900);
  await sp.click(`[data-testid="note-row-${made.id}"]`);
  await sp.waitForTimeout(500);
  // 不用 Backspace 触发：点开那一行会把焦点交给编辑区，`typing` 一真那条快捷键守卫就**故意不动作**
  // （它不该把删除当成删字符）—— 于是"没 toast"量的会是键盘守卫，不是 toast 这一格。改点删除那颗。
  await sp.click('[data-testid="trash-note"]');
  await sp.waitForTimeout(800);
  const trashed = (await cmd('list_notes', { folderId: null, trash: true })).some((n) => n.id === made.id);
  const toast = await sp.evaluate(() => {
    const el = document.querySelector('.toast');
    if (!el) return { missing: true };
    const cs = getComputedStyle(el);
    const ack = el.querySelector('[data-testid="toast-ack"]');
    const r = ack ? ack.getBoundingClientRect() : null;
    const hit = r ? document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2) : null;
    return {
      text: el.textContent?.trim().slice(0, 40) ?? '',
      bg: cs.backgroundColor,
      level: el.className,
      ackText: ack?.textContent?.trim() ?? '',
      ackH: r ? Math.round(r.height) : -1,
      ackHit: Boolean(ack && hit && ack.contains(hit)),
    };
  });
  const snapToast = await SNAP();
  check('㉖ Toast：底部居中堆叠、最宽 520，且这一条是**真做了一件事**之后弹的（回核心读到它进了回收站）',
    toast.missing !== true && trashed === true && snapToast.toastHost.centered === true && snapToast.toastHost.w <= 520,
    JSON.stringify({ trashed, toast, host: snapToast.toastHost }));
  check('㉖ Toast 只染边框，不整块染色：底色仍是 --canvas',
    toast.bg === snapToast.t.canvas, JSON.stringify({ bg: toast.bg, canvas: snapToast.t.canvas, cls: toast.level }));
  check('㉖ Toast 带一颗「知道了」，看得见点得着（≥44 高、中心命中自己）',
    toast.ackText === '知道了' && toast.ackH >= 44 && toast.ackHit === true, JSON.stringify(toast));
  check('㉖ 这条 toast 说清了"没有丢什么"（§4.9 的 ✅ 例子，不是"操作失败"那种兜底句）',
    /最近删除/.test(toast.text) && /恢复/.test(toast.text), JSON.stringify({ text: toast.text }));
  check('㉖ 这一腿 console error 为零', sErrors.length === 0, sErrors.slice(0, 3).join(' | '));
  notes.push(`     §4.9 实测：主按钮 ${base.primary.fg} on ${base.primary.bg}（${contrastRatio(base.primary.fg, base.primary.bg).toFixed(2)}:1）、按下 ${pressed.press.transform}、悬停 ${hovered.primary.filter}、禁用 ${disabled.opacity}`);
  await sp.screenshot({ path: `${OUT}/40-states-1440.png` });
  await sctx.close();
  await cmd('purge_note', { id: made.id });
  await purgeByTitle(MARK);
}

/**
 * ㉗ §4.1 保存四格：正在保存 / 有改动没存 / 已存在本机 / 失败（必须带具体原因）。
 *
 * 四格全部走**真的一次保存往返**量出来：`edit_note` 那一发用 route 拦下来 ——
 * 按住不放才能把"正在保存"这一格停在屏幕上读（它天然只有几百毫秒的窗口，不拦就量不到），
 * 放成 400 才能量"那句原因上不上屏"（缺口 G79 的形状：被拒之后屏幕上说的是"还有改动没存"）。
 * 这一腿刻意**不判 console 零错** —— 400 那个回包是注入的，浏览器自己会记一条资源错误，
 * 那条不是产品的问题（判据把环境噪音算成缺陷，下一轮就没人信这条腿了）。
 */
{
  const MARK2 = '保存状态夹具';
  const wctx = await browser.newContext({ viewport: { width: 1440, height: 900 } });
  const wp = await wctx.newPage();
  let mode = 'hold';
  let release = null;
  await wp.route('**/cmd/edit_note', async (route) => {
    if (mode === 'hold') await new Promise((resolve) => { release = resolve; });
    if (mode === 'fail') {
      return route.fulfill({
        status: 400,
        contentType: 'application/json',
        body: JSON.stringify({ code: 'quota_full', messageKey: 'error.quota_full', retryable: false }),
      });
    }
    return route.continue();
  });
  await wp.goto(URL_BASE, { waitUntil: 'networkidle' });
  await wp.waitForSelector('[data-testid="folder-row"]', { timeout: 15000 });

  const made = await cmd('create_note', {
    folderId: null,
    doc: { v: 1, content: [{ id: `sv${stamp}`, type: 'paragraph', content: [{ text: `${MARK2} 原文` }] }] },
  });
  await wp.click(`[data-testid="note-row-${made.id}"]`);
  await wp.waitForSelector('.nb-block .nb-content', { timeout: 15000 });
  await wp.waitForTimeout(600);

  const cell = () => wp.evaluate(() => {
    const el = document.querySelector('[data-testid="save-state"]');
    if (!el) return { missing: true };
    const g = el.querySelector('svg');
    const tok = (n) => {
      const p = document.createElement('span');
      p.style.color = `var(${n})`;
      document.body.appendChild(p);
      const v = getComputedStyle(p).color;
      p.remove();
      return v;
    };
    return {
      kind: el.getAttribute('data-save-state'),
      text: el.textContent.replace(/\s+/g, '').trim(),
      icon: g?.getAttribute('data-icon') ?? '',
      spin: g ? getComputedStyle(g).animationName : '',
      anims: g ? g.getAnimations().length : -1,
      color: getComputedStyle(el).color,
      t: { ok: tok('--ok'), danger: tok('--danger'), body: tok('--body'), mute: tok('--mute') },
    };
  });

  // 「有改动没存」这一格天然只活到那一发出门之前（自动保存 1.2s，而六个字的敲入本身就要几百毫秒），
  // 所以"敲完再读一次"是会读空的 —— 上一版就是这么读成 `saving` 的。
  // 改成**在页面里按 50ms 采样整段序列**，再按状态取第一帧：既证明这一格真的渲染过，
  // 也顺手钉住"dirty 在 saving 之前"这个次序（次序错了就是"用户还没打完就说在保存"）。
  await wp.evaluate(() => {
    const tok = (n) => {
      const p = document.createElement('span');
      p.style.color = `var(${n})`;
      document.body.appendChild(p);
      const v = getComputedStyle(p).color;
      p.remove();
      return v;
    };
    window.__tok = { ok: tok('--ok'), danger: tok('--danger'), body: tok('--body'), mute: tok('--mute') };
    window.__seq = [];
    const tick = () => {
      const el = document.querySelector('[data-testid="save-state"]');
      if (!el) {
        window.__seq.push(null);
        return;
      }
      const g = el.querySelector('svg');
      window.__seq.push({
        kind: el.getAttribute('data-save-state'),
        icon: g ? g.getAttribute('data-icon') : '',
        text: (el.textContent || '').replace(/\s+/g, ''),
        color: getComputedStyle(el).color,
        spin: g ? getComputedStyle(g).animationName : '',
        anims: g ? g.getAnimations().length : -1,
      });
    };
    window.__seqTimer = setInterval(tick, 50);
    tick();
  });
  await wp.dblclick('.nb-block .nb-content', { position: { x: 14, y: 8 } });
  await wp.keyboard.type(' 打进去的字');
  await wp.waitForTimeout(1700);
  const seq = await wp.evaluate(() => {
    clearInterval(window.__seqTimer);
    return window.__seq;
  });
  const dirty = seq.find((s) => s && s.kind === 'dirty') ?? { missing: 'dirty' };
  const saving = seq.find((s) => s && s.kind === 'saving') ?? { missing: 'saving' };
  const order = seq.findIndex((s) => s && s.kind === 'dirty');
  const orderSaving = seq.findIndex((s) => s && s.kind === 'saving');
  const tok = await wp.evaluate(() => window.__tok);

  check('㉗ 样本量：这一段采样真的抓到了两格各自的**第一帧**（否则下面全是空判据）',
    dirty.missing === undefined && saving.missing === undefined, JSON.stringify({ kinds: [...new Set(seq.filter(Boolean).map((s) => s.kind))] }));
  check('㉗ 打了字、那一发还没出门：这一格是「还有改动没存」+ 虚线圆，而且它排在"正在保存"**之前**（§4.1 第二行）',
    dirty.kind === 'dirty' && dirty.icon === 'save-dirty' && dirty.text.includes('还有改动没存')
      && dirty.color === tok.body && dirty.spin === 'none' && order >= 0 && order < orderSaving,
    JSON.stringify({ dirty, order, orderSaving }));
  check('㉗ 那一发在飞（route 按住）：这一格是「正在保存」+ 环形指示，而且**真的在转**（getAnimations 抓到动画对象）',
    saving.kind === 'saving' && saving.icon === 'save-saving' && saving.text.includes('正在保存')
      && saving.spin !== 'none' && saving.anims > 0,
    JSON.stringify(saving));

  mode = 'fail';
  if (release) release();
  await wp.waitForTimeout(700);
  const failed = await cell();
  check('㉗ 那一发被服务端拒掉：屏幕上必须是**那句具体原因**，不是"还有改动没存"（缺口 G79）',
    failed.kind === 'error' && failed.icon === 'save-failed'
      && failed.text.includes('服务器空间不足') && !failed.text.includes('还有改动没存') && failed.color === failed.t.danger,
    JSON.stringify(failed));

  mode = 'pass';
  await wp.keyboard.type(' 再打一次');
  await wp.waitForTimeout(2600); // 等这一发真的走完（debounce + 往返）
  const saved = await cell();
  const stored = await cmd('get_note', { id: made.id });
  const landed = JSON.stringify(stored).includes('再打一次');
  check('㉗ 这一发成功了：「已存在本机」+ 实心勾 + --ok（§4.1 第三行）',
    saved.kind === 'saved' && saved.icon === 'save-saved' && saved.text.includes('已存在本机') && saved.color === saved.t.ok,
    JSON.stringify(saved));
  check('㉗ 界面说"已存在本机"的时候，**库里真的有这些字**（文案与落库对得上，不是自我声明）',
    landed === true, JSON.stringify({ landed }));
  check('㉗ 静止那三格绝不像在忙：dirty / error / saved 三格里 animation-name 都是 none（§2.3 同一条规矩）',
    [dirty, failed, saved].every((s) => s.spin === 'none'), JSON.stringify([dirty.spin, failed.spin, saved.spin]));
  check('㉗ 四格两两不同（同一条基形只换徽标，但**状态必须互相分得开**）',
    new Set([dirty, saving, failed, saved].map((s) => `${s.icon}|${s.text}|${s.color}`)).size === 4,
    JSON.stringify([dirty, saving, failed, saved].map((s) => [s.icon, s.text, s.color])));

  notes.push(`     保存四格实测：${[dirty, saving, failed, saved].map((s) => `${s.icon}/${s.text}`).join(' → ')}`);
  await wp.screenshot({ path: `${OUT}/41-save-states-1440.png` });
  await wctx.close();
  await cmd('purge_note', { id: made.id });
}

/**
 * ㉚ §4.2 第四格「整机只读（库过新）」（缺口 G85）。
 *
 * 这一腿以前拦的是 `list_notes` 回 `db_too_new` —— 那是我以为的生产者，实际不存在：
 * 核心在库过新时让 `Store::open` 直接失败，整条命令面根本没起来（真生产者现在只有
 * 首帧那次 `stats`，它带 `libraryReadOnly`；读侧留着、写侧由 `Store::write_tx` 那道闸门拒。
 * 所以这一腿改造**新契约**那一格：只拦 `stats`，其余全部走真桥。
 *
 * 量的四件事：横幅在不在（几何，不是 DOM 存在）、说的是不是下一步、它是不是常驻
 * （不是 4.5 秒就消失的 toast）、以及"只读"这一位有没有真把写关在门外 —— 最后这条
 * 打在**真核心**上：整段之后回读那一篇，rev 与正文必须一个字没变。
 */
{
  const MARK = '只读夹具';
  await purgeByTitle(MARK);
  const made = await cmd('create_note', {
    folderId: null,
    doc: { v: 1, content: [{ id: `ro${stamp}`, type: 'paragraph', content: [{ text: `${MARK} 只读前原文` }] }] },
  });
  const revBefore = (await cmd('get_note', { id: made.id })).rev;

  const rctx = await browser.newContext({ viewport: { width: 1440, height: 900 } });
  const rp = await rctx.newPage();
  let injected = 0;
  let writes = 0;
  await rp.route('**/cmd/stats', (route) => {
    injected += 1;
    return route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ notes: 1, notesInTrash: 0, folders: 1, attachments: 0, ftsEntries: 1, dbBytes: 4096, searchGeneration: 1, inflightOps: 0, libraryReadOnly: true }),
    });
  });
  await rp.route(/\/cmd\/(edit_note|create_note|delete_note|set_note_pinned|purge_note|restore_note)/, (route) => {
    writes += 1;
    return route.continue();
  });
  await rp.goto(URL_BASE, { waitUntil: 'networkidle' });
  await rp.waitForTimeout(1500);

  const banner = await rp.evaluate(() => {
    const el = document.querySelector('[data-testid="banner-db"]');
    if (!el) return { missing: true };
    const r = el.getBoundingClientRect();
    const hit = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
    const inToastHost = Boolean(el.closest('.toast-host'));
    const cs = getComputedStyle(el);
    return {
      text: (el.textContent ?? '').replace(/\s+/g, ' ').trim(),
      h: Math.round(r.height),
      inViewport: r.top >= 0 && r.bottom <= window.innerHeight,
      hitSelf: Boolean(hit && el.contains(hit)),
      inToastHost,
      bg: cs.backgroundColor,
      // 横幅必须排在正文之前：它说的是"这台设备现在只能看"，藏在列表下面等于没说。
      aboveContent: r.top <= (document.querySelector('.app-body')?.getBoundingClientRect().top ?? 1e9) + 1,
    };
  });

  check('㉚ 仪器自检：首帧那次 `stats` 真的被我拦下了（没拦到就是这一格根本没造出来）',
    injected >= 1, JSON.stringify({ injected }));
  check('㉚ 库过新时有一条**全局横幅**：看得见、中心命中自己、排在正文之前，而且不是 toast',
    banner.missing !== true && banner.h > 0 && banner.inViewport === true && banner.hitSelf === true
      && banner.inToastHost === false && banner.aboveContent === true,
    JSON.stringify(banner));
  check('㉚ 那句话给的是**下一步**（§4.2「请升级以编辑」）并说清不会写坏，不是"出错了"三个字',
    /请升级以编辑/.test(banner.text) && /不会写/.test(banner.text), JSON.stringify({ text: banner.text }));
  check('㉚ 「只读」不等于"没了"：列表与正文都还读得到（真桥的真数据，这一格不许是空屏）',
    await rp.locator(`[data-testid="note-row-${made.id}"]`).count() === 1,
    JSON.stringify({ noteId: made.id }));

  await rp.click(`[data-testid="note-row-${made.id}"]`);
  await rp.waitForSelector('.nb-block .nb-content', { timeout: 15000 });
  const editable = await rp.evaluate(() => {
    const el = document.querySelector('.nb-block .nb-content');
    return {
      contenteditable: el?.getAttribute('contenteditable') ?? null,
      ariaReadonly: el?.getAttribute('aria-readonly') ?? null,
      note: (document.querySelector('.editor-note')?.textContent ?? '').replace(/\s+/g, ' ').trim(),
    };
  });
  check('㉚ 编辑区真的关掉了写：contenteditable=false 且 aria-readonly=true（§4.2 那一行不是只有横幅一句话）',
    editable.contenteditable === 'false' && editable.ariaReadonly === 'true', JSON.stringify(editable));
  check('㉚ 编辑区那句话点明的是"这一版只能看"，不是把整机只读误说成"这条在回收站"',
    /只能查看/.test(editable.note) && !/最近删除/.test(editable.note), JSON.stringify(editable));

  // 点击与打字都要容错：这一格的正常结果就是"点不动、打不进"，仪器不许因此把整轮带走。
  await rp.click('.nb-block .nb-content').catch(() => {});
  await rp.keyboard.type('这一笔不许落').catch(() => {});
  await rp.waitForTimeout(1800);

  await rp.waitForTimeout(5200);
  const stillThere = await rp.evaluate(() => {
    const el = document.querySelector('[data-testid="banner-db"]');
    // 别在这里截断文本：判据要的那句"请升级以编辑"在句子后半段，截 24 个字符会把它切掉
    // （上一版就是这么红的 —— 横幅明明还在，红的是仪器）。
    return { exists: Boolean(el), text: (el?.textContent ?? '').replace(/\s+/g, ' ').trim() };
  });
  check('㉚ 5.2 秒之后横幅仍在（这一格不许借用 4.5 秒就消失的 toast 通道）',
    stillThere.exists === true && /请升级以编辑/.test(stillThere.text), JSON.stringify(stillThere));

  const after = await cmd('get_note', { id: made.id });
  check('㉚ 打了字也不许降级写：写命令 0 发，且**真核心**回读的那一篇 rev 与正文都没变',
    writes === 0 && after.rev === revBefore && !JSON.stringify(after.doc).includes('这一笔不许落'),
    JSON.stringify({ writes, revBefore, revAfter: after.rev }));

  notes.push(`     库过新横幅实测：拦下 stats ${injected} 次；横幅=${JSON.stringify(stillThere.text)}；写命令 ${writes} 次；rev ${revBefore}→${after.rev}`);
  await rp.screenshot({ path: `${OUT}/42-db-too-new-1440.png` });
  await rctx.close();
  await cmd('purge_note', { id: made.id });
}

/**
 * ㉛ §2.3 第五格的第三句「需要重新填写口令」（G86）与 §4.7 那一档的显著告警（G88）。
 *
 * 这一腿拦的是 `account`（不是同步事件）：第三句的判据是"配置里挂着引用、这一轮拿不到"，
 * 而那是 `account` 的载荷直接说的事 —— 核心在缺凭据时发的 `badge: Offline` +
 * `sync.needsCredentials` 把这件事报成了"离线"（§4.3 里那句说的是"改动会先存在本机"）。
 * 量的都是渲染后的几何与计算样式，不是 DOM 存在。
 */
{
  const c31 = await browser.newContext({ viewport: { width: 1440, height: 900 } });
  const p31 = await c31.newPage();
  let syncNowSent = 0;
  await p31.route('**/cmd/account', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({
        id: 'acct-31',
        baseUrl: 'https://dav.invalid/dav',
        username: 'u',
        enabled: true,
        hasCredential: true,
        credentialLive: false,
        credentialPersistent: false,
      }),
    }),
  );
  await p31.route('**/cmd/sync_now', (route) => {
    syncNowSent += 1;
    return route.continue();
  });
  await p31.goto(URL_BASE, { waitUntil: 'networkidle' });
  await p31.waitForSelector('[data-testid="sync-badge"]', { timeout: 15000 });
  await p31.waitForTimeout(800);

  const cell = await p31.evaluate(() => {
    const btn = document.querySelector('[data-testid="sync-badge"]');
    const glyph = btn?.querySelector('svg');
    const bar = document.querySelector('[data-testid="syncbar"]');
    const desc = document.querySelector('[data-testid="sync-detail"]');
    const anims = glyph ? glyph.getAnimations().length : -1;
    const r = btn?.getBoundingClientRect();
    const hit = r ? document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2) : null;
    return {
      text: (btn?.textContent ?? '').replace(/\s+/g, ' ').trim(),
      icon: glyph?.getAttribute('data-icon') ?? null,
      spin: glyph ? getComputedStyle(glyph).animationName : null,
      anims,
      h: Math.round(r?.height ?? 0),
      hitSelf: Boolean(hit && btn?.contains(hit)),
      badgeAttr: bar?.getAttribute('data-badge') ?? null,
      desc: desc ? (desc.textContent ?? '').replace(/\s+/g, ' ').trim() : null,
      descH: desc ? Math.round(desc.getBoundingClientRect().height) : 0,
    };
  });
  check('㉛ 缺口令那一格说的是第三句，不是"离线"也不是"未配置同步"（§2.3 三句分得开）',
    cell.text.includes('需要重新填写口令') && !cell.text.includes('离线') && !cell.text.includes('未配置'),
    JSON.stringify(cell));
  check('㉛ 那一格用的是第五格的字形（云 + 一道横杠），而且**绝对静止**：animation-name=none、getAnimations 抓到 0 个',
    cell.icon === 'sync-idle' && cell.spin === 'none' && cell.anims === 0 && cell.badgeAttr === 'idle',
    JSON.stringify(cell));
  check('㉛ 原因那一句必须**看得见**（屏幕上那一行有高度），不许只挂在 title / aria-live 上',
    cell.desc !== null && cell.descH > 0 && /重填|重新填/.test(cell.desc),
    JSON.stringify({ desc: cell.desc, descH: cell.descH }));
  check('㉛ 那颗徽标本身是个点得着的目标（≥44 高、中心命中自己）',
    cell.h >= 44 && cell.hitSelf === true, JSON.stringify({ h: cell.h, hitSelf: cell.hitSelf }));

  // 第三句的出口：点它要走到"重填口令"那一格，而不是发一轮注定 407 的同步。
  await p31.click('[data-testid="sync-badge"]');
  await p31.waitForTimeout(700);
  const afterClick = await p31.evaluate(() => ({
    onSettings: document.querySelector('[data-testid="account-password"]') !== null,
    pwdPlaceholder: document.querySelector('[data-testid="account-password"]')?.getAttribute('placeholder') ?? '',
  }));
  check('㉛ 点那一格真的把人带去设置里重填（不是原地读标语）',
    afterClick.onSettings === true, JSON.stringify(afterClick));
  check('㉛ 这一路**一发 sync_now 都没出门**：注定 407 的一轮不该把"重填口令"翻成"同步失败"',
    syncNowSent === 0, JSON.stringify({ syncNowSent }));

  // §4.7：最危险那一档的显著告警。要真把它选出来，量渲染出来的那条，而不是源文件里的类名。
  const pickTls = async (label) => {
    // ⚠ 这里不许按 Escape "清理残留面板"：在这一页 Escape 的语义是**离开设置**（上一刀就是这么红的），
    // 于是按钮 itself 都不在屏幕上了。开面板有两条路：点它；点不动就聚焦后按 Enter（键盘等价路径，§5）。
    const panelUp = () =>
      p31
        .waitForSelector('[data-testid="app-select-panel"]', { state: 'visible', timeout: 2500 })
        .then(() => true)
        .catch(() => false);
    await p31.click('[data-testid="account-tls"]').catch(() => {});
    if (!(await panelUp())) {
      await p31.locator('[data-testid="account-tls"]').focus().catch(() => {});
      await p31.keyboard.press('Enter').catch(() => {});
      if (!(await panelUp())) return false;
    }
    const opt = p31.locator('[role="option"]').filter({ hasText: label }).first();
    if ((await opt.count()) === 0) {
      await p31.locator('[data-testid="account-tls"]').focus().catch(() => {});
      return false;
    }
    await opt.click().catch(() => {});
    await p31.waitForTimeout(500);
    return true;
  };
  check('㉛ 仪器自检：TLS 那一档真的选得出来（选不出来下面几条全是空判据）',
    (await pickTls('不校验')) === true, JSON.stringify({ picked: '不校验' }));
  const warn = await p31.evaluate(() => {
    const el = document.querySelector('[data-testid="warn-cert-skip"]');
    if (!el) return { missing: true };
    // 这条在设置那一栏的下方：先把它滚进可视范围，再量"看得见"。
    // 在自身会滚的栏里量"整块在视口内"量的其实是滚动位置，不是这条告警画没画（仪器自己会红错方向）。
    el.scrollIntoView({ block: 'center' });
    const r = el.getBoundingClientRect();
    const cs = getComputedStyle(el);
    const hit = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
    return {
      text: (el.textContent ?? '').replace(/\s+/g, ' ').trim(),
      h: Math.round(r.height),
      inViewport: r.top >= 0 && r.bottom <= window.innerHeight,
      hitSelf: Boolean(hit && el.contains(hit)),
      role: el.getAttribute('role'),
      color: cs.color,
      bg: cs.backgroundColor,
      // 同屏那句明文 HTTP 的告警不该把这一档的词也念一遍
      httpCopyShown: (document.body.textContent ?? '').includes('未加密传输：只有内网才建议这样设置'),
    };
  });
  check('㉛ 选了"不校验"那一档，屏幕上立刻出现一条**显著告警**（role=alert、有高度、滚到中间时中心命中自己）',
    warn.missing !== true && warn.role === 'alert' && warn.h > 0 && warn.inViewport === true && warn.hitSelf === true,
    JSON.stringify(warn));
  check('㉛ 那句话点明的是"链路上别人能读到、还能伪装成你的服务器"，不是一句温和的建议',
    /能读到/.test(warn.text) && /伪装/.test(warn.text) && /内网/.test(warn.text),
    JSON.stringify({ text: warn.text }));
  check('㉛ 这一档的词与明文 HTTP 那一档**不是同一句**（共用一句 = 最危险的那档听起来最轻）',
    warn.httpCopyShown === false && warn.text.includes('跳过证书校验'),
    JSON.stringify({ httpCopyShown: warn.httpCopyShown }));
  check('㉛ 告警文字的对比度按渲染出来的那对颜色量要读得清（§5 ≥4.5:1）',
    warn.missing !== true && contrastRatio(warn.color, warn.bg) >= 4.5,
    JSON.stringify({ color: warn.color, bg: warn.bg, ratio: Number(contrastRatio(warn.color ?? '', warn.bg ?? '').toFixed(2)) }));

  // 正对照：能力不存在就不画那条 —— 换回严格档之后屏幕上不该还留着警告。
  const backToStrict = await pickTls('严格');
  const gone = (await p31.locator('[data-testid="warn-cert-skip"]').count()) === 0;
  check('㉛ 换回"严格"那一档之后这条告警就不画（不是灰着留着）',
    backToStrict === true && gone, JSON.stringify({ backToStrict, gone }));

  notes.push(`     第五格第三句实测：${cell.text}；字形 ${cell.icon}/${cell.spin}/anims ${cell.anims}；sync_now 出门 ${syncNowSent} 次`);
  await p31.screenshot({ path: `${OUT}/43-fifth-cell-password-1440.png` });
  await c31.close();
}

/**
 * ㉜ §4.3「服务器丢了很多条记录 → 人工确认」（G87）的那一条横幅与那颗按钮。
 *
 * 引擎侧的"整轮停"由 `notera-sync/tests/engine.rs` 那两条判据守着（真停、真什么都不做）；
 * 这一腿只管界面这两件事：**说得出少了多少**、**给得出一个动作**。
 * 拦的是 `sync_status`（那两个数的唯一来源是核心，界面上不许自己算），
 * 点下去之后断言的是**请求到底发没发**（`accept_divergence` 出门 1 次）。
 */
{
  const c32 = await browser.newContext({ viewport: { width: 1440, height: 900 } });
  const p32 = await c32.newPage();
  let held = true;
  let accepted = 0;
  await p32.route('**/cmd/sync_status', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({
        phase: 'idle',
        badge: 'offline',
        lastSuccessAt: null,
        pendingOps: 0,
        openConflicts: 0,
        messageKey: 'sync.divergenceHeld',
        retryable: false,
        divergenceHeld: held ? { cachedRecords: 300, receivedRecords: 10 } : null,
      }),
    }),
  );
  await p32.route('**/cmd/accept_divergence', (route) => {
    accepted += 1;
    held = false;
    return route.continue();
  });
  await p32.goto(URL_BASE, { waitUntil: 'networkidle' });
  await p32.waitForTimeout(1600);

  const bar = await p32.evaluate(() => {
    const el = document.querySelector('[data-testid="banner-divergence"]');
    if (!el) return { missing: true };
    const r = el.getBoundingClientRect();
    const hit = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
    const btn = el.querySelector('[data-testid="divergence-accept"]');
    const br = btn?.getBoundingClientRect();
    const bhit = br ? document.elementFromPoint(br.left + br.width / 2, br.top + br.height / 2) : null;
    return {
      text: (el.textContent ?? '').replace(/\s+/g, ' ').trim(),
      role: el.getAttribute('role'),
      h: Math.round(r.height),
      hitSelf: Boolean(hit && el.contains(hit)),
      aboveContent: r.top <= (document.querySelector('.app-body')?.getBoundingClientRect().top ?? 1e9) + 1,
      btnText: (btn?.textContent ?? '').replace(/\s+/g, ' ').trim(),
      btnH: Math.round(br?.height ?? 0),
      btnHit: Boolean(bhit && btn?.contains(bhit)),
    };
  });
  check('㉜ 停下来的时候有一条**全局横幅**：看得见、中心命中自己、排在正文之前、role=alert',
    bar.missing !== true && bar.role === 'alert' && bar.h > 0 && bar.hitSelf === true && bar.aboveContent === true,
    JSON.stringify(bar));
  check('㉜ 那句话把**少了多少**说出来（两个数都得上屏，"少了一大截"不算交代）',
    /300/.test(bar.text) && /10/.test(bar.text) && !/操作失败/.test(bar.text),
    JSON.stringify({ text: bar.text }));
  check('㉜ 有一条能点的动作（≥44 高、中心命中它自己），而且说的就是"确认这一版"',
    bar.btnH >= 44 && bar.btnHit === true && /确认这一版/.test(bar.btnText),
    JSON.stringify({ btnH: bar.btnH, btnHit: bar.btnHit, btnText: bar.btnText }));

  await p32.click('[data-testid="divergence-accept"]');
  await p32.waitForTimeout(1200);
  const after = await p32.evaluate(() => ({
    still: document.querySelector('[data-testid="banner-divergence"]') !== null,
  }));
  check('㉜ 点下去真的发了那一发（调用边：accept_divergence 出门 1 次）',
    accepted === 1, JSON.stringify({ accepted }));
  check('㉜ 核心不再报停之后，这条横幅要真的收回去（不许留在屏幕上假装还要确认）',
    after.still === false, JSON.stringify(after));

  notes.push(`     G87 横幅实测：${JSON.stringify(bar.text).slice(0, 60)}…；accept_divergence ${accepted} 次`);
  await p32.screenshot({ path: `${OUT}/44-divergence-held-1440.png` });
  await c32.close();
}

/**
 * ㉝ §3.4 的工具条溢出：「横向放不下时右缘 28px 渐隐，最右的「更多 ›」收纳溢出项」。
 *
 * 这一腿量的不是"有没有那颗按钮"，而是三件容易被互相冒充的事：
 * ① **收纳**：条上不许留任何一颗在视野外（滚动宽度 == 可视宽度），被放不下的必须换成面板里的行；
 * ② **一颗都不丢**：条上的格子 + 面板里的格子 = 全部格子（收走一半、忘掉一半是同一族事故）；
 * ③ 浮层**不参与布局**（§4.9）：打开面板不许把工具条或第一行顶下去。
 * 宽档反过来：放得下就不许画那颗「更多」（§2.5「能力不存在就根本不渲染那颗控件」）。
 */
{
  const c33 = await browser.newContext({ viewport: { width: 390, height: 844 } });
  const p33 = await c33.newPage();
  await p33.goto(URL_BASE, { waitUntil: 'networkidle' });
  await p33.waitForSelector('[data-testid="note-row"]', { timeout: 15000 }).catch(() => {});
  await p33.click('[data-testid="mobile-new"]').catch(() => {});
  await p33.waitForSelector('.tb', { timeout: 15000 });
  await p33.waitForTimeout(700);

  const narrow = await p33.evaluate(() => {
    const bar = document.querySelector('.tb');
    const more = document.querySelector('[data-testid="tb-more"]');
    const onBar = Array.from(document.querySelectorAll('.tb [data-tb-key]')).length;
    if (!bar) return { missing: true };
    const fade = getComputedStyle(document.querySelector('.tb-wrap') ?? bar, '::after').backgroundImage;
    const r = more?.getBoundingClientRect();
    const hit = r ? document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2) : null;
    return {
      total: document.querySelectorAll('[data-tb-key]').length,
      onBar,
      moreThere: Boolean(more),
      moreH: Math.round(r?.height ?? 0),
      moreHit: Boolean(hit && more?.contains(hit)),
      moreText: (more?.textContent ?? '').replace(/\s+/g, ' ').trim(),
      moreRight: Math.round(bar.getBoundingClientRect().right - (r?.right ?? 0)),
      dataMoreRight: bar.getAttribute('data-more-right'),
      fadeIsGradient: /gradient/.test(fade),
      scrollWidth: Math.round(bar.scrollWidth),
      clientWidth: Math.round(bar.clientWidth),
    };
  });
  check('㉝ 仪器自检：390 那一档真的放不下（放得下的话下面全是空判据）',
    narrow.missing !== true && narrow.moreThere === true && narrow.onBar < 15,
    JSON.stringify(narrow));
  check('㉝ 「更多 ›」画在最右、看得见点得着（≥44 高、中心命中自己）',
    narrow.moreHit === true && narrow.moreH >= 44 && /更多/.test(narrow.moreText),
    JSON.stringify(narrow));
  check('㉝ 收纳是真的收纳：条上不留任何视野外的格子（滚动宽 == 可视宽）',
    narrow.scrollWidth <= narrow.clientWidth + 1,
    JSON.stringify({ scrollWidth: narrow.scrollWidth, clientWidth: narrow.clientWidth }));
  check('㉝ 渐隐那句话还在（§3.4 两件事都要：一道 28px 提示 + 一颗收口的按钮）',
    narrow.dataMoreRight === 'true' && narrow.fadeIsGradient === true,
    JSON.stringify({ dataMoreRight: narrow.dataMoreRight, fadeIsGradient: narrow.fadeIsGradient }));

  const before = await p33.evaluate(() => {
    const bar = document.querySelector('.tb');
    return { top: Math.round(bar?.getBoundingClientRect().top ?? 0), docTop: Math.round(document.querySelector('.editor-doc')?.getBoundingClientRect().top ?? 0) };
  });
  await p33.click('[data-testid="tb-more"]');
  await p33.waitForTimeout(600);
  const panel = await p33.evaluate(() => {
    const pop = document.querySelector('[data-testid="tb-more-menu"]');
    const bar = document.querySelector('.tb');
    const onBar = Array.from(document.querySelectorAll('.tb [data-tb-key]')).length;
    const rows = Array.from(document.querySelectorAll('[data-testid="tb-more-menu"] [data-tb-key]'));
    const first = rows[0]?.querySelector('button')?.getBoundingClientRect();
    const hit = first ? document.elementFromPoint(first.left + first.width / 2, first.top + first.height / 2) : null;
    return {
      exists: Boolean(pop),
      role: pop?.getAttribute('role') ?? null,
      rows: rows.length,
      onBar,
      total: onBar + rows.length,
      text: (pop?.textContent ?? '').replace(/\s+/g, ' ').trim(),
      rowH: Math.round(first?.height ?? 0),
      rowHit: Boolean(hit && rows[0]?.querySelector('button')?.contains(hit)),
      inBody: pop?.parentElement?.tagName === 'BODY',
      barTop: Math.round(bar?.getBoundingClientRect().top ?? 0),
      docTop: Math.round(document.querySelector('.editor-doc')?.getBoundingClientRect().top ?? 0),
      insideBar: Boolean(pop && bar?.contains(pop)),
    };
  });
  const narrowUnion = panel.onBar + panel.rows;
  check('㉝ 打开面板：条上 + 面板里都有格子，且条上那一半没被重复画进面板',
    panel.exists === true && panel.role === 'menu' && panel.rows > 0 && panel.onBar > 0,
    JSON.stringify({ onBar: panel.onBar, rows: panel.rows, union: narrowUnion }));
  check('㉝ 面板里的行是整行、读得出名字、点得着（≥44 高，中心命中自己）',
    panel.rowHit === true && panel.rowH >= 44 && /[一-龥]/.test(panel.text),
    JSON.stringify({ rowH: panel.rowH, rowHit: panel.rowHit, text: panel.text.slice(0, 80) }));
  check('㉝ 浮层不参与布局：面板打开时工具条与正文第一行的位置一个像素都不许动',
    panel.barTop === before.top && panel.docTop === before.docTop,
    JSON.stringify({ before, after: { barTop: panel.barTop, docTop: panel.docTop } }));
  check('㉝ 面板 Teleport 出那个裁剪盒（在 overflow-x:auto 里会被整个裁掉 —— G29 同一族）',
    panel.inBody === true && panel.insideBar === false,
    JSON.stringify({ inBody: panel.inBody, insideBar: panel.insideBar }));

  await p33.click('[data-testid="tb-more"]');
  await p33.waitForTimeout(400);
  const closed = await p33.evaluate(() => ({
    open: document.querySelector('[data-testid="tb-more-menu"]') !== null,
    expanded: document.querySelector('[data-testid="tb-more"]')?.getAttribute('aria-expanded') ?? null,
  }));
  check('㉝ 再点一下收得回去，aria-expanded 跟着落回 false（不许留一层看不见的浮层）',
    closed.open === false && closed.expanded === 'false', JSON.stringify(closed));
  await p33.screenshot({ path: `${OUT}/45-toolbar-overflow-390.png` });
  await p33.close();
  await c33.close();

  // 宽档反过来：放得下就不画那颗「更多」。
  const w33 = await browser.newContext({ viewport: { width: 1440, height: 900 } });
  const pw = await w33.newPage();
  await pw.goto(URL_BASE, { waitUntil: 'networkidle' });
  await pw.waitForSelector('.tb', { timeout: 15000 });
  await pw.waitForTimeout(700);
  const wide = await pw.evaluate(() => {
    const bar = document.querySelector('.tb');
    return {
      more: document.querySelector('[data-testid="tb-more"]') !== null,
      onBar: document.querySelectorAll('.tb [data-tb-key]').length,
      fade: bar?.getAttribute('data-more-right') ?? null,
      overflowing: Math.round(bar ? bar.scrollWidth - bar.clientWidth : 0),
    };
  });
  check('㉝ 正对照（宽档）：放得下就不画「更多」，也不画那道渐隐（能力不存在就别摆控件）',
    wide.more === false && wide.fade === null && wide.onBar > 10,
    JSON.stringify(wide));
  // 收纳不许多也不少：窄档"条上 + 面板里"必须等于宽档"全在条上"。
  // 这条不用 15 那种魔数 —— 两档各量一次，表改了它跟着动。
  check('㉝ 收纳不丢格子：窄档（条上 + 面板）与宽档（全在条上）是同一批格子',
    wide.onBar === narrowUnion,
    JSON.stringify({ narrowUnion, wideOnBar: wide.onBar }));
  await w33.close();
}

/**
 * ㉞ §5 最后一行没有机器判据的那一条：「焦点：焦点可见，逻辑顺序合理」。
 *
 * 为什么单独一腿而不是塞进第 ⑯ 腿那种"扫 CSS 里有没有 :focus-visible"：
 * 声明在 CSS 里 ≠ 画得出来 —— 一个 `outline: 0` 的后代规则、一处 `overflow:hidden` 的祖先、
 * 或者焦点停在一个 0×0 的包裹层上，都会让那句话变成假话。只有真的按 Tab 一站一站走，
 * 读 `document.activeElement` 的**计算样式与盒子**，量的才是界面。
 *
 * 三件事一起判：① 每一站的焦点环真的画上（solid、≥2px、颜色压底色 ≥3:1 —— 非文字对比度那条）；
 * ② 停靠顺序符合阅读顺序（侧栏 → 列表 → 编辑器 → 底栏，跨栏不许回跳）；
 * ③ 每一站都在视口里且看得见名字（焦点跑到视口外等于焦点不可见）。
 * 再补一档 `prefers-reduced-motion: reduce`：时长归零不许把焦点环一起归掉。
 */
{
  const c34 = await browser.newContext({ viewport: { width: 1440, height: 900 } });
  const p34 = await c34.newPage();
  await p34.goto(URL_BASE, { waitUntil: 'networkidle' });
  await p34.waitForSelector('[data-testid="sidebar"]', { timeout: 15000 });
  await p34.waitForTimeout(800);

  const probeStop = () => p34.evaluate(() => {
    const el = document.activeElement;
    if (!el || el === document.body) return { tag: 'BODY' };
    // 这一站是不是**来过**：Tab 序的圈数没法先验知道（列表是滚到哪才挂哪一行，
    // 静态数一遍 DOM 会低估），所以用"元素自己带个记号"来判"走回原点了"。
    const seen = el instanceof HTMLElement && el.dataset.qTabSeen === '1';
    if (el instanceof HTMLElement) el.dataset.qTabSeen = '1';
    const cs = getComputedStyle(el);
    const r = el.getBoundingClientRect();
    const zone = el.closest('.pane--sidebar') ? 0
      : el.closest('.pane--list') ? 1
        : el.closest('.pane--editor') ? 2
          : el.closest('.dock') ? 3
            : 9;
    const tok = (name) => {
      const s = document.createElement('span');
      s.style.color = `var(${name})`;
      document.body.appendChild(s);
      const v = getComputedStyle(s).color;
      s.remove();
      return v;
    };
    const accent = tok('--accent');
    // §5 要的是"焦点看得见"，不是"必须是 outline"。正文那一格用的是画在格子里边的
    // 2px 内阴影条（`outline: none; box-shadow: inset 2px 0 0 var(--accent)`），
    // 那只手是刻意的：外描边会被版心的 overflow 裁一半，看着像没焦点。
    const outlined = cs.outlineStyle !== 'none' && Number.parseFloat(cs.outlineWidth) >= 2;
    const bar = /inset/.test(cs.boxShadow) && Number.parseFloat((cs.boxShadow.match(/(\d+(\.\d+)?)px/) ?? [])[1] ?? '0') >= 2;
    return {
      tag: el.tagName.toLowerCase(),
      seen,
      testid: el.getAttribute('data-testid'),
      name: (el.getAttribute('aria-label') || el.getAttribute('title') || (el.textContent ?? '').trim()).slice(0, 20),
      zone,
      ring: outlined ? 'outline' : bar ? 'inset-bar' : 'none',
      outline: cs.outlineStyle,
      outlineWidth: cs.outlineWidth,
      ringColor: outlined ? cs.outlineColor : accent,
      accent,
      canvas: tok('--canvas'),
      shadow: cs.boxShadow.slice(0, 60),
      w: Math.round(r.width),
      h: Math.round(r.height),
      inViewport: r.width > 0 && r.height > 0 && r.top >= 0 && r.bottom <= window.innerHeight,
    };
  });

  // ⚠ 起点必须是"文档的第一个焦点位"，不是"当前焦点"：应用打开时会把光标送进正文那一格，
  // 从那儿按 Tab 量到的第一站是第 3 栏，看着就像"顺序倒了"（第一版就红错在这）。
  // skip-link 就是那个第一位（它排在所有栏之前）；没有它才退回侧栏第一颗。
  const started = await p34.evaluate(() => {
    const el = document.querySelector('.skip-link') ?? document.querySelector('[data-testid="nav-all"]');
    if (!el) return false;
    el.focus();
    return document.activeElement === el;
  });
  check('㉞ 仪器自检：能从「文档第一个焦点位」起步（否则量到的是应用自动聚焦的形状）',
    started === true, JSON.stringify({ started }));
  // 走到哪儿停：① 进了编辑器那一栏就收（后面的栏不该由这一腿判），② 或者**走回了来过的元素**
  // （= 这一圈绕完还没到编辑器，那是顺序/栏内焦点位变了，由下面那条自检去红，不是这里静默降级）。
  // 以前这里写死 140 站，实测"走到编辑器"要 117 站 —— 上限迟早被开发库的行数撞穿（假红）。
  // 也不能改成"先数一遍 DOM 里的可聚焦元素"：实测数出来 90 < 117，因为列表是滚到哪才挂哪一行，
  // 静态数一遍会**低估**，比写死更糟。真上界是 Tab 序的周期，而这个只有走一遍才知道。
  // 只留一个纯防跑飞的大数（正常远到不了：走到编辑器就 break）。
  const stops = [];
  let wrapped = false;
  for (let i = 0; i < 600; i += 1) {
    await p34.keyboard.press('Tab');
    await p34.waitForTimeout(80);
    const stop = await probeStop();
    stops.push(stop);
    if (stop.zone === 2) break;
    if (stop.seen === true) { wrapped = true; break; }
  }
  const landed = stops.filter((s) => s.tag !== 'BODY' && s.zone !== 9);
  check('㉞ 仪器自检：Tab 真的从侧栏一路走到编辑器（走不到就是次数或栏内焦点位变了，这条不许静默降级）',
    new Set(landed.map((s) => s.zone)).size >= 3 && landed.some((s) => s.zone === 2),
    JSON.stringify({ landed: landed.length, wrapped, zones: [...new Set(landed.map((s) => s.zone))] }));
  // 样本量守卫：底下四条用的都是 `every`，空数组会让它们**全绿**。
  // 变异 I 实测正是这个形状：走不到编辑器时 landed=0，那四条一条都没红，只有这条红 ——
  // 所以"这一腿到底量到了几站"必须是一条独立的、看得见的判据，不能靠下游顺带发现。
  check('㉞ 样本量守卫：至少真的量到 20 站（`every` 在空数组上为真 —— 没走到就等于没检查）',
    landed.length >= 20, JSON.stringify({ landed: landed.length, walked: stops.length }));
  check('㉞ 每一站的焦点指示都真的画上：outline ≥2px，或正文那种 2px 内描条（"CSS 里写了"不等于屏幕上画了）',
    landed.every((s) => s.ring === 'outline' || s.ring === 'inset-bar'),
    JSON.stringify(landed.filter((s) => s.ring === 'none').slice(0, 4)));
  check('㉞ 焦点指示的颜色就是 --accent，压在底色上够 3:1（§5 非文字对比度 1.4.11）',
    landed.every((s) => s.ringColor === s.accent && contrastRatio(s.ringColor, s.canvas) >= 3),
    JSON.stringify(landed.slice(0, 3).map((s) => ({ ring: s.ringColor, bg: s.canvas, ratio: Number(contrastRatio(s.ringColor, s.canvas).toFixed(2)) }))));
  check('㉞ 停靠顺序符合阅读顺序：侧栏 → 列表 → 编辑器 → 底栏，不许回跳到上一栏',
    landed.every((s, i) => i === 0 || s.zone >= landed[i - 1].zone),
    JSON.stringify(landed.map((s) => s.zone)));
  check('㉞ 焦点不许停在视口外或 0 尺寸的东西上（那等于"焦点看不见"）',
    landed.every((s) => s.inViewport === true),
    JSON.stringify(landed.filter((s) => !s.inViewport).slice(0, 3)));
  check('㉞ 每一站都带着可读的名字（焦点环 + 无名控件 = 键盘用户听到的是"按钮"）',
    landed.every((s) => (s.name ?? '').trim().length > 0),
    JSON.stringify(landed.filter((s) => !(s.name ?? '').trim()).slice(0, 3)));

  await p34.screenshot({ path: `${OUT}/46-focus-ring-1440.png` });
  await p34.close();
  await c34.close();

  // 动效关掉那一档：时长归零 ≠ 焦点环归零。
  const rmCtx = await browser.newContext({ viewport: { width: 1440, height: 900 }, reducedMotion: 'reduce' });
  const rmPage = await rmCtx.newPage();
  await rmPage.goto(URL_BASE, { waitUntil: 'networkidle' });
  await rmPage.waitForSelector('[data-testid="sidebar"]', { timeout: 15000 });
  await rmPage.evaluate(() => document.activeElement?.blur());
  await rmPage.keyboard.press('Tab');
  await rmPage.waitForTimeout(300);
  const rmStop = await rmPage.evaluate(() => {
    const el = document.activeElement;
    const cs = getComputedStyle(el);
    const first = document.querySelector('.syncbar__glyph');
    return {
      motionless: first ? getComputedStyle(first).animationName : null,
      outline: cs.outlineStyle,
      width: cs.outlineWidth,
      shadow: cs.boxShadow.slice(0, 60),
    };
  });
  check('㉞ prefers-reduced-motion 下：该静止的静止了，焦点指示还在（时长归零不许顺手把可见性也归掉）',
    (rmStop.outline !== 'none' && Number.parseFloat(rmStop.width) >= 2) || /inset/.test(rmStop.shadow),
    JSON.stringify(rmStop));
  await rmCtx.close();
}

/**
 * ㉟ §2.4 那句「文字类（加粗/斜体）用 SVG 路径画，不用 B/I 字形」。
 *
 * 这一条为什么不能由 `iconSystem.spec.ts` 那条源码扫描来守：它的判据是"元素的全部文字内容是一个
 * **符号或标点**"，而**拉丁字母被注释明确放过了**，放过的理由写的是「§2.4 认的通用认知」——
 * 那是把 §2.4 读反了。原话是「用 SVG 路径画，不用 B/I 字形 …… 但语义沿用 B/I/U/S 的通用认知」：
 * 后半句说的是**画出来的形状要还像那几个字母**，不是许可继续打字母。
 * 这正是 G77 记过的那个形状：**判据按类别兜，别按枚举** —— 按 `\p{S}` 枚举，落在 `\p{L}` 里的
 * B/I/U/S/A/H 永远抓不到，而它的危害与 ☰ ▾ 完全相同（四端字体回退画出四种形状，
 * 且与旁边 1.75px 描边的图标不成套）。
 *
 * 判据打在渲染后的 DOM 上，形状是：**一个 `<button>` 的直接文本子节点恰是一个字母或一个数字，
 * 而它的可访问名来自 `aria-label`/`title`** ⇒ 那个字符不是读给人听的，是在当图标位。
 * 为什么限定"直接文本子节点"：字号那颗显示的当前档位（`l` / `xl`）包在自己的 span 里，是状态读数；
 * 块型菜单里的 `H1`/`H2`/`H3` 是文字标签且没有 `aria-label`。两种都不是图标位。
 */
{
  const c35 = await browser.newContext({ viewport: { width: 1440, height: 900 } });
  const p35 = await c35.newPage();
  await p35.goto(URL_BASE, { waitUntil: 'networkidle' });
  await p35.waitForSelector('[data-testid="sidebar"]', { timeout: 15000 });
  await p35.click('[data-testid="nav-all"]');
  await p35.waitForTimeout(500);
  await p35.click('[data-testid^="note-row-"]').catch(() => {});
  await p35.waitForTimeout(900);

  const scan = () => p35.evaluate(() => {
    const rows = [];
    for (const b of document.querySelectorAll('button')) {
      const r = b.getBoundingClientRect();
      if (r.width < 1 && r.height < 1) continue; // 没渲染出来的那颗不在这一条的范围内
      const own = Array.from(b.childNodes)
        .filter((n) => n.nodeType === 3)
        .map((n) => (n.textContent ?? '').trim())
        .join('');
      const svg = b.querySelector('svg');
      rows.push({
        key: b.closest('[data-tb-key]')?.getAttribute('data-tb-key') ?? null,
        inPanel: b.closest('[data-testid="tb-more-menu"]') !== null,
        name: b.getAttribute('aria-label') || b.getAttribute('title') || '',
        own,
        icon: svg?.getAttribute('data-icon') ?? null,
        paths: svg ? svg.querySelectorAll('path').length : 0,
        iconW: svg ? Math.round(svg.getBoundingClientRect().width) : 0,
        stroke: svg?.querySelector('g')?.getAttribute('stroke-width') ?? null,
        w: Math.round(r.width),
        h: Math.round(r.height),
      });
    }
    return rows;
  });

  const seen = await scan();
  await p35.click('[data-testid="nav-settings"]');
  await p35.waitForTimeout(600);
  const all = [...seen, ...(await scan())];
  await p35.click('[data-testid="nav-all"]');
  await p35.waitForTimeout(500);
  await p35.click('[data-testid^="note-row-"]').catch(() => {});
  await p35.waitForTimeout(900);

  check('㉟ 仪器自检：这一腿真的扫到了足够多颗按钮（扫到几颗就判几颗，等于没扫）',
    all.length >= 60, JSON.stringify({ buttons: all.length }));
  const offenders = all.filter((b) => /^[\p{L}\p{N}]$/u.test(b.own) && b.name.length > 0);
  check('㉟ 没有任何一颗按钮在拿"一个字母/一个数字"当图标位（可访问名来自 aria-label，可见内容却只有一个字符）',
    offenders.length === 0, JSON.stringify(offenders.slice(0, 5)));

  const MARK_CELLS = ['bold', 'italic', 'underline', 'strike', 'size', 'color'];
  const cells = MARK_CELLS.map((key) => ({
    key,
    cell: all.find((b) => b.key === key && !b.inPanel) ?? null,
  }));
  check('㉟ 工具条那六颗（四颗文字标记 + 字号 + 颜色）画的都是 mark-* 图标，不再是字母',
    cells.every((c) => c.cell !== null && (c.cell.icon ?? '').startsWith('mark-')),
    JSON.stringify(cells.map((c) => ({ key: c.key, icon: c.cell?.icon ?? null }))).slice(0, 400));
  check('㉟ 每颗都真的画得出东西：svg 里有 path、盒子不是 0×0（"有 svg"不等于"看得见"）',
    cells.every((c) => c.cell !== null && c.cell.paths >= 1 && c.cell.iconW >= 14),
    JSON.stringify(cells.map((c) => ({ key: c.key, paths: c.cell?.paths, iconW: c.cell?.iconW }))));
  check('㉟ 六颗都还有可读名字、触摸目标 ≥44、条上没有残留的字母文本',
    cells.every((c) => c.cell !== null && c.cell.name.length > 0 && c.cell.w >= 44 && c.cell.h >= 44 && c.cell.own === ''),
    JSON.stringify(cells.map((c) => ({ key: c.key, w: c.cell?.w, h: c.cell?.h, own: c.cell?.own }))));
  const strokes = new Set(cells.map((c) => c.cell?.stroke).filter(Boolean));
  check('㉟ §2.1 同屏同粗细：这一排图标只用一个描边值（混粗细是"看着不专业"最常见的来源）',
    strokes.size === 1, JSON.stringify([...strokes]));

  // 选区浮出来的那一条：六颗与工具条同一份表，形状不许换。
  await p35.evaluate(() => window.getSelection()?.removeAllRanges());
  const at = await p35.evaluate(() => {
    const el = document.querySelector('.nb-block .nb-content');
    if (!el) return null;
    const range = document.createRange();
    range.selectNodeContents(el);
    const tr = range.getBoundingClientRect();
    if (tr.width < 20) return null;
    return { x: Math.round(tr.left + 12), y: Math.round(tr.top + 8) };
  });
  if (at) {
    await p35.mouse.dblclick(at.x, at.y);
    await p35.waitForTimeout(700);
  }
  const sel = await p35.evaluate(() => Array.from(document.querySelectorAll('[data-testid="selection-bar"] button')).map((b) => {
    const svg = b.querySelector('svg');
    return {
      icon: svg?.getAttribute('data-icon') ?? null,
      paths: svg ? svg.querySelectorAll('path').length : 0,
      own: Array.from(b.childNodes).filter((n) => n.nodeType === 3).map((n) => (n.textContent ?? '').trim()).join(''),
      name: b.getAttribute('aria-label') ?? '',
    };
  }));
  check('㉟ 选区浮条那六颗也换成 SVG 了，且颗颗有名字（与工具条读同一份表）',
    sel.length >= 4 && sel.every((s) => (s.icon ?? '').startsWith('mark-') && s.paths >= 1 && s.own === '' && s.name !== ''),
    JSON.stringify({ count: sel.length, bad: sel.filter((s) => !(s.icon ?? '').startsWith('mark-') || s.own !== '') }));

  await p35.screenshot({ path: `${OUT}/47-mark-icons-1440.png` });
  await p35.close();
  await c35.close();

  // 手机宽：格子搬进「更多 ›」面板之后，形状必须是同一份（两处各写一遍迟早一份是字母一份是图）。
  const c35b = await browser.newContext({ viewport: { width: 390, height: 844 } });
  const p35b = await c35b.newPage();
  await p35b.goto(URL_BASE, { waitUntil: 'networkidle' });
  await p35b.waitForSelector('[data-testid^="note-row-"]', { timeout: 15000 });
  await p35b.click('[data-testid^="note-row-"]');
  await p35b.waitForSelector('.nb-block .nb-content', { timeout: 15000 });
  await p35b.waitForTimeout(900);
  await p35b.click('[data-testid="tb-more"]').catch(() => {});
  await p35b.waitForTimeout(500);
  const panel = await p35b.evaluate(() => Array.from(document.querySelectorAll('[data-testid="tb-more-menu"] [data-tb-key]')).map((wrap) => {
    const b = wrap.querySelector('button');
    const svg = b?.querySelector('svg') ?? null;
    return {
      key: wrap.getAttribute('data-tb-key'),
      icon: svg?.getAttribute('data-icon') ?? null,
      own: b ? Array.from(b.childNodes).filter((n) => n.nodeType === 3).map((n) => (n.textContent ?? '').trim()).join('') : null,
    };
  }));
  const panelMarks = panel.filter((r) => ['size', 'color', 'bold', 'italic', 'underline', 'strike'].includes(r.key ?? ''));
  check('㉟ 手机宽「更多 ›」面板里那几行仍是 SVG（面板不是另一套 markup）',
    panelMarks.length >= 2 && panelMarks.every((r) => (r.icon ?? '').startsWith('mark-')),
    JSON.stringify({ panelMarks, panel }));
  await p35b.close();
  await c35b.close();
}

/**
 * ㊱ §3.1 那三个断点与两条栏宽，量的是**边界上那一像素**渲染成什么形状。
 *
 * 为什么单独一腿：以前的各腿都停在"某一档下长什么样"（900 / 1100 / 1440 / 1800 / 390 / 520），
 * 没有一条量"档在哪儿分"。`layoutFor` 有实现、`--sidebar-w/--list-w` 在 tokens 里，
 * 但把 1180 改成 1200 全仓不会红 —— 而 §3.1 那句"≥1180 三栏 / 820–1179 两栏 / <820 单栏"
 * 是一整套布局承诺的地基。单测 `layoutBreakpoints.spec.ts` 钉的是数，这一腿钉的是**画出来的形状**
 * （数对了但 CSS 那侧的 `[data-layout]` 规则写反，只有这里抓得到）。
 */
{
  /** §3.1：边界那一像素归上一档。 */
  const CASES = [
    { w: 819, layout: 'one' },
    { w: 820, layout: 'two' },
    { w: 1179, layout: 'two' },
    { w: 1180, layout: 'three' },
  ];
  for (const c of CASES) {
    const ctx = await browser.newContext({ viewport: { width: c.w, height: 900 } });
    const p = await ctx.newPage();
    await p.goto(URL_BASE, { waitUntil: 'networkidle' });
    await p.waitForSelector('[data-testid="sidebar"]', { timeout: 15000 });
    await p.waitForTimeout(500); // 抽屉那条 transform 是 220ms，读得太早量到的是过渡中间值
    const geo = await p.evaluate(() => {
      const rect = (sel) => {
        const el = document.querySelector(sel);
        if (!el) return null;
        const r = el.getBoundingClientRect();
        return { x: Math.round(r.x), right: Math.round(r.right), w: Math.round(r.width) };
      };
      return {
        layout: document.querySelector('.app-shell')?.getAttribute('data-layout') ?? null,
        sidebar: rect('.pane--sidebar'),
        list: rect('.pane--list'),
        editor: rect('[data-testid="editor-pane"]'),
        back: document.querySelector('[data-testid="back-to-list"]') !== null,
      };
    });
    check(`㊱ ${c.w}px 这一档归 §3.1 的「${c.layout}」栏（壳上那位属性就是渲染依据）`,
      geo.layout === c.layout, JSON.stringify(geo));
    if (c.layout === 'three') {
      check(`㊱ ${c.w}px 三栏：侧栏在流内、贴着左缘那一侧、宽 260，列表宽 340，编辑器占剩下的`,
        // 侧栏的 x 不是 0 而是 4 —— 壳自己让出 `--sp-1` 那一圈内衬（§3.2 常驻结构那一条腿量的就是它），
        // 所以"在流内"的判据是"紧靠左缘那一侧"，不是"坐标为零"。第一版写死 0 是我想当然。
        geo.sidebar !== null && geo.sidebar.x >= 0 && geo.sidebar.x <= 8 && geo.sidebar.w === 260
          && geo.list !== null && geo.list.w === 340
          && geo.editor !== null && geo.editor.w > 100,
        JSON.stringify(geo));
      check(`㊱ ${c.w}px 三栏不该有「‹ 返回」（那是单栏那一档的出口）`, geo.back === false, JSON.stringify({ back: geo.back }));
    } else {
      check(`㊱ ${c.w}px ${c.layout} 栏：侧栏真的在屏幕外（抽屉没开时不许看得见），且 DOM 里还在`,
        geo.sidebar !== null && geo.sidebar.right <= 0, JSON.stringify(geo.sidebar));
      if (c.layout === 'two') {
        check(`㊱ ${c.w}px 两栏：列表与编辑器同时在（"侧栏变抽屉"不等于"少一栏"）`,
          geo.list !== null && geo.list.w > 100 && geo.editor !== null && geo.editor.w > 100,
          JSON.stringify({ list: geo.list, editor: geo.editor }));
        check(`㊱ ${c.w}px 两栏不该有「‹ 返回」`, geo.back === false, JSON.stringify({ back: geo.back }));
      }
    }
    if (c.layout === 'one') {
      // 单栏那一档要真的能"列表 ⇄ 编辑器"，并且 ‹ 是那条回去的路
      await p.click('[data-testid^="note-row-"]').catch(() => {});
      await p.waitForTimeout(600);
      const opened = await p.evaluate(() => ({
        list: document.querySelector('.pane--list') !== null,
        editor: document.querySelector('[data-testid="editor-pane"]')?.getBoundingClientRect().width ?? 0,
        back: document.querySelector('[data-testid="back-to-list"]') !== null,
      }));
      check(`㊱ ${c.w}px 单栏：打开一篇之后只剩编辑器，且「‹ 返回」出现在那儿`,
        opened.list === false && opened.editor > 100 && opened.back === true, JSON.stringify(opened));
      await p.click('[data-testid="back-to-list"]');
      await p.waitForTimeout(600);
      const back = await p.evaluate(() => ({
        list: (document.querySelector('.pane--list')?.getBoundingClientRect().width ?? 0) > 100,
        editor: document.querySelector('[data-testid="editor-pane"]') !== null,
      }));
      check(`㊱ ${c.w}px 单栏：那颗「‹」真的把界面带回列表（不是只隐藏编辑器）`,
        back.list === true && back.editor === false, JSON.stringify(back));
    }
    if (c.w === 820 || c.w === 1180) {
      await p.screenshot({ path: `${OUT}/48-breakpoints-${c.w}.png` });
    }
    await p.close();
    await ctx.close();
  }
}

/** 按标题前缀清夹具（跑之前清一次、跑完再清一次 —— 中途崩了也不许把开发库堆脏）。 */
async function purgeByTitle(prefix) {
  for (const trash of [false, true]) {
    const rows = await cmd('list_notes', { folderId: null, trash });
    for (const n of Array.isArray(rows) ? rows : []) {
      if ((n.title ?? '').startsWith(prefix)) await cmd('purge_note', { id: n.id });
    }
  }
}

await browser.close();
// 夹具清干净：这条门禁反复跑，不许每次往开发库里多堆 31 篇。
for (const id of [...fx.ids, fx.longId]) await cmd('purge_note', { id });
console.log(notes.join('\n'));
console.log(failures.length ? `\n${failures.join('\n')}\n>>> 布局门禁 FAIL` : '\n>>> 布局门禁 PASS');
process.exit(failures.length ? 1 : 0);
