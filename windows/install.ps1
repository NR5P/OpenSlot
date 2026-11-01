# Builds Openslot for Windows and installs it for this user (no administrator needed):
# openslot.exe with the XARA modem built in, a Start menu and a desktop shortcut.
# Needs the XARA checkout next to this repository (Desktop\xara beside Desktop\varac-rust).
#
#   powershell -ExecutionPolicy Bypass -File windows\install.ps1            # build and install
#   powershell -ExecutionPolicy Bypass -File windows\install.ps1 -NoBuild   # install what is built
#   powershell -ExecutionPolicy Bypass -File windows\install.ps1 -Uninstall
param([switch]$NoBuild, [switch]$Uninstall)
$ErrorActionPreference = "Stop"

$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$dest = Join-Path $env:LOCALAPPDATA "Programs\Openslot"
$menu = Join-Path ([Environment]::GetFolderPath("Programs")) "Openslot.lnk"
$desk = Join-Path ([Environment]::GetFolderPath("Desktop")) "Openslot.lnk"

if ($Uninstall) {
    Get-Process openslot -ErrorAction SilentlyContinue | Where-Object { $_.Path -like "$dest*" } | Stop-Process -Force
    Remove-Item $menu, $desk -ErrorAction SilentlyContinue
    Remove-Item $dest -Recurse -Force -ErrorAction SilentlyContinue
    Write-Host "Openslot removed (settings and logs stay in $env:APPDATA\openslot)"
    exit 0
}

if (-not $NoBuild) {
    cargo build --release --manifest-path (Join-Path $here "Cargo.toml")
    if ($LASTEXITCODE -ne 0) { throw "building Openslot failed" }
}

# A running copy holds its exe open.
Get-Process openslot -ErrorAction SilentlyContinue | Where-Object { $_.Path -like "$dest*" } | Stop-Process -Force
New-Item -ItemType Directory -Force $dest | Out-Null
Copy-Item (Join-Path $here "target\release\openslot.exe") $dest -Force

$shell = New-Object -ComObject WScript.Shell
foreach ($lnk in $menu, $desk) {
    $s = $shell.CreateShortcut($lnk)
    $s.TargetPath = Join-Path $dest "openslot.exe"
    $s.WorkingDirectory = $dest
    $s.Description = "Openslot (VarAC compatible)"
    $s.Save()
}
Write-Host "Openslot installed in $dest, with Start menu and desktop shortcuts."
