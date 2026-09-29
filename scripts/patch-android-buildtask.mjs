#!/usr/bin/env node
// Android 出包时，tauri 生成的 gradle 任务 `:app:rustBuild*` 会**回头再叫一次 tauri CLI**
// （读的是 buildSrc/src/main/kotlin/BuildTask.kt：`executable = """{{tauri-binary}}"""` +
// `args = listOf({{tauri-binary-args}})`，并且 `workingDir = File(projectDir, rootDirRel)`
// = `<app>/src-tauri`）。CLI 记进去的那串是**启动它时所在目录的相对路径**
// （run #13 实测：记成裸 `tauri` ⇒ node 在 src-tauri 里找 `src-tauri/tauri` ⇒ MODULE_NOT_FOUND）。
// 这里把可执行文件与第一个参数钉成**绝对路径**，让那次回头调用与启动目录无关。
import { readFileSync, writeFileSync } from 'node:fs';
import { isAbsolute } from 'node:path';

const [target, cliEntry] = process.argv.slice(2);
if (!target || !cliEntry) {
  console.error('用法：patch-android-buildtask.mjs <BuildTask.kt> <tauri.js 绝对路径>');
  process.exit(2);
}
if (!isAbsolute(cliEntry)) {
  console.error(`CLI 入口必须是绝对路径，给的是 ${cliEntry}`);
  process.exit(2);
}

const source = readFileSync(target, 'utf8');
const patched = source
  .replace(/val executable = """.*?"""/, 'val executable = """node"""')
  .replace(/val args = listOf\(([^)]*)\)/, (_match, inner) => {
    const tokens = inner.split(',').map((token) => token.trim()).filter(Boolean);
    if (tokens.length === 0) throw new Error('args 列表是空的，补丁不能默默通过');
    if (!/^".*"$/.test(tokens[0])) {
      throw new Error(`第一个 token 不是带引号的字符串，模板形状和预期不同：${tokens[0]}`);
    }
    tokens[0] = JSON.stringify(cliEntry);
    return `val args = listOf(${tokens.join(', ')})`;
  });

if (patched === source) {
  console.error(`没改动任何字节 —— ${target} 里没有预期的 executable/args 两行`);
  process.exit(1);
}
if (!patched.includes('val executable = """node"""') || !patched.includes(JSON.stringify(cliEntry))) {
  console.error('补丁后的文件缺关键内容');
  process.exit(1);
}
writeFileSync(target, patched);
console.log(`已钉死 BuildTask：node ${cliEntry}`);
