#!/usr/bin/env node
// §4 那句"各平台包可正常安装、启动"在 Android 这一腿上的**结构校验**（CI-CD.md L6 承诺的那一条）：
// 真机安装要用户的设备（§49），但"这个 APK 到底是什么"可以不依赖设备读出来。
// 读的是 `aapt dump badging <apk>` 的原文 —— 断的是四件事：包名、versionName、有没有打进 native 库、
// 有没有可启动的 Activity。
// 为什么这四件都要断：run #13 之前量到过 `tauri.conf.json` 不写 version 时 **Android 会退回 1.0**
// （tauri-utils 的原文行为），而 native 库缺失（`libnotera_mobile_lib.so` 没进去）的 APK
// 装上也会一开就崩 —— 那两种形状在"job 绿了"里都看不出来。
import { readFileSync } from 'node:fs';

const argv = process.argv.slice(2);
const arg = (name, fallback = null) => {
  const i = argv.indexOf(name);
  return i >= 0 && argv[i + 1] !== undefined ? argv[i + 1] : fallback;
};
const file = argv.find((a) => !a.startsWith('--'));
const expect = {
  version: arg('--version'),
  package: arg('--package', 'app.notera'),
  abi: arg('--abi', 'arm64-v8a'),
  minSdk: arg('--min-sdk', '24'),
};
if (!file || !expect.version) {
  console.error('用法：check-apk-badging.mjs <badging.txt> --version <版本> [--package …] [--abi …] [--min-sdk …]');
  process.exit(2);
}

let text;
try {
  text = readFileSync(file, 'utf8');
} catch (e) {
  console.error(`读不到 badging 输出：${e.message}`);
  process.exit(2);
}
// aapt 空输出（APK 根本不存在时 `aapt dump` 也可能只回一行警告）不能当"通过"
if (text.trim().length < 20) {
  console.error(`badging 输出只有 ${text.trim().length} 字节，不像一份 APK 的读数`);
  process.exit(1);
}

const failures = [];
const notes = [];

const pkg = /^package: name='([^']+)' versionCode='([^']*)' versionName='([^']*)'/m.exec(text);
if (!pkg) {
  failures.push('badging 里没有 `package: name=… versionCode=… versionName=…` 这一行');
} else {
  const [, name, code, version] = pkg;
  notes.push(`package=${name} versionCode=${code} versionName=${version}`);
  if (name !== expect.package) failures.push(`包名是 ${name}，期望 ${expect.package}`);
  if (version !== expect.version) {
    failures.push(`versionName 是 ${version}，期望 ${expect.version}（Android 上不写 version 会退回 1.0）`);
  }
  if (!/^\d+$/.test(code) || code === '0') failures.push(`versionCode 不是正整数：${code}`);
}

const sdk = /^sdkVersion:'(\d+)'/m.exec(text);
if (!sdk) failures.push("badging 里没有 `sdkVersion:'…'`");
else if (sdk[1] !== expect.minSdk) failures.push(`minSdk 是 ${sdk[1]}，期望 ${expect.minSdk}`);

const native = /^native-code:\s*(.+)$/m.exec(text);
if (!native) {
  failures.push(`badging 里没有 \`native-code:\` —— ${expect.abi} 的 Rust 库根本没打进这个 APK`);
} else if (!native[1].includes(expect.abi)) {
  failures.push(`native-code 是 ${native[1].trim()}，不含 ${expect.abi}`);
} else {
  notes.push(`native-code=${native[1].trim()}`);
}

const launch = /^launchable-activity: name='([^']+)'/m.exec(text);
if (!launch) failures.push("badging 里没有 `launchable-activity:` —— 装上了也没有能启动的入口");
else notes.push(`launchable=${launch[1]}`);

for (const line of notes) console.log(`PASS  ${line}`);
if (failures.length > 0) {
  for (const f of failures) console.error(`FAIL  ${f}`);
  console.error(`check-apk-badging: ${failures.length} 条不过`);
  process.exit(1);
}
console.log(`check-apk-badging: 四项全过（${expect.package} / ${expect.version} / ${expect.abi} / minSdk ${expect.minSdk}）`);
