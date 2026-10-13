param([switch]$Installer)
$ErrorActionPreference = 'Stop'
$taskRoot = 'J:\CS2ClipStudio'
$env:TEMP = Join-Path $taskRoot 'temp/build'
$env:TMP = $env:TEMP
$env:APPDATA = Join-Path $taskRoot 'cache/build-appdata'
$env:LOCALAPPDATA = Join-Path $taskRoot 'cache/build-localappdata'
$env:CARGO_HOME = Join-Path $taskRoot 'tools/rust/cargo'
$env:RUSTUP_HOME = Join-Path $taskRoot 'tools/rust/rustup'
$env:CARGO_TARGET_DIR = Join-Path $taskRoot 'app/target'
$env:npm_config_cache = Join-Path $taskRoot 'cache/desktop-npm'
$env:Path = (Join-Path $env:CARGO_HOME 'bin') + ';' + $env:Path
New-Item -ItemType Directory -Force -Path $env:TEMP,$env:APPDATA,$env:LOCALAPPDATA | Out-Null
Set-Location (Join-Path $taskRoot 'app')
if ($Installer) {
    & '.\node_modules\.bin\tauri.cmd' build --bundles nsis
} else {
    & '.\node_modules\.bin\tauri.cmd' build --no-bundle
}
if ($LASTEXITCODE -ne 0) { throw "Native build failed ($LASTEXITCODE)." }
& node scripts/portable.mjs
if ($LASTEXITCODE -ne 0) { throw 'Portable artifact packaging failed.' }
