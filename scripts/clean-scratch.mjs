#!/usr/bin/env node
/**
 * 本地工装残骸的清理与预算看守（缺口 G97）。
 *
 * 为什么要它：验证类工装各自往 `.logs/` 与 `.notera-dev/` 里写东西，而"用完就删"没人执行 ——
 * 一次 `verify-perf` 的 20000 条夹具就是 43 MB 的 sqlite，换一个 `RUN_TAG` 又多一份；
 * 实测 `.logs/` 涨到 **197 MB / 1155 个文件**。git 里看不见（`.gitignore` 早就挡了），
 * 所以它红不了任何门禁 —— 只能由这个脚本按预算砍。
 *
 *   node scripts/clean-scratch.mjs              # 清理并打印
 *   node scripts/clean-scratch.mjs --check      # 只报不删，超预算 exit 1（CI / 门禁那一头用这个）
 *   node scripts/clean-scratch.mjs --self-test  # "何时该删"那套数学的三臂自检
 *
 * 三条口径写死在这里，改口径就改这一处：
 *  · **过期的就删**：超过 `logsKeepDays` 的删掉；`perf-*` 夹具目录只等 `perfKeepDays`
 *    （一次 20000 条的实验就是 43 MB，按三天等不起）；
 *  · **预算优先**：总量超 `logsMaxMb` 就从最旧的接着砍，砍到预算内为止 ——
 *    光按天数砍挡不住"一次实验造出 1 GB"那一形；
 *  · **现场保护**：最近 `logsKeepNewest` 项在预算那一轮不许动（正在排查这一轮的东西不是垃圾），
 *    砍不动就报 leftover，不假装成功。
 * EPERM（Windows 上文件还被进程握着）不当失败：重试两次，剩下的一律记成 leftover 报出来，
 * 由下一次运行接着收 —— 上一版 `verify-layout` 就是在这儿抛出去把整条门禁打断的。
 */
import { readdirSync, statSync, rmSync, existsSync } from 'node:fs';
import { join } from 'node:path';

const ROOT = new URL('..', import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, '$1');
const DAY = 86400000;

export const BUDGET = {
  logsDir: '.logs',
  logsMaxMb: 60,
  logsKeepDays: 3,
  logsKeepNewest: 40,
  perfKeepDays: 1,
  backupsDir: '.notera-dev/backups',
  backupsKeep: 1,
};

/**
 * 纯函数：什么时候删。输入是"一项东西"的读数（路径、字节、age 天数、是不是 perf 夹具目录），
 * 输出是该删的路径与删完剩下的字节。
 * 做成纯函数是为了能被合成序列自测 —— 判据若只能拿真磁盘跑，"0 个问题"与"扫描压根没跑"就分不开。
 */
export function planPrune(entries, b = BUDGET) {
  const maxBytes = b.logsMaxMb * 1048576;
  const sorted = [...entries].sort((x, y) => x.ageDays - y.ageDays);
  const doomed = new Set();
  let bytes = entries.reduce((a, e) => a + e.bytes, 0);
  // 第一轮：过期。 newest-N 那条地板只管预算那一轮 —— 放了三天的一篇日志不是"现场"。
  for (const e of sorted) {
    if (e.perf ? e.ageDays > b.perfKeepDays : e.ageDays > b.logsKeepDays) { doomed.add(e.path); bytes -= e.bytes; }
  }
  // 第二轮：预算。从**最旧**的开始砍到预算内，但最近 `logsKeepNewest` 项不动（现场保护）。
  // 注意 `sorted` 是"新→旧"，所以这一轮要反着走 —— 上一版顺着走，砍的是刚写的那一份。
  const floor = new Set(sorted.slice(0, b.logsKeepNewest).map((e) => e.path));
  for (const e of [...sorted].reverse()) {
    if (bytes <= maxBytes) break;
    if (floor.has(e.path) || doomed.has(e.path)) continue;
    doomed.add(e.path); bytes -= e.bytes;
  }
  return { doomed: [...doomed], remainingBytes: bytes };
}

function stat(path) {
  try { return statSync(path); } catch { return null; }
}

function entriesOf(dir, nowMs) {
  if (!existsSync(dir)) return [];
  const out = [];
  for (const name of readdirSync(dir)) {
    const full = join(dir, name);
    const s = stat(full);
    if (!s) continue;
    out.push({
      path: full,
      name,
      bytes: s.isDirectory() ? dirBytes(full) : s.size,
      ageDays: (nowMs - s.mtimeMs) / DAY,
      perf: /^perf-/.test(name),
    });
  }
  return out;
}

function dirBytes(dir) {
  let sum = 0;
  for (const name of readdirSync(dir)) {
    const full = join(dir, name);
    const s = stat(full);
    if (!s) continue;
    sum += s.isDirectory() ? dirBytes(full) : s.size;
  }
  return sum;
}

function removeTolerant(path, leftovers) {
  for (let attempt = 0; attempt < 3; attempt += 1) {
    try { rmSync(path, { recursive: true, force: true }); return; } catch (error) {
      if (attempt === 2) leftovers.push(`${path} —— ${error.code ?? error.message}`);
      else Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 120);
    }
  }
}

/** 干活的那一半：算出该删的、删掉（EPERM 容忍）、回报。`dryRun` 只算不删。 */
export function sweep(b = BUDGET, { dryRun = false, nowMs = Date.now() } = {}) {
  const logs = join(ROOT, b.logsDir);
  const items = entriesOf(logs, nowMs);
  const plan = planPrune(items, b);
  const mb = (n) => (n / 1048576).toFixed(1);
  const report = {
    files: items.length,
    beforeMb: mb(items.reduce((a, e) => a + e.bytes, 0)),
    doomed: plan.doomed,
    afterMb: mb(plan.remainingBytes),
    overBudget: plan.remainingBytes > b.logsMaxMb * 1048576,
    leftovers: [],
    backupsRemoved: 0,
  };
  const leftovers = report.leftovers;
  for (const p of plan.doomed) if (!dryRun) removeTolerant(p, leftovers);

  const backups = join(ROOT, b.backupsDir);
  if (existsSync(backups)) {
    const rows = readdirSync(backups)
      .map((name) => ({ name, s: stat(join(backups, name)) }))
      .filter((r) => r.s && r.s.isFile())
      .sort((x, y) => y.s.mtimeMs - x.s.mtimeMs);
    for (const r of rows.slice(b.backupsKeep)) {
      if (!dryRun) removeTolerant(join(backups, r.name), leftovers);
      report.backupsRemoved += 1;
    }
  }
  return report;
}

function selfTest() {
  const b = { ...BUDGET, logsKeepNewest: 2, logsKeepDays: 3, perfKeepDays: 1, logsMaxMb: 10 };
  const MB = 1048576;
  const cases = [
    {
      name: '放了九天的该删，昨天的留着',
      in: [{ path: 'a', bytes: MB, ageDays: 0 }, { path: 'b', bytes: MB, ageDays: 1 }, { path: 'c', bytes: MB, ageDays: 9 }],
      want: ['c'],
    },
    {
      name: 'perf 夹具目录过一天就砍（一次实验 43 MB，按三天等不起）；同齡的普通日志不砍',
      in: [{ path: 'p', bytes: 43 * MB, ageDays: 1.5, perf: true }, { path: 'q', bytes: MB, ageDays: 1.5 }],
      want: ['p'],
    },
    {
      name: '都年轻但总量超预算 ⇒ 从**最旧**的开始砍到预算内，最近那两项不动',
      in: [{ path: 'a', bytes: 4 * MB, ageDays: 0 }, { path: 'b', bytes: 4 * MB, ageDays: 0.2 },
        { path: 'c', bytes: 4 * MB, ageDays: 0.4 }, { path: 'd', bytes: 4 * MB, ageDays: 0.6 }],
      want: ['d', 'c'],
    },
    {
      name: '全是刚写的（都在现场保护那一档里）⇒ 一条都不许砍，差额由调用方报 leftover',
      in: [{ path: 'a', bytes: 40 * MB, ageDays: 0 }, { path: 'b', bytes: 40 * MB, ageDays: 0.1 }],
      want: [],
    },
    { name: '空输入不该编出删除项', in: [], want: [] },
  ];
  let bad = 0;
  for (const c of cases) {
    const got = planPrune(c.in, b).doomed;
    const ok = JSON.stringify(got) === JSON.stringify(c.want);
    if (!ok) bad += 1;
    console.log(`${ok ? 'ok  ' : 'FAIL'} ${c.name} ⇒ 该删 ${JSON.stringify(c.want)}，实际 ${JSON.stringify(got)}`);
  }
  console.log(bad === 0 ? '五臂全过（这条看守自己是活的）' : `${bad} 臂不符 —— 判据坏了，别看磁盘结论`);
  return bad === 0 ? 0 : 1;
}

// 只有被当命令跑才动手；被 import（arch-check 那条规则要的是纯函数）时不碰磁盘、不 exit。
const isCli = (process.argv[1] ?? '').replace(/\\/g, '/').endsWith('scripts/clean-scratch.mjs');
if (isCli) {
  const arg = process.argv[2] ?? '';
  if (arg === '--self-test') process.exit(selfTest());
  const r = sweep(BUDGET, { dryRun: arg === '--check' });
  console.log(`${arg === '--check' ? '预算检查' : '清理'}：${r.files} 项 / ${r.beforeMb} MB ⇒ 删 ${r.doomed.length} 项（备份清掉 ${r.backupsRemoved} 份）⇒ 剩 ${r.afterMb} MB`);
  for (const l of r.leftovers) console.log(`  带不走（下次再收）：${l}`);
  if (r.overBudget) console.log(`  ⚠ 砍完仍超预算（还剩 ${r.afterMb} MB > ${BUDGET.logsMaxMb} MB）—— 只剩"最近 ${BUDGET.logsKeepNewest} 项"那一形，请人工看`);
  process.exit(arg === '--check' && (r.overBudget || r.doomed.length > 0) ? 1 : 0);
}
