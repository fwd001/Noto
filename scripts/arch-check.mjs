#!/usr/bin/env node
/**
 * 架构适应度检查（ARCHITECTURE-MAP §1 依赖方向 + §5 禁止模式的机器版）。
 *
 * 为什么要有这个脚本：本仓库的层次约束全部是"违反即 P0"级别的（同步逻辑漏进前端 =
 * 数据安全事件），而它们靠人读文档守不住。这里把可机械判定的那部分钉成测试。
 *
 *   node scripts/arch-check.mjs        # exit 0 = 全部通过；exit 1 = 有违规
 */
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';

const ROOT = process.argv[2] ?? new URL('..', import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, '$1');
const results = [];

function check(id, doc, violated, detail) {
  results.push({ id, doc, ok: violated.length === 0, detail: violated.length === 0 ? '' : detail });
}

/** 解析 Cargo.toml 的 `[dependencies]` / `[dev-dependencies]` 段落里的依赖名。 */
function cargoDeps(manifestPath) {
  const text = readFileSync(manifestPath, 'utf8');
  const sections = { dependencies: [], 'dev-dependencies': [], 'build-dependencies': [] };
  let current = null;
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim();
    const header = /^\[([a-z-]+)\]$/.exec(line);
    if (header) {
      const name = header[1];
      current = name in sections ? name : null;
      continue;
    }
    if (!current || !line) continue;
    const dep = /^([A-Za-z0-9_-]+)\s*=/.exec(line);
    if (dep) sections[current].push(dep[1]);
  }
  return sections;
}

const crates = readdirSync(join(ROOT, 'crates'))
  .filter((d) => statSync(join(ROOT, 'crates', d)).isDirectory())
  .map((d) => [d, cargoDeps(join(ROOT, 'crates', d, 'Cargo.toml'))]);
const desktop = ['notera-desktop', cargoDeps(join(ROOT, 'apps/desktop/src-tauri/Cargo.toml'))];
const all = [...crates, desktop];
const byName = Object.fromEntries(all);

/** 收集源文件（.rs / .ts / .vue），排除测试与目标目录。 */
function sources(dir, exts, { includeTests = true } = {}) {
  const out = [];
  if (!statSafe(dir)) return out;
  for (const entry of readdirSync(dir)) {
    if (['node_modules', 'dist', 'target', 'target-gnu', '.logs', 'fixtures'].includes(entry)) continue;
    const p = join(dir, entry);
    if (statSafe(p, true)) {
      out.push(...sources(p, exts, { includeTests }));
    } else if (exts.some((e) => entry.endsWith(e))) {
      const isTest = !includeTests && (p.includes(`${sep()}tests`) || entry.includes('.spec.') || entry.includes('test'));
      if (!isTest) out.push(p);
    }
  }
  return out;
}
function sep() {
  return '/' /* normalize 后统一用 / 判断 */;
}
function statSafe(p, dir = false) {
  try {
    return statSync(p).isDirectory() === dir;
  } catch {
    return false;
  }
}
function read(p) {
  return readFileSync(p, 'utf8');
}
function rel(p) {
  return relative(ROOT, p).replace(/\\/g, '/');
}

// ------------------------------------------------------ 依赖方向（Cargo.toml）---

const FORBIDDEN = {
  'notera-core': ['notera-richtext', 'notera-crypto', 'notera-store', 'notera-net', 'notera-webdav', 'notera-sync', 'notera-config', 'notera-host', 'notera-importer'],
  'notera-richtext': ['notera-store', 'notera-net', 'notera-webdav', 'notera-sync', 'notera-host', 'notera-config'],
  'notera-crypto': ['notera-store', 'notera-net', 'notera-webdav', 'notera-sync', 'notera-host'],
  'notera-store': ['notera-net', 'notera-webdav', 'notera-sync', 'notera-host', 'notera-config', 'notera-importer'],
  'notera-net': ['notera-store', 'notera-webdav', 'notera-sync', 'notera-host', 'notera-config'],
  'notera-webdav': ['notera-store', 'notera-sync', 'notera-host'],
  'notera-config': ['notera-sync', 'notera-webdav', 'notera-net', 'notera-host'],
  'notera-sync': ['notera-host', 'tauri'],
  'notera-importer': ['notera-net', 'notera-webdav', 'notera-host'],
};
for (const [crate, banned] of Object.entries(FORBIDDEN)) {
  const sec = byName[crate];
  if (!sec) {
    check(`dep:${crate}`, 'ARCHITECTURE-MAP §1', ['crate 缺失'], `crates/${crate} 不在 workspace 里`);
    continue;
  }
  const normal = new Set(sec.dependencies);
  const bad = banned.filter((b) => normal.has(b));
  check(`dep:${crate}`, 'ARCHITECTURE-MAP §1 依赖方向', bad, `${crate} 的正常依赖里出现了禁止项：${bad.join(', ')}`);
}

// ------------------------------------------------- 单一出口（依赖 + 源码 grep）---

const withReqwest = all.filter(([, s]) => s.dependencies.includes('reqwest')).map(([n]) => n);
check('egress:reqwest-dep', 'PROXY.md §1（唯一 HTTP 出口）', withReqwest.filter((n) => n !== 'notera-net'),
  `除 notera-net 外还有 crate 直接依赖 reqwest：${withReqwest.join(', ')}`);

const sqlCrates = crates.filter(([n]) => n !== 'notera-store' && !n.startsWith('notera-test-'))
  .map(([n]) => [n, sources(join(ROOT, 'crates', n), ['.rs'])])
  .flatMap(([n, files]) => files.filter((f) => /"(SELECT |INSERT INTO |UPDATE |DELETE FROM |PRAGMA )/.test(read(f))).map((f) => rel(f)));
check('layer:sql-literal', 'ARCHITECTURE-MAP §5（host/UI 不得写 SQL）', sqlCrates,
  `notera-store 之外出现 SQL 字面量：\n    ${sqlCrates.join('\n    ')}`);

const uiFiles = sources(join(ROOT, 'apps/desktop/src'), ['.ts', '.vue']);
const uiLeaks = uiFiles.filter((f) => /\b(PROPFIND|PROPPATCH|If-Match|If-None-Match|propstat|DaviCk|\.notes\/manifest)\b/.test(read(f))).map(rel);
check('layer:ui-protocol-vocab', '主需求 §禁止把 WebDAV 同步逻辑放进前端', uiLeaks,
  `前端出现了同步协议词汇：\n    ${uiLeaks.join('\n    ')}`);

const rawNet = all.filter(([n]) => n !== 'notera-net' && !n.startsWith('notera-test-'))
  .map(([n]) => [n, sources(join(ROOT, 'crates', n, 'src'), ['.rs'])])
  .flatMap(([n, files]) => files.filter((f) => /std::net::(TcpListener|TcpStream|UdpSocket)/.test(read(f))).map((f) => rel(f)));
check('egress:raw-socket', 'PROXY.md §1（唯一出口；devserver 必须 debug-only）', rawNet,
  `notera-net 之外直接开 socket：\n    ${rawNet.join('\n    ')}`);

const dbOnly = all.filter(([, s]) => s.dependencies.includes('rusqlite')).map(([n]) => n).filter((n) => n !== 'notera-store');
check('layer:rusqlite-dep', 'ARCHITECTURE-MAP §1（只有 store 会说话给 SQLite）', dbOnly,
  `notera-store 之外依赖 rusqlite：${dbOnly.join(', ')}`);

// -------------------------------------------------------- 测试服务器不得进产品 ---

const testSrv = all.filter(([, s]) => s.dependencies.includes('notera-test-webdav')).map(([n]) => n)
  .filter((n) => n !== 'notera-test-webdav');
check('hygiene:test-server-not-a-runtime-dep', 'ADR-0013（测试服务器只能进 dev-dependencies）', testSrv,
  `产品 crate 在正常依赖里带了测试服务器：${testSrv.join(', ')}`);

// ------------------------------------------------------------- 前端禁用的 API ---

const fsInUi = uiFiles.filter((f) => /\brequire\(|process\.env\.|eval\(/.test(read(f))).map(rel);
check('hygiene:ui-no-node-apis', 'ARCHITECTURE-MAP §5（前端不做 IO，不碰宿主 API）', fsInUx(fsInUi),
  `前端用了宿主/动态求值 API：\n    ${fsInUi.join('\n    ')}`);
function fsInUx(list) {
  return list.filter((p) => !p.includes('vitest.config') && !p.includes('.spec.'));
}

// ------------------------------------------------------------------------- 输出 ---

let failed = 0;
for (const r of results) {
  if (!r.ok) failed++;
  console.log(`${r.ok ? 'PASS' : 'FAIL'}  ${r.id}  [${r.doc}]`);
  if (!r.ok) console.log(r.detail.split('\n').map((l) => `      ${l}`).join('\n'));
}
console.log(`\narch-check: ${results.length - failed}/${results.length} 条通过`);
process.exit(failed === 0 ? 0 : 1);
