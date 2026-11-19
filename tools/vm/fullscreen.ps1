# Test helper (runs in the VM desktop session): shows a borderless fullscreen window like a game
# for a few seconds, takes a hover screenshot in the middle, then closes.
param([int]$Seconds = 9, [string]$Out = 'C:\NetIsland\fs.png')
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
Add-Type @'
using System; using System.Runtime.InteropServices;
public static class NiF {
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern int GetSystemMetrics(int i);
}
'@
[NiF]::SetProcessDPIAware() | Out-Null
$f = New-Object System.Windows.Forms.Form
$f.FormBorderStyle = 'None'
$f.WindowState = 'Maximized'
$f.BackColor = [System.Drawing.Color]::FromArgb(30, 120, 60)
$f.Text = 'fake game'
$t = New-Object System.Windows.Forms.Timer
$t.Interval = 2500
$step = 0
$t.Add_Tick({
    $script:step++
    if ($script:step -eq 1) {
        $sw = [NiF]::GetSystemMetrics(0)
        [NiF]::SetCursorPos([int]($sw / 2), 20) | Out-Null   # hover the pill
    } elseif ($script:step -eq 2) {
        $sw = [NiF]::GetSystemMetrics(0)
        $bmp = New-Object System.Drawing.Bitmap 1400, 300
        $g = [System.Drawing.Graphics]::FromImage($bmp)
        $g.CopyFromScreen([int](($sw - 1400) / 2), 0, 0, 0, $bmp.Size)
        $bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
        $g.Dispose(); $bmp.Dispose()
    } elseif ($script:step -ge 3) {
        $f.Close()
    }
})
$f.Add_Shown({ $f.Activate(); $t.Start() })
[System.Windows.Forms.Application]::Run($f)
'done' | Set-Content -Encoding ascii ($Out + '.txt')
