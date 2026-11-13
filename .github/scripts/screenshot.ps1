# 启动刚编译好的 nexsh，依次截下主窗口、添加主机对话框、会话标签页和右键菜单。
# 开发机不编译也不运行界面，这些截图是检查界面实际渲染效果的途径。
param([string]$Exe, [string]$Out)
$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
Add-Type @"
using System; using System.Runtime.InteropServices;
public static class Native {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, UIntPtr e);
}
"@

# 默认分辨率放不下整个窗口。
try { Set-DisplayResolution -Width 1920 -Height 1080 -Force } catch { Write-Host "resolution unchanged: $_" }

# 示例主机：地址指向本机一个没人监听的端口，点开后立刻显示连接失败的样子。
$config = Join-Path $env:APPDATA "nexsh\config"
New-Item -ItemType Directory -Force -Path $config | Out-Null
$hosts = @(
  @("web-01", "生产"), @("web-02", "生产"), @("db-primary", "生产"),
  @("staging", "测试"), @("build-runner", "测试"), @("nas", "")
)
$toml = ""
foreach ($h in $hosts) {
  $toml += "[[host]]`nname = `"$($h[0])`"`nhostname = `"127.0.0.1`"`nport = 9`nuser = `"deploy`"`n"
  if ($h[1]) { $toml += "group = `"$($h[1])`"`n" }
  $toml += "`n"
}
[IO.File]::WriteAllText((Join-Path $config "hosts.toml"), $toml, (New-Object Text.UTF8Encoding $false))

New-Item -ItemType Directory -Force -Path $Out | Out-Null
$process = Start-Process -FilePath $Exe -PassThru
for ($i = 0; $i -lt 40 -and $process.MainWindowHandle -eq 0; $i++) { Start-Sleep -Milliseconds 500; $process.Refresh() }
if ($process.MainWindowHandle -eq 0) { throw "nexsh 没有打开窗口" }
$window = $process.MainWindowHandle
[Native]::SetForegroundWindow($window) | Out-Null
Start-Sleep -Seconds 4

$rect = New-Object Native+RECT
[Native]::GetWindowRect($window, [ref]$rect) | Out-Null
Write-Host "window: $($rect.Left),$($rect.Top) - $($rect.Right),$($rect.Bottom)"

function Shot([string]$name) {
  $width = $rect.Right - $rect.Left; $height = $rect.Bottom - $rect.Top
  $bitmap = New-Object Drawing.Bitmap $width, $height
  $graphics = [Drawing.Graphics]::FromImage($bitmap)
  $graphics.CopyFromScreen($rect.Left, $rect.Top, 0, 0, $bitmap.Size)
  $bitmap.Save((Join-Path $Out $name), [Drawing.Imaging.ImageFormat]::Png)
  $graphics.Dispose(); $bitmap.Dispose()
}
# 坐标相对窗口左上角；标题栏约 31 像素高，左边有约 8 像素的不可见边框。
function Click([int]$x, [int]$y, [switch]$Right) {
  [Native]::SetCursorPos($rect.Left + 8 + $x, $rect.Top + 31 + $y) | Out-Null
  Start-Sleep -Milliseconds 300
  if ($Right) { [Native]::mouse_event(0x8, 0, 0, 0, [UIntPtr]::Zero); [Native]::mouse_event(0x10, 0, 0, 0, [UIntPtr]::Zero) }
  else { [Native]::mouse_event(0x2, 0, 0, 0, [UIntPtr]::Zero); [Native]::mouse_event(0x4, 0, 0, 0, [UIntPtr]::Zero) }
  Start-Sleep -Seconds 2
}

Shot "01-main.png"
Click 243 19            # 侧栏右上角的加号
Shot "02-form.png"
[Windows.Forms.SendKeys]::SendWait("{ESC}")
Start-Sleep -Seconds 1
Click 120 126           # 第一个分组里的第一台主机
Start-Sleep -Seconds 3
Shot "03-session.png"
Click 120 158 -Right    # 第二台主机的右键菜单
Shot "04-menu.png"

Stop-Process -Id $process.Id -Force
