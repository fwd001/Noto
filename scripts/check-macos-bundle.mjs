#!/usr/bin/env node
// §4 "各平台包可正常安装、启动"在 macOS 这一腿上的结构校验（CI-CD.md L6 承诺的那一条）。
// 读的是 CI 里 `hdiutil attach` 挂载 .dmg 之后、对 bundle 的 `plutil -p Info.plist` 与 `file` 的原文。
// 断的是四件 + 一件：版本号、bundle id、可执行文件名字、`CFBundlePackageType == APPL`
// （不是 helper/插件那种装上了没有主程序的壳），以及那个可执行文件**真的是 Mach-O**。
// 为什么值得单独断：`.dmg` 与 `.app` 都在"打包成功"里长得一模一样，
// 而 versionName 退回默认值 / 打进去的是别的 target 的壳，只有读 bundle 自己才看得见。
import { readFileSync } from 'node:fs';

const argv = process.argv.slice(2);
const arg = (name, fallback = null) => {
  const i = argv.indexOf(name);
  return i >= 0 && argv[i + 1] !== undefined ? argv[i + 1] : fallback;
};
const plistFile = argv.find((a) => !a.startsWith('--') && !a.startsWith('-'));
const version = arg('--version');
const identifier = arg('--identifier', 'app.notera');
const executable = arg('--executable', 'notera-desktop');
const fileOut = arg('--file');
if (!plistFile || !version || !fileOut) {
  console.error('用法：check-macos-bundle.mjs <plutil.txt> --version <版本> --file <file(1) 输出> [--identifier …] [--executable …]');
  console.error('（--file 是必需项：少了它 = "主程序其实不是可执行文件"这一格没人看，那种缺口不能靠调用方记得传）');
  process.exit(2);
}

const read = (f, what) => {
  try {
    return readFileSync(f, 'utf8');
  } catch (e) {
    console.error(`读不到${what}：${e.message}`);
    process.exit(2);
  }
};
const plist = read(plistFile, ' Info.plist 的 plutil 输出');
const fileText = read(fileOut, ' file(1) 的输出');
if (plist.trim().length < 20) {
  console.error('plutil 输出短得不像一份 Info.plist');
  process.exit(1);
}

const failures = [];
const notes = [];
const field = (key) => {
  const m = new RegExp(`"${key}"\\s*=>\\s*"([^"]*)"`).exec(plist);
  return m ? m[1] : null;
};

for (const [key, expected, label] of [
  ['CFBundleShortVersionString', version, '版本'],
  ['CFBundleIdentifier', identifier, 'bundle id'],
  ['CFBundleExecutable', executable, '主程序名'],
  ['CFBundlePackageType', 'APPL', '包类型'],
]) {
  const actual = field(key);
  if (actual === null) {
    failures.push(`Info.plist 里没有 ${key} —— ${label}这一格没被声明`);
  } else if (actual !== expected) {
    failures.push(`${key} = ${actual}，期望 ${expected}`);
  } else {
    notes.push(`${key}=${actual}`);
  }
}

if (!/Mach-O/.test(fileText)) {
  failures.push(`可执行文件的 file(1) 读数里没有 Mach-O：${fileText.trim().slice(0, 120)}`);
} else {
  notes.push(`macho=${fileText.trim().slice(-40)}`);
}

for (const line of notes) console.log(`PASS  ${line}`);
if (failures.length > 0) {
  for (const f of failures) console.error(`FAIL  ${f}`);
  console.error(`check-macos-bundle: ${failures.length} 条不过`);
  process.exit(1);
}
console.log(`check-macos-bundle: 全过（${identifier} / ${version} / APPL / Mach-O）`);
