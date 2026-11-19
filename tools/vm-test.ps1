# Host-side driver for a Windows test VM reachable over SSH (OpenSSH server, admin account,
# a user logged on at the desktop). Target and key come from environment variables:
#   MICABAR_VM      user@host of the VM          (required)
#   MICABAR_VM_KEY  private key file for ssh/scp (optional; default ssh identity)
#   .\tools\vm-test.ps1 -Deploy           copy exe (+ geo dbs, shot script) and restart the app
#   .\tools\vm-test.ps1 -Shot a.png       screenshot of the top-center area
#   .\tools\vm-test.ps1 -Shot b.png -X 1129 -Y 18   move the cursor first (hover)
#   .\tools\vm-test.ps1 -Log              tail the app log
param(
    [switch]$Deploy,
    [string]$Shot,
    [int]$X = -1,
    [int]$Y = -1,
    [int]$Wait = 1200,
    [string]$Clicks = '',
    [string]$Drag = '',
    [switch]$Hotkey,
    [switch]$Full,
    [int]$Burst = 0,
    [string]$Keys = '',
    [string]$Wheel = '',
    [switch]$MaxEdge,
    [switch]$RestoreEdge,
    [switch]$Log,
    [switch]$Stop,
    # settings patch, e.g. '{"shape":{"style":"notch"}}': merged into settings.json, app restarted
    [string]$Patch
)
$ErrorActionPreference = 'Stop'
$vm = if ($env:MICABAR_VM) { $env:MICABAR_VM } else { [Environment]::GetEnvironmentVariable('MICABAR_VM', 'User') }
if (-not $vm) { throw 'set MICABAR_VM to user@host of the test VM' }
$key = if ($env:MICABAR_VM_KEY) { $env:MICABAR_VM_KEY } else { [Environment]::GetEnvironmentVariable('MICABAR_VM_KEY', 'User') }
$sshArgs = @('-o', 'ConnectTimeout=8', '-o', 'BatchMode=yes')
if ($key) { $sshArgs = @('-i', $key) + $sshArgs }
$root = Split-Path $PSScriptRoot -Parent
$outDir = Join-Path $root 'tools\shots'
New-Item -ItemType Directory -Force $outDir | Out-Null

function Invoke-Vm([string]$cmd) { & ssh @sshArgs $vm $cmd }

if ($Stop -or $Deploy) {
    Invoke-Vm "Get-Process micabar -ErrorAction SilentlyContinue | Stop-Process -Force; schtasks /End /TN MicaBar 2>`$null | Out-Null; 'stopped'"
}
if ($Deploy) {
    Invoke-Vm "New-Item -ItemType Directory -Force C:\MicaBar\geo | Out-Null"
    & scp @sshArgs "$root\src-tauri\target\release\micabar.exe" "${vm}:C:/MicaBar/micabar.exe"
    & scp @sshArgs "$root\tools\vm\shot.ps1" "${vm}:C:/MicaBar/shot.ps1"
    $geo = "$env:APPDATA\mihomo-party\work"
    $have = Invoke-Vm "Test-Path C:\MicaBar\geo\ASN.mmdb"
    if ($have -ne 'True') {
        & scp @sshArgs "$geo\country.mmdb" "$geo\ASN.mmdb" "${vm}:C:/MicaBar/geo/"
    }
    Invoke-Vm "schtasks /Create /TN MicaBar /TR C:\MicaBar\micabar.exe /SC ONCE /ST 23:59 /IT /RL HIGHEST /F | Out-Null; schtasks /Run /TN MicaBar | Out-Null; Start-Sleep 4; Get-Process micabar -ErrorAction SilentlyContinue | Select-Object Id, SessionId | Format-Table -HideTableHeaders"
}
if ($Patch) {
    $b64 = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($Patch))
    $remote = @"
Get-Process micabar -ErrorAction SilentlyContinue | Stop-Process -Force; Start-Sleep -Milliseconds 500
`$p = Join-Path `$env:LOCALAPPDATA 'MicaBar\settings.json'
`$s = if (Test-Path `$p) { [IO.File]::ReadAllText(`$p) | ConvertFrom-Json } else { [pscustomobject]@{} }
`$patch = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('$b64')) | ConvertFrom-Json
foreach (`$sec in `$patch.PSObject.Properties) {
  if (-not `$s.PSObject.Properties[`$sec.Name]) { `$s | Add-Member -NotePropertyName `$sec.Name -NotePropertyValue ([pscustomobject]@{}) }
  foreach (`$k in `$sec.Value.PSObject.Properties) {
    if (`$s.(`$sec.Name).PSObject.Properties[`$k.Name]) { `$s.(`$sec.Name).(`$k.Name) = `$k.Value } else { `$s.(`$sec.Name) | Add-Member -NotePropertyName `$k.Name -NotePropertyValue `$k.Value }
  }
}
[IO.File]::WriteAllText(`$p, (`$s | ConvertTo-Json -Depth 8), (New-Object Text.UTF8Encoding(`$false)))
schtasks /Run /TN MicaBar | Out-Null; Start-Sleep 6; 'patched'
"@
    Invoke-Vm $remote
}
if ($Shot) {
    $tr = "powershell -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File C:\MicaBar\shot.ps1 -X $X -Y $Y -Wait $Wait"
    if ($Clicks) { $tr += " -Clicks $Clicks" }
    if ($Drag) { $tr += " -Drag $Drag" }
    if ($Hotkey) { $tr += " -Hotkey" }
    if ($Full) { $tr += " -CropW 4000 -CropH 3000" }
    if ($Burst -gt 0) { $tr += " -Burst $Burst -CropH 120" }
    if ($Keys) { $tr += " -Keys $Keys" }
    if ($Wheel) { $tr += " -Wheel $Wheel" }
    if ($MaxEdge) { $tr += " -MaxEdge" }
    if ($RestoreEdge) { $tr += " -RestoreEdge" }
    Invoke-Vm "Remove-Item C:\MicaBar\shot.png -ErrorAction SilentlyContinue; schtasks /Create /TN NI-Shot /TR '$tr' /SC ONCE /ST 23:59 /IT /RL HIGHEST /F | Out-Null; schtasks /Run /TN NI-Shot | Out-Null; `$i=0; while (-not (Test-Path C:\MicaBar\shot.png.txt) -and `$i -lt 90) { Start-Sleep -Milliseconds 300; `$i++ }; Start-Sleep -Milliseconds 300; Get-Content C:\MicaBar\shot.png.txt; Remove-Item C:\MicaBar\shot.png.txt"
    & scp @sshArgs "${vm}:C:/MicaBar/shot.png" (Join-Path $outDir $Shot)
    for ($i = 1; $i -le $Burst; $i++) {
        $name = $Shot -replace '\.png$', "-$i.png"
        & scp @sshArgs "${vm}:C:/MicaBar/shot-$i.png" (Join-Path $outDir $name)
    }
    Join-Path $outDir $Shot
}
if ($Log) {
    $local = Join-Path $outDir 'vm.log'
    Invoke-Vm "Copy-Item (Join-Path `$env:LOCALAPPDATA 'MicaBar\micabar.log') C:\MicaBar\micabar.log -Force" | Out-Null
    & scp @sshArgs "${vm}:C:/MicaBar/micabar.log" $local
    Get-Content $local -Tail 40 -Encoding UTF8
}
