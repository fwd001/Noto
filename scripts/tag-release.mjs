#!/usr/bin/env node
/**
 * 打发布 tag 的唯一入口（§5：正式版本靠 Git Tag + GitHub Release 管理）。
 *
 * 用法：
 *   node scripts/tag-release.mjs            # 只检查 + 在本地建 annotated tag
 *   node scripts/tag-release.mjs --push     # 检查 + 建 tag + 把 tag 推出去（触发 release.yml 出包）
 *   node scripts/tag-release.mjs --check    # 只做检查，不建 tag
 *
 * 为什么要有这个脚本，而不是手打 `git tag -a`：`release.yml` 的第一个 job 会拿
 * **Cargo.toml 的版本**去比 tag 名，不一致就直接失败 —— 那时构建已经排队、runner 已经起来，
 * 报错来得很晚。这里把同样的三条判据在**本地**先跑一遍，便宜且立刻：
 *   ① 版本单源一致（复用 check-versions.mjs，权威 = 根 Cargo.toml）；
 *   ② CHANGELOG 里有标着「版本 … → <version>」的那一条 —— §5 要求发布时同步更新"更新说明"，
 *      Release 正文就是从 CHANGELOG 那一块抽的，抽不到就是缺项（Release 里会显式写"缺项"）；
 *   ③ 工作树干净（tag 要指着一个"就是这些内容"的提交，指着带未提交改动的工作区等于自欺）。
 * 第 ② 条与 release.yml 里那段 awk 用**同一个判据**（`index($0, "→ " v)`），
 * 不是两套写法 —— 两边不一致的话，本地过了 CI 还是抽不到正文。
 */
import { readFileSync, existsSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { join } from 'node:path';

const ROOT = new URL('..', import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, '$1');
const args = process.argv.slice(2);
const doPush = args.includes('--push');
const checkOnly = args.includes('--check');

const fail = (msg) => {
  console.error(`tag-release: ${msg}`);
  process.exit(1);
};

const git = (...a) =>
  execFileSync('git', a, { cwd: ROOT, encoding: 'utf8' }).trim();

const text = readFileSync(join(ROOT, 'Cargo.toml'), 'utf8');
const version = text.match(/^\[workspace\.package\][\s\S]*?^version\s*=\s*"([^"]+)"/m)?.[1];
if (!version) fail('Cargo.toml 里找不到 [workspace.package] version');

// ① 版本单源。直接把判定权交给那个脚本：它自己会比对三处派生位置 + Cargo.lock。
try {
  execFileSync(process.execPath, [join(ROOT, 'scripts/check-versions.mjs')], {
    cwd: ROOT,
    stdio: 'pipe',
    encoding: 'utf8',
  });
} catch (e) {
  fail(
    `版本单源不一致，先修再打 tag：\n${String(e.stdout ?? '') + String(e.stderr ?? '')}`.trimEnd(),
  );
}

// ② CHANGELOG 有这一版的条目（判据与 release.yml 里那段 awk 同源）。
const changelog = existsSync(join(ROOT, 'CHANGELOG.md'))
  ? readFileSync(join(ROOT, 'CHANGELOG.md'), 'utf8')
  : '';
const marker = `→ ${version}`;
const entry = changelog.split('\n').filter((l) => l.startsWith('- **') && l.includes(marker));
if (entry.length === 0) {
  fail(
    `CHANGELOG.md 里没有标着「版本 … → ${version}」的条目。` +
      '§5 要的是"版本号、构建产物、更新说明"三样一起走；补完那一条再打 tag' +
      '（纯文档/测试批次不升 patch，本来就没有这一条 —— 那就说明这个版本不该被发布）',
  );
}
if (entry.length > 1) {
  fail(`「版本 … → ${version}」在 CHANGELOG 里出现 ${entry.length} 次，Release 正文会抽错那一条`);
}

// ③ 工作树干净。
const dirty = git('status', '--porcelain=v1');
if (dirty) fail(`工作树有未提交改动，tag 要指着"就是这些内容"的提交：\n${dirty}`);

const sha = git('rev-parse', 'HEAD');
const tagName = `v${version}`;
if (git('tag', '-l', tagName)) fail(`tag ${tagName} 已存在（要重发得先决定是删旧 tag 还是升版本）`);

console.log(`检查通过：版本 ${version}，HEAD ${sha.slice(0, 7)}，Release 正文取自：`);
console.log(`  ${entry[0].slice(0, 160)}`);
if (checkOnly) {
  console.log('tag-release: --check，没建 tag');
  process.exit(0);
}

execFileSync('git', ['tag', '-a', tagName, '-m', `Noto ${version}\n\n由 release.yml 出包（Windows / macOS；tag 推送触发）`], {
  cwd: ROOT,
  stdio: 'inherit',
});
console.log(`已建本地 tag ${tagName}`);
if (doPush) {
  execFileSync('git', ['push', 'origin', tagName], { cwd: ROOT, stdio: 'inherit' });
  console.log(`已推送 ${tagName} → GitHub Actions 的「出包与发布」会被触发（Release 建成草稿）`);
} else {
  console.log('（没推。要触发 CI 出包：node scripts/tag-release.mjs --push）');
}
