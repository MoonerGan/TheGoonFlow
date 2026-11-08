# Proves the panic_gen guard (#23): a stale escalation watcher from a quick
# tap must NOT fire when a NEW hold straddles the tap's 3 s deadline but
# releases before its own 3 s is up. Timeline (t = seconds from launch):
#   t=0.0  tap down/up          -> episode 1 (gen 1), watcher A armed for t=3
#   t=2.0  fresh press held     -> episode 2 (gen 2), watcher B armed for t=5
#   t=3.7  release              -> episode 2 was only 1.7 s old: a TAP, not erase
#   t=3.0  watcher A deadline   -> must be SUPPRESSED (gen mismatch)
# Without panic_gen, watcher A would see panic_hold && modifiers-down at t=3
# (episode 2 still holding) and crypto-erase after a 1 s hold. Fail mode here
# is irreversible, so this ships as a live regression check.
#
#   !! SAFETY !! Safe only against a build with NO vault attached beside the
#   exe (no flow.cfg / goonflow-vault): nuke_core is then a no-op even if the
#   guard regressed. The script refuses to run otherwise.
$ErrorActionPreference = "Stop"
$exe = if ($args.Count -gt 0) { $args[0] } else { "C:\Source\TheGoonFlow\target\release\GoonFlow.exe" }
$dir = Split-Path $exe -Parent
if ((Test-Path (Join-Path $dir "flow.cfg")) -or (Test-Path (Join-Path $dir "goonflow-vault"))) {
  Write-Output "ABORT: a vault is attached beside $exe. Use a clean build."; exit 1
}
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class W2 {
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, IntPtr extra);
  public const uint UP = 2;
  public static void Down() { keybd_event(0x11,0,0,IntPtr.Zero); keybd_event(0x12,0,0,IntPtr.Zero); keybd_event(0x4C,0,0,IntPtr.Zero); }
  public static void KeyUp() { keybd_event(0x4C,0,UP,IntPtr.Zero); keybd_event(0x12,0,UP,IntPtr.Zero); keybd_event(0x11,0,UP,IntPtr.Zero); }
}
"@
$log = Join-Path $env:TEMP "goon-stale.log"
$p = Start-Process $exe -RedirectStandardError $log -PassThru
try {
  [W2]::KeyUp(); Start-Sleep -Seconds 5
  [W2]::Down(); Start-Sleep -Milliseconds 150; [W2]::KeyUp()   # episode 1: tap
  Start-Sleep -Milliseconds 1850                               # t ~= 2.0
  [W2]::Down(); Start-Sleep -Milliseconds 1700; [W2]::KeyUp()  # episode 2: 1.7 s hold
  Start-Sleep -Seconds 3                                       # both deadlines passed
  $seen = Get-Content $log -ErrorAction SilentlyContinue | Select-String 'crypto-erasing'
  if ($seen) { "STALE-WATCHER FAIL: a nuke fired (panic_gen regressed)" }
  else { "STALE-WATCHER PASS: no nuke (panic_gen suppressed the stale watcher)" }
} finally {
  [W2]::KeyUp()
  Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
  "--- stderr ---"; Get-Content $log -ErrorAction SilentlyContinue
}
