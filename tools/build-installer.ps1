# Builds the release binary and wraps it in the Inno Setup installer.
#   powershell -ExecutionPolicy Bypass -File tools\build-installer.ps1
# Output: dist\Tokpaek-Setup-<version>.exe
param([string]$Iscc = "D:\Programs\Inno Setup 6\ISCC.exe")

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

# Version format: <yy>.<day_of_year>
$version = "$((Get-Date).ToString('yy')).$((Get-Date).DayOfYear)"
Write-Output "Tokpaek $version"

if (-not (Test-Path $Iscc)) { throw "Inno Setup not found: $Iscc" }

cargo build --release
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

New-Item -ItemType Directory -Force -Path dist | Out-Null
& $Iscc "/DAppVersion=$version" "installer\tokpaek.iss"
if ($LASTEXITCODE -ne 0) { throw "ISCC failed" }

$setup = "dist\Tokpaek-Setup-$version.exe"
"{0}  {1:N2} MB" -f $setup, ((Get-Item $setup).Length / 1MB)
