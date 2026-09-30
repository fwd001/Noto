/**
 * Rust 依赖漏洞审计（不装任何工具的那条路）。
 *
 * 为什么要有这一条：G12 结掉前端那一半之后，台账里剩下那句照实写着的账是
 * "Rust 那 12 个 crate 的依赖审计本机没装也没跑过"。常规做法是 `cargo audit` / `cargo deny`，
 * 那要在你的机器上**装东西** —— 那一句得你点头。这一条不装：把 `Cargo.lock` 里
 * **每一个** `name@version` 批量发给 OSV 的查询接口（`api.osv.dev/v1/querybatch`），
 * 它收录的就是 RustSec 那份 advisory-db —— 同一个数据源，只是不需要本地扫描器。
 *
 * 三件事必须说清楚，别拿"命令跑通了"当结论：
 * * **这条会把依赖清单发出去**（包名 + 版本，发给 api.osv.dev）。这是审计本身的要求；
 *   不发就什么都查不到。发出去的只有清单，没有源码、没有内容、没有任何凭据。
 * * **"0 条"只有在扫描真的看过全部条目时才算数**：HTTP 非 2xx、返回条数 ≠ 查询条数、
 *   lockfile 解析出 0 个包、结果与查询索引对不齐 —— 任何一条不对都**硬失败**，不判"干净"。
 * * **判据先自测**：`--self-test` 拿已知有漏洞的包与已知干净的包混在同一次查询里跑 ——
 *   前者必须报出来（不然"0 条"没有意义），后者必须不报（报了说明结果按错了索引；
 *   第一版就错过一次：查 6 个只把前 4 个当键，第 5、6 条结果读到 undefined）。
 *
 * 2026-09-30 补的那一块（`checkEdgeClosure`）：上面那些守卫只保证"发出去的没被吞"，
 * 不保证**发出去的是全部**。`parseLock` 少解析一个 `[[package]]` 块，表现是查询数从 624
 * 悄悄变成 623、照样 PASS —— 而少掉的那一个正是"新加的依赖没进审计"这一类，
 * 与"扫到 0 项 ≠ 检查过"是同一个坑。所以改成用 lockfile 自己的边做对账：每一条
 * `dependencies` 都必须指向一个**这次真被查过**的包，指不到就硬失败。解析器丢块 ⇒ 那个包
 * 仍被它的父块引用 ⇒ 边落空 ⇒ 红。变异验证见本文件末尾的注释。
 *
 * 豁免表 `WAIVERS` 的规矩：**每一条必须写"为什么豁免"与"什么时候回头看"**，不许写"暂时忽略"
 * （§39）。它只把"已经判定过、而我们这一层修不动"的那几条从阻断里拿掉，新冒出来的照样红 ——
 * 豁免是把噪音关掉，不是把判据关掉。每条在 `docs/PRODUCTION-READINESS.md` §7 都有对应的一格。
 *
 * 跑法：
 *   node scripts/audit-rust-deps.mjs               # 审 Cargo.lock 的全部包（CI 的 gates job 里也是这条，阻断）
 *   node scripts/audit-rust-deps.mjs --self-test    # 只验判据本身
 *   node scripts/audit-rust-deps.mjs --lock <路径>  # 换一份 lockfile：给变异验证用（判据本身别绕这条）
 */
import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = resolve(fileURLToPath(new URL('.', import.meta.url)), '..');
const ENDPOINT = 'https://api.osv.dev/v1/querybatch';
// OSV 给 Rust 用的生态名是 `crates.io`（写成 "Rust" 会被它直接 400，而这条脚本对 400 是硬失败）。
const ECOSYSTEM = 'crates.io';

/**
 * 已判定过、但不在我们这一层的：键 `<包>@<版本>`，值写理由与回看时机。
 * 加一条就要在台账里同步一格，否则这里不该有它。
 */
const WAIVERS = {
  'glib@0.18.5':
    'RUSTSEC-2024-0429（`glib::VariantStrIter` 的 `Iterator` impl 不安全）来自 **Linux/gtk 那半张依赖图**' +
    '（tauri → gtk → glib），修在 0.20.0，要上游抬版本才动得了；本轮不产出 Linux 产物，这半张图不进二进制。' +
    '回看时机：真要接 Linux 构建之前（§48 缺口 G7 那一格）。',
  'proc-macro-error@1.0.4':
    'RUSTSEC-2024-0370 是 **unmaintained 通告**（没有"修于哪个版本"，也没有可利用的缺陷），' +
    '由构建期宏带进来、不进产物。回看时机：等把它拉进来的那条链（proc-macro-error 的父依赖）不再要它。',
};

/**
 * 解析 `Cargo.lock` 的 `[[package]]` 块 —— 用 lockfile 而不是 `cargo tree`：不跑 cargo、不装工具。
 * 顺带把每个块的 `dependencies` 边收下来，留给 `checkEdgeClosure` 对账。
 *
 * v4 锁里的一条边有三种写法，缺一种就是解析器在骗人：
 *   `"name"`            —— 全图里这个名字只有一个版本，cargo 省略了版本
 *   `"name version"`    —— 同名多版本（本图里 `sha2`、`cipher` 就是这样）
 *   `"name version (src)"` —— 还要靠来源区分（当前树里没有；出现也不当噪音跳过）
 */
export function parseLock(text) {
  const packages = [];
  const edges = [];
  for (const block of text.split('[[package]]').slice(1)) {
    const name = /^name = "([^"]+)"/m.exec(block)?.[1];
    const version = /^version = "([^"]+)"/m.exec(block)?.[1];
    if (!name || !version) continue;
    packages.push({ name, version });
    const from = `${name}@${version}`;
    const deps = /^dependencies = \[([^\]]*)\]/m.exec(block)?.[1] ?? '';
    for (const quoted of deps.match(/"[^"]*"/g) || []) {
      edges.push({ from, text: quoted.slice(1, -1).trim() });
    }
  }
  return { packages, edges };
}

/**
 * 边对账：每条 `dependencies` 必须指向一个**这次真被查过**的包。
 *
 * 为什么这一样是判据的一部分而不是锦上添花：审计的"0 条"只覆盖发出去的那批，
 * 而"发出去的是不是全部"原先没人管。解析器漏一块（改锁格式、名字里带引号、
 * 块顺序变了）表现就是少查一个 + 照样 PASS。加了这道账之后，漏解析必红 ——
 * 因为漏掉的那个包一定被它的父块引用着。
 */
export function checkEdgeClosure(packages, edges) {
  const queried = new Set(packages.map((p) => `${p.name}@${p.version}`));
  const byName = new Map();
  for (const p of packages) {
    if (!byName.has(p.name)) byName.set(p.name, []);
    byName.get(p.name).push(p.version);
  }
  const misses = [];
  for (const e of edges) {
    const tok = e.text.split(/\s+/);
    const [name, version, source] = tok;
    let target;
    if (tok.length === 1) {
      const versions = byName.get(name) ?? [];
      if (versions.length === 0) {
        // 这一支就是第一版漏掉的那一支：`?? null` + `if (target && …)` 把"引用了一个不存在的
        // 包"当成了"没事"，两个变异（删掉 rcgen / aws-lc-sys 整块）于是都笑着过了。
        misses.push(`${e.from} → ${name}：被引用，但它不在被查询的包里（少解析了一个块 ⇒ 覆盖不全）`);
        continue;
      }
      if (versions.length > 1) {
        misses.push(`${e.from} → ${e.text}：同名有 ${versions.length} 个版本却没写版本号 —— 边指不到确定的包`);
        continue;
      }
      target = `${name}@${versions[0]}`;
    } else if (tok.length === 2 || (tok.length === 3 && /^\(.+\)$/.test(source))) {
      target = `${name}@${version}`;
    } else {
      misses.push(`${e.from} → ${e.text}：这条边的写法不在已知的三种里 —— 不猜，判失败`);
      continue;
    }
    if (!queried.has(target)) {
      misses.push(`${e.from} → ${target}：被引用，却没进这次查询（它没被看过，"0 条"不含它）`);
    }
  }
  if (misses.length > 0) {
    const shown = misses.slice(0, 10).map((m) => `  · ${m}`).join('\n');
    throw new Error(
      `lockfile 的依赖边有 ${misses.length} 条落不到被查询的包上 —— 这一跑覆盖不全，不能判"干净"：\n${shown}` +
        (misses.length > 10 ? `\n  …还有 ${misses.length - 10} 条` : ''),
    );
  }
  return queried.size;
}

async function queryBatch(packages) {
  const res = await fetch(ENDPOINT, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({
      queries: packages.map((p) => ({ package: { name: p.name, ecosystem: ECOSYSTEM }, version: p.version })),
    }),
  });
  if (!res.ok) throw new Error(`OSV 批量查询失败：HTTP ${res.status} ${(await res.text()).slice(0, 200)}`);
  const j = await res.json();
  if (!Array.isArray(j.results)) throw new Error('OSV 的返回里没有 results 数组 —— 这条扫描是空的，不能当"没有漏洞"');
  if (j.results.length !== packages.length) {
    throw new Error(`OSV 返回 ${j.results.length} 条，而我查了 ${packages.length} 个 —— 少掉的那部分没被看过，不能算干净`);
  }
  return j.results;
}

/** OSV 的结果 → {id, rustsec, severity, pkg, summary}。索引对不齐一律硬失败，不悄悄跳过。 */
function toFindings(packages, results) {
  const findings = [];
  results.forEach((r, idx) => {
    const p = packages[idx];
    if (!p) throw new Error(`结果第 ${idx} 项没有对应的查询项 —— 索引错位，这批数不作数`);
    for (const v of r?.vulns ?? []) {
      findings.push({
        id: v.id,
        rustsec: (v.aliases || []).find((a) => a.startsWith('RUSTSEC-')) || null,
        severity: (v.severity || []).map((s) => s.score).join(' / '),
        summary: (v.summary || v.details || '').replace(/\s+/g, ' ').slice(0, 140),
        pkg: `${p.name}@${p.version}`,
      });
    }
  });
  return findings;
}

function uniq(list) {
  const m = new Map();
  for (const x of list) if (!m.has(`${x.pkg}|${x.id}`)) m.set(`${x.pkg}|${x.id}`, x);
  return [...m.values()];
}

async function selfTest() {
  const dirty = [
    { name: 'time', version: '0.1.44' }, // RUSTSEC-2020-0071
    { name: 'atom', version: '0.3.5' }, // RUSTSEC-2020-0044
    { name: 'chrono', version: '0.2.25' }, // RUSTSEC-2020-0159
    // 后三条是 **TLS 那半张图的正向对照**（2026-09-30 现测）：我们查的是 rustls 0.23.45 /
    // rustls-webpki 0.103.15 / ring 0.17.14，而"这几个没通告"有两种可能 —— 真的干净，
    // 或 OSV 压根不收这个包。拿同族的旧版本当对照，它们必须报，才说明这一族的读数是量出来的：
    //   rustls@0.21.5 → RUSTSEC-2024-0336，rustls-webpki@0.101.0 → RUSTSEC-2023-0053（+3 条），
    //   ring@0.16.20 → RUSTSEC-2025-0009 / -0010。
    // 反面要说清：`rcgen`、`tokio-rustls`、`aws-lc-rs`/`aws-lc-sys` 拿不出这种对照 ——
    // 这一族历史上就没发过通告。对它们，"0 条"的意思是"上游没报过"，不是"被证明看过"
    // （边对账只证明**发出去了**）。
    { name: 'rustls', version: '0.21.5' }, // RUSTSEC-2024-0336
    { name: 'rustls-webpki', version: '0.101.0' }, // RUSTSEC-2023-0053 等
    { name: 'ring', version: '0.16.20' }, // RUSTSEC-2025-0009 / -0010
  ];
  // 对照样本自己也得先验一次：第一版拿 `rand@0.9.2` 当"干净"，结果它真的中了一条
  // （RUSTSEC-2026-0097）—— 那是数据不是判据坏，但对照样本就废了。
  const clean = [
    { name: 'serde', version: '1.0.210' },
    { name: 'itoa', version: '1.0.11' },
  ];
  const all = [...dirty, ...clean];
  const findings = uniq(toFindings(all, await queryBatch(all)));
  const dirtySet = new Set(dirty.map((d) => `${d.name}@${d.version}`));
  const cleanSet = new Set(clean.map((c) => `${c.name}@${c.version}`));
  const dirtyHits = findings.filter((f) => dirtySet.has(f.pkg));
  const cleanHits = findings.filter((f) => cleanSet.has(f.pkg));
  console.log(
    `SELF-TEST 已知有漏洞的 ${dirty.length} 个 → 报出 ${dirtyHits.length} 条（该 > 0）：` +
      (dirtyHits.map((x) => `${x.pkg}→${x.rustsec || x.id}`).join('，') || '（空）'),
  );
  console.log(`SELF-TEST 已知干净的 ${clean.length} 个 → 报出 ${cleanHits.length} 条（该 0；报了就是结果按错了索引）`);
  if (dirtyHits.length === 0) {
    console.error('判据是坏的：连已知的漏洞都报不出来，那"0 条"什么都不能说明');
    process.exitCode = 1;
    return;
  }
  if (cleanHits.length > 0) {
    console.error('结果对错了包：干净的那几个也报了 —— 这条扫描的对应关系不可信');
    process.exitCode = 1;
    return;
  }
  console.log('SELF-TEST OK（会红，也不会把干净的算成脏）');
}

async function main() {
  if (process.argv.includes('--self-test')) return selfTest();

  // `--lock` 只为了能把判据自己拿去跑变异（删掉一个块必须红）。默认路径不动。
  const lockAt = process.argv.indexOf('--lock');
  const lockPath = lockAt === -1 ? join(ROOT, 'Cargo.lock') : resolve(process.argv[lockAt + 1] ?? '');
  if (lockAt !== -1 && !lockPath) throw new Error('--lock 后面要跟一个路径');
  const { packages, edges } = parseLock(readFileSync(lockPath, 'utf8'));
  if (packages.length === 0) throw new Error(`${lockPath} 里一个 [[package]] 都没解析出来 —— 这条扫描在空转`);
  const pkgs = [...new Map(packages.map((p) => [`${p.name}@${p.version}`, p])).values()];
  console.log(`· ${lockPath}：${packages.length} 个条目，去重后 ${pkgs.length} 个 name@version，生态标为 ${ECOSYSTEM}`);

  // 先对账再查：覆盖不全时不该花一次网络请求去换一个看起来干净的数。
  const seen = checkEdgeClosure(packages, edges);
  console.log(`· 依赖边 ${edges.length} 条，全部落到被查询的包上（覆盖 ${seen} 个 name@version）`);

  const CHUNK = 400;
  const results = [];
  for (let at = 0; at < pkgs.length; at += CHUNK) {
    const part = pkgs.slice(at, at + CHUNK);
    results.push(...(await queryBatch(part)));
    console.log(`  已查 ${Math.min(at + CHUNK, pkgs.length)}/${pkgs.length}`);
  }

  const findings = uniq(toFindings(pkgs, results));
  const blocked = findings.filter((f) => !WAIVERS[f.pkg]);
  const waived = findings.filter((f) => WAIVERS[f.pkg]);
  const byKey = (a, b) => a.pkg.localeCompare(b.pkg) || a.id.localeCompare(b.id);

  for (const f of [...waived].sort(byKey)) {
    console.log(`  豁免 · ${f.pkg} → ${f.rustsec || f.id}（理由与回看时机在 WAIVERS 与 PRODUCTION-READINESS §7）`);
  }
  if (blocked.length === 0) {
    console.log(
      `PASS · ${pkgs.length} 个依赖里，OSV（含 RustSec）没有报出**未被豁免**的已知漏洞` +
        `（其中豁免 ${waived.length} 条，逐条有理由）。`,
    );
    console.log('  边界：它只看已发布的 Rust 依赖通告，不看我们的代码，也不看构建工具链。');
    return;
  }
  console.log(`FAIL · ${blocked.length} 条命中的漏洞（不在豁免表里）：`);
  for (const f of [...blocked].sort(byKey)) {
    console.log(`  · ${f.pkg} → ${f.rustsec || f.id}${f.severity ? ' [' + f.severity + ']' : ''}${f.summary ? '：' + f.summary : ''}`);
  }
  // 用 exitCode 而不是 process.exit()：在 Windows 的 node 上带未完结句柄硬退会撞到
  // libuv 的内部断言（本机实测 `Assertion failed: !(handle->flags & UV_HANDLE_CLOSING)`），
  // 一个会崩的门禁比不阻断更糟 —— 退码就没法信了。
  process.exitCode = 1;
}

await main();

/**
 * 变异验证记录（2026-09-30，本机 `.logs/audit-mut/`，用 `--lock` 指过去跑的）：
 * 每一条都要"红 + 说清为什么红"，因为边对账本身也要被证明会失败。
 *
 * * 删掉 `rcgen` 整块 → 红：`notera-test-webdav@0.0.46 → rcgen：被引用，但它不在被查询的包里`
 * * 删掉 `aws-lc-sys` 整块 → 红：`aws-lc-rs@1.18.1 → aws-lc-sys：…`
 * * 删掉 `sha2@0.11.0`（同名两版本里的那一个）→ 红，**3 条**落空（三个直接依赖各自引用它）
 * * 把一条边写成 `"base64 0.22.1 extra junk"` → 红：`这条边的写法不在已知的三种里`
 * * 原样拷一份 → 绿（对照组：红是变异造成的，不是脚本坏）
 *
 * 第一版这里翻过一次车，值得留着：name-only 那条分支写的是 `?? null` + `if (target && …)`，
 * 于是"引用了一个没被解析到的包"被当成了没事 —— 前两个变异**笑着跑完全量查询并打印 PASS · 623 个**。
 * 也就是说：守卫差一行的时候，"覆盖不全"和"覆盖全但干净"在输出上长一个样，只有数不一样，
 * 而那正是没人会去对的一个数。改成 `versions.length === 0 ⇒ miss` 之后才红。
 */
