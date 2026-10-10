/**
 * 真窗口验证：通过 WebView2 的远程调试端口连进 **Tauri 壳里的 WebView**，
 * 走真正的 `invoke('notera_command', …)` 通道（不是 dev HTTP 桥），
 * 并让 WebView 自己截图（屏幕抓取抓不到 GPU 合成的 surface，CDP 抓得到）。
 *
 * 前置：
 *   pnpm --dir apps/desktop build          # 资源是**编译期内嵌**的，dist 必须先是新的
 *   cargo build [--release] -p notera-desktop
 *   WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9223 notera-desktop.exe
 *   node scripts/verify-tauri-window.mjs
 *
 * 注意：壳的 `default = ["custom-protocol"]` 一旦生效，debug 与 release 都从内嵌的
 * `frontendDist` 取资源（不再走 5173）。所以这一步测的是"将要发出去的那份前端"，
 * 改了 .vue 却忘了 `pnpm build`，这里绿了也不算数。
 */
const PW = process.env.PW_CORE || 'file:///C:/Users/lhcz-fu/node_modules/playwright-core/index.js';
const CDP = process.env.CDP || 'http://127.0.0.1:9223';
const OUT = 'D:/code/Notes/docs/evidence/app-tauri-window.png';
const pw = await (await import(PW)).default;
const { chromium } = pw;

const rows = [];
function record(name, ok, detail) {
  rows.push({ name, ok });
  console.log(`${ok ? 'PASS' : 'FAIL'}  ${name}${detail ? `  —— ${detail}` : ''}`);
}
async function step(name, fn) {
  try {
    record(name, true, await fn());
  } catch (e) {
    record(name, false, String(e).split('\n').slice(0, 3).join(' / '));
  }
}

const browser = await chromium.connectOverCDP(CDP, { timeout: 15000 });
const contexts = browser.contexts();
const pages = contexts.flatMap((c) => c.pages());
const page = pages.find((p) => (p.url() || '').length > 0) ?? pages[0];
const consoleErrors = [];
page.on('console', (m) => { if (m.type() === 'error') consoleErrors.push(m.text().slice(0, 160)); });
page.on('pageerror', (e) => consoleErrors.push(String(e).slice(0, 160)));

await step('CDP 能连进 Tauri 的 WebView', async () => page.url());
await step('页面里确实有 Tauri 运行时（不是浏览器兜底）', async () => {
  const has = await page.evaluate(() => typeof window.__TAURI_INTERNALS__ === 'object');
  if (!has) throw new Error('__TAURI_INTERNALS__ 不存在 → 这是普通浏览器页面');
  return 'invoke 通道在';
});
await step('invoke 走真命令通道：stats 的键就是契约那 8 个', async () => {
  const v = await page.evaluate(async () => {
    const core = window.__TAURI_INTERNALS__;
    return await core.invoke('notera_command', { name: 'stats', args: {} });
  });
  // 这条断言是活的序列化证据：界面上的每一个数字都靠这些名字取到。
  // 以前这里读的是 `v.user_version`（存储层的 snake_case 原名），而前端读的是 camelCase ——
  // 两个消费方各自对了一半，于是谁都没发现 wire 上是哪套名字。
  // **2026-10-10 补两格**：`deviceId`（第 45 刀）与 `libraryReadOnly`（G85）之后 DTO 长到 10 个键，
  // 这条 step 自 0.0.58 没跑过 —— 第一次跑就红在这里：读数是**行为过期**，不是产品漂了。
  const want = ['attachments', 'dbBytes', 'deviceId', 'folders', 'ftsEntries', 'inflightOps', 'libraryReadOnly', 'notes', 'notesInTrash', 'searchGeneration'];
  const keys = Object.keys(v || {}).sort();
  if (keys.join(',') !== [...want].sort().join(',')) {
    throw new Error(`stats 键集合漂了：${keys.join(', ')}（期望 ${want.join(', ')}）`);
  }
  if (typeof v.deviceId !== 'string' || v.deviceId.length === 0) {
    throw new Error(`deviceId 不是非空字符串（第 45 刀那一格会显示成占位符）：${JSON.stringify(v.deviceId)}`);
  }
  if (typeof v.libraryReadOnly !== 'boolean') {
    throw new Error(`libraryReadOnly 不是布尔（G85 那条横幅读它）：${JSON.stringify(v.libraryReadOnly)}`);
  }
  const numeric = ['attachments', 'dbBytes', 'folders', 'ftsEntries', 'inflightOps', 'notes', 'notesInTrash', 'searchGeneration'];
  for (const k of numeric) if (typeof v[k] !== 'number') throw new Error(`${k} 不是数字，界面会显示占位符：${JSON.stringify(v[k])}`);
  return `notes=${v.notes} 回收站=${v.notesInTrash} 附件=${v.attachments} 待发=${v.inflightOps} 占用=${v.dbBytes}B`;
});
await step('托盘与全局快捷键**真的注册上了**（能力由注册结果写，不是平台猜的）', async () => {
  // 这条断言看的是运行期事实：`report_native_cap` 只在 attach_tray / 注册快捷键
  // 成功之后才把它翻成 true。所以"编译得过但系统没让挂"这种情形会在这里红，
  // 而不是等到用户去设置页找那个不存在的开关。
  const v = await page.evaluate(async () => {
    const core = window.__TAURI_INTERNALS__;
    return await core.invoke('notera_command', { name: 'platform_caps', args: {} });
  });
  for (const k of ['tray', 'globalShortcuts', 'nativeMenu', 'notifications']) {
    if (v[k] !== true) throw new Error(`${k} 不是 true（拿到 ${JSON.stringify(v[k])}）—— 这项原生能力没挂上，设置页必须显示为不支持`);
  }
  return 'tray / globalShortcuts / nativeMenu / notifications 全部为 true';
});
const stamp = Date.now();
const title = `真窗口笔记 ${stamp}`;
await step('invoke 新建笔记 → 落进真实 SQLite', async () => {
  const note = await page.evaluate(async (t) => {
    const core = window.__TAURI_INTERNALS__;
    const folders = await core.invoke('notera_command', { name: 'list_folders', args: {} });
    const doc = { v: 1, content: [{ id: 'blk0000001', type: 'paragraph', content: [{ text: t }] }] };
    return await core.invoke('notera_command', { name: 'create_note', args: { folderId: folders[0].id, doc } });
  }, title);
  if (!note || !note.id) throw new Error(`创建失败：${JSON.stringify(note).slice(0, 160)}`);
  const back = await page.evaluate(async (id) => {
    return await window.__TAURI_INTERNALS__.invoke('notera_command', { name: 'get_note', args: { id } });
  }, note.id);
  if (!back || back.title !== title) throw new Error(`读回不一致：${JSON.stringify(back).slice(0, 120)}`);
  return `id=${note.id.slice(0, 8)}… rev=${back.rev}`;
});
const needle = title.replace(/\D+/g, '');
await step('UI 能看到这条笔记（刷新后由列表读回，不是内存态）', async () => {
  await page.reload({ waitUntil: 'networkidle' });
  await page.waitForTimeout(900);
  const hit = await page.locator(`[data-testid^="note-row-"]:has-text(${JSON.stringify(needle)})`).count();
  if (hit === 0) {
    await page.reload({ waitUntil: 'networkidle' }).catch(() => {});
    await page.waitForTimeout(1200);
    const again = await page.locator(`[data-testid^="note-row-"]:has-text(${JSON.stringify(needle)})`).count();
    if (again === 0) throw new Error('列表里没有刚创建的笔记');
    return `${again} 行（刷新后）`;
  }
  return `${hit} 行`;
});
await step('真窗口里点这条笔记 → 正文显示（选中即打开）', async () => {
  const row = page.locator(`[data-testid^="note-row-"]:has-text(${JSON.stringify(needle)})`).first();
  await row.waitFor({ timeout: 6000 });
  await row.click();
  await page.waitForTimeout(900);
  if ((await page.locator('.editor-blank').count()) > 0) throw new Error('选中后落在空面板上：编辑器没跟着 selectedId 打开');
  const field = page.locator('[data-testid="editor-doc"] [contenteditable="true"]').first();
  await field.waitFor({ timeout: 5000 });
  // **条件等待而不是睡固定时长**：把正文填进 contenteditable 是渲染层的异步动作。
  // 0.0.36 那次 release 真窗口跑成 8/9 就是读早了 —— 失败消息里的"预期正文"只剩一个换行。
  // 超时仍然要失败：点了没显示、或显示的是另一条笔记，都必须红；不许用重试把问题掩盖掉。
  await page.waitForFunction(
    ([sel, want]) => {
      const el = document.querySelector(sel);
      if (!el) return false;
      const nbsp = String.fromCharCode(160);
      return (el.innerText || '').split(nbsp).join(' ').includes(want);
    },
    ['[data-testid="editor-doc"] [contenteditable="true"]', title],
    { timeout: 8000, polling: 150 },
  );
  // 渲染层把空格写成 U+00A0 以保持连续空格，读回时折回 U+0020（dom.ts:104）
  const text = (await field.innerText()).replace(/ /g, ' ');
  if (!text.includes(title)) throw new Error(`正文不含预期：${JSON.stringify(text.slice(0, 60))}`);
  return text.slice(0, 30);
});

await step('真窗口里点「固定」→ 那颗键与行上的标记一起翻面（走的是真 invoke，不是 dev 桥）', async () => {
  // 缺口 G32 的另一半证据。坏的地方在 `dispatch` 的臂（`j(app.to_dto(x))` 少一个 `?`），
  // 而两条通道共用一条契约 —— 所以壳里同样是 `{"Ok":…}`，同样是"点下去屏幕上什么都没动"。
  // dev 桥那一步（verify-app 第 44 步）钉的是界面；这一格钉的是**真 invoke 回来的载荷前端读得到**。
  // **2026-10-10 按 v2 实况重写**：置顶现在是**独立一颗** `note-pin-toggle`（`aria-pressed` 常显），
  // 不再是"动作簇里第一颗按钮"—— 旧读数点的位置现在是「标记颜色」，点开的是色板
  // （这条自 0.0.58 没跑过，第一次跑就红在这里：行为过期，不是产品坏）。
  const row = page.locator(`[data-testid^="note-row-"]:has-text(${JSON.stringify(needle)})`).first();
  await row.waitFor({ timeout: 6000 });
  await row.hover();
  const testid = await row.evaluate((el) => el.getAttribute('data-testid'));
  const pinBtn = row.locator('[data-testid="note-pin-toggle"]');
  const pressedBefore = await pinBtn.getAttribute('aria-pressed');
  const labelBefore = await pinBtn.getAttribute('aria-label');
  await pinBtn.click();
  // 条件等待而不是睡固定时长：这一支要一次真 invoke 往返 + 列表重排。
  const flip = await page
    .waitForFunction(
      ([tid, prev]) => {
        const r = document.querySelector(`[data-testid="${tid}"]`);
        if (!r) return null;
        const p = r.querySelector('[data-testid="note-pin-toggle"]');
        if (!p || p.getAttribute('aria-pressed') === prev) return null;
        return {
          pressed: p.getAttribute('aria-pressed'),
          label: p.getAttribute('aria-label'),
          marker: !!r.querySelector('.row-item__pin--on'),
        };
      },
      [testid, pressedBefore],
      { timeout: 8000, polling: 150 },
    )
    .then((h) => h.jsonValue());
  if (!flip) throw new Error(`点了置顶之后 8 秒内 aria-pressed 没翻面 ⇒ 真 invoke 通道的成功载荷前端读不到（G32 在壳里没修好）`);
  if (flip.marker !== (flip.pressed === 'true')) {
    throw new Error(`aria-pressed 说 ${flip.pressed}、行上的标记说 ${flip.marker} ⇒ 那格和那颗键说的不是一件事`);
  }
  return `aria-pressed ${pressedBefore} → ${flip.pressed}，label「${labelBefore}」→「${flip.label}」，标记 ${flip.marker}`;
});

await step('G75：事件回流在壳里通了（before 是被 ACL 拒的原文）', async () => {
  // 2026-10-10 实测（零 capability 的自建壳）：`plugin:event|listen` 回
  // "event.listen not allowed. Permissions associated with this command: core:event:allow-listen…"，
  // 而 bridge 把订阅失败 **catch 掉了** ⇒ 壳里界面从不因事件刷新、原生菜单点了没反应，
  // 而且这条坏从来没有红过一次（这就是它活到今天的原因）。capability 装上之后再问一次。
  const r = await page.evaluate(async () => {
    const core = window.__TAURI_INTERNALS__;
    try {
      const handler = core.transformCallback(() => {}, true);
      const unlisten = await core.invoke('plugin:event|listen', {
        event: 'notera://probe', target: { kind: 'Any' }, handler,
      });
      return { ok: true, unlisten: String(unlisten).slice(0, 24) };
    } catch (e) {
      return { ok: false, err: String(e && e.message ? e.message : e).slice(0, 200) };
    }
  });
  if (!r.ok) throw new Error(`plugin:event|listen 仍被拒（缺 capability？）：${r.err}`);
  return `listen 允许，unlisten id=${r.unlisten}`;
});

await step('G75：标题栏「最大化」真点一下、再点回来（before 这三颗与拖拽全被拒）', async () => {
  // before 读数（逐字）：`window.minimize / toggle_maximize / start_dragging not allowed` ——
  // 标题栏那三颗键与拖拽区在壳里是死的，而 windowAction 把错误 catch 成"什么都不做"。
  // 最小化/关闭会把窗口藏掉或关掉（lane 后面还有步要跑），所以实点**可逆**的最大化那一颗。
  //
  // 窗口状态的读回**不走插件也不走 CDP**：`is_maximized` 是要授权的读命令（不给 lane 另开权限），
  // 而 CDP 的 `Browser.getWindowBounds.windowState` 在这个 WebView2 窗口上**恒报 `normal`** ——
  // 实测同一刻 OS 层的 `IsZoomed=True`（仪器在骗人，命令是真的）。所以问 user32 的 `IsZoomed`，
  // 读的是操作系统那一层的窗口状态。
  const { execFileSync } = await import('node:child_process');
  const ps = [
    "$p = Get-Process notera-desktop -ErrorAction SilentlyContinue | Select-Object -First 1",
    "if (-not $p) { Write-Output 'no-process'; exit }",
    "Add-Type -Namespace Noto -Name Win -MemberDefinition '[DllImport(\"user32.dll\")] public static extern bool IsZoomed(System.IntPtr h);'",
    "Write-Output ('zoomed=' + [Noto.Win]::IsZoomed($p.MainWindowHandle))",
  ].join('\n');
  const enc = Buffer.from(ps, 'utf16le').toString('base64');
  // execFile + argv：不经过 shell（不拼接命令串），-EncodedCommand 也不吃引号；
  // stderr 吞掉（首次加载 PS 模块会往 stderr 打一段 CLIXML 进度，那与读数无关）。
  const zoomed = () =>
    execFileSync('powershell', ['-NoProfile', '-EncodedCommand', enc], {
      encoding: 'utf8',
      stdio: ['ignore', 'pipe', 'ignore'],
    }).includes('zoomed=True');
  const before = zoomed();
  const btn = page.locator('[data-testid="titlebar"] .titlebar__button').nth(1); // 最小 / 最大 / 关闭
  await btn.waitFor({ timeout: 5000 });
  await btn.click();
  let flipped = before;
  for (let i = 0; i < 25 && flipped === before; i++) {
    await page.waitForTimeout(200);
    flipped = zoomed();
  }
  if (flipped === before) throw new Error(`点了「最大化或还原」之后 IsZoomed 没变（仍 ${before}）⇒ 那三颗键的 capability 没生效`);
  await btn.click();
  let back = flipped;
  for (let i = 0; i < 25 && back === flipped; i++) {
    await page.waitForTimeout(200);
    back = zoomed();
  }
  if (back !== before) throw new Error(`再点一次没有回到 ${before}（现为 ${back}）`);
  return `IsZoomed ${before} → ${flipped} → ${back}（user32 实读，系统那一层）`;
});
await step('WebView 内截图', async () => {
  const buf = await page.screenshot({ path: OUT, fullPage: false });
  return `${(buf ?? Buffer.alloc(0)).length ?? 0} 字节 → app-tauri-window.png`;
});
await step('G75：导出「浏览…」真的叫起系统文件对话框（真发 plugin:dialog|save、ACL 放行）', async () => {
  // 这一格量的是**调用边**（"按钮画出来了"不算数）：点下去要真发 save、选项要带 zip 过滤器、
  // 而且 ACL 要放行。放行的判据用**第一次调用的结局**分辨：被拒是立刻 reject（带 "not allowed" 原文）；
  // 放行则原生对话框弹出来、promise 一直悬着（= pending）—— 不再发第二枪，免得叠两个对话框。
  // 这是最后一条交互步：原生对话框是模态的，它一开后续点击都不作数。
  // 先走到设置页：这条 lane 从 0.0.58 之后的第一次跑就红在这里 —— 它以前从没导航过，
  // 而「浏览…」在设置页的数据卡里（旧读数默认"按钮就在眼前"，不成立）。
  await page.locator('[data-testid="nav-settings"]').click();
  await page.waitForSelector('[data-testid="export-path"]', { timeout: 8000 });
  await page.evaluate(() => document.querySelector('#sec-data')?.scrollIntoView({ block: 'center' }));
  const btn = page.locator('[data-testid="export-browse"]');
  await btn.waitFor({ timeout: 5000 });
  // 门禁探针：Tauri 把 `window.__TAURI_INTERNALS__` 锁成**不可写不可配置**（`writable:false /
  // configurable:false`，2026-10-10 实测；Proxy 也套不上）—— 门外挂 invoke spy **静默挂不上**，
  // 早先那条"点了没反应"的假读数就是这么来的（真对话框都弹出来了，spy 一条没记）。
  // 换**由桥自己报**：设一个 `window.__NOTERA_DEBUG_EVENTS__`，桥在发命令前报一发、落定时报一发。
  await page.evaluate(() => {
    window.__g75 = { events: [] };
    window.__NOTERA_DEBUG_EVENTS__ = (kind, detail) => window.__g75.events.push({ kind, detail });
  });
  await btn.click();
  await page.waitForFunction(() => window.__g75.events.some((e) => e.kind === 'dialog-pick'), null, { timeout: 4000 });
  const pick = await page.evaluate(() => window.__g75.events.find((e) => e.kind === 'dialog-pick'));
  if (pick.detail.command !== 'plugin:dialog|save') throw new Error(`发的不是 save：${pick.detail.command}`);
  const exts = pick.detail.options?.filters?.[0]?.extensions ?? [];
  if (!Array.isArray(exts) || exts[0] !== 'zip') throw new Error(`过滤器不对：${JSON.stringify(pick.detail.options).slice(0, 140)}`);
  // ACL 判定用**结局**分辨：被拒是立刻 reject（带 "not allowed" 原文），放行则原生对话框弹出、promise 一直悬着。
  await page.waitForTimeout(900);
  const settled = await page.evaluate(() => window.__g75.events.find((e) => e.kind === 'dialog-settled'));
  if (settled && String(settled.detail?.outcome ?? '').startsWith('rejected')) {
    throw new Error(`dialog:save 被拒（缺 dialog:allow-save？）：${settled.detail.outcome}`);
  }
  return `真发 save（filter zip）· 结局 ${settled ? settled.detail.outcome : 'pending（原生对话框已弹出）'}`;
});
await step('交互期间控制台零 error（CDP 抓取）', async () => {
  if (consoleErrors.length > 0) throw new Error(`${consoleErrors.length} 条：${consoleErrors.slice(0, 3).join(' | ')}`);
  return `0 条（监听覆盖 ${rows.length} 步交互）`;
});
const failed = rows.filter((r) => !r.ok).length;
console.log(`\nverify-tauri-window: ${rows.length - failed}/${rows.length} 步通过`);
await browser.close();
process.exit(failed === 0 ? 0 : 1);
