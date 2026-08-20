# Build a portable release: unzip, double-click, done.
#
# The Windows twin of package.sh. Kept as a separate script rather than asking
# people to find a bash: the whole point of this file is that it runs in the
# PowerShell window that is already open.
#
# The zip holds three executables and needs none of them installed:
#
#   usage-watcher.exe   the app: tray icon, panel, and the collector inside it
#   uw.exe              one-shot read from a terminal
#   uwd.exe             the collector on its own, for a headless box or WSL
#
# Usage: powershell -ExecutionPolicy Bypass -File scripts\package.ps1 [outdir]
param([string]$OutDir)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
if (-not $OutDir) { $OutDir = Join-Path $root "dist" }

$version = (Select-String -Path (Join-Path $root "Cargo.toml") -Pattern '^version\s*=\s*"(.+)"' |
            Select-Object -First 1).Matches[0].Groups[1].Value
$name  = "usage-watcher-$version-windows-x86_64"
$stage = Join-Path $OutDir $name

if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
New-Item -ItemType Directory -Force -Path $stage | Out-Null

Write-Host "==> building the panel"
npm --prefix "$root\widget" ci
npm --prefix "$root\widget" run build

Write-Host "==> building the app"
# --no-bundle: the bare executable, not an installer around it.
npm --prefix "$root\widget" exec -- tauri build --no-bundle

Write-Host "==> building the command line"
cargo build --release --manifest-path "$root\Cargo.toml" -p uw-cli -p uwd

Copy-Item "$root\widget\src-tauri\target\release\usage-watcher.exe" $stage
Copy-Item "$root\target\release\uw.exe"  $stage
Copy-Item "$root\target\release\uwd.exe" $stage
if (Test-Path "$root\LICENSE") { Copy-Item "$root\LICENSE" $stage }

@'
Usage Watcher
=============

Double-click `usage-watcher.exe`. It puts an icon in the notification area;
click that to open the panel. (Windows hides new tray icons by default — if you
cannot see it, click the "^" chevron, or drag it out of the overflow.)

The first screen is empty, with an "Add provider" button. Adding a provider
walks you through signing in to it — for most that means a browser window, and
the app waits for you to come back.

Nothing else needs installing and nothing needs to be running first: the
collector runs inside the app.

Also in this folder, for anyone who wants them:

  uw.exe     read your usage in a terminal:  uw
  uwd.exe    run the collector on its own, e.g. inside WSL, with the panel on
             Windows pointed at it

Your settings live in %APPDATA%\usage-watcher\config.toml. Credentials do not:
they go to the Windows Credential Manager, never into the config file.

To remove it: quit from the tray menu and delete this folder. Run
`uw provider remove <name>` first if you also want the stored credentials gone.
'@ | Set-Content -Encoding UTF8 (Join-Path $stage "README.txt")

Write-Host "==> zipping"
$zip = Join-Path $OutDir "$name.zip"
if (Test-Path $zip) { Remove-Item -Force $zip }
Compress-Archive -Path $stage -DestinationPath $zip
Write-Host $zip
