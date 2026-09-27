#!/usr/bin/env node
/**
 * 版本一致性检查（CI-CD §版本与单一版本源）。
 *
 * 权威位置只有一个：根 `Cargo.toml` 的 `[workspace.package] version`。其余都是派生：
 * `apps/desktop/package.json`、`apps/desktop/src-tauri/tauri.conf.json`，以及所有 crate 的
 * `version.workspace = true`。派生位置一旦能被人"顺手改一下"，就会出现"装出去的产品
 * 与库里的版本号不是同一个"这种查起来极贵的问题。
 *
 * 唯一允许的写入口是 `scripts/bump-version.mjs`；本脚本只读、只报。
 */
import { readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';

const ROOT = process.argv[2] ?? new URL('..', import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, '$1');

/** 返回漂移描述数组；空数组 = 一致。 */
export function checkVersions(root = ROOT) {
  const drifts = [];
  const read = (p) => readFileSync(join(root, p), 'utf8');

  const workspace = read('Cargo.toml').match(/^\[workspace\.package\][\s\S]*?^version\s*=\s*"([^"]+)"/m);
  if (!workspace) return ['根 Cargo.toml 的 [workspace.package] 里没有 version —— 权威位置本身缺失'];
  const want = workspace[1];
  if (!/^\d+\.\d+\.\d+$/.test(want)) return [`权威版本号 "${want}" 不是 x.y.z 形式`];

  const pkg = read('apps/desktop/package.json').match(/^\s*"version"\s*:\s*"([^"]+)"/m);
  if (!pkg) drifts.push('apps/desktop/package.json 里找不到 version 字段');
  else if (pkg[1] !== want) drifts.push(`apps/desktop/package.json = ${pkg[1]}，权威是 ${want}`);

  const conf = read('apps/desktop/src-tauri/tauri.conf.json').match(/"version"\s*:\s*"([^"]+)"/);
  if (!conf) drifts.push('tauri.conf.json 里找不到 version 字段');
  else if (conf[1] !== want) drifts.push(`tauri.conf.json = ${conf[1]}，权威是 ${want}`);

  // 所有 crate 一律不许自己写版本号：必须走 workspace
  const manifests = [
    'apps/desktop/src-tauri/Cargo.toml',
    ...readdirSync(join(root, 'crates')).filter((d) => d !== 'node_modules').map((d) => `crates/${d}/Cargo.toml`),
  ];
  for (const rel of manifests) {
    const text = read(rel);
    const own = text.match(/^version\s*=\s*"([^"]+)"/m);
    if (own) {
      drifts.push(`${rel} 自己写死了 version = "${own[1]}"（应为 version.workspace = true）`);
    } else if (!/^version\.workspace\s*=\s*true$/m.test(text)) {
      drifts.push(`${rel} 既没有 version.workspace = true，也没有版本号 —— 版本来源不明`);
    }
  }
  return drifts;
}

if (process.argv[1] && String(process.argv[1]).endsWith('check-versions.mjs')) {
  const drifts = checkVersions(ROOT);
  if (drifts.length === 0) {
    console.log(`check-versions: 一致（权威 = 根 Cargo.toml）`);
    process.exit(0);
  }
  console.error('check-versions: 版本漂移，只有 scripts/bump-version.mjs 可以改版本号');
  for (const d of drifts) console.error('  - ' + d);
  process.exit(1);
}
