#!/usr/bin/env node
// 把一段诊断文本贴成 **commit 评论**。为什么走这条路：本仓库的 CI 读数在没有 GitHub 令牌时
// 只有两条通道 —— 未认证能读 commit 评论列表（实测 200，全文可读），也能读产物**名字**；
// 但 job 日志正文 = 403、产物 zip 的字节 = 401（这两条是本批实测的，不是推测）。
// 于是"失败时把现场贴成评论"是唯一能把整段读数递回来的形状。
// 端点形状也踩过坑：创建评论是 `POST /repos/{owner}/{repo}/commits/{sha}/comments`，
// 写成 `/comments/{sha}` 是"改/删某条评论"那条路，会 4xx。
//
// **这是临时工装**：出包流水线绿了之后，这条与 release.yml 里那两处调用一起删掉。
import { readFileSync } from 'node:fs';

const argv = process.argv.slice(2);
const arg = (name) => {
  const i = argv.indexOf(name);
  return i >= 0 ? argv[i + 1] : undefined;
};
const file = arg('--file');
const label = arg('--label') ?? 'CI 现场（诊断贴文，修好就删）';
const token = process.env.GITHUB_TOKEN;
const repo = process.env.GITHUB_REPOSITORY;
const sha = process.env.GITHUB_SHA;

const die = (code, msg) => {
  console.error(`post-ci-diagnostic: ${msg}`);
  process.exit(code);
};
if (!file) die(2, '缺 --file');
if (!token) die(2, '环境里没有 GITHUB_TOKEN（这条通道依赖 job 里的 contents: write）');
if (!repo || !sha) die(2, `缺 GITHUB_REPOSITORY/GITHUB_SHA：repo=${repo} sha=${sha}`);

let body;
try {
  body = readFileSync(file, 'utf8');
} catch (e) {
  die(2, `读不到 ${file}：${e.message}`);
}
// GitHub 对 comment body 有 65536 字节的硬上限，超了是 422 —— 宁可少递一段，也别整条通道失败。
const MAX = 60000;
const text = `### ${label}\n\n\`\`\`\n${body.slice(0, MAX)}${body.length > MAX ? '\n…（截断，全文见 job 日志）' : ''}\n\`\`\``;

const url = `https://api.github.com/repos/${repo}/commits/${sha}/comments`;
let res;
try {
  res = await fetch(url, {
    method: 'POST',
    headers: {
      authorization: `Bearer ${token}`,
      accept: 'application/vnd.github+json',
      'content-type': 'application/json',
      'x-github-api-version': '2022-11-28',
    },
    body: JSON.stringify({ body: text }),
  });
} catch (e) {
  // 网络层坏了也要留一条能读的读数，而不是一坨 uncaught TypeError（这条通道存在的意义就是"读得到"）
  die(1, `POST 没发出去：${e.message}（cause=${e.cause?.message ?? '无'}）`);
}
const json = await res.text();
console.log(`HTTP=${res.status} 贴了 ${text.length} 字到 ${url}`);
// 失败要响：这条通道上一次"报 success 而实际什么都没递出去"（URL 写错 + `|| echo` 吞掉退出码），
// 递证据的步骤自己假绿比没有证据更糟。
if (!res.ok || !json.includes('"html_url"')) {
  console.error(json.slice(0, 500));
  die(1, '评论没建起来（上面是响应）');
}
console.log('评论已建好：', /"html_url":\s*"([^"]+)"/.exec(json)?.[1] ?? '(读不到链接)');
