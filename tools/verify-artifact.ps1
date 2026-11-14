$ErrorActionPreference = 'Continue'
Set-Location C:\Source\TheGoonFlow
$exe = 'C:\Source\TheGoonFlow\target\release\GoonFlow.exe'
function Read-Cfg([string]$dir) {
  $f = Join-Path $dir 'flow.cfg'
  if (-not (Test-Path $f)) { return $null }
  $o = @{}
  foreach ($l in [System.IO.File]::ReadAllLines($f)) {
    $i = $l.IndexOf('='); if ($i -gt 0) { $o[$l.Substring(0, $i)] = $l.Substring($i + 1) }
  }
  return $o
}
"exe sha256 = " + ((Get-FileHash $exe -Algorithm SHA256).Hash).ToUpper()
"git tip    = " + (& 'C:\Program Files\Git\bin\git.exe' -C C:\Source\TheGoonFlow rev-parse HEAD)
""
"=== 0. the exe must carry its own UI (no vite dev server) ==="
& 'C:\Source\TheGoonFlow\tools\webview-embedded-test.ps1' 2>&1 | Out-String -Width 200
if ($LASTEXITCODE -ne 0) { "step 0 FAILED - do not hand this exe over"; exit 1 }

"=== 1. smoke on the real vault (window find + panic tap hide/reveal) ==="
& 'C:\Source\TheGoonFlow\tools\smoke-release.ps1' 2>&1 | Out-String -Width 200

"=== 2. close-the-window-exits + second launch hands off (scratch vault) ==="
& 'C:\Source\TheGoonFlow\tools\close-exit-test.ps1' 2>&1 | Out-String -Width 200

"=== 3. panic tap on the scratch copy (real window + real haptics) ==="
$sandbox = Join-Path $env:TEMP 'goon-panic-sandbox'
New-Item -ItemType Directory -Force -Path $sandbox | Out-Null
Copy-Item $exe (Join-Path $sandbox 'GoonFlow.exe') -Force
Start-Process -FilePath (Join-Path $sandbox 'GoonFlow.exe')
$cfg = $null
for ($i = 0; $i -lt 20 -and -not $cfg; $i++) {
  Start-Sleep -Seconds 2
  $f = Join-Path $sandbox 'flow.cfg'
  if (Test-Path $f) {
    $o = @{}
    foreach ($l in [System.IO.File]::ReadAllLines($f)) {
      $j = $l.IndexOf('='); if ($j -gt 0) { $o[$l.Substring(0, $j)] = $l.Substring($j + 1) }
    }
    $cfg = $o
  }
}
if (-not $cfg) { "no flow.cfg appeared in the scratch folder - skipping the API part"; }
else {
  "scratch companion port = $($cfg.api_port)"
  & 'C:\Source\TheGoonFlow\tools\panic-tap-test.ps1' -ApiPort ([int]$cfg.api_port) 2>&1 | Out-String -Width 200
}
Get-Process GoonFlow -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Seconds 2
"processes left = $((Get-Process GoonFlow -ErrorAction SilentlyContinue | Measure-Object).Count)"
