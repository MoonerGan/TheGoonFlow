# Window probe: lists every top-level window of a running GoonFlow with its size,
# position and extended styles. Answers "is there a phantom window / is this thing
# on screen?" questions (#41: the panic-hidden window, and the single-instance
# hand-off window dev.goonflow.desktop-siw).
#
#   vis      IsWindowVisible  (a layered + transparent helper window can be
#            "visible" and still paint nothing -- check with a screen-region diff)
#   tool     WS_EX_TOOLWINDOW  -> no taskbar button, not in Alt+Tab
#   noact    WS_EX_NOACTIVATE  -> can never take focus
#
# Usage: powershell -ExecutionPolicy Bypass -File tools\window-probe.ps1 [-Name GoonFlow]
param([string]$Name = 'GoonFlow')
$ErrorActionPreference = 'Stop'
Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class WinProbe {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern IntPtr GetWindowLongPtrW(IntPtr h, int idx);
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  public struct RECT { public int left, top, right, bottom; }
}
"@
$procs = @(Get-Process $Name -ErrorAction SilentlyContinue)
if ($procs.Count -eq 0) { Write-Output "no '$Name' process running"; exit 1 }
$ids = @($procs | Select-Object -ExpandProperty Id)

# Results go into a list: output written from inside the EnumWindows callback is
# swallowed by the native call.
$res = New-Object System.Collections.ArrayList
$cb = [WinProbe+EnumProc] {
  param($h, $l)
  $own = 0
  [void][WinProbe]::GetWindowThreadProcessId($h, [ref]$own)
  if ($ids -contains [int]$own) {
    $sb = New-Object System.Text.StringBuilder 256
    $n = [WinProbe]::GetWindowTextW($h, $sb, 256)
    $r = New-Object WinProbe+RECT
    [void][WinProbe]::GetWindowRect($h, [ref]$r)
    $ex = [WinProbe]::GetWindowLongPtrW($h, -20).ToInt64()   # GWL_EXSTYLE
    [void]$res.Add(("pid={0} vis={1,-5} icon={2,-5} {3}x{4} at ({5},{6}) tool={7} noact={8} title='{9}'" -f `
      $own, [WinProbe]::IsWindowVisible($h), [WinProbe]::IsIconic($h),
      ($r.right - $r.left), ($r.bottom - $r.top), $r.left, $r.top,
      (($ex -band 0x80L) -ne 0), (($ex -band 0x08000000L) -ne 0),
      $sb.ToString(0, [Math]::Max($n, 0))))
  }
  return $true
}
[void][WinProbe]::EnumWindows($cb, [IntPtr]::Zero)
Write-Output "processes: $($ids -join ', ')"
$res | ForEach-Object { Write-Output $_ }
