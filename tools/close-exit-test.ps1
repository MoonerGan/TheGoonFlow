# #41 live check: what actually happens when you close GoonFlow, and when the
# panic key hides it. Run against a *copy* of the release exe in a scratch
# folder so no real vault is ever attached (no flow.cfg beside it).
#
#   Q1 close the window (X / Alt+F4) -> does the process exit?
#   Q2 panic-tap hides it           -> is the process still alive, and is the
#                                      window reachable again without the hotkey?
#   Q3 launch a second copy while a hidden one runs -> one instance or two?
#
# Ctrl+Alt+L is the *tap* path only. Never hold it: holding is the erase path.
$ErrorActionPreference = "Stop"
Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
using System.Collections.Generic;
public class Win2 {
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, IntPtr extra);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern IntPtr SendMessageW(IntPtr h, uint msg, IntPtr w, IntPtr l);
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  public const uint UP = 2, WM_CLOSE = 0x10;
  public static void TapPanic() {
    keybd_event(0x11, 0, 0, IntPtr.Zero); keybd_event(0x12, 0, 0, IntPtr.Zero);
    keybd_event(0x4C, 0, 0, IntPtr.Zero);
    keybd_event(0x4C, 0, UP, IntPtr.Zero); keybd_event(0x12, 0, UP, IntPtr.Zero); keybd_event(0x11, 0, UP, IntPtr.Zero);
  }
  public static List<IntPtr> Titled(uint target) {
    var list = new List<IntPtr>();
    EnumWindows((h, l) => {
      uint pid; GetWindowThreadProcessId(h, out pid);
      if (pid == target) {
        var sb = new StringBuilder(256);
        if (GetWindowTextW(h, sb, 256) > 0) list.Add(h);
      }
      return true;
    }, IntPtr.Zero);
    return list;
  }
  public static string Describe(uint target) {
    var parts = new List<string>();
    EnumWindows((h, l) => {
      uint pid; GetWindowThreadProcessId(h, out pid);
      if (pid == target) {
        var sb = new StringBuilder(256);
        int n = GetWindowTextW(h, sb, 256);
        if (n > 0) parts.Add((IsWindowVisible(h) ? "VIS " : "HID ") + sb.ToString(0, n));
      }
      return true;
    }, IntPtr.Zero);
    return string.Join(" | ", parts);
  }
}
"@

$src = "C:\Source\TheGoonFlow\target\release\GoonFlow.exe"
$sandbox = Join-Path $env:TEMP "goon-exit-test"
Remove-Item $sandbox -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Path $sandbox -Force | Out-Null
$exe = Join-Path $sandbox "GoonFlow.exe"
Copy-Item $src $exe
"exe copy: $exe  (no flow.cfg here -> no vault is attached)"
""

function Alive($proc) {
  $p = Get-Process -Id $proc.Id -ErrorAction SilentlyContinue
  if (-not $p) { return $null }
  return $p
}

function StartOne() {
  $p = Start-Process $exe -RedirectStandardError "$sandbox\stderr.log" -PassThru
  Start-Sleep -Seconds 5
  return $p
}

# ---- Q1: does closing the window exit the process? --------------------------
$p = StartOne
"Q1  launched pid $($p.Id)"
"    windows      : $([Win2]::Describe($p.Id))"
$wins = [Win2]::Titled($p.Id)
if ($wins.Count -eq 0) { "    NO titled window - test inconclusive"; Stop-Process -Id $p.Id -Force }
else {
  [void][Win2]::SendMessageW($wins[0], [Win2]::WM_CLOSE, [IntPtr]::Zero, [IntPtr]::Zero)
  "    WM_CLOSE sent to hwnd $($wins[0])"
  Start-Sleep -Seconds 4
  $alive = Get-Process -Id $p.Id -ErrorAction SilentlyContinue
  "Q1  after close  : process $(if ($alive) { 'STILL RUNNING (pid ' + $p.Id + ')' } else { 'gone' })"
  if ($alive) { Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue }
}
Start-Sleep -Seconds 2

# ---- Q2/Q3: hidden by panic tap, then a second launch ----------------------
$p = StartOne
"Q2  launched pid $($p.Id)"
"    before tap   : $([Win2]::Describe($p.Id))"
[Win2]::TapPanic()
Start-Sleep -Milliseconds 2500
"    after tap    : $([Win2]::Describe($p.Id))"
$alive = Get-Process -Id $p.Id -ErrorAction SilentlyContinue
"Q2  after hide   : process $(if ($alive) { 'STILL RUNNING (pid ' + $p.Id + ')' } else { 'gone' })"

# Q3 (the #41 fix): the second launch must hand off to the running instance —
# the hidden window comes back and only ONE process is left alive.
$q = StartOne
Start-Sleep -Seconds 3
"Q3  second launch: pid $($q.Id); after handoff the window is $([Win2]::Describe($p.Id))"
$still = Get-Process -Id $p.Id -ErrorAction SilentlyContinue
$second = Get-Process -Id $q.Id -ErrorAction SilentlyContinue
$all = @(Get-Process GoonFlow -ErrorAction SilentlyContinue)
"Q3  first  instance (hidden one): $(if ($still) { 'alive, pid ' + $p.Id } else { 'gone' })"
"Q3  second instance             : $(if ($second) { 'STILL RUNNING (pid ' + $q.Id + ') - single instance NOT working' } else { 'exited after handing off' })"
"Q3  live GoonFlow processes     : $($all.Count) (pids $($all.Id -join ', '))"

"--- stderr of the last instance ---"
Get-Content "$sandbox\stderr.log" -ErrorAction SilentlyContinue | Select-Object -First 12
Get-Process GoonFlow -ErrorAction SilentlyContinue | ForEach-Object { Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue }
""
"done (everything stopped)"
