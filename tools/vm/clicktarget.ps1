# Test helper: a plain window right under the island (top centre of the primary screen) that
# counts clicks, to check that clicks pass through the collapsed pill. Closes after $Seconds.
param([int]$Seconds = 40, [string]$Out = 'C:\MicaBar\clicks.txt')
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
$scr = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
$f = New-Object System.Windows.Forms.Form
$f.FormBorderStyle = 'None'
$f.StartPosition = 'Manual'
$f.Bounds = New-Object System.Drawing.Rectangle ([int]($scr.Width / 2 - 400)), 0, 800, 160
$f.BackColor = [System.Drawing.Color]::FromArgb(255, 250, 230, 160)
$l = New-Object System.Windows.Forms.Label
$l.Dock = 'Fill'; $l.TextAlign = 'BottomCenter'; $l.Font = New-Object System.Drawing.Font('Microsoft YaHei UI', 16)
$l.Text = 'clicks: 0'
$f.Controls.Add($l)
$n = 0
$h = { $script:n++; $l.Text = "clicks: $script:n"; Set-Content -Path $Out -Value $script:n }
$l.Add_MouseDown($h)
Set-Content -Path $Out -Value 0
$t = New-Object System.Windows.Forms.Timer
$t.Interval = $Seconds * 1000
$t.Add_Tick({ $f.Close() })
$t.Start()
[void]$f.ShowDialog()
