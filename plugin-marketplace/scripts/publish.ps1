#Requires -Version 5.1
<#
.SYNOPSIS
  打包插件 zip + 算 SHA256 + 更新 registry.json

.DESCRIPTION
  Hardware Workbench 插件市场发布脚本。

  把指定插件源码目录打包成 <plugin-id>-<version>.zip，计算 SHA256，
  复制 plugin.json 到版本目录，并在 registry.json 里新增/替换该插件条目。

  zip 内部结构：以插件目录名为根（解压后直接得到 <plugin-dir>/plugin.json 等），
  与应用端 discover_roots 扫描的目录结构一致。

.PARAMETER PluginId
  插件 id（必须与 plugin.json 里的 id 一致）

.PARAMETER Version
  要发布的版本号（必须与 plugin.json 里的 version 一致）

.PARAMETER SourcePath
  插件源码目录路径（包含 plugin.json 的目录）。默认使用仓库根目录下的
  plugins/<PluginId>。

.PARAMETER MarketplaceRoot
  市场仓库根目录路径（含 registry.json）。默认为脚本所在目录的上一级。

.PARAMETER Owner
  GitHub 用户名/组织名，用于拼 download_url。默认 "Tydwdh"。

.PARAMETER Repo
  GitHub 仓库名。默认 "serial_tool"。

.PARAMETER MarketplacePath
  市场在仓库内的相对路径。默认 "plugin-marketplace"。

.PARAMETER Branch
  市场仓库默认分支。默认 "main"。

.EXAMPLE
  .\plugin-marketplace\scripts\publish.ps1 `
    -PluginId gcode-sender -Version 0.4.0
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$PluginId,

    [Parameter(Mandatory = $true)]
    [string]$Version,

    [string]$SourcePath,

    [string]$MarketplaceRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path,

    [string]$Owner = 'Tydwdh',
    [string]$Repo = 'serial_tool',
    [string]$MarketplacePath = 'plugin-marketplace',
    [string]$Branch = 'main'
)

$ErrorActionPreference = 'Stop'

# ── 路径与输入校验 ──
if ([string]::IsNullOrWhiteSpace($SourcePath)) {
    $repositoryRoot = Split-Path -Parent $MarketplaceRoot
    $SourcePath = Join-Path (Join-Path $repositoryRoot 'plugins') $PluginId
}
$sourcePath = (Resolve-Path $SourcePath -ErrorAction SilentlyContinue).Path
if (-not $sourcePath) {
    throw "源目录不存在: $SourcePath"
}
$manifestPath = Join-Path $sourcePath 'plugin.json'
if (-not (Test-Path $manifestPath)) {
    throw "源目录下找不到 plugin.json: $manifestPath"
}

# 读 manifest 并校验 id/version 一致
$manifest = Get-Content $manifestPath -Raw -Encoding UTF8 | ConvertFrom-Json
if ($manifest.id -ne $PluginId) {
    throw "plugin.json 的 id('$($manifest.id)') 与参数 PluginId('$PluginId') 不一致"
}
if ($manifest.version -ne $Version) {
    throw "plugin.json 的 version('$($manifest.version)') 与参数 Version('$Version') 不一致"
}

# ── 目标目录 ──
$pluginDir = Join-Path $MarketplaceRoot "plugins/$PluginId/$Version"
$zipName = "$PluginId-$Version.zip"
$zipPath = Join-Path $pluginDir $zipName
$manifestCopyPath = Join-Path $pluginDir 'plugin.json'

if (-not (Test-Path $pluginDir)) {
    New-Item -ItemType Directory -Path $pluginDir -Force | Out-Null
}

# ── 打包 zip ──
# .NET ZipFile.CreateFromDirectory 会把 sourcePath 本身作为 zip 根目录。
# 这正是我们要的：解压后得到 <目录名>/plugin.json。
Add-Type -AssemblyName System.IO.Compression.FileSystem

# 先删旧 zip（重新发布同版本时覆盖）
if (Test-Path $zipPath) {
    Remove-Item $zipPath -Force
}

# ── 打包 zip ──
# 关键：zip 内部必须以插件目录名（用 PluginId 作为目录名）为根，
# 这样解压到 app_dir/plugins/ 后得到 app_dir/plugins/<PluginId>/plugin.json，
# 与 discover_roots 期望的"每个插件一个子目录"结构一致。
# .NET ZipFile.CreateFromDirectory 默认不带顶层目录名（内容平铺），
# 所以这里手动逐条创建 entry 并加前缀。
Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem

$tempZip = Join-Path ([System.IO.Path]::GetTempPath()) "hw-publish-$([guid]::NewGuid()).zip"
$dirName = $PluginId  # 解压后的目录名用插件 id

try {
    $zip = [System.IO.Compression.ZipFile]::Open($tempZip, [System.IO.Compression.ZipArchiveMode]::Create)
    try {
        # 递归收集源目录下所有文件（相对路径）
        $files = Get-ChildItem -Path $sourcePath -Recurse -File
        foreach ($file in $files) {
            $relative = $file.FullName.Substring($sourcePath.Length + 1).Replace('\', '/')
            $segments = $relative.Split('/')
            if ($segments -contains '.git' -or $file.Extension -in @('.tmp', '.bak')) {
                continue
            }
            $entryName = "$dirName/$relative"
            $entry = $zip.CreateEntry($entryName, [System.IO.Compression.CompressionLevel]::Optimal)
            $entry.LastWriteTime = $file.LastWriteTime
            $stream = $entry.Open()
            try {
                $fs = [System.IO.File]::OpenRead($file.FullName)
                try {
                    $fs.CopyTo($stream)
                } finally {
                    $fs.Dispose()
                }
            } finally {
                $stream.Dispose()
            }
        }
    } finally {
        $zip.Dispose()
    }
    Move-Item $tempZip $zipPath
} catch {
    if (Test-Path $tempZip) { Remove-Item $tempZip -Force }
    throw "打包 zip 失败: $_"
}

# ── 校验 zip 内不含危险扩展名（与 updater 安全模型一致） ──
$unsafeExtensions = @('.dll', '.exe', '.sys', '.cpl', '.ocx', '.drv', '.scr',
                       '.bat', '.cmd', '.ps1', '.vbs', '.sh', '.msi')
Add-Type -AssemblyName System.IO.Compression.FileSystem
$zip = [System.IO.Compression.ZipFile]::OpenRead($zipPath)
try {
    $badEntries = $zip.Entries | Where-Object {
        $ext = [System.IO.Path]::GetExtension($_.FullName)
        $unsafeExtensions -contains $ext.ToLower()
    } | Select-Object -ExpandProperty FullName
    if ($badEntries) {
        throw "zip 包含危险扩展名文件: $($badEntries -join ', ')"
    }
} finally {
    $zip.Dispose()
}

# ── 复制 plugin.json ──
Copy-Item $manifestPath $manifestCopyPath -Force

# ── 算 SHA256 + size ──
$sha256 = (Get-FileHash $zipPath -Algorithm SHA256).Hash.ToLower()
$size = (Get-Item $zipPath).Length

# ── 更新 registry.json ──
$registryPath = Join-Path $MarketplaceRoot 'registry.json'
if (-not (Test-Path $registryPath)) {
    throw "找不到 registry.json: $registryPath"
}
$registry = Get-Content $registryPath -Raw -Encoding UTF8 | ConvertFrom-Json

# 拼下载 URL（GitHub raw）
$marketplaceUrlPath = $MarketplacePath.Trim('/')
$downloadUrl = "https://raw.githubusercontent.com/$Owner/$Repo/$Branch/$marketplaceUrlPath/plugins/$PluginId/$Version/$zipName"

# 查找或新建条目
$entryIndex = -1
for ($i = 0; $i -lt $registry.plugins.Count; $i++) {
    if ($registry.plugins[$i].id -eq $PluginId) {
        $entryIndex = $i
        break
    }
}

# 当前时间（UTC，ISO 8601）
$now = ([datetime]::UtcNow).ToString('o')

$entry = [ordered]@{
    id            = $PluginId
    name          = $manifest.name
    version       = $Version
    api_version   = $manifest.api_version
    description   = $manifest.description
    author        = $manifest.author
    homepage      = $manifest.homepage
    repository    = $manifest.repository
    license       = $manifest.license
    category      = $manifest.category
    icon          = $manifest.icon
    permissions   = @($manifest.permissions)
    download_url  = $downloadUrl
    sha256        = $sha256
    size          = $size
    published     = $now
}

if ($entryIndex -ge 0) {
    # 替换已有条目（保留 registry 里其它字段顺序——直接整条替换）
    $registry.plugins[$entryIndex] = [PSCustomObject]$entry
    Write-Host "更新已有条目: $PluginId v$Version"
} else {
    $registry.plugins += [PSCustomObject]$entry
    Write-Host "新增条目: $PluginId v$Version"
}

$registry.updated = $now

# 写回（UTF-8 无 BOM）
$json = $registry | ConvertTo-Json -Depth 10
[System.IO.File]::WriteAllText($registryPath, $json, [System.Text.UTF8Encoding]::new($false))

Write-Host ""
Write-Host "发布完成:"
Write-Host "  zip     : $zipPath"
Write-Host "  sha256  : $sha256"
Write-Host "  size    : $size bytes"
Write-Host "  url     : $downloadUrl"
Write-Host ""
Write-Host "下一步: git add -A && git commit -m `"publish $PluginId v$Version`" && git push"
