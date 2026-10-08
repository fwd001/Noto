#!/usr/bin/env node
/**
 * 架构适应度检查（ARCHITECTURE-MAP §1 依赖方向 + §5 禁止模式的机器版）。
 *
 * 为什么要有这个脚本：本仓库的层次约束全部是"违反即 P0"级别的（同步逻辑漏进前端 =
 * 数据安全事件），而它们靠人读文档守不住。这里把可机械判定的那部分钉成测试。
 *
 *   node scripts/arch-check.mjs        # exit 0 = 全部通过；exit 1 = 有违规
 */
import { readFileSync, readdirSync, statSync, existsSync } from 'node:fs';
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

// 命令面 `j(...)` 里的 Result **必须就地传播（`?`）**，不许把 `Result` 本身交给序列化。
// 为什么单独立一条（缺口 G32 的根因）：`dispatch` 的臂写成 `j(app.to_dto(x))`（少了一个 `?`）时，
// serde 对 `Result` 用的是**外部标签**，于是成功载荷变成 `{"Ok":{…}}` 而不是裸 DTO。这条边有多静默：
// HTTP 仍是 200、库里真的写进去了、前端 `unwrap()` 照原样返回那个封套，`applyNoteUpdate` 发现
// `note.id` 不是字符串就**直接 return** —— 用户看到的是"点下去屏幕上什么都没动"。
// 上面那条 camelCase 规则对它是盲的：它看的是类型有没有声明 rename_all，而 `Result<NoteDto,…>`
// 这种类型压根没声明位置（`to_dto` 返回的是 `Result`，噪声词表里又有 `Result`/`CmdError`）。
// 所以这一条只看形状：**返回 Result 的调用，作为 `j()` 的唯一实参时必须带 `?`**。
const taggedResultArms = [];
for (const m of commandsSrc.matchAll(/\bj\(app\.([a-z_]+)\(/g)) {
  const ret = typeReturnOf(m[1]);
  if (!ret || !ret.trimStart().startsWith('Result')) continue;
  // 从 `j(` 的左括号起做平衡扫描，拿到 `j()` 的完整实参文本。
  const open = commandsSrc.indexOf('(', m.index);
  let depth = 0;
  let end = -1;
  for (let i = open; i < commandsSrc.length; i++) {
    const ch = commandsSrc[i];
    if (ch === '(') depth++;
    else if (ch === ')') {
      depth--;
      if (depth === 0) {
        end = i;
        break;
      }
    }
  }
  if (end < 0) continue;
  const arg = commandsSrc.slice(open + 1, end).trim();
  if (arg.endsWith('?')) continue;
  // **必须点上命令名**：`to_dto` 是好几条臂共用的函数，只报函数名会把两处缺陷
  // 去重成一处（"扫到 1 处"而实际坏 2 处 —— 那是门禁自己骗自己）。往回找这一臂的 `"xxx" =>`。
  const head = commandsSrc.slice(0, m.index);
  const names = [...head.matchAll(/"([a-z_]+)"\s*=>/g)];
  const cmd = names.length ? names[names.length - 1][1] : `(第 ${commandsSrc.slice(0, m.index).split('\n').length} 行)`;
  taggedResultArms.push(`${cmd}：j(app.${m[1]}(…)) → ${ret}（这个 Result 没被 ? 传播，wire 上是 {"Ok":…}）`);
}
check('edge:command-wire-propagates-result', 'ARCHITECTURE-MAP §5（成功载荷是裸 DTO，不是 Result 的外部标签）', [...new Set(taggedResultArms)],
  `这些臂把 Result 直接交给 j() 序列化，成功载荷会变成 {"Ok":{…}}，前端 unwrap 之后当 DTO 用就全是 undefined：\n    ${[...new Set(taggedResultArms)].join('\n    ')}`);

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

// 错误码必须**写成字面量**。上一条只认 `CmdError::of("字面量")` 那个形状：码一旦是从函数
// 算出来的（`CmdError::of(e.code())`），扫描就看不见它，于是"漏登记文案"既不编译报错、
// 也不测试失败 —— 界面安静退成那句通用兜底，谁都不知道少了哪一条能照着办事的说明。
// 0.0.29 的三条凭据码就是这么漏过去的（非 Windows 配账号时看到的是通用兜底，而不是
// "这台设备的凭据库还没接上"），这一条把那个洞本身堵住：算出来的码一律判红。
const computedCodes = [];
for (const f of [
  ...sources(join(ROOT, 'crates/notera-host/src'), ['.rs']),
  ...sources(join(ROOT, 'crates/notera-store/src'), ['.rs']),
]) {
  for (const m of read(f).matchAll(/CmdError::of\(\s*([^,)\s][^,)]*)[,)]/g)) {
    const arg = m[1].trim();
    if (arg.startsWith('"')) continue;
    computedCodes.push(`${rel(f)} → CmdError::of(${arg})`);
  }
}
check('hygiene:error-code-must-be-literal', '§45（算出来的错误码扫不到，漏登记就是静默的）', computedCodes,
  `这些命令错误码不是字面量，上一条"错误码必须登记"的门禁看不见它们：\n    ${computedCodes.join('\n    ')}`);

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

// 或断言（`assert!(a || b)`）是"不会红的门禁"最常见的诞生方式：本轮真出过一次 ——
// `got.get("code").is_some() || got.get("data")…is_none()`，而 dispatch 根本没有 {ok,data}
// 那层包，于是右半对任何成功响应恒真、左半对任何错误恒真，整条断言**数学上不可能失败**，
// 却被算进了"§27 有几条实证"。合理的二选一确实存在（本仓两处：空轮次事件的两种等价形态、
// 窗口上限的两种达成方式），所以规则不是禁用，而是**必须就地写一句为什么**：
// 在本行或上一行写标记 `// 或断言：` + 理由。
//
// 只看断言的**条件那一段**（第一个逗号之前、括号深度为 0 的位置），不然
// `assert!(v.is_empty(), "...")` 前面那个 `.filter(|p| a || b)` 闭包会被误当成或断言。
const disjViolations = [];
let disjSites = 0;
function assertCondition(text, start) {
  // text[start] 起是 `assert!(` 之后第一个字符；返回条件部分（到顶层逗号或收尾括号为止）
  let depth = 0;
  let out = '';
  for (let i = start; i < text.length && i < start + 4000; i++) {
    const c = text[i];
    if ('([{'.includes(c)) depth++;
    else if (')]}'.includes(c)) {
      if (depth === 0) return out;
      depth--;
    } else if (c === ',' && depth === 0) return out;
    out += c;
  }
  return out;
}
for (const dir of ['crates', 'apps/desktop/src']) {
  for (const f of sources(join(ROOT, dir), ['.rs', '.ts', '.vue', '.js'])) {
    const r = rel(f);
    // 只看测试面：生产代码里的 || 是逻辑，不是断言
    if (!/(^|[\\/])tests[\\/]|\.spec\.|(^| )src[\\/]/.test(r)) continue;
    const text = read(f);
    const lines = text.split(/\r?\n/);
    // 用一个游标遍历 `assert!(`：绝不能再 slice(text) 去 exec，那会原地打转（实测卡死过一次）
    let cur = 0;
    for (;;) {
      const hit = text.indexOf('assert!', cur);
      if (hit < 0) break;
      const open = text.indexOf('(', hit);
      if (open < 0) {
        cur = hit + 7;
        continue;
      }
      cur = open + 1;
      const cond = assertCondition(text, cur);
      if (!cond.includes('||')) continue;
      const line = text.slice(0, hit).split('\n').length - 1;
      disjSites++;
      const here = lines[line] || '';
      const prev = line > 0 ? lines[line - 1] : '';
      if (!/或断言：/.test(here) && !/或断言：/.test(prev)) {
        disjViolations.push(`${r}:${line + 1} 有一条「或」断言没写理由`);
      }
    }
  }
}
// 空转保护：一条都没扫到就是本条坏了（入口守卫写反毁掉过 8 条源码规则）
if (disjSites === 0) disjViolations.push('一条「或」断言都没扫到 —— 本条在空转（遍历或匹配模式坏了）');
check('hygiene:disjunctive-assertions-justified', '§45（不会红的断言比没有断言更糟）', disjViolations,
  `这些「或」断言没有就地写明为什么允许二选一：\n    ${disjViolations.join('\n    ')}`);


// 被 .gitignore 吃掉的**源文件** = 干净检出编不过，而本机一切绿灯都是假的。
// 这条是踩出来的：0.0.29 新增的 `crates/notera-host/src/credential_store.rs` 原本叫 `secrets.rs`，
// 而仓库的 ignore 里有一条 `secrets.*`（那是防本地口令文件的）—— 于是这个源文件被静默忽略：
// `cargo check`、clippy、workspace 测试、浏览器 lane 全绿，只有 CI 的干净检出会炸。
const ignoredSources = [];
{
  const { execFileSync } = await import('node:child_process');
  let listed = null;
  try {
    listed = execFileSync(
      'git',
      ['-C', ROOT, 'ls-files', '--others', '--ignored', '--exclude-standard', '--', 'crates', 'apps', 'scripts'],
      { encoding: 'utf8', maxBuffer: 32 * 1024 * 1024 },
    );
  } catch {
    listed = null;
  }
  if (listed === null) {
    ignoredSources.push('git ls-files 跑不动 —— 本条在空转（没有 git 就只能靠人记得住）');
  } else if (listed.trim() === '') {
    ignoredSources.push('git 一个未跟踪文件都没列出来 —— 本条在空转');
  }
  for (const raw of listed ? listed.split('\n') : []) {
    const f = raw.trim().replace(/\\/g, '/');
    if (!f) continue;
    if (!/^(crates|apps|scripts)\//.test(f)) continue;
    if (!/\.(rs|ts|tsx|vue|mjs|cjs|css)$/.test(f)) continue;
    if (/node_modules|\/dist\/|src-tauri\/gen\//.test(f)) continue;
    ignoredSources.push(f);
  }
}
check('hygiene:no-ignored-source-file', '§45（本机编得过、干净检出编不过，是最贵的一种假绿）', ignoredSources,
  `这些源码文件被 .gitignore 挡住了，不会进版本库：\n    ${ignoredSources.join('\n    ')}`);


// ------------------------------------------------------------- 32. CI 里真有这些门 ---
// 起因是 2026-09-30 对 §45 时撞出来的一条假账：`audit-rust-deps.mjs` 的注释写着"CI 里也是这条"，
// 而 `ci.yml` 里**从来没有那一步** —— 那条门从写进注释那天起只在本机跑过。
// 单靠"记得写注释"治不了这一类，所以把它变成判据：**这些命令必须真的出现在 gates job 的 `run:` 里，
// 而且那一步不许带 `continue-on-error`**（一条会容错的红门禁不是门禁，只是日志）。
// 两条变异各自验过会红：删掉依赖审计那一步、给 `cargo test` 那一步加上 `continue-on-error: true`。
const CI_REQUIRED_GATES = [
  ['node scripts/check-versions.mjs', '版本单源'],
  ['node scripts/arch-check.mjs', '架构适应度'],
  ['node scripts/audit-rust-deps.mjs --self-test', 'Rust 依赖审计的判据自测'],
  ['node scripts/audit-rust-deps.mjs', 'Rust 依赖审计全量'],
  ['cargo fmt --all --check', '格式检查'],
  ['cargo clippy --workspace', 'Clippy'],
  ['cargo test --workspace', 'Rust 全量测试'],
  ['pnpm audit', '前端依赖审计'],
];
const ciGateProblems = [];
{
  const ciPath = join(ROOT, '.github', 'workflows', 'ci.yml');
  const ci = existsSync(ciPath) ? readFileSync(ciPath, 'utf8') : null;
  if (ci === null) {
    ciGateProblems.push('读不到 .github/workflows/ci.yml —— 本条在空转');
  } else {
    // 按"6 个空格 + `- ` 开头"切步骤块；这一步只要求**同一块里**同时看到命令与容错标记。
    const blocks = ci.split(/\n {6}- /);
    for (const [cmd, label] of CI_REQUIRED_GATES) {
      const hit = blocks.find((b) => /^\s+run:.*/m.test(b) && b.includes(cmd));
      if (!hit) ciGateProblems.push(`${label}：ci.yml 里没有任何一步真的在跑 \`${cmd}\``);
      else if (/continue-on-error:\s*true/.test(hit)) ciGateProblems.push(`${label}：那一步带 continue-on-error，红也不拦`);
    }
  }
}
check('ci:gates-are-actually-blocking', 'CI-CD §流水线分层 / 实际落地的 PR 门禁', ciGateProblems,
  ciGateProblems.join('\n    '));


// ------------------------------------------------------------------------- 输出 ---

// "扫了 0 个文件"和"扫了但没问题"必须能区分开：前者是门禁在空转，
// 历史上真出过这种事（入口守卫写反 → 8 条源码规则全绿却一条没看）。
const vacuous = scans.filter((s) => s.files === 0).map((s) => rel(s.dir));
check('hygiene:no-vacuous-source-scan', 'ARCHITECTURE-MAP §8（门禁必须真的看了文件）', vacuous,
  `这些源码扫描一个文件都没看到，等于没检查：\n    ${vacuous.join('\n    ')}`);

/**
 * 工装残骸的看守自己也得是活的（缺口 G97）。
 * 两臂：① 拿合成序列验"何时该删"那套数学（`--self-test` 的五臂）；
 * ② 对盘上的 `.logs` 做一次真预算检查 —— 这一臂以前**根本没有**，
 *   所以 `verify-perf` 每换一个 RUN_TAG 留下 43 MB 也没人红，实测涨到 197 MB。
 * `.logs` 不存在（CI 的干净工作树）时第二臂跳过，不编造违规。
 */
{
  const { spawnSync } = await import('node:child_process');
  const { BUDGET } = await import('./clean-scratch.mjs');
  const bytesOf = (dir) => {
    let sum = 0;
    for (const name of readdirSync(dir)) {
      const full = join(dir, name);
      let s = null;
      try { s = statSync(full); } catch { continue; }
      sum += s.isDirectory() ? bytesOf(full) : s.size;
    }
    return sum;
  };
  const scratch = [];
  const probe = spawnSync(process.execPath, [join(ROOT, 'scripts', 'clean-scratch.mjs'), '--self-test'], { encoding: 'utf8' });
  if (probe.status !== 0) scratch.push(`clean-scratch --self-test 退出码 ${probe.status}\n    ${(probe.stdout ?? '') + (probe.stderr ?? '')}`);
  const logsDir = join(ROOT, '.logs');
  if (existsSync(logsDir)) {
    const mb = bytesOf(logsDir) / 1048576;
    if (mb > BUDGET.logsMaxMb) scratch.push(`.logs 已 ${mb.toFixed(1)} MB > 预算 ${BUDGET.logsMaxMb} MB —— 跑 \`node scripts/clean-scratch.mjs\` 收`);
  }
  check('hygiene:scratch-budget-guarded', 'ARCHITECTURE-MAP §8（工装残骸要有上界，删除判据要能被合成序列验）', scratch,
    scratch.join('\n    '));
}

let failed = 0;
for (const r of results) {
  if (!r.ok) failed++;
  console.log(`${r.ok ? 'PASS' : 'FAIL'}  ${r.id}  [${r.doc}]`);
  if (!r.ok) console.log(r.detail.split('\n').map((l) => `      ${l}`).join('\n'));
}
console.log(`\narch-check: ${results.length - failed}/${results.length} 条通过`);
process.exit(failed === 0 ? 0 : 1);
