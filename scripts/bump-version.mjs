#!/usr/bin/env node
/**
 * 版本号的唯一写入口（CI-CD §版本与单一版本源）。
 *
 * 用法：
 *   node scripts/bump-version.mjs patch|minor|major   # 递增一位，另两位清零规则见下
 *   node scripts/bump-version.mjs 0.3.1               # 直接指定
 *
 * 为什么"改版本"必须是**独立的一步**、且只经这个脚本：派生位置各由不同的人手改，
 * 迟早出现"安装包是 0.1.0、库里 app_version 是 0.0.9"。脚本改完立刻自证一致，
 * 不一致就直接失败 —— 不留"看起来改好了"的状态。
 *
 * 递增规则（本项目按用户的决定：从 0.0.0 起，改产品的 commit 就升 patch）：
 *   patch +1；minor/major 归零其后位。
 */
import { readFileSync, writeFileSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { join } from 'node:path';
import { checkVersions } from './check-versions.mjs';

const ROOT = new URL('..', import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, '$1');
const arg = process.argv[2];
if (!arg) {
  console.error('用法：node scripts/bump-version.mjs patch|minor|major|x.y.z');
  process.exit(2);
}

const text = readFileSync(join(ROOT, 'Cargo.toml'), 'utf8');
const cur = text.match(/^\[workspace\.package\][\s\S]*?^version\s*=\s*"([^"]+)"/m)?.[1];
if (!cur) {
  console.error('根 Cargo.toml 里找不到 [workspace.package] version，拒绝瞎改');
  process.exit(1);
}

let next;
if (/^\d+\.\d+\.\d+$/.test(arg)) {
  next = arg;
} else if (['patch', 'minor', 'major'].includes(arg)) {
  const [a, b, c] = cur.split('.').map(Number);
  next = arg === 'major' ? `${a + 1}.0.0` : arg === 'minor' ? `${a}.${b + 1}.0` : `${a}.${b}.${c + 1}`;
} else {
  console.error(`不认识的参数 "${arg}"（要 patch|minor|major|x.y.z）`);
  process.exit(2);
}

const edits = [
  ['Cargo.toml', /^\[workspace\.package\]([\s\S]*?)^version\s*=\s*"[^"]+"/m, (g) => `[workspace.package]${g}version = "${next}"`],
  ['apps/desktop/package.json', /^(\s*"version"\s*:\s*")([^"]+)"/m, (g) => `${g}${next}"`],
  ['apps/desktop/src-tauri/tauri.conf.json', /("version"\s*:\s*")([^"]+)"/, (g) => `${g}${next}"`],
  ['apps/mobile/src-tauri/tauri.conf.json', /("version"\s*:\s*")([^"]+)"/, (g) => `${g}${next}"`],
];

for (const [rel, re, build] of edits) {
  const p = join(ROOT, rel);
  const src = readFileSync(p, 'utf8');
  const m = src.match(re);
  if (!m) {
    console.error(`${rel} 里没找到版本号位置，中止（不动任何文件）`);
    process.exit(1);
  }
  writeFileSync(p, src.replace(re, (...gs) => build(gs[1])));
}

// Cargo.lock 里每个 workspace crate 都带自己的版本号，而它不属于"三处派生位置"：
// 只改源文件的话 lock 会慢一个版本，得等下一次 cargo 跑起来才自己追平 —— 于是提交
// 出去的状态是不一致的（0.0.1、0.0.2 各漂过一次，第二次没人记得手工补）。
// 这一步只改 lock 的元数据，不编译，所以不需要指定 toolchain。
try {
  execFileSync('cargo', ['update', '--workspace', '--offline', '--manifest-path', join(ROOT, 'Cargo.toml')], {
    stdio: 'pipe',
    encoding: 'utf8',
  });
} catch (e) {
  console.error(`版本号已写成 ${next}，但 Cargo.lock 没能自动追平（${e.message}）。\n` +
    `请手工跑 \`cargo update --workspace --offline\` 再提交 —— 不许带着漂移的 lock 提交。`);
  process.exit(1);
}

const drifts = checkVersions(ROOT);
if (drifts.length !== 0) {
  console.error(`已经写成 ${next}，但一致性检查没过：\n  - ${drifts.join('\n  - ')}`);
  process.exit(1);
}
console.log(`版本：${cur} → ${next}（四处派生位置 + Cargo.lock 已同步，check-versions 通过）`);
