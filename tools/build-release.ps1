# Builds a signed release into dist\release-<version>\ :
#   MicaBar_<v>_x64-setup.exe (+ .sig)   installer, used by the in-app updater
#   MicaBar_<v>_portable.exe             single exe, no installer (no auto-update)
#   latest.json                            updater manifest (upload with the release)
#   SHA256SUMS.txt
# Upload all of them to a GitHub release tagged v<version> on aileboeth/MicaBar.
# The signing key lives in keys\micabar.key (git-ignored, never published).
param([string]$Notes = '')
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root

# Rust from CARGO_HOME (user environment variable) or PATH
$cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { [Environment]::GetEnvironmentVariable('CARGO_HOME', 'User') }
if ($cargoHome) {
    $env:CARGO_HOME = $cargoHome
    $env:Path = "$cargoHome\bin;$env:Path"
}
if (-not $env:RUSTUP_HOME) { $env:RUSTUP_HOME = [Environment]::GetEnvironmentVariable('RUSTUP_HOME', 'User') }
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { throw 'cargo not found: install Rust (rustup) first' }
if (-not (Test-Path keys\micabar.key)) { throw 'keys\micabar.key missing: the updater signing key is required' }
$env:TAURI_SIGNING_PRIVATE_KEY = (Get-Content keys\micabar.key -Raw).Trim()
$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = ''

$ver = (Get-Content src-tauri\tauri.conf.json -Raw | ConvertFrom-Json).version
Write-Host "building MicaBar $ver"
node scripts/copy-flags.mjs
# via cmd: pnpm/cargo write progress to stderr, which PowerShell 5 would turn into errors
cmd /c "pnpm tauri build 2>&1"
if ($LASTEXITCODE -ne 0) { throw "tauri build failed ($LASTEXITCODE)" }

$bundle = "src-tauri\target\release\bundle\nsis"
$setup = Join-Path $bundle "MicaBar_${ver}_x64-setup.exe"
if (-not (Test-Path $setup)) { throw "installer not found: $setup" }
$out = "dist\release-$ver"
New-Item -ItemType Directory -Force $out | Out-Null
Copy-Item $setup $out -Force
Copy-Item "$setup.sig" $out -Force
Copy-Item 'src-tauri\target\release\micabar.exe' (Join-Path $out "MicaBar_${ver}_portable.exe") -Force

$manifest = [ordered]@{
    version   = $ver
    notes     = $Notes
    pub_date  = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
    platforms = [ordered]@{
        'windows-x86_64' = [ordered]@{
            signature = (Get-Content "$setup.sig" -Raw).Trim()
            url       = "https://github.com/aileboeth/MicaBar/releases/download/v$ver/MicaBar_${ver}_x64-setup.exe"
        }
    }
}
[IO.File]::WriteAllText((Join-Path (Resolve-Path $out) 'latest.json'), ($manifest | ConvertTo-Json -Depth 5), (New-Object Text.UTF8Encoding($false)))

$sums = Get-ChildItem $out -File | Where-Object { $_.Name -ne 'SHA256SUMS.txt' } | ForEach-Object { "{0}  {1}" -f (Get-FileHash $_.FullName -Algorithm SHA256).Hash.ToLower(), $_.Name }
[IO.File]::WriteAllText((Join-Path (Resolve-Path $out) 'SHA256SUMS.txt'), ($sums -join "`n") + "`n", (New-Object Text.UTF8Encoding($false)))
Get-ChildItem $out | Select-Object Name, Length | Format-Table -AutoSize
