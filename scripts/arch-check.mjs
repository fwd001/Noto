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
import { join, relative, dirname, resolve, normalize } from 'node:path';

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
const PORT_ITEMS = new Set(['RemotePort', 'Commit', 'RemoteError', 'EntryRef', 'Manifest', 'PeerLease']);
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

// --------------------------------------------------- stats 这条边的键名不许漂 ---

// 这里守的是一起真实事故：`App::stats` 曾经把存储层的 `StoreStats` 原样序列化发给界面，
// 于是 wire 上是 snake_case 的 notes_trash / fts_rows / outbox_pending，而契约图与界面
// 读的是 notesInTrash / ftsEntries / inflightOps。TypeScript 的类型是断言不是校验，
// undefined 悄悄变成占位符 —— 设置页"回收站 / 占用空间 / 待同步"三行恒为 —，
// 侧栏回收站恒为 0，而前端单测喂的正是 camelCase 假数据，把这个洞完整盖住了。
// 规则：界面能读到的每一个 stats 键，核心那份 DTO 必须真的发得出来。
const commandsSrc = read(join(ROOT, 'crates/notera-host/src/commands.rs'));
// 结构体上方紧邻的属性行也要一起看：光有 snake_case 字段名 + camel() 换算是自欺，
// 真正把 wire 变成 camelCase 的是 serde 的 rename_all（实测过"属性被删、门照样绿"）。
const statsHeader = /#\[serde\(rename_all = "camelCase"\)\]\s*\npub struct StatsDto \{([\s\S]*?)\n\}/.exec(commandsSrc);
const statsBlock = statsHeader?.[1] ?? '';
const camel = (s) => s.replace(/_+([a-z0-9])/g, (_, c) => c.toUpperCase());
const statsDtoKeys = new Set([...statsBlock.matchAll(/^\s*pub ([a-z0-9_]+):/gm)].map((m) => camel(m[1])));
const statsTypesTs = read(join(ROOT, 'apps/desktop/src/api/types.ts'));
const declaredStats = [
  ...[...statsTypesTs.matchAll(/export interface StoreStats \{([\s\S]*?)\n\}/g)].flatMap((m) =>
    [...m[1].matchAll(/^\s*(\w+)\??\s*:/gm)].map((x) => x[1]),
  ),
];
const statsReads = [];
for (const f of uiFiles) {
  for (const m of read(f).matchAll(/\b(?:settings\.stats|stats\.value)\??\.([A-Za-z][A-Za-z0-9_]*)/g)) {
    statsReads.push({ key: m[1], at: rel(f) });
  }
}
const statsDrift = statsHeader
  ? [
      ...declaredStats.filter((k) => !statsDtoKeys.has(k)).map((k) => `${k}  <- api/types.ts:StoreStats`),
      ...statsReads.filter((r) => !statsDtoKeys.has(r.key)).map((r) => `${r.key}  <- ${r.at}`),
    ]
  : ['StatsDto 没有 #[serde(rename_all = "camelCase")]，或结构体没找到（wire 会是 snake_case，界面一律读不到）'];
check('edge:stats-dto-covers-ui-reads', 'ARCHITECTURE-MAP §5（命令面 DTO = 界面读到的键）', [...new Set(statsDrift)],
  `界面读到/声明了核心发不出的 stats 键（会静默变成 — 或 0）：\n    ${[...new Set(statsDrift)].join('\n    ')}`);

// 命令面发出去的每一个结构体都必须**显式**声明 camelCase。
// Rust 的默认是 snake_case，`StoreStats` 就是这么把 notes_trash / outbox_pending 漏到
// 界面上的（见上一条规则）。这条不看具体字段，只把"存储层类型没声明视图命名策略就直接
// 上 wire"这一整类挡掉 —— 命中面比逐字段核对更宽，误报为零（`serde_json::Value` 是手搓的，跳过）。
const TYPE_NOISE = new Set(['Result', 'Vec', 'Option', 'Some', 'Ok', 'serde_json', 'Value', 'String', 'str', 'bool',
  'u8', 'u16', 'u32', 'u64', 'i32', 'i64', 'f32', 'f64', 'usize', 'Arc', 'Box', 'HashMap', 'BTreeMap', 'PathBuf', 'CmdError']);
const structSources = [
  ...sources(join(ROOT, 'crates/notera-host/src'), ['.rs']),
  ...sources(join(ROOT, 'crates/notera-store/src'), ['.rs']),
  ...sources(join(ROOT, 'crates/notera-core/src'), ['.rs']),
  ...sources(join(ROOT, 'crates/notera-config/src'), ['.rs']),
].map((f) => [rel(f), read(f)]);
function typeReturnOf(name) {
  const [, body] = structSources.find(([r]) => r.endsWith('notera-host/src/lib.rs')) ?? [];
  if (!body) return null;
  const at = body.indexOf(`pub fn ${name}(`);
  if (at < 0) return null;
  const brace = body.indexOf('{', at);
  const sig = body.slice(at, brace < 0 ? at + 400 : brace);
  const arrow = sig.lastIndexOf('->');
  return arrow < 0 ? null : sig.slice(arrow + 2).trim();
}
function structAttrs(name) {
  for (const [, text] of structSources) {
    const at = text.search(new RegExp(`pub (struct|enum) ${name}\\b`));
    if (at < 0) continue;
    if (/pub enum/.test(text.slice(at, at + 12))) return 'enum';
    const head = text.slice(Math.max(0, text.lastIndexOf('\n\n', at)), at);
    return head.split(/\r?\n/).filter((l) => !l.trim().startsWith('///')).join('\n');
  }
  return null;
}
const camelArmViolations = [];
for (const arm of commandsSrc.matchAll(/j\(app\.([a-z_]+)\(/g)) {
  const ret = typeReturnOf(arm[1]);
  if (!ret) continue;
  for (const t of new Set([...ret.matchAll(/[A-Za-z_][A-Za-z0-9_]*/g)].map((m) => m[0]).filter((n) => !TYPE_NOISE.has(n)))) {
    const attrs = structAttrs(t);
    if (attrs === null || attrs === 'enum') continue;
    if (!/rename_all\s*=\s*"camelCase"/.test(attrs)) camelArmViolations.push(`${arm[1]} → ${t}（缺 #[serde(rename_all = "camelCase")]）`);
  }
}
check('edge:command-wire-is-camelCase', 'ARCHITECTURE-MAP §5（命令面出参的键名是契约）', [...new Set(camelArmViolations)],
  `这些命令出参的类型没声明 camelCase，wire 上会是 snake_case 而界面按 camelCase 取值：\n    ${[...new Set(camelArmViolations)].join('\n    ')}`);

// 核心每一个 `CmdError::of("<code>")` 都要有 `error.<code>` 文案。
// 为什么单独一条：`messageFor` 把 `cmd.<code>` 剥前缀后查 `error.<code>`，查不到就退成
// 通用兜底；而前端那份对齐表（i18n.spec.ts 的 COMMAND_CODES）是手抄的 —— 手抄就会漏，
// `read_failed`/`attachment_missing` 实际上就漏在表外。这条从 Rust 侧扫，漏了直接判红。
const codeLines = [
  ...sources(join(ROOT, 'crates/notera-host/src'), ['.rs']),
  ...sources(join(ROOT, 'crates/notera-store/src'), ['.rs']),
].flatMap((f) => [...read(f).matchAll(/CmdError::of\(\s*"([a-z_]+)"/g)].map((m) => m[1]));
const errorKeys = new Set([...i18nText.matchAll(/^\s*'error\.([a-z_]+)':/gm)].map((m) => m[1]));
const unregisteredCodes = [...new Set(codeLines)].filter((c) => !errorKeys.has(c));
check('hygiene:rust-error-codes-registered', 'ARCHITECTURE-MAP §5（错误码 → 文案，一处不漏）', unregisteredCodes,
  `这些命令错误码没有对应的 error.* 文案（界面会退化成通用兜底）：${unregisteredCodes.join(', ')}`);

// 前端声明的每一个命令名，核心 dispatch 里必须真有那条分支。
// 缺席不会编译报错、也不会测试失败：调用时静默收到 unknown_command，而调用方普遍
// 有"拿不到就退回已有内容"的兜底 —— 于是功能看着在，其实每次都没走到。
// `preview_text`（冲突并排预览）就是这么藏了很久的一条死边。
const declaredCommands = new Set(
  [...read(join(ROOT, 'apps/desktop/src/api/types.ts')).matchAll(/^\s{2}[a-zA-Z]+:\s*'([a-z_]+)',$/gm)].map((m) => m[1]),
);
const handledCommands = new Set([...commandsSrc.matchAll(/^\s*"([a-z_]+)" =>/gm)].map((m) => m[1]));
const missingCommands = [...declaredCommands].filter((n) => !handledCommands.has(n)).sort();
check('edge:declared-commands-exist', 'ARCHITECTURE-MAP §5（命令面 = 前端声明的那一份）', missingCommands,
  `前端声明了核心没有的命令（调用必得 unknown_command）：${missingCommands.join(', ')}`);

// 每个可交互控件都必须有"读得出来的名字"（§26：无障碍要有自动化验收，不能只靠
// 对比度那种设计期证据）。静态扫模板而不是挂组件：挂载要看状态，而图标按钮、条件
// 分支里的那一支，恰恰是"这次没渲染到"就漏掉的那一个 —— 静态扫每一支都看得见。
// 名字来源按 WCAG 认：aria-label / aria-labelledby / <label for> / 包裹用的 <label>
// / title / 可见文字（含 {{ t('…') }} 这类插值）。
// 版本单源（CI-CD §版本与单一版本源）。这张表原来只是文档：规定要有 check-versions
// 与 bump-version 两个脚本，但脚本并不存在 —— 于是"派生位置不许顺手改"没有任何东西在守。
// 现在复用 check-versions 的同一个函数，两边判据不会分叉。
const versionDrifts = await (await import('./check-versions.mjs')).checkVersions(ROOT);
check('hygiene:version-single-source', 'CI-CD §版本与单一版本源（权威 = 根 Cargo.toml）', versionDrifts,
  `版本号漂移（只有 scripts/bump-version.mjs 可以改版本号）：\n    ${versionDrifts.join('\n    ')}`);

const OPEN_TAG = /<([a-zA-Z][\w-]*)\b([^>]*)>/g;
// 什么算"可交互"：原生标签，以及带交互 role 的自定义控件（块把手就是 `div role=button`
// —— 原生标签扫不到的那一批，恰恰是名字最容易漏的）。
const INTERACTIVE_TAGS = new Set(['button', 'input', 'select', 'textarea', 'a']);
const INTERACTIVE_ROLE = /\brole\s*=\s*"(button|switch|checkbox|radio|tab|textbox|slider|menuitem|link|combobox)"/;
// 只看模板里的真控件。第一次跑就把注释里那句 `<input type=file>` 当成了控件 —— 注释、
// script、style 一律抹掉（按长度抹成空格，偏移不变，`<label>` 的包裹判断还要用前文）。
const maskNonTemplate = (text) => {
  const blank = (s) => s.replace(/[^\n]/g, ' ');
  let out = text;
  for (const re of [/<script\b[\s\S]*?<\/script>/gi, /<style\b[\s\S]*?<\/style>/gi, /<!--[\s\S]*?-->/g]) {
    out = out.replace(re, blank);
  }
  return out;
};
const NAME_ATTR = /\b(aria-label|aria-labelledby|title|placeholder)\b(?=\s*=)|\bid\s*=\s*"([^"]+)"/;
const NAMED_BY_LABEL = (text, start, id) => {
  // 两种合法情形：这一行之前有没闭合的 <label>（包裹式），或同文件里有 for="id" 指过来
  const open = text.lastIndexOf('<label', start);
  const close = text.lastIndexOf('</label>', start);
  if (open > close) return true;
  return id ? new RegExp(`\\bfor\\s*=\\s*"${id}"`).test(text) : false;
};
const unlabeled = [];
const vueFiles = sources(join(ROOT, 'apps/desktop/src'), ['.vue']);
for (const f of vueFiles) {
  const text = maskNonTemplate(read(f));
  for (const m of text.matchAll(OPEN_TAG)) {
    const [, rawTag, attrs] = m;
    const tag = rawTag.toLowerCase();
    const roleM = INTERACTIVE_ROLE.exec(attrs);
    if (!INTERACTIVE_TAGS.has(tag) && !roleM) continue;
    if (/\baria-hidden\s*=\s*"true"/.test(attrs)) continue; // 明确排除在辅助技术之外
    if (/\btype\s*=\s*"hidden"/.test(attrs)) continue;
    const inline = attrs.slice(0, attrs.length);
    // `v-editable` 的元素由指令负责命名（见 editor/editableDirective.ts 的注释：
    // 在**那个元素上**加响应式 `:aria-label` 实测会把打字内容挡在模型之外）。
    // 这条豁免只认指令名，不是通用后门。
    if (/\bv-editable\b/.test(inline)) continue;
    const idm = /\bid\s*=\s*"([^"]+)"/.exec(inline);
    // 需要"读得出名字"的一类：按钮 / 链接 / role 控件（含自闭合的 role=button）
    const needsBody = tag === 'button' || tag === 'a' || (roleM && !['input', 'select', 'textarea'].includes(tag));
    if (/\b(aria-label|aria-labelledby|title)\s*=/.test(inline)) continue;
    if (needsBody) {
      const bodyStart = m.index + m[0].length;
      if (/\/>$/.test(m[0])) {
        // 自闭合：没有内容可读，名字只能来自属性，而属性那一关上面已经过了
        unlabeled.push(`${rel(f)}: <${tag}${inline.slice(0, 60)}…> 自闭合且没有 aria-label / title`);
        continue;
      }
      const body = text.slice(bodyStart, bodyStart + 400);
      const end = body.indexOf(`</${rawTag}`);
      const inner = end >= 0 ? body.slice(0, end) : body;
      if (/\{\{[^}]+\}\}/.test(inner) || /[A-Za-z\u4e00-\u9fa5]/.test(inner.replace(/<[^>]*>/g, ''))) continue;
      unlabeled.push(`${rel(f)}: <${tag}${inline.slice(0, 60)}…> 里既没有文字也没有名字`);
      continue;
    }
    if (/\bplaceholder\s*=/.test(inline)) continue;
    if (NAMED_BY_LABEL(text, m.index, idm && idm[1])) continue;
    unlabeled.push(`${rel(f)}: <${tag}${inline.slice(0, 60)}…> 没有任何可读名字`);
  }
}
check('hygiene:interactive-controls-labeled', '§26（每个交互控件都要有可读名字）', unlabeled,
  `这些控件既没有可见文字也没有 aria-label / <label>，读屏软件只会念出"按钮"：\n    ${unlabeled.join('\n    ')}`);

// 编译期嵌入的文件必须在版本库里。`include_str!` 指向一个未跟踪的文件时，本机一定
// 编得过（工作树里有那个文件），干净检出直接编不过 —— 0007 迁移就是这么漏出去的：
// 那次提交改了 migrate.rs 的 include_str! 列表，却漏列了 .sql 本身。所有门禁都在
// 自己那台已经检出过的工作树上跑，所以没有一条看得见它。
const untrackedIncludes = [];
{
  let tracked = null;
  try {
    const { execFileSync } = await import('node:child_process');
    tracked = new Set(
      execFileSync('git', ['-C', ROOT, 'ls-files'], { encoding: 'utf8' })
        .split(/\r?\n/)
        .filter(Boolean),
    );
  } catch (e) {
    untrackedIncludes.push(`读不到 git 索引（${e.message}）—— 本条无从判断，按失败处理而不是放过`);
  }
  if (tracked) {
    const rustFiles = [...walkSources(join(ROOT, 'crates'), ['.rs']), ...walkSources(join(ROOT, 'apps'), ['.rs'])];
    let sites = 0;
    for (const f of rustFiles) {
      for (const m of read(f).matchAll(/\binclude_(?:str|bytes)!\s*\(\s*"([^"]+)"/g)) {
        sites++;
        const abs = normalize(resolve(dirname(f), m[1])).replaceAll('\\', '/');
        if (!statSafe(abs) && !statSafe(abs, true)) {
          untrackedIncludes.push(`${rel(f)}: include_str! 指向 "${m[1]}"，这个文件在磁盘上都不存在`);
          continue;
        }
        const r = relative(ROOT, abs).replace(/\\/g, '/');
        if (!tracked.has(r)) untrackedIncludes.push(`${rel(f)}: "${r}" 没进版本库 —— 本机编得过，干净检出编不过`);
      }
    }
    // 空转保护：一条都没扫到就是门禁坏了（迁移序列至少嵌在 migrate.rs 里）
    if (sites === 0) untrackedIncludes.push('一个 include_str!/include_bytes! 都没扫到 —— 本条在空转');
  }
}
check('hygiene:include-paths-tracked', 'ARCHITECTURE-MAP §5（编译期嵌入的文件要进版本库）', untrackedIncludes,
  `这些编译期嵌入的目标不在版本库里：\n    ${untrackedIncludes.join('\n    ')}`);

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
