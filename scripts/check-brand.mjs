/**
 * 品牌名门禁：可见层只准叫 Noto，且不许再出现旧名。
 *
 * 两条判据分别钉两件不同的事：
 *  1) 逐点核对（productName / 窗口 title / 页面 title / app.name / 托盘 tooltip）——
 *     这几处是"派生位置"，历史上版本号就在这类地方漏过第 4 处，名字同理；
 *  2) 全量扫描 `apps/` 下不该再有 "Notera" 字面量 —— 新增文案漂移不用登记就会红。
 * `identifier` 反过来钉死为 `app.notera`：它决定数据目录，改了会把已有本机数据留在原地变孤儿。
 * crate 名（notera-store 等）与 `notera.` 文案键前缀是内部标识，不在可见层，不扫。
 */
import { readFileSync, readdirSync, statSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const read = (p) => readFileSync(path.join(repoRoot, p), 'utf8');

const failures = [];
const checks = [];

function expect(name, actual, wanted) {
  checks.push(`${name} = ${JSON.stringify(actual)}`);
  if (actual !== wanted) failures.push(`${name} 是 ${JSON.stringify(actual)}，应为 ${JSON.stringify(wanted)}`);
}

for (const conf of ['apps/desktop/src-tauri/tauri.conf.json', 'apps/mobile/src-tauri/tauri.conf.json']) {
  const c = JSON.parse(read(conf));
  expect(`${conf} · productName`, c.productName, 'Noto');
  expect(`${conf} · 窗口 title`, c.app.windows[0].title, 'Noto');
  expect(`${conf} · identifier（改它等于换数据目录）`, c.identifier, 'app.notera');
  if (!c.bundle.longDescription.startsWith('Noto')) {
    failures.push(`${conf} · longDescription 没以 Noto 开头：${c.bundle.longDescription}`);
  }
}

expect('index.html · <title>', /<title>([^<]*)<\/title>/.exec(read('apps/desktop/index.html'))?.[1], 'Noto');
expect("i18n · 'app.name'", /'app\.name':\s*'([^']*)'/.exec(read('apps/desktop/src/i18n.ts'))?.[1], 'Noto');
expect('托盘 tooltip', /\.tooltip\("([^"]*)"\)/.exec(read('apps/desktop/src-tauri/src/lib.rs'))?.[1], 'Noto');

const SCAN_ROOTS = ['apps/desktop/src', 'apps/desktop/src-tauri/src', 'apps/mobile/src-tauri/src'];
const SCAN_FILES = ['apps/desktop/vite.config.ts', 'apps/desktop/index.html'];

function walk(dir, out) {
  for (const entry of readdirSync(path.join(repoRoot, dir))) {
    const rel = path.posix.join(dir, entry);
    const abs = path.join(repoRoot, rel);
    if (statSync(abs).isDirectory()) walk(rel, out);
    else if (/\.(ts|vue|rs|json|html|css)$/.test(entry)) out.push(rel);
  }
  return out;
}

const files = SCAN_FILES.filter((f) => statSync(path.join(repoRoot, f)).isFile());
for (const root of SCAN_ROOTS) walk(root, files);

for (const rel of files) {
  const text = read(rel);
  const line = text.split('\n').findIndex((l) => l.includes('Notera'));
  if (line >= 0) {
    failures.push(`${rel}:${line + 1} 还有旧品牌名 "Notera"：${text.split('\n')[line].trim().slice(0, 90)}`);
  }
}

for (const c of checks) console.log(c);
console.log(`扫描了 ${files.length} 个可见层文件`);
if (failures.length) {
  console.error(`\n品牌名门禁未通过：\n${failures.map((f) => `  - ${f}`).join('\n')}`);
  process.exit(1);
}
console.log('品牌名门禁通过：可见层统一为 Noto，identifier 保持 app.notera，旧名 0 处残留。');
