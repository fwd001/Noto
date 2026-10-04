
### 记忆文件被 PowerShell 截断过一次（已修复）

用 `Add-Content -NoNewline` 往记忆文件追加，**把整个文件截断了**
（382 行 ← 原本 661 行，第八轮整段丢失，尾部还进了GBK 乱码）。

原因：`Add-Content -NoNewline` 在这个环境下不是"追加"而是"重写"。

⇒ **写记忆文件一律用 node 的 `fs.appendFileSync`**，不要用 PowerShell 的 `Add-Content`。
发现后靠「找首个 `�` 并回退到换行」修回，**好在那些内容在会话上下文里还在**。
⇒ 这类文件**不在 git 里**（`git status` 查不到），所以截断了没法 `git checkout` 回来。
