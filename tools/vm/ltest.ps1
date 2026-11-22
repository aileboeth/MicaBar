# VM test helper for the live activities (run over ssh in the test VM, as the logged-on user).
# ASCII only (PowerShell 5 reads BOM-less files in the ANSI code page); Chinese text is \u-escaped.
#   -Mode prompt|ask|write|stop|esc   fake Claude Code hook calls for one session (+ transcript)
#   -Mode newapp                      a copy of curl.exe outside Windows (run the download yourself:
#                                     an ssh session's children die with it)
#   -Mode sysoff|syson                switch the WinINet system proxy off / back to 127.0.0.1:7890
#   -Mode dnsbad|dnsgood              point the uplink's DNS at a dead LAN address / back to DHCP
param([string]$Mode = 'prompt', [string]$Session = 'vm-live', [string]$Project = 'C:\work\expense-tracking')
$ErrorActionPreference = 'Stop'
$tr = "C:\MicaBar\$Session.jsonl"
$utf8 = New-Object Text.UTF8Encoding($false)
function Send-Hook([string]$event, [hashtable]$extra) {
    $tok = (Get-Content "$env:LOCALAPPDATA\MicaBar\notify-token" -Raw).Trim()
    $body = @{ session_id = $Session; cwd = $Project; transcript_path = $tr } + $extra | ConvertTo-Json -Compress
    $bf = "C:\MicaBar\hook-$event.json"
    [IO.File]::WriteAllText($bf, $body, $utf8)
    & curl.exe -s -m 3 -o NUL -w "$event %{http_code}`n" --data-binary "@$bf" "http://127.0.0.1:47900/n/claude/$event`?t=$tok"
    Remove-Item $bf -Force
}
function Add-Line([string]$json) { [IO.File]::AppendAllText($tr, $json + "`n", $utf8) }

switch ($Mode) {
    'prompt' {
        [IO.File]::WriteAllText($tr, '{"type":"user","message":{"role":"user","content":"\u505a\u4e00\u4e0b\u7edf\u8ba1\u9875"}}' + "`n", $utf8)
        Send-Hook 'prompt' @{ prompt = 'stats page' }
    }
    'ask' {
        Add-Line '{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"npm test"}}]}}'
        Send-Hook 'notify' @{ message = 'Claude needs your permission to use Bash' }
    }
    'write' { Add-Line '{"type":"user","message":{"content":[{"type":"tool_result","content":"ok"}]}}'; 'wrote' }
    'esc' { Add-Line '{"type":"user","message":{"role":"user","content":[{"type":"text","text":"[Request interrupted by user]"}]}}'; 'interrupted' }
    'stop' {
        Add-Line '{"type":"assistant","message":{"content":[{"type":"text","text":"\u7edf\u8ba1\u9875\u505a\u597d\u4e86\uff0c\u6d4b\u8bd5\u5168\u90e8\u901a\u8fc7\u3002"}]}}'
        Send-Hook 'stop' @{}
        Start-Sleep -Seconds 2
        Remove-Item $tr -Force -ErrorAction SilentlyContinue
    }
    'newapp' {
        $dir = 'C:\MicaBar\newtool'
        New-Item -ItemType Directory -Force $dir | Out-Null
        Copy-Item C:\Windows\System32\curl.exe "$dir\SyncHelper.exe" -Force
        'copied curl.exe to C:\MicaBar\newtool\SyncHelper.exe'
    }
    'sysoff' {
        $k = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Internet Settings'
        "before: enable=$((Get-ItemProperty $k).ProxyEnable) server=$((Get-ItemProperty $k).ProxyServer)"
        Set-ItemProperty $k ProxyEnable 0
        'system proxy off'
    }
    'syson' {
        $k = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Internet Settings'
        Set-ItemProperty $k ProxyServer '127.0.0.1:7890'
        Set-ItemProperty $k ProxyEnable 1
        "now: enable=$((Get-ItemProperty $k).ProxyEnable) server=$((Get-ItemProperty $k).ProxyServer)"
    }
    'dnsbad' {
        $a = Get-NetIPConfiguration | Where-Object { $_.IPv4DefaultGateway -and $_.NetAdapter.Status -eq 'Up' } | Select-Object -First 1
        "adapter $($a.InterfaceAlias) dns before: $($a.DNSServer.ServerAddresses -join ',')"
        Set-DnsClientServerAddress -InterfaceIndex $a.InterfaceIndex -ServerAddresses 192.168.10.250
        Clear-DnsClientCache
        'dns -> 192.168.10.250 (no such host on the LAN)'
    }
    'fwon' {
        # dead-man switch first: the firewall goes off again in 4 minutes whatever happens
        $at = (Get-Date).AddMinutes(4).ToString('HH:mm')
        schtasks /Create /TN MB-FwOff /TR 'netsh advfirewall set allprofiles state off' /SC ONCE /ST $at /RU SYSTEM /F | Out-Null
        netsh advfirewall set allprofiles state on | Out-Null
        "firewall on, auto-off at $at"
    }
    'fwoff' {
        netsh advfirewall set allprofiles state off | Out-Null
        schtasks /Delete /TN MB-FwOff /F 2>$null | Out-Null
        'firewall off, dead-man task removed'
    }
    'fwclean' {
        netsh advfirewall firewall delete rule name=all program=C:\MicaBar\newtool\SyncHelper.exe 2>$null | Out-Null
        $p = Join-Path $env:LOCALAPPDATA 'MicaBar\known-apps.json'
        $j = [IO.File]::ReadAllText($p) | ConvertFrom-Json
        $j.apps.PSObject.Properties.Remove('c:\micabar\newtool\synchelper.exe')
        [IO.File]::WriteAllText($p, ($j | ConvertTo-Json -Depth 5 -Compress), $utf8)
        'SyncHelper forgotten, its rule deleted'
    }
    'dnsgood' {
        $a = Get-NetIPConfiguration | Where-Object { $_.IPv4DefaultGateway -and $_.NetAdapter.Status -eq 'Up' } | Select-Object -First 1
        Set-DnsClientServerAddress -InterfaceIndex $a.InterfaceIndex -ResetServerAddresses
        Clear-DnsClientCache
        "adapter $($a.InterfaceAlias) dns now: $((Get-DnsClientServerAddress -InterfaceIndex $a.InterfaceIndex -AddressFamily IPv4).ServerAddresses -join ',')"
    }
}
