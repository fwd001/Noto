/**
 * 真窗口验证：通过 WebView2 的远程调试端口连进 **Tauri 壳里的 WebView**，
 * 走真正的 `invoke('notera_command', …)` 通道（不是 dev HTTP 桥），
 * 并让 WebView 自己截图（屏幕抓取抓不到 GPU 合成的 surface，CDP 抓得到）。
 *
 * 前置：
 *   npm --prefix apps/desktop run build          # 资源是**编译期内嵌**的，dist 必须先是新的
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
  const want = ['attachments', 'dbBytes', 'folders', 'ftsEntries', 'inflightOps', 'notes', 'notesInTrash', 'searchGeneration'];
  const keys = Object.keys(v || {}).sort();
  if (keys.join(',') !== [...want].sort().join(',')) {
    throw new Error(`stats 键集合漂了：${keys.join(', ')}（期望 ${want.join(', ')}）`);
  }
  for (const k of want) if (typeof v[k] !== 'number') throw new Error(`${k} 不是数字，界面会显示占位符：${JSON.stringify(v[k])}`);
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
  // 渲染层把空格写成 U+00A0 以保持连续空格，读回时折回 U+0020（dom.ts:104）
  const text = (await field.innerText()).replace(/ /g, ' ');
  if (!text.includes(title)) throw new Error(`正文不含预期：${JSON.stringify(text.slice(0, 60))}`);
  return text.slice(0, 30);
});

await step('WebView 内截图', async () => {
  const buf = await page.screenshot({ path: OUT, fullPage: false });
  return `${(buf ?? Buffer.alloc(0)).length ?? 0} 字节 → app-tauri-window.png`;
});
await step('交互期间控制台零 error（CDP 抓取）', async () => {
  if (consoleErrors.length > 0) throw new Error(`${consoleErrors.length} 条：${consoleErrors.slice(0, 3).join(' | ')}`);
  return `0 条（监听覆盖 ${rows.length} 步交互）`;
});
const failed = rows.filter((r) => !r.ok).length;
console.log(`\nverify-tauri-window: ${rows.length - failed}/${rows.length} 步通过`);
await browser.close();
process.exit(failed === 0 ? 0 : 1);
