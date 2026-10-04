#!/usr/bin/env node
/**
 * 在**本机**出出包 —— 认平台，只出当前平台能出的那种。
 *
 * 用法：
 *   npm run dist              # 认平台，出默认那一种
 *   npm run dist -- --list    # 只说当前平台会出什么、缺什么前置
 *   npm run dist -- --bundles nsis      # 覆盖该平台的默认打包格式
 *   npm run dist -- --target aarch64    # 覆盖 target
 *   npm run dist -- --debug             # 出 debug 壳（快，不做优化）
 *
 * 为什么要有这个：CI（`.github/workflows/release.yml`）能出三平台，但那要推tag。
 * 「改完想在自己机器上装一下看看」这件事此前只能手抄一长串命令
 * （`RUSTUP_TOOLCHAIN=…-gnu pnpm --dir apps/desktop tauri build --target … --bundles …`），
 * 而那串命令里有两个**不写就会踩**的坑（见下）。本脚本把 CI 里跑通过的形状搬回本机。
 *
 * ## 两个必须知道的坑（都来自 CI-CD.md §环境与网络限制，本机同样成立）
 *
 * 1. **Windows 必须显式钉 `RUSTUP_TOOLCHAIN`**，只给 `--target` 不够：
 *    build script 与 proc macro 永远按 **host triple** 编，而这台机器的 host 是 msvc
 *    ⇒ 它会去找被 Git Bash coreutils 顶掉的 `link.exe`，报一串"第三方 crate 编不过"。
 * 2. **Android 的 NDK**：本机若没有 `ANDROID_HOME`/NDK，脚本会**明说缺什么并退出**，
 *    而不是硬跑一遍然后给你一屏看不懂的 gradle 报错。
 *
 * ## 为什么不做"跨平台打包"
 *
 * macOS 的 `.dmg` 只能在 macOS 上出（需要 hdiutil），Android 的 `.apk` 需要 NDK+JDK。
 * 在 Windows 上强行跑 `tauri build --bundles dmg` 得到的不是 dmg。
 * ⇒ 本脚本只出**本平台能出**的包，并把它说清楚；另两个平台请走 CI（推 tag）。
 */
import { spawnSync } from 'node:child_process';
import { copyFileSync, existsSync, mkdirSync, readdirSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';

// Windows 控制台默认 GBK/936，中文会印成 `鍑哄寘` 这种（实测踩过）。
//
// 真正管用的是**逐字节写 UTF-8**：console.log 在 GBK 控制台上会把中文转成 GBK 再输出，
// 而工具链的读数（路径、报错）常常是 UTF-8，于是两边错位、变成乱码。
// 绕法是自己 write，绕开 console 的编码转换。
const OUT = process.stdout;
const say = (s) => OUT.write(`${s}\n`);
const warn = (s) => OUT.write(`⚠ ${s}\n`);
const fail = (s) => {
  process.stderr.write(`✗ ${s}\n`);
};

const ROOT = new URL('..', import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, '$1');
const FRONTEND = join(ROOT, 'apps', 'desktop');
const MOBILE = join(ROOT, 'apps', 'mobile');

/** 本仓库版本号的权威位置（与 check-versions.mjs 一致）。 */
function readVersion() {
  const cargo = readFileSync(join(ROOT, 'Cargo.toml'), 'utf8');
  return cargo.match(/^\[workspace\.package\][\s\S]*?^version\s*=\s*"([^"]+)"/m)?.[1] ?? '?';
}

const PLATFORMS = {
  win32: {
    label: 'Windows',
    // msi 需要 WiX 工具链（缺 `light.exe` 时会报 LGHT0311，文案里不能有码页装不下的字符），
    // nsis 纯自带。所以默认两个都出，msi 失败会明确报出来而不是悄悄少一个。
    bundles: ['nsis', 'msi'],
    target: 'x86_64-pc-windows-gnu',
    toolchain: 'stable-x86_64-pc-windows-gnu',
    note: '需要 rustup 的 stable-x86_64-pc-windows-gnu（不装会在第2 步报 host 工具链问题）',
  },
  darwin: {
    label: 'macOS',
    bundles: ['dmg', 'app'],
    target: null,
    toolchain: 'stable',
    note: '未签名未公证；首次打开需在系统设置里手动允许',
  },
  linux: {
    label: 'Linux',
    bundles: ['deb'],
    target: null,
    toolchain: 'stable',
    note: 'deb 的产出在 CI 里未验过；本机第一次出可能还要装系统依赖',
  },
};

const argv = process.argv.slice(2);
const flag = (name) => argv.includes(`--${name}`);
const opt = (name) => {
  const i = argv.indexOf(`--${name}`);
  return i >= 0 ? argv[i + 1] : undefined;
};

// ---------------------------------------------------------------- Android ---

/** Android 不在 PLATFORMS 里：它的产物路径、构建目录、工具链都另成一套。 */
function androidPlan() {
  return {
    label: 'Android',
    bundleDir: join(MOBILE, 'src-tauri', 'gen', 'android', 'app', 'build', 'outputs', 'apk'),
    note: '需要 ANDROID_HOME + NDK + JDK 17；出的是 debug 签名 APK（debug keystore，无需证书）',
  };
}

function androidMissing() {
  const missing = [];
  const sdk = process.env.ANDROID_HOME || process.env.ANDROID_SDK_ROOT;
  if (!sdk) {
    missing.push('ANDROID_HOME（未设）');
  } else {
    const ndkRoot = join(sdk, 'ndk');
    if (!existsSync(ndkRoot)) missing.push(`NDK（${ndkRoot} 不存在）`);
  }
  // JDK：java 在 PATH 里就算有。版本要看 17（AGP 8 的硬要求）。
  const jv = spawnSync('java', ['-version'], { encoding: 'utf8' });
  if (jv.error) missing.push('JDK（java 不在 PATH）');
  return missing;
}

/** 移动壳的 Android 也要桌面那份 dist（它的 frontendDist 指向 ../../desktop/dist）。 */
function buildFrontend() {
  say('→ 先出前端产物（移动壳的 frontendDist 指向 apps/desktop/dist）');
  const r = spawnSync(
    join(FRONTEND, 'node_modules', '.bin', process.platform === 'win32' ? 'vite.cmd' : 'vite'),
    ['build'],
    { cwd: FRONTEND, stdio: 'inherit', shell: process.platform === 'win32' },
  );
  if (r.status !== 0) {
    fail('✗ 前端构建失败（这一步不过，后面的壳也编不出来）');
    process.exit(r.status ?? 1);
  }
}

// ------------------------------------------------------------------- 打印 ---

const version = readVersion();
const p = PLATFORMS[process.platform];
const isAndroid = flag('android') || (flag('target') && opt('target')?.startsWith('aarch64'));

if (flag('list')) {
  say(`Noto ${version} · 本机（${process.platform}）能出的包：`);
  if (p) {
    say(`  ${p.label}: --bundles ${p.bundles.join(',')}  —— ${p.note}`);
  }
  const a = androidPlan();
  const miss = androidMissing();
  say(`  ${a.label}: .apk —— ${a.note}`);
  if (miss.length) say(`         ⚠ 当前缺：${miss.join('、')}`);
  else say('         ✓ 前置齐全');
  say('\n出包：npm run dist          （桌面）');
  say('出 APK：npm run dist -- --android');
  process.exit(0);
}

// ---------------------------------------------------------------- Android ---

if (isAndroid) {
  const missing = androidMissing();
  if (missing.length) {
    fail(`✗ 出APK 的前置不齐：${missing.join('、')}`);
    fail('  这些是外部依赖，装好之后重跑同一条命令即可。');
    process.exit(2);
  }
  const cli = join(FRONTEND, 'node_modules', '.bin', process.platform === 'win32' ? 'tauri.cmd' : 'tauri');
  buildFrontend();
  // **工作目录必须在 apps/mobile**：tauri 的 app path 取自启动目录，
  // 留在 apps/desktop 会拿桌面壳去链 aarch64（run #5 的 13 条错误就是这么来的）。
  say('→ android init');
  run(cli, ['android', 'init'], MOBILE);
  say('→ 出 APK（debug 签名）');
  run(cli, ['android', 'build', '--apk', '--debug', '--target', opt('target') || 'aarch64'], MOBILE);
  const dir = androidPlan().bundleDir;
  const apk = listFiles(dir).filter((f) => f.endsWith('.apk'));
  say(apk.length ? `\n✓ 出包：\n${apk.map((f) => '  ' + f).join('\n')}` : `\n✗ 没找到 .apk（找过 ${dir}）`);
  process.exit(apk.length ? 0 : 1);
}

// ------------------------------------------------------------------ 桌面 ---

if (!p) {
  fail(`✗ 不认识本平台 ${process.platform}，且没有 --android。`);
  fail('  已知的出包路径只有 Windows / macOS / Android；别的平台请走 CI。');
  process.exit(2);
}

const bundles = (opt('bundles') || p.bundles.join(',')).split(',').map((s) => s.trim()).filter(Boolean);
const target = opt('target') || p.target;

say(`Noto ${version} · 本机 ${p.label} 出包`);
say(`  bundles: ${bundles.join(',')}`);
if (target) say(`  target : ${target}`);
if (p.note) say(`  说明   : ${p.note}`);

// 桌面端不需要为出包先单独 build —— tauri 的 beforeBuildCommand 会做。
// 但**类型检查**要过（pnpm run build 里有 vue-tsc），所以顺带先跑一次，出错早停。
say('→ 前端类型检查 + 构建');
{
  const tsc = join(FRONTEND, 'node_modules', '.bin', process.platform === 'win32' ? 'vue-tsc.cmd' : 'vue-tsc');
  const r = spawnSync(tsc, ['--noEmit'], { cwd: FRONTEND, stdio: 'inherit', shell: process.platform === 'win32' });
  if (r.status !== 0) {
    fail('✗ 类型检查没过，先修类型再出包（不然装上去是坏的）');
    process.exit(r.status ?? 1);
  }
}

const cli = join(FRONTEND, 'node_modules', '.bin', process.platform === 'win32' ? 'tauri.cmd' : 'tauri');
const args = ['build'];
if (target) args.push('--target', target);
args.push('--bundles', bundles.join(','), '-v');

// ⚠ PATH 分隔符 Windows 是 `;`、POSIX 是 `:`。写错会让补进去的目录直接失效（还不报错）。
const delimiter = process.platform === 'win32' ? ';' : ':';

const env = { ...process.env };
if (p.toolchain) {
  // 坑 1：必须在 env 里，不能只靠 --target。
  env.RUSTUP_TOOLCHAIN = p.toolchain;
  say(`  工具链 : ${p.toolchain}（已钉住，build script 按 host triple 编）`);
}

  // ---- windres（坑 4）----
  // `tauri-winres` → `embed_resource::compile(..).unwrap()` 会去找 **`windres`**（不带前缀），
  // 找不到就 `NotAttempted("windres")` panic，Rust 编译在**链接前**就停。
  // 报错里完全没有"缺 windres"这四个字，只有 `tauri-winres/src/lib.rs:543` 一个 panic。
  //
  // 本机的 windres 有两份（winget 的 WinLibs）：
  //   mingw64\bin\windres.exe                    ← 这个才是 embed_resource 要找的
  //   mingw64\x86_64-w64-mingw32\bin\windres.exe
  // ⚠ 两个都在 PATH 里会造成"看起来在、但 lib 找不到"的错觉，所以只放前者，
  //   LIB 指向 mingw64\lib。
  const windresExe = findWindres();
  if (windresExe) {
    const bin = dirname(windresExe);
    env.PATH = `${bin}${delimiter}${env.PATH ?? ''}`;
    // 链接期要能找到 libkernel32.a 那一组。只加 windres 目录是不够的 ——
    // 那会让 windres 自己能跑，然后死在 ld 找不到库上。
    const libDir = join(dirname(bin), 'lib');
    if (existsSync(libDir)) env.LIB = env.LIB ? `${libDir}${delimiter}${env.LIB}` : libDir;
    say(`  工具链 : windres ← ${bin}`);
  } else if (process.platform === 'win32') {
    warn('  ⚠ 找不到 windres —— Rust 编译会在链接前 panic（tauri-winres 的 lib.rs:543）。');
    warn('     装一个 MinGW：winget install BrechtSanders.WinLibs.POSIX.UCRT');
  }

// 坑 3（本机实测撞到，其实是**两处独立**的问题）：
//
// (a) **node 与 pnpm 都不在 PATH**。`tauri.conf.json` 的 beforeBuildCommand 是 `pnpm build`，
//     而 tauri 用 `cmd /S /C` 起它 —— 继承的是本脚本的 `env`。本机 node 装在
//     `C:\Program Files\nodejs`、pnpm 装在 `%APPDATA%\npm`，两者都不在 Git Bash 的默认 PATH。
//     CI 上它们天然在（setup-node + pnpm/action-setup）⇒ **这个坑只在本机出现**。
//
// (b) **pnpm 的 shim 指向一个无扩展名的路径**。`%APPDATA%\npm\pnpm.cmd` 内部拼出
//     `…\node_modules\pnpm\pnpm`（没有 .cmd/.mjs），cmd 于是报
//     「'…\pnpm' 不是内部或外部命令」。而 pnpm 12.5.1 的真入口是
//     `node_modules/pnpm/bin/pnpm.mjs`，要用 node 跑它。
//
//修法：造一个临时 shim 目录（node.cmd / pnpm.cmd / cargo.cmd 都转发到真入口），
//塞到 PATH 最前面。比"只改 PATH"可靠：cmd 解析 shim 时对"目录里有没有同名可执行文件"很敏感，
//而 `C:\Program Files\nodejs` 这种**带空格**的目录恰恰是它容易出问题的形态。
//
// (c) **cargo 也要补**（实测第二次撞到）：`tauri` 是 node 写的，它起
//     `cargo metadata --no-deps` 来定位 workspace —— 那是**子进程**，
//     继承本脚本 env。本机的 cargo 装在 `~/.cargo/bin`，不在 Git Bash 默认 PATH 里，
//     于是报「failed to run 'cargo metadata' ... program not found」。
//     ⚠ 这个错误出现在**出包最开始**，很容易被读成"环境坏了/项目坏了"。
//     我第一版 shim 只放了 node/pnpm/windres，漏了它。
const shimDir = ensureShims();
env.PATH = `${shimDir}${delimiter}${env.PATH ?? ''}`;

/**
 * 造临时 shim 目录（node.cmd / pnpm.cmd），返回其路径。
 *
 * 判据一律用"**试着跑起来**"，不用 PATH 字符串匹配：Windows 的 PATH 里那个路径
 * 可能大写、可能是 `C:\...` 也可能是 `/c/...`，字符串匹配会漏
 *（第一版就是这么写的，结果该补的时候没补、不该补的时候乱补）。
 */
function ensureShims() {
  const dir = join(ROOT, 'target', '.dist-shims');
  mkdirSync(dir, { recursive: true });

  // ---- node ----
  let nodeExe = [
    'C:/Program Files/nodejs/node.exe',
    '/c/Program Files/nodejs/node.exe',
  ].find((x) => existsSync(x));
  if (!nodeExe) {
    const r = spawnSync('where', ['node'], { encoding: 'utf8' });
    nodeExe = (r.stdout || '').split(/\r?\n/).map((s) => s.trim()).find((s) => s.endsWith('node.exe')) || null;
  }
  if (!nodeExe) {
    fail('找不到 node —— 出不了包。先装 Node 20+，或把 node 放进 PATH。');
    process.exit(2);
  }
  // cmd 的 shim：**必须**用 CRLF。写成 LF 时 cmd 会把下一行当命令的一部分，
  // 表现是"第一行能跑、参数全丢"（实测踩过）。
  writeFileSync(join(dir, 'node.cmd'), `@ECHO off\r\n"${nodeExe}" %*\r\n`, 'utf8');

  // ---- pnpm ----
  const pnpmMjs = findPnpmMjs();
  if (pnpmMjs) {
    writeFileSync(join(dir, 'pnpm.cmd'), `@ECHO off\r\n"${nodeExe}" "${pnpmMjs}" %*\r\n`, 'utf8');
    say(`  shim   : node ← ${nodeExe}`);
    say(`  shim   : pnpm ← ${pnpmMjs}`);
  } else {
    // 退路：项目里可能自带（pnpm/action-setup 装过依赖的话）
    const local = join(FRONTEND, 'node_modules', '.bin', 'pnpm.cmd');
    if (existsSync(local)) {
      writeFileSync(join(dir, 'pnpm.cmd'), `@ECHO off\r\ncall "${local}" %*\r\n`, 'utf8');
      say(`  shim   : node ← ${nodeExe}；pnpm ← 项目 node_modules`);
    } else {
      warn('  ⚠ 找不到可用的 pnpm。');
      warn('     beforeBuildCommand 是 `pnpm build`，会红在出包第一步。装一个：npm i -g pnpm');
    }
  }

  // ---- windres（坑 4）----
  // `tauri-winres` 的 build script 会调 `windres` 编 Windows 资源（图标/版本/清单），
  // 找不到就 `NotAttempted("windres")` panic，**整个 Rust 编译在链接前就停**。
  // 本机的 windres 来自 WinLibs（winget 装的 MinGW），装在：
  //   %LOCALAPPDATA%\Microsoft\WinGet\Packages\<pkg>\mingw64\bin\windres.exe
  // 它不在 PATH ⇒ 要么手工加，要么像这里这样找出来并塞进 shim 目录。
  const windres = findWindres();
  if (windres) {
    writeFileSync(join(dir, 'windres.cmd'), `@ECHO off\r\n"${windres}" %*\r\n`, 'utf8');
    // tauri-winres 找的是 `x86_64-w64-mingw32-windres`（带前缀那个名）
    writeFileSync(join(dir, 'x86_64-w64-mingw32-windres.cmd'), `@ECHO off\r\n"${windres}" %*\r\n`, 'utf8');
    say(`  shim   : windres ← ${windres}`);
  } else {
    warn('  ⚠ 找不到 windres —— Rust 编译会在链接前 panic（tauri-winres）。');
    warn('     装一个 MinGW：winget install BrechtSanders.WinLibs.POSIX.UCRT');
  }

  // ---- cargo（坑 c）----
  // `tauri` 起`cargo metadata --no-deps` 来定位 workspace。
  //
  // ⚠⚠ 这里**不能只放 `cargo.cmd`**：tauri 用 Rust 的 `Command::new("cargo")`
  //   **不带 shell** 去 spawn。Windows 上 `CreateProcess` 不会把 `foo.cmd`
  //   当可执行文件解析（cmd 的 PATHEXT 只对**交互式/cmd 内部**的查找生效），
  //   于是症状是 `program not found` —— **哪怕 `cargo.cmd` 就在 PATH 最前面**。
  //   （我第一版就只放了 `.cmd`，shim 明明打出来了还是报找不到。）
  //⇒ 放进 shim 目录的必须是**真的 `cargo.exe`**。
  const cargoExe = findCargo();
  if (cargoExe) {
    const link = join(dir, 'cargo.exe');
    try {
      // copyFile 不行（shim 目录在 target/ 下，删 target 会连带删掉），
      // 硬链接最省事；跨卷时退回copy。
      copyFileSync(cargoExe, link);
    } catch {
      try {
        copyFileSync(cargoExe, link);
      } catch {
        warn(`  ⚠ cargo.exe 复制不到 shim 目录（${cargoExe}），仍可能失败。`);
      }
    }
    if (existsSync(link)) {
      say(`  shim   : cargo ← ${cargoExe}`);
    } else {
      // 退路：把 cargo 所在目录直接加进 PATH（对不带 shell 的 spawn 同样有效）
      env.PATH = `${dirname(cargoExe)}${delimiter}${env.PATH ?? ''}`;
      say(`  PATH   : 已补入 cargo 所在目录（${dirname(cargoExe)}）`);
    }
  } else {
    warn('  ⚠ 找不到 cargo —— tauri 会在出包第一步就失败（cargo metadata 找不到程序）。');
  }

  return dir;
}

/** 找 cargo.exe（rustup 装在 `~/.cargo/bin`）。 */
function findCargo() {
  const candidates = [
    join(process.env.USERPROFILE || '', '.cargo', 'bin', 'cargo.exe'),
    '/c/Users/lhcz-fu/.cargo/bin/cargo.exe',
    '/usr/local/cargo/bin/cargo.exe',
  ];
  const found = candidates.find((p) => existsSync(p));
  if (found) return found;
  // PATH 里本来就有就不必造 shim
  const r = spawnSync('where', ['cargo'], { encoding: 'utf8' });
  return (r.stdout || '').split(/\r?\n/).map((s) => s.trim()).find(Boolean) || null;
}

/**
 * 找 `windres.exe`。
 *
 * ⚠ 命中多个时**必须优先不带前缀的那个**（`mingw64\bin\windres.exe`），
 * 而不是 `mingw64\x86_64-w64-mingw32\bin\windres.exe`：
 * `embed_resource` 找的是 `windres`，而带前缀那份在 `x86_64-w64-mingw32` 子目录里 ——
 * 选错的话 PATH 里有它、却仍然找不到可用的库，表现为"补了 windres 还是红"。
 */
function findWindres() {
  const roots = [
    join(process.env.LOCALAPPDATA || '', 'Microsoft', 'WinGet', 'Packages'),
    'C:/msys64/mingw64/bin',
    'C:/mingw64/bin',
  ];
  const plain = [];
  const prefixed = [];
  for (const root of roots) {
    if (!existsSync(root)) continue;
    const stack = [root];
    let seen = 0;
    while (stack.length && seen < 500) {
      const d = stack.pop();
      for (const name of safeReaddir(d)) {
        const full = join(d, name);
        if (name === 'windres.exe' || name === 'windres') {
          if (full.includes('x86_64-w64-mingw32')) prefixed.push(full);
          else plain.push(full);
        } else if (isDir(full)) {
          stack.push(full);
        }
      }
      seen++;
    }
    if (plain.length || prefixed.length) break;
  }
  return plain[0] || prefixed[0] || null;
}

/** 在 pnpm store 里逐层找 `node_modules/pnpm/bin/pnpm.mjs`（版本与 hash 各一层）。 */
function findPnpmMjs() {
  const roots = [
    'D:/.pnpm-store/v11/links/@/pnpm',
    join(process.env.USERPROFILE || '', '.pnpm-store'),
  ];
  for (const root of roots) {
    if (!existsSync(root)) continue;
    for (const ver of safeReaddir(root)) {
      const vdir = join(root, ver);
      if (!isDir(vdir)) continue;
      for (const hash of safeReaddir(vdir)) {
        const c = join(vdir, hash, 'node_modules', 'pnpm', 'bin', 'pnpm.mjs');
        if (existsSync(c)) return c;
      }
    }
  }
  return null;
}

function safeReaddir(d) {
  try { return readdirSync(d); } catch { return []; }
}

function isDir(p) {
  try { return statSync(p).isDirectory(); } catch { return false; }
}

say('\n→ tauri build（会比较慢，链接期尤其慢）');
const r = spawnSync(cli, args, { cwd: FRONTEND, stdio: ["inherit","inherit","inherit"], env, shell: process.platform === 'win32' });
if (r.status !== 0) {
  fail('\n✗ 出包失败。上面最后 30 行是真正的读数 —— 往上翻的多数是无关的依赖编译日志。');
  process.exit(r.status ?? 1);
}

const out = findArtifacts(p, bundles, target);
say(out.length ? `\n✓ 出包完成：\n${out.map((f) => '  ' + f).join('\n')}` : '\n✗ 打包命令过了，但没找到产物（bundle 目录形状与预期不符）');
process.exit(out.length ? 0 : 1);

// ------------------------------------------------------------------ 工具 ---

function run(cmd, args, cwd) {
  const r = spawnSync(cmd, args, { cwd, stdio: 'inherit', shell: process.platform === 'win32' });
  if (r.status !== 0) process.exit(r.status ?? 1);
}

/** 在 tauri 的 bundle 目录里找刚出的产物。目录形状随 target 变，所以两条路径都看。 */
function findArtifacts(plan, kinds, target) {
  const roots = target
    ? [join(ROOT, 'target', target, 'release', 'bundle')]
    : [join(ROOT, 'target', 'release', 'bundle')];
  const hits = [];
  for (const r of roots) {
    if (!existsSync(r)) continue;
    for (const file of listFiles(r)) {
      if (kinds.some((k) => file.endsWith('.' + k)) && statSync(file).mtimeMs > Date.now() - 3 * 3600_000) {
        hits.push(file);
      }
    }
  }
  return hits;
}

function listFiles(dir) {
  const out = [];
  if (!existsSync(dir)) return out;
  for (const name of readdirSync(dir)) {
    const full = join(dir, name);
    try {
      if (statSync(full).isDirectory()) out.push(...listFiles(full));
      else out.push(full);
    } catch {
      /* 读不到就跳过 */
    }
  }
  return out;
}
