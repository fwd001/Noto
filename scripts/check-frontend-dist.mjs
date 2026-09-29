#!/usr/bin/env node
// 移动壳复用桌面那份前端产物（`apps/mobile/src-tauri/tauri.conf.json` 的 `frontendDist`）。
// 这条检查回答一个 APK 那边问不出来的问题：**要被嵌进去的那份 dist 到底在不在、是不是真产物**。
//   - `index.html` 必须在（tauri 的入口）；
//   - 至少一个 `.js` 必须在（不然打开是白屏）；
//   - 总体积要有个下限（`vue-tsc` + vite 出来的东西不可能只有几 KB，太小就是构建坏了）。
// 为什么放在这儿而不是去 APK 里找 JS：tauri v2 是把前端资源**嵌进 Rust 的 `.so`**（`tauri_build`
// 在编壳时把它们打进二进制），APK 的 `assets/` 里根本不会有 `.js`
// —— 这条是 run #17 的 `aapt list` 读数教出来的（那包里 `assets/` 只有 `tauri.conf.json`，
//   而 `native-code: arm64-v8a` 是有的）。所以"UI 进没进包"要在**嵌之前**看，不在包里看。
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';

const argv = process.argv.slice(2);
const arg = (name, fallback) => {
  const i = argv.indexOf(name);
  return i >= 0 && argv[i + 1] !== undefined ? argv[i + 1] : fallback;
};
const root = argv.find((a) => !a.startsWith('--'));
const minBytes = Number(arg('--min-bytes', 40 * 1024));
if (!root) {
  console.error('用法：check-frontend-dist.mjs <dist 目录> [--min-bytes 40960]');
  process.exit(2);
}

const walk = (dir, acc) => {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    const st = statSync(p);
    if (st.isDirectory()) walk(p, acc);
    // 相对路径统一成正斜杠：Windows 上 `join` 给的是反斜杠，
    // 不统一的话 `basename` 切不出来，"index.html 引用了哪个 chunk"这条会永远判不过（本机就是这么红的）。
    else acc.push({ path: p.slice(root.length + 1).split(/[\\/]+/).join('/'), size: st.size });
  }
  return acc;
};

let files;
try {
  files = walk(root, []);
} catch (e) {
  console.error(`读不到 dist 目录 ${root}：${e.message}`);
  process.exit(2);
}
const failures = [];
const total = files.reduce((s, f) => s + f.size, 0);
const index = files.find((f) => f.path === 'index.html');
if (!index) failures.push(`没有 ${root}/index.html —— tauri 的入口不在，壳起来没有页面可加载`);
else if (index.size < 200) failures.push(`index.html 只有 ${index.size} 字节，不像构建出来的入口`);

const js = files.filter((f) => f.path.endsWith('.js'));
if (js.length === 0) failures.push('dist 里一个 .js 都没有 —— 打开必然是白屏');
const css = files.filter((f) => f.path.endsWith('.css'));
if (total < minBytes) failures.push(`dist 总共 ${total} 字节，小于下限 ${minBytes}，像是一次坏掉的构建`);

// 入口得真的引用某个 js/chunk，否则"文件在但没人加载"也是白屏
if (index) {
  const html = readFileSync(join(root, 'index.html'), 'utf8');
  const referenced = js.some((f) => html.includes(f.path.split('/').pop()));
  if (!referenced) failures.push('index.html 里没有引用任何一个 dist 里的 .js —— 界面加载不到');
}

if (failures.length > 0) {
  for (const f of failures) console.error(`FAIL  ${f}`);
  console.error(`check-frontend-dist: ${failures.length} 条不过`);
  process.exit(1);
}
console.log(`check-frontend-dist: 全过（${files.length} 个文件 / ${total} 字节 / JS ${js.length} 个 / CSS ${css.length} 个）`);
