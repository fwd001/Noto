# CI 里"结构校验这两个安装包"这一步的**本体**（YAML 里只留一行调用）。
# 为什么要放进仓库文件而不是写在 workflow 的 run: 里：Windows runner 上 GH 是把 run 体写成一个
# 无 BOM 的临时 .ps1 再喂给 PowerShell 的，中文串在 Windows PowerShell 5.1 那条路上会被当 ANSI 读，
# 解析器直接报"字符串缺少终止符"（本机复演就是这么暴露的）。仓库里这份带 BOM，两边都读得对。
# 用法（CI 与本机同一条命令）：
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts/verify-windows-package.ps1 `
#     -BundleDir <bundle 根> -Version <版本> -TempDir <临时目录> [-StepSummary <摘要文件>]
param(
    [Parameter(Mandatory = $true)][string]$BundleDir,
    [Parameter(Mandatory = $true)][string]$Version,
    [Parameter(Mandatory = $true)][string]$TempDir,
    [string]$StepSummary = ''
)

$ErrorActionPreference = 'Stop'

function Resolve-Single($pattern, $what) {
    $all = @(Get-ChildItem -Path $pattern -ErrorAction SilentlyContinue)
    if ($all.Count -eq 0) { throw "$what 没找到（$pattern）—— 出包那步没产出这个东西" }
    # **按版本名挑，不"取第一个"**：本机复演时 bundle 目录里留着 0.0.28 到 0.0.41 一整排，
    # `Select -First 1` 拿到的是**最旧**那个 —— 于是"校验过了"校验的是上一个包。
    # 那正是 §45 要防的形状（绿的是别的东西）。挑不到带这个版本名的就直接红。
    $hit = $all | Where-Object { $_.Name -like "*$Version*" } | Select-Object -First 1
    if (-not $hit) {
        throw "$pattern 下有 $($all.Count) 个候选，但没有一个文件名里带着 $Version —— 不能拿别的版本冒充这一版"
    }
    return $hit.FullName
}

$msi = Resolve-Single (Join-Path $BundleDir 'msi/*.msi') 'MSI 安装包'
$nsis = Resolve-Single (Join-Path $BundleDir 'nsis/*.exe') 'NSIS 安装包'

$report = Join-Path $TempDir 'win-report.txt'
$outDir = Join-Path $TempDir 'notera-winx'
$here = Split-Path -Parent $MyInvocation.MyCommand.Path

& (Join-Path $here 'inspect-windows-package.ps1') -Msi $msi -Nsis $nsis -OutDir $outDir -Report $report
# `$LASTEXITCODE` 在 `& script.ps1` 之后**不代表那个脚本**（它只由原生命令设置），
# 本机就是这么把一个永远为假的判断写进 CI 的。判据换成"报告真在、且有内容"。
if (-not (Test-Path $report)) { throw "生成器没写出报告文件：$report" }
if ((Get-Item $report).Length -lt 40) { throw "报告文件只有 $((Get-Item $report).Length) 字节，不像一次真解包" }

Write-Output '--- 现场报告 ---'
Get-Content $report

node (Join-Path $here 'check-windows-package.mjs') $report --version $Version
if ($LASTEXITCODE -ne 0) { throw "Windows 产物结构校验没过（退出 $LASTEXITCODE）" }

if ($StepSummary -ne '') {
    $lines = @(
        '### Windows 产物核对（§4）'
        "- ``$(Split-Path -Leaf $msi)`` 与 ``$(Split-Path -Leaf $nsis)``"
        '- msiexec /a 解包：退出 0，解出 notera-desktop.exe 与 WebView2Loader.dll'
    )
    $lines += Get-Content $report | Where-Object { $_ -match '^(DESKTOP_|NSIS_|MSI_BYTES|WEBVIEW2_BYTES|EXTRACTED)' } | ForEach-Object { "- $_" }
    Add-Content -Path $StepSummary -Value ($lines -join "`n")
}
Write-Output "verify-windows-package: OK（$Version）"
