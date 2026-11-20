# VM test helper for MicaBar notifications. Runs in the logged-on desktop session (scheduled task).
#   -Mode flash   start Notepad minimized and flash its taskbar button (like WeChat does)
#   -Mode toast   send a Windows toast as PowerShell (notifications must be enabled)
#   -Mode ai      post a Claude Code "stop" event to MicaBar's hook port (with a fake transcript)
param([string]$Mode = 'flash', [int]$Count = 1)
$ErrorActionPreference = 'Stop'
$log = 'C:\MicaBar\ntest.log'
function Say($m) { Add-Content -Path $log -Value ("{0:HH:mm:ss} {1}" -f (Get-Date), $m) -Encoding utf8 }

if ($Mode -eq 'flash') {
    Add-Type @'
using System; using System.Runtime.InteropServices;
public static class Fl {
  [StructLayout(LayoutKind.Sequential)] public struct FLASHWINFO { public uint cbSize; public IntPtr hwnd; public uint dwFlags; public uint uCount; public uint dwTimeout; }
  [DllImport("user32.dll")] public static extern bool FlashWindowEx(ref FLASHWINFO f);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int c);
}
'@
    $p = Start-Process notepad -PassThru
    Start-Sleep -Milliseconds 1500
    $h = (Get-Process -Id $p.Id).MainWindowHandle
    [Fl]::ShowWindow($h, 6) | Out-Null   # minimize: no longer the foreground window
    Start-Sleep -Milliseconds 800
    for ($i = 0; $i -lt $Count; $i++) {
        $f = New-Object Fl+FLASHWINFO
        $f.cbSize = [Runtime.InteropServices.Marshal]::SizeOf($f); $f.hwnd = $h; $f.dwFlags = 2; $f.uCount = 3; $f.dwTimeout = 0
        [Fl]::FlashWindowEx([ref]$f) | Out-Null
        Say "flashed notepad $h ($($i + 1))"
        Start-Sleep -Seconds 4
    }
    Start-Sleep -Seconds 6
    Stop-Process -Id $p.Id -Force
}
elseif ($Mode -eq 'toast') {
    [Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null
    [Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom.XmlDocument, ContentType = WindowsRuntime] | Out-Null
    $aumid = '{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\WindowsPowerShell\v1.0\powershell.exe'
    $xml = New-Object Windows.Data.Xml.Dom.XmlDocument
    $xml.LoadXml("<toast><visual><binding template='ToastGeneric'><text>李四</text><text>测试：文件发你了，看一下 &amp; 回个话</text></binding></visual></toast>")
    [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier($aumid).Show([Windows.UI.Notifications.ToastNotification]::new($xml))
    Say 'toast sent'
}
elseif ($Mode -eq 'ai') {
    $tok = (Get-Content "$env:LOCALAPPDATA\MicaBar\notify-token" -Raw).Trim()
    $tr = 'C:\MicaBar\fake-transcript.jsonl'
    $line = '{"type":"assistant","message":{"content":[{"type":"text","text":"## 完成\n通知模块写好了：岛上弹卡片、叠放、悬停展开，虚拟机里验证通过。"}]}}'
    [IO.File]::WriteAllText($tr, $line + "`n", (New-Object Text.UTF8Encoding($false)))
    $body = @{ session_id = 'vm-test'; cwd = 'C:\work\MicaBar'; transcript_path = $tr; hook_event_name = 'Stop' } | ConvertTo-Json -Compress
    $bf = 'C:\MicaBar\fake-stop.json'
    [IO.File]::WriteAllText($bf, $body, (New-Object Text.UTF8Encoding($false)))
    & curl.exe -s -m 3 -o NUL -w "%{http_code}" --data-binary "@$bf" "http://127.0.0.1:47900/n/claude/stop?t=$tok" | ForEach-Object { Say "ai stop -> $_" }
    Start-Sleep -Seconds 3 # MicaBar reads the transcript after answering
    Remove-Item $bf, $tr -Force
}
