# #23 hold-escalation live check.
#
#   !! SAFETY !! This injects a real 4-second Ctrl+Alt+L hold. It is safe
#   ONLY because it targets a GoonFlow build that has NO vault attached
#   (no flow.cfg / goonflow-vault beside the exe), so nuke_core is a no-op
#   error. NEVER run this against an instance that can see a real vault.
#
# What it proves (against target\release\GoonFlow.exe):
#   - holding >= 3 s escalates: stderr shows "panic hold: crypto-erasing
#     vault" and the window hides *during* the hold
#   - the key-up release storm stays inert: window stays HID after release
$ErrorActionPreference = "Stop"
$exe = if ($args.Count -gt 0) { $args[0] } else { "C:\Source\TheGoonFlow\target\release\GoonFlow.exe" }

# Refuse to run if a vault pointer sits beside the exe.
$dir = Split-Path $exe -Parent
if ((Test-Path (Join-Path $dir "flow.cfg")) -or (Test-Path (Join-Path $dir "goonflow-vault"))) {
  Write-Output "ABORT: a vault is attached beside $exe. Move this script away or use a clean build."
  exit 1
}

Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
using System.Collections.Generic;
public class WinH {
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, IntPtr extra);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  public const uint UP = 2;
  public static void Down() { keybd_event(0x11,0,0,IntPtr.Zero); keybd_event(0x12,0,0,IntPtr.Zero); keybd_event(0x4C,0,0,IntPtr.Zero); }
  public static void UpKeys() { keybd_event(0x4C,0,UP,IntPtr.Zero); keybd_event(0x12,0,UP,IntPtr.Zero); keybd_event(0x11,0,UP,IntPtr.Zero); }
  public static List<string> Titled(uint pid) {
    var list = new List<string>();
    EnumWindows((h,l)=>{ uint p; GetWindowThreadProcessId(h,out p); if(p==pid){ var sb=new StringBuilder(256); int n=GetWindowTextW(h,sb,256); if(n>0) list.Add((IsWindowVisible(h)?"VIS ":"HID ")+sb.ToString(0,n)); } return true; }, IntPtr.Zero);
    return list;
  }
}
"@

$log = Join-Path $env:TEMP "goon-hold.log"
$p = Start-Process $exe -RedirectStandardError $log -PassThru
try {
  [WinH]::UpKeys()            # clear any stuck modifier from a prior run
  Start-Sleep -Seconds 5
  "before hold : $([WinH]::Titled($p.Id) -join ' | ')"
  [WinH]::Down()
  Start-Sleep -Seconds 4      # > PANIC_HOLD_MS (3 s)
  "during hold : $([WinH]::Titled($p.Id) -join ' | ')"
  [WinH]::UpKeys()
  Start-Sleep -Milliseconds 1500
  "after release: $([WinH]::Titled($p.Id) -join ' | ')   (must stay HID - storm inert)"
  $seen = Get-Content $log -ErrorAction SilentlyContinue | Select-String 'crypto-erasing'
  if ($seen) { "PANIC HOLD OK (escalation observed; nuke_core no-op, no vault attached)" }
  else { "PANIC HOLD INCONCLUSIVE (no escalation line in stderr)" }
} finally {
  [WinH]::UpKeys()
  Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
  "--- stderr ---"
  Get-Content $log -ErrorAction SilentlyContinue
}
