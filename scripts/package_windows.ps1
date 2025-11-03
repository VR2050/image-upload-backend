# package_windows.ps1
# PowerShell 脚本：在 Windows 环境下构建并打包为 zip。
# 依赖：Rust toolchain（MSVC 或 GNU），zip (PowerShell 的 Compress-Archive 可用)
param(
    [string]$Target = "x86_64-pc-windows-msvc",
    [string]$Configuration = "Release",
    [string]$OutDir = "dist\windows"
)

$BinName = "image-upload-system.exe"
$BuildDir = "target\$Target\$Configuration"

Write-Host "开始构建: target=$Target, config=$Configuration"

# 构建
cargo build --release --target $Target

if (-not (Test-Path $OutDir)) {
    New-Item -ItemType Directory -Path $OutDir | Out-Null
}

$TempDir = Join-Path -Path $env:TEMP -ChildPath ([System.Guid]::NewGuid().ToString())
New-Item -ItemType Directory -Path $TempDir | Out-Null
New-Item -ItemType Directory -Path (Join-Path $TempDir "frontend") | Out-Null
New-Item -ItemType Directory -Path (Join-Path $TempDir "uploads") | Out-Null

# 复制可执行
$ExePath = Join-Path $BuildDir $BinName
if (Test-Path $ExePath) {
    Copy-Item $ExePath -Destination $TempDir
} else {
    Write-Error "找不到构建产物: $ExePath"
    exit 1
}

# 复制前端
Copy-Item -Path "frontend\*" -Destination (Join-Path $TempDir "frontend") -Recurse -Force

# 复制 uploads（可选）
if (Test-Path "uploads") {
    Copy-Item -Path "uploads" -Destination (Join-Path $TempDir "uploads") -Recurse -Force
}

$ZipFile = Join-Path $OutDir ("image-upload-system-$Target.zip")
if (Test-Path $ZipFile) { Remove-Item $ZipFile }

Compress-Archive -Path (Join-Path $TempDir "*") -DestinationPath $ZipFile

Remove-Item -Recurse -Force $TempDir

Write-Host "打包完成: $ZipFile"
