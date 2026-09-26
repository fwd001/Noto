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

/**
 * 收集源文件（.rs / .ts / .vue），排除测试与目标目录。
 * 外层包一层"扫描台账"：递归的子目录不该记账，只记调用方要扫的那一个根。
 */
const scans = [];
function sources(dir, exts, opts = {}) {
  const out = walkSources(dir, exts, opts);
  scans.push({ dir, files: out.length });
  return out;
}
function walkSources(dir, exts, { includeTests = true } = {}) {
  const out = [];
  // 必须传 `true`：`statSafe(p)` 默认判的是"不是目录"，用它当入口守卫会
  // 让每次遍历都在第一行返回空表 —— 那等于所有基于源码的门禁全是摆设（实测过）。
  if (!statSafe(dir, true)) return out;
  for (const entry of readdirSync(dir)) {
    if (['node_modules', 'dist', 'target', 'target-gnu', '.logs', 'fixtures'].includes(entry)) continue;
    const p = join(dir, entry);
    if (statSafe(p, true)) {
      out.push(...walkSources(p, exts, { includeTests }));
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
  // webdav→sync 是**端口边**：sync 只暴露 RemotePort/Commit/RemoteError 契约（它自己
  // 不依赖任何 crate，无环），实现方必须指名它。其余 sync 类型出现即为泄漏。
  'notera-webdav': ['notera-store', 'notera-host'],
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

// 核心发出的 messageKey 必须在 i18n 表里登记。`messageFor` 查不到就退回一句通用
// 兜底文案 —— 于是"有版本要你决定"这种要紧的提示会安静地变成"操作未完成"。
// 前端源码里用到的键由 vitest 扫（apps/desktop/src/i18n.spec.ts），跨语言这一侧只有这里守。
const KEY_LINES = /message_key|Toast \{|=> "/;
const i18nText = read(join(ROOT, 'apps/desktop/src/i18n.ts'));
const registeredKeys = new Set([...i18nText.matchAll(/^\s*'([a-z_]+\.[A-Za-z0-9_]+)':/gm)].map((m) => m[1]));
const normalizeKey = (k) =>
  k
    .replace(/^(notera|error|sync|toast|state|note|link|settings|cmd)\./, '')
    .replace(/([a-z0-9])([A-Z])/g, '$1_$2')
    .replace(/[\s./-]+/g, '_')
    .toLowerCase();
// `cmd.<code>` 是一路映射到 `error.<code>` 的，那张表由前端契约测试逐条对齐，不在这里重复判。
const rustKeyed = [
  ...sources(join(ROOT, 'crates/notera-host/src'), ['.rs']),
  ...sources(join(ROOT, 'crates/notera-core/src'), ['.rs']),
  ...sources(join(ROOT, 'apps/desktop/src-tauri/src'), ['.rs']),
];
const unregistered = [];
for (const f of rustKeyed) {
  for (const line of read(f).split(/\r?\n/)) {
    if (!KEY_LINES.test(line)) continue;
    for (const m of line.matchAll(/"([a-z_]+\.[A-Za-z][A-Za-z0-9_]+)"/g)) {
      const k = m[1];
      if (k.startsWith('cmd.') || k.startsWith('error.')) continue;
      if (registeredKeys.has(k) || registeredKeys.has(`error.${normalizeKey(k)}`)) continue;
      unregistered.push(`${k}  <- ${rel(f)}`);
    }
  }
}
check('hygiene:rust-message-keys-registered', 'ARCHITECTURE-MAP §5（文案键唯一登记处 = i18n.ts）', [...new Set(unregistered)],
  `核心发出了未登记的 messageKey（界面会退化成兜底文案）：\n    ${[...new Set(unregistered)].join('\n    ')}`);

const RAW_SOCKET = /std::net::(TcpListener|TcpStream|UdpSocket)|std::net::\{[^}]*\b(TcpListener|TcpStream|UdpSocket)\b/;
// 桌面壳不在 crates/ 下：按名字取源码目录，否则 `crates/notera-desktop/src` 这个
// 不存在的目录会让"壳有没有自己开 socket"这条检查空转（实测就是这样漏的）。
const srcDirOf = (n) => (n === 'notera-desktop' ? join(ROOT, 'apps/desktop/src-tauri/src') : join(ROOT, 'crates', n, 'src'));
const rawNet = all.filter(([n]) => n !== 'notera-net' && !n.startsWith('notera-test-'))
  .map(([n]) => [n, sources(srcDirOf(n), ['.rs'])])
  .flatMap(([n, files]) => files.filter((f) => RAW_SOCKET.test(read(f))).map((f) => `${rel(f)}  [${n}]`));
check('egress:raw-socket', 'PROXY.md §1（唯一出口；devserver 必须 debug-only）', rawNet.filter((f) => !f.includes('/devserver.rs')),
  `notera-net 之外直接开 socket：\n    ${rawNet.join('\n    ')}`);
// webdav→sync 只允许"端口契约"这一条边：越界用到 sync 的内部类型就是把同步逻辑
// 搬进了传输适配器（那正是本仓库反复强调要避免的那类错误）。
// `Manifest` 在名单里是因为 CAS 提交要**自校验刚写出去的清单**：解析必须由 schema
// 属主（notera-sync）来做，适配器自带第二份解析器才是数据风险。
const PORT_ITEMS = new Set(['RemotePort', 'Commit', 'RemoteError', 'EntryRef', 'Manifest']);
// 判定的是"紧跟在 notera_sync:: 后面的那个类型/模块"，并支持四种写法：
//   notera_sync::RemoteError   notera_sync::{A, B}   notera_sync::manifest::{A, B}
//   notera_sync::RemoteError::Variant（变体不算新的引用面）
// 注释行不算引用。
const PORT_PATH = /notera_sync::((?:[A-Za-z_][A-Za-z0-9_]*|\{[^}]*\})(?:::(?:[A-Za-z_][A-Za-z0-9_]*|\{[^}]*\}))*)/g;
function portItemsOf(path) {
  const segs = path.split('::');
  const last = segs[segs.length - 1];
  if (last.startsWith('{')) {
    return last.slice(1, -1).split(',').map((s) => s.trim()).filter((s) => s && s !== 'self');
  }
  // 模块前缀（manifest::）取末端，其余取头一个标识符
  return [segs[0] === 'manifest' ? last : segs[0]];
}
const portLeaks = sources(join(ROOT, 'crates/notera-webdav/src'), ['.rs'])
  .flatMap((f) =>
    read(f)
      .split(/\r?\n/)
      .filter((line) => !line.trim().startsWith('//'))
      .flatMap((line) => [...line.matchAll(PORT_PATH)].flatMap((m) => portItemsOf(m[1]))),
  )
  .filter((n) => !PORT_ITEMS.has(n));
check('edge:webdav-uses-only-ports', 'ARCHITECTURE-MAP §1（webdav 只见 sync 的端口契约）', [...new Set(portLeaks)],
  `notera-webdav 引用了 notera-sync 的非端口项：${[...new Set(portLeaks)].join(', ')}`);

const devserverSrc = join(ROOT, 'crates/notera-host/src/devserver.rs');
const devserverGated = statSafe(devserverSrc)
  && /(if !cfg!\(debug_assertions\)|#\[cfg\(not\(debug_assertions\)\)\])/.test(read(devserverSrc).replace(/\r\n/g, '\n'))
  && /dev 桥只在 debug 构建启用|debug 构建/.test(read(devserverSrc));
check('egress:devserver-debug-only', 'devserver.rs 头注释（release 必须关掉本地桥）', devserverGated ? [] : ['devserver 未在 debug_assertions 下门控'],
  'release 构建仍带 loopback HTTP 桥，可无凭据驱动真实 Store');

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

// "扫了 0 个文件"和"扫了但没问题"必须能区分开：前者是门禁在空转，
// 历史上真出过这种事（入口守卫写反 → 8 条源码规则全绿却一条没看）。
const vacuous = scans.filter((s) => s.files === 0).map((s) => rel(s.dir));
check('hygiene:no-vacuous-source-scan', 'ARCHITECTURE-MAP §8（门禁必须真的看了文件）', vacuous,
  `这些源码扫描一个文件都没看到，等于没检查：\n    ${vacuous.join('\n    ')}`);

let failed = 0;
for (const r of results) {
  if (!r.ok) failed++;
  console.log(`${r.ok ? 'PASS' : 'FAIL'}  ${r.id}  [${r.doc}]`);
  if (!r.ok) console.log(r.detail.split('\n').map((l) => `      ${l}`).join('\n'));
}
console.log(`\narch-check: ${results.length - failed}/${results.length} 条通过`);
process.exit(failed === 0 ? 0 : 1);
