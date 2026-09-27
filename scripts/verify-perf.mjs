/**
 * 性能基线采集（PERF-01 冷启动 / PERF-13 大列表滚动）。
 *
 * 为什么要单独一条 lane：TEST-PLAN 里这些阈值一直是"暂定 + 待建基线"，而"暂定"不能当放行
 * 依据（§Q8）。这里的数字全部来自**真实 release 壳**（`notera-desktop.exe`，内嵌前端产物），
 * 不是 dev 桥 + vite —— 量错路径的数字比没有数字更坏。
 *
 * 库是预先灌好的：`NOTERA_DATA_DIR` 让壳指向一个 scratch 目录（与 notera-cli 同一个入口），
 * 绝不往用户真实的 app data 里写测试数据。
 *
 * 读数口径（两个数都要看，别混）：
 *   spawn→cdp  = 进程创建到 WebView2 的调试端口能接上（含 WebView2 自身启动）
 *   spawn→首帧 = 上面那一段 + 前端起来 + 核心 boot + 列表画出第一行
 * 后者才是"用户双击图标到能看见内容"。前者用来说明剩下的钱花在哪一侧。
 *
 * 前置（脚本自己灌库，但要这两个产物已经编出来）：
 *   cargo build --release -p notera-desktop
 *   cargo build -p notera-cli
 *
 *   node scripts/verify-perf.mjs
 *   NOTES=20000 node scripts/verify-perf.mjs     # 换规模
 *   REPS=3 node scripts/verify-perf.mjs          # 每个场景跑几遍，取最好的一次
 */
const PW = process.env.PW_CORE || 'file:///C:/Users/lhcz-fu/node_modules/playwright-core/index.js';
const CDP = process.env.CDP || 'http://127.0.0.1:9223';
const SHELL = process.env.SHELL_EXE || 'D:/code/Notes/target/release/notera-desktop.exe';
const CLI = process.env.CLI_EXE || 'D:/code/Notes/target/debug/notera-cli.exe';
const OUT = 'D:/code/Notes/docs/evidence';
const NOTES = Number(process.env.NOTES || 5000);
const REPS = Number(process.env.REPS || 2);
const BRIDGE_PORT = 17324;
const BUDGET_EMPTY = Number(process.env.BUDGET_EMPTY || 800);
const BUDGET_BIG = Number(process.env.BUDGET_BIG || 1500);

const fs = await import('node:fs').then((m) => m.default);
const { spawn, spawnSync } = await import('node:child_process');
const pw = await (await import(PW)).default;

const RUN_TAG = process.env.RUN_TAG || 'cur';
const EMPTY_DIR = `D:/code/Notes/.logs/perf-empty-${RUN_TAG}`;
const BIG_DIR = `D:/code/Notes/.logs/perf-big-${RUN_TAG}`;

const rows = [];
const numbers = {};
let currentStep = '(未开始)';
function record(ok, detail) {
  rows.push({ step: currentStep, ok, detail });
  console.log(`${ok ? 'PASS' : 'FAIL'}  ${currentStep}${detail ? `  —— ${detail}` : ''}`);
}
async function step(name, fn) {
  currentStep = name;
  try {
    record(true, (await fn()) ?? '');
  } catch (e) {
    record(false, String(e).replace(/\n/g, '\n      ').slice(0, 500));
    // 不在这里关窗：超预算那一步也要把窗口留给 PERF-13 那一步滚（脚本收尾统一关）。
    // 真留下脏窗口的话，下一步的"没有别的窗口在跑"自检会立刻红 —— 那是想要的信号，不是事故。
  }
}
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
// 注意：从 node 直接起进程时参数**不经过 MSYS**，所以这里用单斜杠。
// （在 bash 里才需要 `//PID`；当初照抄过来，taskkill 收到 "//PID" 直接报错，
// 于是窗口从来没被关掉，下一步的脏窗自检就永远为真 —— 读数全丢。）
const kill = (pid) =>
  pid && spawnSync('taskkill', ['/PID', String(pid), '/F'], { stdio: 'ignore' }).status;
const desktopRunning = () => {
  // /FI 在"什么都没找到"时打的是 `INFO: No tasks are running which match ... notera-desktop.exe`
  // —— 里面**就带着镜像名**，直接 grep 会永远为真（本机实测把整条 lane 的第一步顶死）。
  // 所以只认 CSV 行首那一列。
  const r = spawnSync('tasklist', ['/FI', 'IMAGENAME eq notera-desktop.exe', '/NH', '/FO', 'CSV'], {
    encoding: 'utf8',
  });
  return /^"notera-desktop\.exe"/im.test(r.stdout || '');
};
/** RSS（工作集，MiB）。用系统的 Get-Process 读，不去前端里猜一个数。 */
const workingSetMiB = (pid) => {
  const r = spawnSync(
    'powershell',
    ['-NoProfile', '-Command', `(Get-Process -Id ${pid}).WorkingSet64 / 1MB`],
    { encoding: 'utf8' },
  );
  const n = Number.parseFloat((r.stdout || '').trim().split(/\r?\n/).pop());
  return Number.isFinite(n) ? Math.round(n * 10) / 10 : NaN;
};

const held = {};
function closeWindow(key) {
  const h = held[key];
  if (!h) return;
  Promise.resolve(h.browser?.close()).catch(() => {});
  kill(h.shell?.pid);
  delete held[key];
}

function rmrf(dir) {
  for (let i = 0; i < 8; i += 1) {
    try {
      fs.rmSync(dir, { recursive: true, force: true });
      return;
    } catch {
      // 上一批桥进程可能还在收尾（Windows 上目录被占就是 EPERM）：占着这个目录的
      // 就是灌库用的那座桥，直接收掉它。
      spawnSync('taskkill', ['/IM', 'notera-cli.exe', '/F'], { stdio: 'ignore' });
      sleep(500);
    }
  }
  fs.rmSync(dir, { recursive: true, force: true });
}

async function startBridge(dir) {
  const bridge = spawn(CLI, ['--data-dir', dir, 'serve', '--port', String(BRIDGE_PORT)], {
    stdio: 'ignore',
  });
  for (let i = 0; i < 60; i += 1) {
    try {
      if ((await fetch(`http://127.0.0.1:${BRIDGE_PORT}/health`)).ok) return bridge;
    } catch {
      await sleep(250);
    }
  }
  kill(bridge.pid);
  throw new Error('灌库用的桥 15 秒没就绪');
}

/**
 * 灌库：走 debug 桥的 `create_note` —— 与产品同一条写入路径，不是手写 SQL。
 *
 * 每 1000 条换一次桥：一次灌到 2000 条左右实测 `fetch failed`，那是**测试工装**的毛病
 * （这条桥一个请求一条连接，临时端口/TIME_WAIT 被吃光），不是产品的写入路径有问题。
 */
async function seed(dir, n) {
  rmrf(dir);
  fs.mkdirSync(dir, { recursive: true });
  const doc = (i) => ({
    v: 1,
    content: [
      {
        id: 'blk000001',
        type: 'paragraph',
        content: [{ text: `性能基线笔记 ${i}：一段够长的正文用来量渲染与检索` }],
      },
    ],
  });
  const t0 = Date.now();
  let done = 0;
  while (done < n) {
    const bridge = await startBridge(dir);
    try {
      const folders = await (
        await fetch(`http://127.0.0.1:${BRIDGE_PORT}/cmd/list_folders`)
      ).json();
      const folderId = Array.isArray(folders) ? folders[0].id : null;
      if (!folderId) throw new Error('没有可写入的文件夹');
      const one = async (i) => {
        const send = () =>
          fetch(`http://127.0.0.1:${BRIDGE_PORT}/cmd/create_note`, {
            method: 'POST',
            headers: { 'content-type': 'application/json', origin: 'http://127.0.0.1:5173' },
            body: JSON.stringify({ folderId, doc: doc(i) }),
          });
        let r = await send().catch(() => null);
        if (!r) {
          await sleep(200);
          r = await send().catch(() => null);
          if (!r) throw new Error(`第 ${i} 条两次都发不出去（桥断了）`);
        }
        if (!r.ok) {
          // 业务拒绝是 400，光看状态码定不了位 —— 把核心回的错误码/详情一起打出来
          const body = await r.text().catch(() => '');
          throw new Error(`第 ${i} 条失败：HTTP ${r.status} ${body.slice(0, 200)}`);
        }
      };
      const stop = Math.min(n, done + 1000);
      for (let at = done; at < stop; at += 6) {
        const batch = Array.from({ length: Math.min(6, stop - at) }, (_, k) => one(at + k));
        await Promise.all(batch);
        done = Math.min(stop, at + 6);
      }
    } finally {
      kill(bridge.pid);
    }
    await sleep(1200);
  }
  return `${n} 条用了 ${((Date.now() - t0) / 1000).toFixed(1)} s`;
}

/** 起一次真实 release 壳，返回"进程创建→CDP 可接"与"进程创建→列表首帧"。 */
async function oneColdStart(dir, marker) {
  if (desktopRunning()) throw new Error('还有 notera-desktop 在跑：9223 会接到错的窗口，读数就是假的');
  fs.mkdirSync(dir, { recursive: true });
  const env = {
    ...process.env,
    NOTERA_DATA_DIR: dir,
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: '--remote-debugging-port=9223',
  };
  const t0 = Date.now();
  const shell = spawn(SHELL, [], { env, stdio: 'ignore' });
  let browser = null;
  try {
    let page = null;
    let cdpMs = 0;
    for (let i = 0; i < 120 && !page; i += 1) {
      try {
        browser = await pw.chromium.connectOverCDP(CDP, { timeout: 1500 });
        const ctx = browser.contexts()[0];
        const pg = ctx?.pages().find((p) => !p.url().startsWith('devtools'));
        if (pg) {
          page = pg;
          cdpMs = Date.now() - t0;
        } else {
          await browser.close();
          browser = null;
          await sleep(120);
        }
      } catch {
        await sleep(120);
      }
    }
    if (!page) throw new Error('CDP 15 秒内没接上真窗口');
    await page.waitForSelector(marker, { timeout: 25000 });
    const ms = Date.now() - t0;
    return { ms, cdpMs, rss: workingSetMiB(shell.pid), shell, browser, page };
  } catch (e) {
    kill(shell.pid);
    if (browser) await browser.close().catch(() => {});
    throw e;
  }
}

/** 跑 REPS 遍取最好的一次；每次之间等窗口真的消失（否则撞上面那条自检）。 */
async function coldStartBest(dir, marker, key, keepLast = false) {
  const runs = [];
  for (let i = 0; i < REPS; i += 1) {
    const r = await oneColdStart(dir, marker);
    runs.push(r);
    const last = i === REPS - 1;
    if (last && keepLast) {
      held[key] = r;
      continue;
    }
    held[key] = r;
    closeWindow(key);
    for (let w = 0; w < 60 && desktopRunning(); w += 1) await sleep(250);
  }
  const best = runs.reduce((a, b) => (b.ms < a.ms ? b : a));
  return { best, runs: runs.map((r) => `${r.ms}/${r.cdpMs}`) };
}

try {
  if (!fs.existsSync(SHELL)) throw new Error(`没有 release 壳产物：${SHELL}`);
  spawnSync('taskkill', ['/IM', 'notera-cli.exe', '/F'], { stdio: 'ignore' });
  spawnSync('taskkill', ['/IM', 'notera-desktop.exe', '/F'], { stdio: 'ignore' });
  for (let w = 0; w < 40 && desktopRunning(); w += 1) await sleep(250);

  await step(`灌库：${NOTES} 条走产品写入路径（debug 桥 create_note）`, async () => {
    // SKIP_SEED=1：沿用上一轮灌好的库（换滚深、换口径时不必再花几分钟灌库）。
    // 但必须验它真的是那个规模 —— 沿用了一个空目录就等于量了个空库。
    if (process.env.SKIP_SEED === '1') {
      const info = fs.statSync(`${BIG_DIR}/notera.sqlite`, { throwIfNoEntry: false });
      if (!info?.isFile()) throw new Error(`SKIP_SEED 但 ${BIG_DIR} 里没有库，先灌一遍再说`);
      return `SKIP_SEED：沿用 ${BIG_DIR}`;
    }
    return await seed(BIG_DIR, NOTES);
  });

  await step('PERF-01 冷启动（空库）：进程创建 → 列表区可见', async () => {
    const { best, runs } = await coldStartBest(EMPTY_DIR, '[data-testid="list-empty"], [data-testid="empty-state"], [data-testid^="note-row-"]', 'empty');
    numbers.coldEmptyMs = best.ms;
    numbers.coldEmptyCdpMs = best.cdpMs;
    numbers.rssEmptyMiB = best.rss;
    const info = `${best.ms} ms（CDP 可接 ${best.cdpMs} ms），RSS 起步 ${best.rss} MiB，${REPS} 遍各 ${runs.join(' ms, ')} ms（ms/cdp）· 预算 ≤${BUDGET_EMPTY}`;
    if (best.ms > BUDGET_EMPTY) throw new Error(`超暂定预算：${info}`);
    return info;
  });

  await step(`PERF-01 冷启动（${NOTES} 条）：进程创建 → 第一行列表可见`, async () => {
    const { best, runs } = await coldStartBest(BIG_DIR, '[data-testid^="note-row-"]', 'big', true);
    numbers.coldBigMs = best.ms;
    numbers.coldBigCdpMs = best.cdpMs;
    numbers.rssBigMiB = best.rss;
    const info = `${best.ms} ms（CDP 可接 ${best.cdpMs} ms），RSS 起步 ${best.rss} MiB，${REPS} 遍各 ${runs.join(' ms, ')}（ms/cdp）· 预算 ≤${BUDGET_BIG}`;
    if (best.ms > BUDGET_BIG) {
      // 超预算也要把窗口留在手里给下一步滚：判据已经红了，不必为了省一步再重开一次
      numbers.coldBigOverBudget = true;
      throw new Error(`超暂定预算：${info}`);
    }
    return info;
  });

  await step('PERF-13 大列表滚动：真滚轮事件下的掉帧', async () => {
    const h = held.big;
    if (!h) throw new Error('列表窗口没开着（上一步没能收尾）');
    const page = h.page;
    const list = page.locator('[data-testid="note-list"]').first();
    const box = await list.boundingBox();
    if (!box) throw new Error('量不到列表位置');
    // 帧间隔在页面里用 rAF 记：真实渲染节拍才作数，不看 JS 主线程自报。
    await page.evaluate(() => {
      window.__frames = [];
      let prev = performance.now();
      const tick = (now) => {
        window.__frames.push(now - prev);
        prev = now;
        requestAnimationFrame(tick);
      };
      requestAnimationFrame(tick);
    });
    const x = box.x + box.width / 2;
    const y = box.y + Math.min(box.height / 2, 300);
    // 列表是虚拟化的（DOM 里只留可视 + overscan 那几十行），滚得不够深就等于没量到分页。
    // 所以滚完之后把"到底滚到哪、DOM 里出现过多少行、滚动条总高"一起报出来 ——
    // 覆盖范围是这条基线的一部分，不是可以省掉的脚注。
    const notches = Number(process.env.WHEEL || 120);
    const rowsSeen = new Set();
    for (let i = 0; i < notches; i += 1) {
      await page.mouse.move(x, y);
      await page.mouse.wheel(0, 420);
      if (i % 6 === 0) {
        for (const id of await page.locator('[data-testid^="note-row-"]').evaluateAll((els) =>
          els.map((e) => e.getAttribute('data-testid')),
        ))
          rowsSeen.add(id);
      }
      await sleep(25);
    }
    const frames = await page.evaluate(() => {
      const f = window.__frames.slice(2);
      delete window.__frames;
      return f;
    });
    if (frames.length < 20) throw new Error(`只采到 ${frames.length} 帧，滚得不算发生`);
    const sorted = [...frames].sort((a, b) => a - b);
    const p95 = Math.round(sorted[Math.floor(sorted.length * 0.95)]);
    const worst = Math.round(sorted[sorted.length - 1]);
    const visible = await page.locator('[data-testid^="note-row-"]').count();
    const depth = await page
      .locator('.list-viewport')
      .first()
      .evaluate((el) => ({ top: Math.round(el.scrollTop), high: Math.round(el.scrollHeight), box: Math.round(el.clientHeight) }))
      .catch(() => ({ top: 0, high: 0, box: 0 }));
    const rss = workingSetMiB(h.shell.pid);
    fs.mkdirSync(OUT, { recursive: true });
    await page.screenshot({ path: `${OUT}/perf-big-list.png` });
    numbers.scrollP95Ms = p95;
    numbers.scrollWorstMs = worst;
    numbers.scrollNotches = notches;
    numbers.scrollRowsEverInDom = rowsSeen.size;
    numbers.scrollDepthPx = `${depth.top}/${depth.high}（视口 ${depth.box}）`;
    numbers.scrollVisibleRows = visible;
    numbers.rssAfterScrollMiB = rss;
    if (rowsSeen.size < 300) {
      throw new Error(
        `只滚到 ${rowsSeen.size} 行（DOM 常驻 ${visible}），没跨过一页 200 条 —— 这次滚动不算量到分页，加深 WHEEL=`,
      );
    }
    // 预算是"无 >50 ms 掉帧"：判据看 p95，最坏那一帧照样打出来给人看
    // （首屏那一下 300 ms 也是信息，不是可以藏起来的东西）。
    if (p95 > 50) throw new Error(`滚动 p95 帧间隔 ${p95} ms（预算 ≤50 ms），最坏 ${worst} ms，可见行 ${visible}，RSS ${rss} MiB`);
    return `${frames.length} 帧，p95 ${p95} ms（预算 ≤50），最坏 ${worst} ms；${notches} 次滚轮跨过 ${rowsSeen.size} 行、深度 ${depth.top}/${depth.high}；RSS 滚动后 ${rss} MiB`;
  });
} finally {
  for (const k of Object.keys(held)) closeWindow(k);
  spawnSync('taskkill', ['/IM', 'notera-cli.exe', '/F'], { stdio: 'ignore' });
}

const failed = rows.filter((r) => !r.ok);
console.log(`\n${rows.length - failed.length}/${rows.length} 通过`);
console.log(`读数：${JSON.stringify(numbers)}`);
if (failed.length) console.log('失败步骤：' + failed.map((f) => f.step).join(' / '));
process.exit(failed.length ? 1 : 0);
