# 把 Windows 安装包"拆开读一遍"，输出成一份 KEY=VALUE 的现场报告。
# §4 那句"各平台包可正常安装、启动"里，不依赖用户手动装就能核的部分：
#   .msi 能用 msiexec /a（管理员解包，不写注册表、不装系统）解出来吗？
#   解出来的主程序与 WebView2 的加载器在不在？包里的版本号是不是这一版？
# 断言部分在 scripts/check-windows-package.mjs（CI 与本机跑的是同一份生成器，避免两边漂移）。
param(
    [Parameter(Mandatory = $true)][string]$Msi,
    [Parameter(Mandatory = $true)][string]$Nsis,
    [Parameter(Mandatory = $true)][string]$OutDir,
    [Parameter(Mandatory = $true)][string]$Report
)

$ErrorActionPreference = 'Stop'
$lines = @()

function Add-Line($k, $v) { $script:lines += "$k=$v" }

Add-Line 'MSI' (Split-Path -Leaf $Msi)
Add-Line 'MSI_BYTES' ((Get-Item $Msi).Length)
Add-Line 'NSIS' (Split-Path -Leaf $Nsis)
Add-Line 'NSIS_BYTES' ((Get-Item $Nsis).Length)

# NSIS 安装包自己带 VERSIONINFO，直接读
$nv = (Get-Item $Nsis).VersionInfo
Add-Line 'NSIS_FILEVERSION' $nv.FileVersion
Add-Line 'NSIS_PRODUCTVERSION' $nv.ProductVersion

# .msi 走解包：TARGETDIR 是我们给的临时目录，/a 不碰系统也不写注册表
if (Test-Path $OutDir) { Remove-Item -Recurse -Force $OutDir }
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$proc = Start-Process msiexec.exe -ArgumentList '/a', "`"$Msi`"", '/qn', "TARGETDIR=`"$OutDir`"" -Wait -PassThru
Add-Line 'MSIEXEC_EXIT' $proc.ExitCode
Add-Line 'EXTRACTED' $(if ($proc.ExitCode -eq 0 -and (Test-Path (Join-Path $OutDir 'PFiles'))) { '1' } else { '0' })

# 解出来的目录形状按**内容**找，不按固定深度找：PFiles 的层级是 WiX 的事，写死了就是给未来埋雷
$desktop = Get-ChildItem -Recurse -Path $OutDir -Filter 'notera-desktop.exe' -File -ErrorAction SilentlyContinue | Select-Object -First 1
$loader = Get-ChildItem -Recurse -Path $OutDir -Filter 'WebView2Loader.dll' -File -ErrorAction SilentlyContinue | Select-Object -First 1
Add-Line 'HAS_NOTERA_DESKTOP' $(if ($desktop) { '1' } else { '0' })
Add-Line 'HAS_WEBVIEW2_LOADER' $(if ($loader) { '1' } else { '0' })
if ($desktop) {
    Add-Line 'DESKTOP_BYTES' $desktop.Length
    $dv = $desktop.VersionInfo
    Add-Line 'DESKTOP_FILEVERSION' $dv.FileVersion
    Add-Line 'DESKTOP_PRODUCTVERSION' $dv.ProductVersion
}
if ($loader) { Add-Line 'WEBVIEW2_BYTES' $loader.Length }

Set-Content -Path $Report -Value ($lines -join [Environment]::NewLine) -Encoding utf8
$lines | ForEach-Object { Write-Output $_ }
