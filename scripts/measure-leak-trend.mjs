/**
 * 常驻循环的泄漏趋势量具（§48 缺口 G10 后半 / PERF-10 那条"30 min × 5 轮趋势"）。
 *
 * 它做三件事：
 *   1. 编译 `crates/notera-host/tests/leak_trend.rs`（生产那两条常驻循环的长跑夹具）并拿到测试可执行文件；
 *   2. 直接 spawn 那个 exe（**不经 cargo**，这样 child.pid 就是被测进程，RSS 才读得对）；
 *   3. 每 `--sample-secs` 秒用 `Get-Process` 采一次工作集，同时收 Rust 那边打的 `LEAK-SAMPLE` 行
 *      （notes / inflight_ops / db_bytes），收尾把 30 分钟切成 5 个窗口报均值与斜率。
 *
 * 为什么 RSS 在外面读而不是在 Rust 里读：读别人不占被测进程的堆，也不会因为"为了量它而多做一件事"
 * 把趋势弄脏；而且 `verify-perf` 那三档 RSS 用的就是同一个 `Get-Process` 口径，两批数能对着看。
 *
 * 判据口径（**这是我今天定的暂定值，不是用户拍过的**）：30 分钟里「最后一个窗口均值 − 第一个窗口均值」
 * ≤ `--max-growth-mib`（默认 8 MiB）。选它是因为常驻循环每轮都在重建 DTO / 待发队列，真在漏的话
 * 半小时不该只有个位数 MiB 的漂移；一旦红，按 §40 记成缺陷而不是"再跑一次看运气"。
 *
 * 跑法：
 *   node scripts/measure-leak-trend.mjs                 # 默认 30 min，每 30 s 采一点
 *   node scripts/measure-leak-trend.mjs --minutes 2 --sample-secs 20   # 冒烟跑（先证明量具本身没坏）
 *   node scripts/measure-leak-trend.mjs --self-test     # 只验趋势判据：合成序列该红 / 该绿
 */
import { spawn, spawnSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = resolve(fileURLToPath(new URL('.', import.meta.url)), '..');

/**
 * 把"等退出"变成显式判据而不是一个同步读字段：采样窗口结束时孩子可能还差几行没打完，
 * 那时读 `child.exitCode` 会拿到 null —— 我第一版就是这么写出过一个假红（数明明没漏，
 * 却因为收尾没等而被判成"测试没绿"）。等满 `graceSecs` 还不退才是真问题。
 */
function waitExit(child, graceSecs) {
  if (child.exitCode !== null) return Promise.resolve(child.exitCode);
  return new Promise((res) => {
    const to = setTimeout(() => res('TIMEOUT'), graceSecs * 1000);
    child.once('close', (c) => {
      clearTimeout(to);
      res(c);
    });
  });
}

const arg = (name, dflt) => {
  const i = process.argv.indexOf(`--${name}`);
  if (i === -1) return dflt;
  const v = Number.parseFloat(process.argv[i + 1]);
  if (!Number.isFinite(v) || v <= 0) throw new Error(`--${name} 要一个正数，收到 ${process.argv[i + 1]}`);
  return v;
};

/** RSS（工作集，MiB）—— 与 verify-perf 同一个来源。 */
const workingSetMiB = (pid) => {
  const r = spawnSync(
    'powershell',
    ['-NoProfile', '-Command', `(Get-Process -Id ${pid}).WorkingSet64 / 1MB`],
    { encoding: 'utf8' },
  );
  const n = Number.parseFloat((r.stdout || '').trim().split(/\r?\n/).pop());
  return Number.isFinite(n) ? Math.round(n * 10) / 10 : NaN;
};

/**
 * 把一串样本切成 k 个连续窗口，报每窗均值 + 最小二乘斜率（MiB/分钟）+ 判据。
 * 单独抽出来是为了能喂合成序列自测（见 --self-test）。
 */
export function trend(samples, k = 5) {
  const vals = samples.map((s) => s.rssMiB).filter((v) => Number.isFinite(v));
  if (vals.length < k * 2) {
    return { verdict: 'INVALID', reason: `样本只有 ${vals.length} 个点，不够切 ${k} 个窗口` };
  }
  const per = Math.floor(vals.length / k);
  const means = [];
  for (let w = 0; w < k; w += 1) {
    const slice = vals.slice(w * per, w === k - 1 ? vals.length : (w + 1) * per);
    means.push(Math.round((slice.reduce((a, b) => a + b, 0) / slice.length) * 10) / 10);
  }
  // 最小二乘：x 用分钟数（样本间隔换算）
  const dtMin = (samples[samples.length - 1].tSec - samples[0].tSec) / 60 / (samples.length - 1);
  const n = vals.length;
  const xs = vals.map((_, i) => i * dtMin);
  const mx = xs.reduce((a, b) => a + b, 0) / n;
  const my = vals.reduce((a, b) => a + b, 0) / n;
  let num = 0;
  let den = 0;
  for (let i = 0; i < n; i += 1) {
    num += (xs[i] - mx) * (vals[i] - my);
    den += (xs[i] - mx) ** 2;
  }
  const slope = den === 0 ? 0 : num / den;
  const growth = Math.round((means[k - 1] - means[0]) * 10) / 10;
  return { means, growth, slopeMiBperMin: Math.round(slope * 1000) / 1000, points: vals.length, verdict: growth };
}

/** 判据：末窗减首窗的漂移 ≤ 上界。 */
export function judge(t, maxGrowthMiB) {
  if (t.verdict === 'INVALID') return { ok: false, reason: t.reason };
  return {
    ok: t.growth <= maxGrowthMiB,
    reason: `末窗 ${t.means[t.means.length - 1]} MiB − 首窗 ${t.means[0]} MiB = ${t.growth} MiB（上界 ${maxGrowthMiB}）；斜率 ${t.slopeMiBperMin} MiB/min`,
  };
}

/**
 * 判"有没有收敛"比判"涨了多少"更要紧：一条线性、末段斜率不降的上涨，即使总量很小，
 * 也不能写成"没有泄漏"。这里把首 10 / 中 10 / 末 10 的均值与**末 20 点的斜率**一起报出来，
 * 末段斜率不显著小于全程斜率 = 没收敛。
 */
export function segments(samples) {
  const v = samples.map((s) => s.rssMiB).filter((x) => Number.isFinite(x));
  const mean = (a) => Math.round((a.reduce((x, y) => x + y, 0) / a.length) * 100) / 100;
  const slope = (a, stepMin) => {
    const n = a.length;
    const xs = a.map((_, i) => i * stepMin);
    const mx = xs.reduce((x, y) => x + y, 0) / n;
    const my = a.reduce((x, y) => x + y, 0) / n;
    let num = 0;
    let den = 0;
    for (let i = 0; i < n; i += 1) {
      num += (xs[i] - mx) * (a[i] - my);
      den += (xs[i] - mx) ** 2;
    }
    return den === 0 ? 0 : Math.round((num / den) * 1000) / 1000;
  };
  const stepMin = (v.length > 1 ? (samples.at(-1).tSec - samples[0].tSec) / 60 / (samples.length - 1) : 0) || 0.5;
  if (v.length < 30) return null;
  return {
    first10: mean(v.slice(0, 10)),
    mid10: mean(v.slice(Math.floor(v.length / 2) - 5, Math.floor(v.length / 2) + 5)),
    last10: mean(v.slice(-10)),
    lastSlope: slope(v.slice(-20), stepMin),
  };
}

function selfTest() {
  const flat = Array.from({ length: 30 }, (_, i) => ({ tSec: i * 60, rssMiB: 40 }));
  const leak = Array.from({ length: 30 }, (_, i) => ({ tSec: i * 60, rssMiB: 40 + i * 1.5 }));
  const a = judge(trend(flat), 8);
  const b = judge(trend(leak), 8);
  const short = judge(trend(flat.slice(0, 6)), 8);
  console.log(`SELF-TEST 平的序列 → ok=${a.ok}（该 true）· ${a.reason}`);
  console.log(`SELF-TEST 每分钟漏 1.5 MiB → ok=${b.ok}（该 false）· ${b.reason}`);
  console.log(`SELF-TEST 样本不够 → ok=${short.ok}（该 false）· ${short.reason}`);
  // 第二段判据：这条趋势线"收没收敛"也要两向能红。
  const warm = Array.from({ length: 60 }, (_, i) => ({ tSec: i * 30, rssMiB: i < 15 ? 24 + i * 0.4 : 30 }));
  const lin = Array.from({ length: 60 }, (_, i) => ({ tSec: i * 30, rssMiB: 24 + i * 0.03 }));
  const sw = segments(warm);
  const sl = segments(lin);
  const wholeW = trend(warm).slopeMiBperMin;
  const wholeL = trend(lin).slopeMiBperMin;
  console.log(`SELF-TEST 预热后走平：全程 ${wholeW} → 末段 ${sw.lastSlope}（该显著小于全程）`);
  console.log(`SELF-TEST 一直线性涨：全程 ${wholeL} → 末段 ${sl.lastSlope}（该与全程同量级）`);
  if (!(sw.lastSlope < wholeW * 0.7) || !(sl.lastSlope >= wholeL * 0.7)) {
    console.error('收敛判据是坏的：它分不出"预热完走平"和"一直在涨"');
    process.exit(1);
  }
  if (!a.ok || b.ok || short.ok) {
    console.error('判据自己就是坏的：它分不清"没漏"和"在漏"，别信它的数');
    process.exit(1);
  }
  console.log('SELF-TEST OK（判据能红也能绿）');
}

/** 从 `cargo test --no-run --message-format json` 的输出里挑出这个集成测试的 exe。 */
function findTestExe(jsonl, testName) {
  const found = [];
  for (const line of jsonl.split(/\r?\n/)) {
    if (!line.startsWith('{')) continue;
    let m;
    try {
      m = JSON.parse(line);
    } catch {
      continue;
    }
    if (m.reason !== 'compiler-artifact' || !m.executable) continue;
    const kinds = m.target?.kind || [];
    if (!kinds.includes('test')) continue;
    if (!String(m.executable).includes(testName)) continue;
    found.push(m.executable);
  }
  return found;
}

async function main() {
  if (process.argv.includes('--self-test')) return selfTest();

  const minutes = arg('minutes', 30);
  const sampleSecs = arg('sample-secs', 30);
  const maxGrowth = arg('max-growth-mib', 8);
  const testName = 'leak_trend';

  console.log(`· 编译测试可执行（debug profile：泄漏趋势看的是分配行为，不是优化后的手感）`);
  const build = spawnSync(
    'cargo',
    [
      '+stable-x86_64-pc-windows-gnu',
      'test',
      '--manifest-path',
      join(ROOT, 'Cargo.toml'),
      '-p',
      'notera-host',
      '--test',
      testName,
      '--no-run',
      '--message-format',
      'json',
    ],
    { encoding: 'utf8', maxBuffer: 64 * 1024 * 1024 },
  );
  if (build.status !== 0) {
    console.error(`编译失败（退出 ${build.status}）：\n${build.stderr?.slice(-4000) || build.stdout?.slice(-4000)}`);
    process.exit(1);
  }
  const exes = [...new Set(findTestExe(build.stdout || '', testName))].filter(existsSync);
  if (exes.length === 0) {
    // 这里必须红，不能"那就手动指一个路径"：找不到 exe 意味着下面读到的 RSS 是别人的进程。
    console.error(`没从 cargo 的输出里找到 ${testName} 的测试可执行 —— 量具不知道该 spawn 谁`);
    process.exit(1);
  }
  const exe = exes.sort().pop();
  console.log(`· 被测进程 = ${exe}`);
  console.log(`· 时长 ${minutes} min，每 ${sampleSecs} s 采一点，上界 ${maxGrowth} MiB（末窗−首窗）`);

  const child = spawn(exe, [`--exact`, 'resident_loops_leak_trend_run', '--ignored', '--nocapture'], {
    env: {
      ...process.env,
      NOTERA_LEAK_MINUTES: String(minutes),
      NOTERA_LEAK_SAMPLE_SECS: String(sampleSecs),
      NOTERA_LEAK_IDLE: process.argv.includes('--idle') ? '1' : '0',
    },
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  console.log(`· 形态：${process.argv.includes('--idle') ? '空转（只有两条常驻循环在跑，不做用户动作）' : '带用户动作（每 ' + sampleSecs + ' s 建/读/改/列表/搜一轮）'}`);
  const samples = [];
  const lines = [];
  let buf = '';
  child.stdout.on('data', (chunk) => {
    buf += chunk.toString('utf8');
    let nl;
    while ((nl = buf.indexOf('\n')) >= 0) {
      const line = buf.slice(0, nl).trim();
      buf = buf.slice(nl + 1);
      if (!line) continue;
      lines.push(line);
      if (line.startsWith('LEAK-SAMPLE')) console.log(`  ${line}`);
      else console.log(`  · ${line}`);
    }
  });
  child.stderr.on('data', (c) => {
    const s = c.toString('utf8');
    if (/\berror\[|panicked/.test(s)) console.log(`  ! ${s.trim().slice(0, 400)}`);
  });

  const started = Date.now();
  const t0 = performance.now() / 1000;
  // 先等它把测试跑起来（造库要几秒），再开始采第一点。
  await new Promise((r) => setTimeout(r, 5000));
  while (Date.now() - started < minutes * 60_000) {
    if (child.exitCode !== null) break;
    samples.push({ tSec: (performance.now() / 1000 - t0).toFixed(0) * 1, rssMiB: workingSetMiB(child.pid) });
    if (!Number.isFinite(samples.at(-1).rssMiB)) {
      console.error(`第 ${samples.length} 点读不到 RSS（pid ${child.pid}）—— 被测进程是不是已经退了？别把空读数当没漏`);
      child.kill();
      process.exit(1);
    }
    await new Promise((r) => setTimeout(r, sampleSecs * 1000));
  }

  const code = await waitExit(child, 120);
  if (code === 'TIMEOUT') {
    console.error('被测进程在采样窗口结束后 120 s 还没退 —— 长跑没收尾，这一批 RSS 不作数');
    child.kill();
    process.exit(1);
  }
  const tail = lines.filter((l) => l.startsWith('LEAK-')).length;
  console.log(`\n· 被测进程退出码 ${code}；Rust 侧打了 ${tail} 行 LEAK-* 账；本机采到 ${samples.length} 个 RSS 点`);
  if (code !== 0) {
    console.error(`测试没绿（退出码 ${code}）—— 泄漏趋势的数不作数：中间某一步红了`);
    console.error(lines.slice(-12).join('\n'));
    process.exit(1);
  }
  if (tail < 2) {
    // 一条 LEAK-SAMPLE 都没有 = 测试压根没跑（过滤器写错 / --ignored 没给），那 RSS 是空转读到的数
    console.error('Rust 侧没有 LEAK-SAMPLE 行：长跑压根没跑，这一批 RSS 不作数');
    process.exit(1);
  }

  const notes = lines.filter((l) => l.startsWith('LEAK-SAMPLE')).map((l) => /notes=(\d+)/.exec(l)?.[1]).filter(Boolean);
  const db = lines.filter((l) => l.startsWith('LEAK-SAMPLE')).map((l) => /db_bytes=(\d+)/.exec(l)?.[1]).filter(Boolean);
  const inflight = lines
    .filter((l) => l.startsWith('LEAK-SAMPLE'))
    .map((l) => /inflight_ops=(\d+)/.exec(l)?.[1])
    .filter(Boolean);
  console.log(`· 账上：笔记 ${notes[0]} → ${notes.at(-1)}；待发操作 ${inflight[0]} → ${inflight.at(-1)}；库字节 ${db[0]} → ${db.at(-1)}`);
  console.log(`· RSS 逐点：${samples.map((s) => s.rssMiB).join(', ')} MiB`);

  const t = trend(samples);
  console.log(`· 五窗均值：${JSON.stringify(t.means ?? null)} 增长 ${t.growth ?? '—'} MiB 斜率 ${t.slopeMiBperMin ?? '—'} MiB/min`);
  const seg = segments(samples);
  if (seg) {
    console.log(
      `· 收没收敛：首 10 点均值 ${seg.first10} / 中段 ${seg.mid10} / 末 10 点均值 ${seg.last10} MiB；` +
        `末 20 点斜率 ${seg.lastSlope} MiB/min（全程 ${t.slopeMiBperMin}）⇒ ` +
        `${seg.lastSlope >= t.slopeMiBperMin * 0.7 ? '末段没降，**不能写成"没有泄漏"**，只能说涨速在这个窗口内很小' : '末段明显放缓（更像工作集预热而不是每轮残留）'}`,
    );
  }
  const v = judge(t, maxGrowth);
  console.log(`${v.ok ? 'PASS' : 'FAIL'} · ${v.reason}`);
  console.log(
    `边界（照实说）：这一格只覆盖宿主进程（SQLite 句柄 / DTO / 待发队列 / 两条循环的中间产物）；` +
      `WebView2 那一块要按 verify-perf 的壳口径量，真 WebDAV 服务器的行为差异不在这条路上。`,
  );
  process.exit(v.ok ? 0 : 1);
}

await main();
