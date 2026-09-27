/**
 * P11（删除 vs 修改）的**界面侧**验收：冲突卡片右栏渲染的必须真的是对面那一版，
 * 取不回来时必须把"没取回来"说出口。
 *
 * 为什么单独一条 lane：这条链的存储层、引擎侧、DTO 各自有测试（含两台真设备那条），
 * 但"Vue 有没有把 DTO 里那句话画到屏幕上"只有浏览器能证明 —— 而卡片是要用户二选一的，
 * 右栏错一份就等于让他凭哈希做决定。
 *
 * 现场由 `crates/notera-host/tests/conflict_payload_e2e.rs` 的留档夹具产出
 * （两台真设备 + 真 TCP WebDAV），本脚本只负责起桥 + 点界面，不改数据。
 *
 * 前置（脚本不管，与 verify-app.mjs 同一约定）：
 *   npm --prefix apps/desktop run dev            # 5173
 *
 *   node scripts/verify-p11-panel.mjs
 */
const PW = process.env.PW_CORE || 'file:///C:/Users/lhcz-fu/node_modules/playwright-core/index.js';
const CHROME = process.env.CHROME || 'C:/Users/lhcz-fu/AppData/Local/ms-playwright/chromium-1243/chrome-win64/chrome.exe';
const URL_BASE = process.env.APP_URL || 'http://127.0.0.1:5173';
const BRIDGE = 'http://127.0.0.1:17323';
const SCENE = process.env.NOTERA_P11_SCENE || 'D:/code/Notes/.logs/p11-lane';
const OUT = 'D:/code/Notes/docs/evidence';
const CARGO = process.env.CARGO_BIN || 'cargo';

const fs = await import('node:fs').then((m) => m.default);
const { spawnSync, spawn } = await import('node:child_process');
const pw = await (await import(PW)).default;
const browser = await (await import(PW)).default.chromium.launch({ executablePath: CHROME });
const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });

const consoleErrors = [];
const failedRequests = [];
page.on('console', (m) => {
  if (m.type() === 'error') consoleErrors.push(m.text());
});
page.on('pageerror', (e) => consoleErrors.push(String(e)));
page.on('requestfailed', (r) => failedRequests.push(`${r.method()} ${r.url()} → ${r.failure()?.errorText}`));

const rows = [];
let currentStep = '(未进入任何步骤)';
function record(ok, detail) {
  rows.push({ step: currentStep, ok, detail });
  console.log(`${ok ? 'PASS' : 'FAIL'}  ${currentStep}${detail ? `  —— ${detail}` : ''}`);
}
async function step(name, fn) {
  currentStep = name;
  try {
    record(true, (await fn()) ?? '');
  } catch (e) {
    record(false, String(e).replace(/\n/g, '\n      ').slice(0, 600));
  }
}
const must = async (sel, ms = 8000) => {
  await page.waitForSelector(sel, { timeout: ms });
  return page.locator(sel);
};

const KEY_LEAK = /\b(?:editor|settings|sync|tb|cmd|state|slash|note|list|nav|conflict|proxy|error|link|mobile|sidebar|win|app|toast)\.[a-zA-Z][\w.]*\b/g;

let bridge = null;
try {
  await step('留档夹具：两台真设备跑出 P11 现场并留在盘上', () => {
    const r = spawnSync(
      CARGO,
      [
        '+stable-x86_64-pc-windows-gnu',
        'test',
        '-p',
        'notera-host',
        '--test',
        'conflict_payload_e2e',
        '--manifest-path',
        'D:/code/Notes/Cargo.toml',
        '--',
        '--ignored',
        '--nocapture',
        'leave_a_p11_scene',
      ],
      { encoding: 'utf8', timeout: 600000 },
    );
    if (r.status !== 0) {
      throw new Error(`夹具没跑成（退出码 ${r.status}）：\n${(r.stderr || '').split('\n').slice(-25).join('\n')}`);
    }
    if (!fs.existsSync(`${SCENE}/expect.json`)) throw new Error(`没产出 ${SCENE}/expect.json`);
    return `${SCENE}/expect.json`;
  });

  const expect = JSON.parse(fs.readFileSync(`${SCENE}/expect.json`, 'utf8'));

  await step('编译本工作区当前的核心可执行文件（不许拿旧二进制起桥）', () => {
    // 上一次跑这条 lane 就是栽在这里：盘上留着的是迁移 0008 之前编的 exe，
    // 它读不懂现场库的 schema，于是桥没起来，后面 6 步全成了"界面坏了"的假信号。
    const r = spawnSync(
      CARGO,
      ['+stable-x86_64-pc-windows-gnu', 'build', '-p', 'notera-cli', '--manifest-path', 'D:/code/Notes/Cargo.toml'],
      { encoding: 'utf8', timeout: 600000 },
    );
    if (r.status !== 0) throw new Error(`build 失败：\n${(r.stderr || '').split('\n').slice(-15).join('\n')}`);
    return 'notera-cli 与库同代';
  });

  await step('为那台**有冲突的设备**起真核心桥', async () => {
    bridge = spawn(
      'D:/code/Notes/target/debug/notera-cli.exe',
      ['--data-dir', expect.dirB, 'serve', '--port', '17323'],
      { stdio: ['ignore', fs.openSync('D:/code/Notes/.logs/p11-bridge.log', 'w'), fs.openSync('D:/code/Notes/.logs/p11-bridge.log', 'a')] },
    );
    // 轮询 /health，不睡固定秒数：冷启动慢的时候固定 sleep 会让后面每一步都读到半启动状态。
    for (let i = 0; i < 60; i += 1) {
      try {
        const r = await fetch(`${BRIDGE}/health`);
        if (r.ok) return `数据目录 ${expect.dirB}`;
      } catch {
        /* 还没起来 */
      }
      await new Promise((res) => setTimeout(res, 250));
    }
    throw new Error(`桥 15 秒内没就绪，stderr：${fs.readFileSync('D:/code/Notes/.logs/p11-bridge.log', 'utf8').trim()}`);
  });

  await step('界面可达且侧栏给出了冲突入口', async () => {
    await page.goto(URL_BASE, { waitUntil: 'networkidle', timeout: 30000 });
    const nav = await must('[data-testid="nav-conflicts"]');
    return nav.first().innerText().then((t) => t.trim().replace(/\s+/g, ' '));
  });

  await step('点进冲突面板：两张卡片都在，且与现场一致', async () => {
    await page.click('[data-testid="nav-conflicts"]');
    await must('[data-testid="conflicts-view"]');
    const ids = [expect.withPayload.conflictId, expect.noPayload.conflictId];
    for (const id of ids) {
      await must(`[data-testid="conflict-${id}"]`);
    }
    const n = await page.locator('.conflicts__list .conflicts__item').count();
    if (n !== ids.length) throw new Error(`面板上有 ${n} 张卡片，现场只有 ${ids.length} 条冲突`);
    return `卡片 ${ids.join(' / ')}`;
  });

  const paneText = async (which) => {
    const pane = page.locator('.conflicts__panes .conflicts__pane').nth(which);
    return (await pane.locator('.conflicts__text').first().innerText().catch(() => '')) ?? '';
  };

  await step('右栏渲染的是**对面那一版**，不是本机那份的复制', async () => {
    await page.click(`[data-testid="conflict-${expect.withPayload.conflictId}"]`);
    const remote = await paneText(1);
    const local = await paneText(0);
    if (!remote.includes(expect.withPayload.remoteText)) {
      throw new Error(`右栏没有对面那一版：「${remote}」`);
    }
    if (remote.includes(expect.withPayload.localText)) {
      throw new Error(`右栏混进了本机这一版（左右两栏同一份就是当年那个 bug）：「${remote}」`);
    }
    if (!local.includes(expect.withPayload.localText)) {
      throw new Error(`左栏没有本机这一版：「${local}」`);
    }
    return `左「${local}」/ 右「${remote}」`;
  });

  await step('取不回来那一版：面板说的是"没取回来"，不是空白也不是本机内容', async () => {
    await page.click(`[data-testid="conflict-${expect.noPayload.conflictId}"]`);
    const hint = await must('[data-testid="conflict-remote-missing"]');
    const text = (await hint.first().innerText()).trim();
    const remote = page.locator('.conflicts__panes .conflicts__pane').nth(1).locator('.conflicts__text');
    if (await remote.count()) {
      throw new Error(`这张卡片没有载荷，右栏却画出了正文：「${await remote.first().innerText()}」`);
    }
    if (!text.includes('没')) throw new Error(`这句话没说清"没取回来"：「${text}」`);
    return text;
  });

  await step('面板上没有漏出文案键名', async () => {
    const leaked = [...new Set(((await page.evaluate(() => document.body.innerText)) || '').match(KEY_LEAK) ?? [])];
    if (leaked.length) throw new Error(`漏出键名：${leaked.join(', ')}`);
    return '无';
  });

  await step('回正常列表：这条笔记还在，并且带着"有分歧"的标记（§5.1 第 4 步）', async () => {
    await page.click('[data-testid="nav-all"]');
    await must('[data-testid^="note-row-"]');
    const marked = await page.locator('[data-testid="row-contended"]').count();
    const rows = await page.locator('[data-testid^="note-row-"]').count();
    if (marked === 0) throw new Error('列表上没有任何"等你处理"的标记：面板之外的用户不知道有分歧这回事');
    // 标记数必须**严格少于**行数：这个设备上有四条行（两条冲突笔记 + 两条本机副本），
    // 若判据只写"标记 > 0"，那么"每一行都无条件画个 ⚠"的坏实现照样能过 —— 那是假门禁。
    if (marked >= rows) throw new Error(`${rows} 行里 ${marked} 行都带标记：⚠ 不是按冲突在册的笔记画的，等于没有信息`);
    // 标记必须落在**真的卡在冲突里**的那条笔记上，落在别处等于 decoration
    const row = `[data-testid="note-row-${expect.withPayload.noteId}"] [data-testid="row-contended"]`;
    if ((await page.locator(row).count()) === 0) throw new Error(`有 ${marked} 个标记，但那台设备真正冲突的笔记行上没有：${expect.withPayload.noteId}`);
    const title = await page.locator(`[data-testid="note-row-${expect.withPayload.noteId}"]`).first().innerText();
    if (!title.includes(expect.withPayload.localText)) {
      throw new Error(`P11 之后笔记没留在正常列表里（标题「${title.replace(/\s+/g, ' ')}」里没有本机那一版的文字）`);
    }
    return `标记 ${marked} 处，落在冲突那条上：「${title.replace(/\s+/g, ' ').slice(0, 40)}」`;
  });

  await step('截图证据（带载荷 / 无载荷两张卡）', async () => {
    // 上面那一步为了验列表标记已经跳到"全部笔记"了，这里得先回面板再截图。
    if ((await page.locator('[data-testid="conflicts-view"]').count()) === 0) {
      await page.click('[data-testid="nav-conflicts"]');
      await must('[data-testid="conflicts-view"]');
    }
    fs.mkdirSync(OUT, { recursive: true });
    await page.click(`[data-testid="conflict-${expect.withPayload.conflictId}"]`);
    await page.screenshot({ path: `${OUT}/p11-panel-with-payload.png` });
    await page.click(`[data-testid="conflict-${expect.noPayload.conflictId}"]`);
    await page.screenshot({ path: `${OUT}/p11-panel-remote-missing.png` });
    return `${OUT}/p11-panel-*.png`;
  });

  await step('控制台零 error、浏览器侧请求零失败', () => {
    if (consoleErrors.length) throw new Error(consoleErrors.slice(0, 3).join(' | '));
    if (failedRequests.length) throw new Error(failedRequests.slice(0, 3).join(' | '));
    return '干净';
  });
} finally {
  if (bridge) {
    // 单斜杠：node 起的进程参数不过 MSYS，写成 //PID 时 taskkill 收到 "//PID" 直接失败，
    // 桥就留在后台占着 17323 和数据目录（下一次 lane 读到的是上个世代的二进制）。
    spawnSync('taskkill', ['/PID', String(bridge.pid), '/F'], { encoding: 'utf8' });
  }
  await browser.close();
}

const failed = rows.filter((r) => !r.ok);
console.log(`\n${rows.length - failed.length}/${rows.length} 通过`);
if (failed.length) {
  console.log('失败步骤：' + failed.map((f) => f.step).join(' / '));
  process.exit(1);
}
