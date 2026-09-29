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
 * 豁免表 `WAIVERS` 的规矩：**每一条必须写"为什么豁免"与"什么时候回头看"**，不许写"暂时忽略"
 * （§39）。它只把"已经判定过、而我们这一层修不动"的那几条从阻断里拿掉，新冒出来的照样红 ——
 * 豁免是把噪音关掉，不是把判据关掉。每条在 `docs/PRODUCTION-READINESS.md` §7 都有对应的一格。
 *
 * 跑法：
 *   node scripts/audit-rust-deps.mjs               # 审 Cargo.lock 的全部包（CI 里也是这条）
 *   node scripts/audit-rust-deps.mjs --self-test    # 只验判据本身
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

/** 解析 `Cargo.lock` 的 `[[package]]` 块 —— 用 lockfile 而不是 `cargo tree`：不跑 cargo、不装工具。 */
export function parseLock(text) {
  const out = [];
  for (const block of text.split('[[package]]').slice(1)) {
    const name = /^name = "([^"]+)"/m.exec(block)?.[1];
    const version = /^version = "([^"]+)"/m.exec(block)?.[1];
    if (name && version) out.push({ name, version });
  }
  return out;
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

  const packages = parseLock(readFileSync(join(ROOT, 'Cargo.lock'), 'utf8'));
  if (packages.length === 0) throw new Error('Cargo.lock 里一个 [[package]] 都没解析出来 —— 这条扫描在空转');
  const pkgs = [...new Map(packages.map((p) => [`${p.name}@${p.version}`, p])).values()];
  console.log(`· Cargo.lock：${packages.length} 个条目，去重后 ${pkgs.length} 个 name@version，生态标为 ${ECOSYSTEM}`);

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
