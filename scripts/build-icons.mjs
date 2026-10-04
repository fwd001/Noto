/**
 * 从唯一的品牌源图重生成两个壳的全套图标。
 *
 * 存在的原因：`icon.icns` 从来没生成过（macOS 因此拿不到图标），而 .ico 只有 1 个尺寸，
 * 小尺寸下 Windows 任务栏只能把 256 那帧硬缩到 16px。产物不该手改，改样式只改 app-icon.svg。
 */
import { spawnSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const desktop = path.join(repoRoot, 'apps', 'desktop');
const source = path.join('src-tauri', 'icons', 'app-icon.svg');
const cli = path.join(desktop, 'node_modules', '@tauri-apps', 'cli', 'tauri.js');

const targets = [
  { shell: 'desktop', out: path.join('src-tauri', 'icons') },
  { shell: 'mobile', out: path.join('..', 'mobile', 'src-tauri', 'icons') },
];

if (!existsSync(cli)) {
  console.error(`找不到 Tauri CLI：${cli}\n先在 apps/desktop 执行 pnpm install。`);
  process.exit(1);
}
if (!existsSync(path.join(desktop, source))) {
  console.error(`品牌源图缺失：${source}`);
  process.exit(1);
}

for (const t of targets) {
  const r = spawnSync(process.execPath, [cli, 'icon', source, '-o', t.out], {
    cwd: desktop,
    stdio: 'inherit',
  });
  if (r.status !== 0) {
    console.error(`tauri icon 失败（${t.shell}），退出码 ${r.status}`);
    process.exit(r.status ?? 1);
  }
}
console.log('图标已生成。改品牌样式请只改 apps/desktop/src-tauri/icons/app-icon.svg。');
