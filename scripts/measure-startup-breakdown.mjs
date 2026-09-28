/**
 * 冷启动**分解**量具（不是门禁，不判绿）：把"双击图标 → 看见内容"这段等待拆成四个能对着看的
 * 事件，用来回答 D5 那个问题 —— 空库 ≤1000 ms 这条门今天不绿，那这 1000 ms 里到底是
 * 壳/WebView2 的固定开销，还是我们自己那段（DB + 前端首帧）。
 *
 *   node scripts/measure-startup-breakdown.mjs              # 4 遍：全新目录 2 遍 + 同目录 2 遍
 *   SHELL_EXE=<别的 exe 路径> node scripts/measure-startup-breakdown.mjs
 *
 * 记的都是**相对进程创建的墙上毫秒**：
 *   cdp     —— WebView2 的调试端口第一次答话（进程 + 浏览器运行时起来了）
 *   sqlite  —— 数据目录里第一次看见 notera.sqlite（核心的启动与迁移真的跑了）
 *   page    —— CDP 那边枚举到非 devtools 的页面（窗口建好、导航开始）
 *   marker  —— 列表区那个 testid 真的可见（= PERF-01 口径里"用户看见内容"那一刻）
 * 另外从页面里取 `performance.timeOrigin` / `navigation` 那几个数，用来把"文档开始之前"与
 * "文档开始之后"切开 —— 这一刀才是 D5 要看的那一刀。
 *
 * 这条量具**不改产品代码**，也不立判据：它只出数。判绿仍然归 `scripts/verify-perf.mjs`。
 */
const PW = process.env.PW_CORE || 'file:///C:/Users/lhcz-fu/node_modules/playwright-core/index.js';
const SHELL =
  process.env.SHELL_EXE || 'D:/code/Notes/target/x86_64-pc-windows-gnu/release/notera-desktop.exe';
const CDP_PORT = Number(process.env.CDP_PORT || 9223);
const MARKER = '[data-testid="list-empty"], [data-testid="empty-state"], [data-testid^="note-row-"]';
const DIR = process.env.PB_DIR || 'D:/code/Notes/.logs/pb-startup';
const COLD = Number(process.env.COLD || 2);
const WARM = Number(process.env.WARM || 2);

const fs = await import('node:fs').then((m) => m.default);
const { spawn, spawnSync } = await import('node:child_process');
const pw = await (await import(PW)).default;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

function desktopRunning() {
  const out = spawnSync('tasklist', ['/FI', 'imagename eq notera-desktop.exe'], { encoding: 'utf8' });
  return /notera-desktop\.exe/.test(out.stdout || '');
}
function killLeftovers() {
  spawnSync('taskkill', ['/IM', 'notera-desktop.exe', '/F'], { stdio: 'ignore' });
  for (let w = 0; w < 40 && desktopRunning(); w += 1) sleepSync(250);
}
function sleepSync(ms) {
  const t = Date.now();
  while (Date.now() - t < ms) {
    /* 同步等：这一句在轮询循环里，用 await 反而会把事件时间冲淡 */
  }
}

async function httpJson(path) {
  try {
    const res = await fetch(`http://127.0.0.1:${CDP_PORT}${path}`, { signal: AbortSignal.timeout(600) });
    return res.ok ? await res.json() : null;
  } catch {
    return null;
  }
}

async function oneRun(label, wipe) {
  // wipe=true：目录从头建（"新装用户第一次启动"）；false：**沿用上一遍那个目录**（日常第二次启动）
  if (wipe) fs.rmSync(DIR, { recursive: true, force: true });
  fs.mkdirSync(DIR, { recursive: true });
  killLeftovers();
  const env = {
    ...process.env,
    NOTERA_DATA_DIR: DIR,
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${CDP_PORT}`,
  };
  const t0Wall = Date.now();
  const t0 = performance.now();
  const ev = { label, cdp: -1, sqlite: -1, page: -1, marker: -1, dbBytes: -1, webview: null };
  const shell = spawn(SHELL, [], { env, stdio: 'ignore' });
  let browser = null;
  try {
    const deadline = Date.now() + 40000;
    let page = null;
    while (Date.now() < deadline && ev.marker < 0) {
      if (ev.cdp < 0 && (await httpJson('/json/version'))) ev.cdp = Date.now() - t0Wall;
      if (ev.sqlite < 0 && fs.existsSync(`${DIR}/notera.sqlite`)) ev.sqlite = Date.now() - t0Wall;
      if (ev.cdp >= 0 && !page) {
        try {
          browser = await pw.chromium.connectOverCDP(`http://127.0.0.1:${CDP_PORT}`, { timeout: 1200 });
          const pg = browser.contexts()[0]?.pages().find((p) => !p.url().startsWith('devtools'));
          if (pg) {
            page = pg;
            ev.page = Date.now() - t0Wall;
          } else {
            await browser.close();
            browser = null;
          }
        } catch {
          browser = null;
        }
      }
      if (page && ev.marker < 0) {
        const found = await page.$(MARKER).catch(() => null);
        if (found) {
          ev.marker = Date.now() - t0Wall;
          ev.webview = await page
            .evaluate((since) => {
              const nav = performance.getEntriesByType('navigation')[0] || {};
              return {
                // 文档计时原点相对脚本起点（t0）的偏移 = "壳 + WebView2 建窗 + 导航"那一段
                docStartRelativeToT0: Math.round(performance.timeOrigin) - since,
                domContentLoadedMs: Math.round(nav.domContentLoadedEventEnd || 0),
                loadEventMs: Math.round(nav.loadEventStart || 0),
                nowInDocumentMs: Math.round(performance.now()),
              };
            }, t0Wall)
            .catch(() => null);
        }
      }
      if (ev.marker < 0) await sleep(20);
    }
    if (fs.existsSync(`${DIR}/notera.sqlite`)) ev.dbBytes = fs.statSync(`${DIR}/notera.sqlite`).size;
    const w = ev.webview || {};
    console.log(
      `${label}  看见内容 ${ev.marker} ms（CDP 答话 ${ev.cdp} · 页面 ${ev.page} · sqlite ${ev.sqlite} ms/${ev.dbBytes}B）` +
        `  其中 文档之前 ${w.docStartRelativeToT0 ?? '?'} ms + 文档之内 ${w.nowInDocumentMs ?? '?'} ms` +
        `（DCL ${w.domContentLoadedMs ?? '?'} · load ${w.loadEventMs ?? '?'}）`,
    );
    if (ev.marker < 0) throw new Error(`那一步没跑出来：${JSON.stringify(ev)}`);
    return ev;
  } finally {
    if (browser) await browser.close().catch(() => {});
    spawnSync('taskkill', ['/PID', String(shell.pid), '/T', '/F'], { stdio: 'ignore' });
    for (let w = 0; w < 40 && desktopRunning(); w += 1) await sleep(250);
  }
}

if (!fs.existsSync(SHELL)) {
  console.error(`没有 release 壳产物：${SHELL}\n先按 PRODUCTION-READINESS 那条命令出一个（RUSTUP_TOOLCHAIN 不能省）`);
  process.exit(2);
}
console.log(`壳：${SHELL}`);
console.log(`目录：${DIR} · 全新 ${COLD} 遍 + 同目录 ${WARM} 遍`);
const runs = [];
for (let i = 0; i < COLD; i += 1) runs.push(await oneRun(`cold${i + 1}`, true));
for (let i = 0; i < WARM; i += 1) runs.push(await oneRun(`warm${i + 1}`, false));

const nums = runs.map((r) => r.marker);
const sorted = [...nums].sort((a, b) => a - b);
const at = (p) => sorted[Math.min(sorted.length - 1, Math.round(p * (sorted.length - 1)))];
console.log(
  `\n看见内容这一格的分布：min ${sorted[0]} · 中位 ${at(0.5)} · max ${sorted.at(-1)} ms` +
    `（预算那条门是 ≤1000 ms，判绿归 verify-perf）`,
);
const coldOnly = runs.filter((r) => r.label.startsWith('cold')).map((r) => r.marker);
const warmOnly = runs.filter((r) => r.label.startsWith('warm')).map((r) => r.marker);
console.log(`全新目录那些遍：${coldOnly.join(' / ')} ms；同目录再启：${warmOnly.join(' / ')} ms`);
console.log(`⇒ "新库的建库 + 迁移"有没有加进用户那段等待，看这两行的重叠程度，不看单遍的数。`);
