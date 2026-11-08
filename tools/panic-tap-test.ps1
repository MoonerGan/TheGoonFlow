# #23 smoke v2: correct visibility probe (titled windows only, so the
# global-hotkey message window can't confuse MainWindowHandle).
# Tap Ctrl+Alt+L only — NEVER hold, that is the crypto-erase path.
$ErrorActionPreference = "Stop"
Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
using System.Collections.Generic;
public class Win {
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, IntPtr extra);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  public const uint UP = 2;
  public static void Tap() {
    keybd_event(0x11, 0, 0, IntPtr.Zero); keybd_event(0x12, 0, 0, IntPtr.Zero);
    keybd_event(0x4C, 0, 0, IntPtr.Zero);
    keybd_event(0x4C, 0, UP, IntPtr.Zero); keybd_event(0x12, 0, UP, IntPtr.Zero); keybd_event(0x11, 0, UP, IntPtr.Zero);
  }
  public static List<string> TitledWindows(uint target) {
    var list = new List<string>();
    EnumWindows((h, l) => {
      uint pid; GetWindowThreadProcessId(h, out pid);
      if (pid == target) {
        var sb = new StringBuilder(256);
        int n = GetWindowTextW(h, sb, 256);
        if (n > 0) list.Add((IsWindowVisible(h) ? "VIS " : "HID ") + sb.ToString(0, n));
      }
      return true;
    }, IntPtr.Zero);
    return list;
  }
}
"@

$p = Start-Process "C:\Source\TheGoonFlow\target\release\GoonFlow.exe" -RedirectStandardError "$env:TEMP\goon-stderr.log" -PassThru
try {
  Start-Sleep -Seconds 5
  "before      : $([Win]::TitledWindows($p.Id) -join ' | ')"
  [Win]::Tap()
  Start-Sleep -Milliseconds 2500
  "after tap   : $([Win]::TitledWindows($p.Id) -join ' | ')"
  [Win]::Tap()
  Start-Sleep -Milliseconds 1500
  "after tap2  : $([Win]::TitledWindows($p.Id) -join ' | ')"
} finally {
  Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
  "--- stderr ---"
  Get-Content "$env:TEMP\goon-stderr.log" -ErrorAction SilentlyContinue
}
