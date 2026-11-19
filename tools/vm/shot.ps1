# Runs inside the test VM in the logged-on desktop session (started by a scheduled task with /IT).
# Optionally moves the cursor (hover), clicks at points "x,y;x,y", waits, then saves a screenshot
# of the top-center area.
param(
    [int]$X = -1,
    [int]$Y = -1,
    [int]$Wait = 900,
    [string]$Clicks = '',
    # "x1,y1;x2,y2;..." press at the first point, glide through the rest, release at the last
    [string]$Drag = '',
    [int]$AfterDrag = 1500,
    # press Ctrl+Alt+N before the screenshot
    [switch]$Hotkey,
    # take N screenshots one second apart: shot-1.png ... shot-N.png
    [int]$Burst = 0,
    [string]$Keys = '',
    [switch]$MaxEdge,
    [switch]$RestoreEdge,
    # "x,y,notches": scroll the wheel at a point (negative notches = down)
    [string]$Wheel = '',
    # "x1,y1,x2,y2,holdMs,moveMs,alt": press, hold still, move fast, release at once (a fling);
    # alt=1 holds the Alt key meanwhile. Then -Frames screenshots of the top strip, 60 ms apart.
    [string]$Flick = '',
    [int]$Frames = 0,
    [int]$CropW = 1400,
    [int]$CropH = 1000,
    [string]$Out = 'C:\MicaBar\shot.png'
)
Add-Type -AssemblyName System.Drawing
Add-Type @'
using System; using System.Runtime.InteropServices;
public static class NiU {
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern int GetSystemMetrics(int i);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, int dx, int dy, uint d, IntPtr e);
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint f, IntPtr e);
}
'@
[NiU]::SetProcessDPIAware() | Out-Null
Start-Sleep -Milliseconds 300
if ($X -ge 0) { [NiU]::SetCursorPos($X, $Y) | Out-Null }
Start-Sleep -Milliseconds $Wait
if ($Clicks) {
    foreach ($c in $Clicks.Split(';')) {
        $p = $c.Split(',')
        [NiU]::SetCursorPos([int]$p[0], [int]$p[1]) | Out-Null
        Start-Sleep -Milliseconds 150
        [NiU]::mouse_event(0x0002, 0, 0, 0, [IntPtr]::Zero)
        Start-Sleep -Milliseconds 90  # a real click is held ~50-150 ms
        [NiU]::mouse_event(0x0004, 0, 0, 0, [IntPtr]::Zero)
        Start-Sleep -Milliseconds 600
    }
}
if ($Drag) {
    $pts = $Drag.Split(';') | ForEach-Object { $p = $_.Split(','); ,@([int]$p[0], [int]$p[1]) }
    [NiU]::SetCursorPos($pts[0][0], $pts[0][1]) | Out-Null
    Start-Sleep -Milliseconds 200
    [NiU]::mouse_event(0x0002, 0, 0, 0, [IntPtr]::Zero)
    Start-Sleep -Milliseconds 200
    for ($i = 1; $i -lt $pts.Count; $i++) {
        $a = $pts[$i - 1]; $b = $pts[$i]
        for ($s = 1; $s -le 25; $s++) {
            [NiU]::SetCursorPos([int]($a[0] + ($b[0] - $a[0]) * $s / 25), [int]($a[1] + ($b[1] - $a[1]) * $s / 25)) | Out-Null
            Start-Sleep -Milliseconds 20
        }
        Start-Sleep -Milliseconds 900
    }
    [NiU]::mouse_event(0x0004, 0, 0, 0, [IntPtr]::Zero)
    Start-Sleep -Milliseconds $AfterDrag
}
if ($Hotkey) {
    foreach ($k in 0x11, 0x12, 0x4E) { [NiU]::keybd_event([byte]$k, 0, 0, [IntPtr]::Zero) }
    foreach ($k in 0x4E, 0x12, 0x11) { [NiU]::keybd_event([byte]$k, 0, 2, [IntPtr]::Zero) }
    Start-Sleep -Milliseconds 1200
}
if ($Wheel) {
    $w = $Wheel.Split(',')
    [NiU]::SetCursorPos([int]$w[0], [int]$w[1]) | Out-Null
    Start-Sleep -Milliseconds 200
    $n = [int]$w[2]
    for ($i = 0; $i -lt [Math]::Abs($n); $i++) {
        [NiU]::mouse_event(0x0800, 0, 0, [uint32]([int](120 * [Math]::Sign($n)) -band 0xFFFFFFFF), [IntPtr]::Zero)
        Start-Sleep -Milliseconds 60
    }
    Start-Sleep -Milliseconds 600
}
# key sequence: groups separated by ';', keys of a chord by ',' (virtual-key codes), e.g. "17,18,78;40;40"
if ($Keys) {
    foreach ($chord in $Keys.Split(';')) {
        $ks = $chord.Split(',') | ForEach-Object { [byte][int]$_ }
        foreach ($k in $ks) { [NiU]::keybd_event($k, 0, 0, [IntPtr]::Zero) }
        [array]::Reverse($ks)
        foreach ($k in $ks) { [NiU]::keybd_event($k, 0, 2, [IntPtr]::Zero) }
        Start-Sleep -Milliseconds 400
    }
    Start-Sleep -Milliseconds 800
}
# maximize (or restore with -Restore) the Edge window first
if ($MaxEdge -or $RestoreEdge) {
    Add-Type @'
using System; using System.Runtime.InteropServices;
public static class NiW { [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int c); [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h); }
'@
    $h = (Get-Process msedge -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1).MainWindowHandle
    if ($h) { [NiW]::ShowWindow($h, $(if ($MaxEdge) { 3 } else { 9 })) | Out-Null; [NiW]::SetForegroundWindow($h) | Out-Null }
    Start-Sleep -Milliseconds 1500
}
$sw = [NiU]::GetSystemMetrics(0); $sh = [NiU]::GetSystemMetrics(1)
$w = [Math]::Min($CropW, $sw); $h = [Math]::Min($CropH, $sh)
$left = [int](($sw - $w) / 2)
function Save-Shot([string]$file) {
    $bmp = New-Object System.Drawing.Bitmap $w, $h
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($left, 0, 0, 0, $bmp.Size)
    $bmp.Save($file, [System.Drawing.Imaging.ImageFormat]::Png)
    $g.Dispose(); $bmp.Dispose()
}
if ($Flick) {
    $f = $Flick.Split(',') | ForEach-Object { [int]$_ }
    if ($f[6]) { [NiU]::keybd_event(0x12, 0, 0, [IntPtr]::Zero) }
    [NiU]::SetCursorPos($f[0], $f[1]) | Out-Null
    Start-Sleep -Milliseconds 250
    [NiU]::mouse_event(0x0002, 0, 0, 0, [IntPtr]::Zero)
    Start-Sleep -Milliseconds $f[4]
    $steps = 12
    for ($s = 1; $s -le $steps; $s++) {
        [NiU]::SetCursorPos([int]($f[0] + ($f[2] - $f[0]) * $s / $steps), [int]($f[1] + ($f[3] - $f[1]) * $s / $steps)) | Out-Null
        Start-Sleep -Milliseconds ([int]($f[5] / $steps))
    }
    [NiU]::mouse_event(0x0004, 0, 0, 0, [IntPtr]::Zero)
    if ($f[6]) { [NiU]::keybd_event(0x12, 0, 2, [IntPtr]::Zero) }
    for ($i = 1; $i -le $Frames; $i++) {
        $bmp = New-Object System.Drawing.Bitmap $sw, 110
        $g = [System.Drawing.Graphics]::FromImage($bmp)
        $g.CopyFromScreen(0, 0, 0, 0, $bmp.Size)
        $bmp.Save(($Out -replace '\.png$', "-f$i.png"), [System.Drawing.Imaging.ImageFormat]::Png)
        $g.Dispose(); $bmp.Dispose()
        Start-Sleep -Milliseconds 60
    }
    Start-Sleep -Milliseconds 1200
}if ($Burst -gt 0) {
    for ($i = 1; $i -le $Burst; $i++) {
        Save-Shot ($Out -replace '\.png$', "-$i.png")
        Start-Sleep -Milliseconds 1000
    }
}
Save-Shot $Out
"$sw x $sh" | Set-Content -Encoding ascii ($Out + '.txt')
