#!/usr/bin/env node
// Windows 安装包的结构校验断言 —— 读的是 scripts/inspect-windows-package.ps1 生成的 KEY=VALUE 现场报告。
// 断这四件：`.msi` 真能被 msiexec 解出来（这是"安装"这一步不写注册表、不碰系统的等价动作）、
// 解出来的包里有主程序 `notera-desktop.exe`、有 `WebView2Loader.dll`（缺了它壳在没装 Runtime 的机器上
// 起不来，而那台机器有没有装过 WebView2 不是我们能假设的），以及**包里的版本号就是这一版**
// —— 文件名字里写着 0.0.41 不代表打进去的二进制也是 0.0.41，那是两种不同的错。
import { readFileSync } from 'node:fs';

const argv = process.argv.slice(2);
const arg = (name, fallback = null) => {
  const i = argv.indexOf(name);
  return i >= 0 && argv[i + 1] !== undefined ? argv[i + 1] : fallback;
};
const file = argv.find((a) => !a.startsWith('--'));
const version = arg('--version');
if (!file || !version) {
  console.error('用法：check-windows-package.mjs <报告.txt> --version <版本>');
  process.exit(2);
}
let text;
try {
  text = readFileSync(file, 'utf8');
} catch (e) {
  console.error(`读不到现场报告：${e.message}`);
  process.exit(2);
}
const report = {};
for (const line of text.split(/\r?\n/)) {
  const i = line.indexOf('=');
  if (i > 0) report[line.slice(0, i).trim()] = line.slice(i + 1).trim();
}
if (Object.keys(report).length === 0) {
  console.error('现场报告里一个 KEY=VALUE 都没有 —— 生成器八成没跑成');
  process.exit(1);
}

const failures = [];
const notes = [];
const yes = (key, what) => {
  if (report[key] !== '1') failures.push(`${what}（${key}=${report[key] ?? '缺'}）`);
  else notes.push(`${key}=1`);
};

// 报告必须是"**这一版**的包"的读数：挑文件那一步如果拿错了（bundle 目录里常年留着历史包，
// `Select -First 1` 拿到的是最旧那个），后面所有断言都在给旧包作保。文件名里就得有这个版本号。
for (const key of ['MSI', 'NSIS']) {
  const name = report[key] ?? '';
  if (!name) failures.push(`${key} 没写是哪个文件`);
  else if (!name.includes(version)) failures.push(`${key} = ${name}，文件名里没有 ${version} —— 校验的不是这一版`);
  else notes.push(`${key}=${name}`);
}

if (!/^\d+$/.test(report.MSIEXEC_EXIT ?? '') || report.MSIEXEC_EXIT !== '0') {
  failures.push(`msiexec /a 解包退出码 = ${report.MSIEXEC_EXIT ?? '缺'}（0 才算解得开）`);
} else {
  notes.push(`MSIEXEC_EXIT=0`);
}
yes('EXTRACTED', '解出来的目录里没有 PFiles，等于包打不开');
yes('HAS_NOTERA_DESKTOP', '解出来的包里没有主程序 notera-desktop.exe');
yes('HAS_WEBVIEW2_LOADER', '解出来的包里没有 WebView2Loader.dll（没装 Runtime 的机器上壳起不来）');

for (const key of ['DESKTOP_FILEVERSION', 'DESKTOP_PRODUCTVERSION', 'NSIS_FILEVERSION', 'NSIS_PRODUCTVERSION']) {
  const v = report[key];
  if (v === undefined || v === '') {
    failures.push(`${key} 读不到 —— 那个文件没有版本信息？`);
  } else if (!v.startsWith(version)) {
    failures.push(`${key} = ${v}，期望以 ${version} 开头`);
  } else {
    notes.push(`${key}=${v}`);
  }
}

// 空包/截断包也会"解得开"，所以体积要看得见：0 字节肯定不对
for (const key of ['MSI_BYTES', 'NSIS_BYTES', 'DESKTOP_BYTES']) {
  const n = Number(report[key]);
  if (!Number.isFinite(n) || n < 1024 * 1024) failures.push(`${key} = ${report[key] ?? '缺'}，小于 1 MiB 不像一份真产物`);
  else notes.push(`${key}=${n}`);
}

for (const line of notes) console.log(`PASS  ${line}`);
if (failures.length > 0) {
  for (const f of failures) console.error(`FAIL  ${f}`);
  console.error(`check-windows-package: ${failures.length} 条不过`);
  process.exit(1);
}
console.log(`check-windows-package: 全过（解包 ✓ 主程序 ✓ WebView2 加载器 ✓ 版本 ${version} ✓ 体积 ✓）`);
