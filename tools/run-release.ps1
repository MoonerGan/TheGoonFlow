# one-shot: dev env + release build, log to tools\last-build-output.log.
# Stops a running GoonFlow first (cargo cannot replace a live exe), then writes
# tools\last-build-summary.txt with the exe's mtime, size and sha256 so a report
# can say exactly which bytes were tested. Verify with tools\verify-artifact.ps1.
# Usage: powershell -ExecutionPolicy Bypass -File tools\run-release.ps1 -tag m11
param([string]$tag = 'dev')
$ErrorActionPreference = 'Continue'
& "$PSScriptRoot\dev-env.ps1"
Set-Location "$PSScriptRoot\.."
$log = "$PSScriptRoot\last-build-output.log"
$exe = "$PSScriptRoot\..\target\release\GoonFlow.exe"

$running = @(Get-Process GoonFlow -ErrorAction SilentlyContinue)
if ($running.Count) {
  Write-Output "stopping running GoonFlow (PID $($running.Id -join ','))"
  $running | ForEach-Object { $_.Kill() }
  Start-Sleep -Seconds 2
}

cargo build --release --workspace --message-format short 2>&1 | Out-File -Encoding utf8 $log
$exit = $LASTEXITCODE
if (Test-Path $exe) {
  $sha = (Get-FileHash $exe -Algorithm SHA256).Hash
  $item = Get-Item $exe
  $sha | Set-Content -Encoding utf8 "$PSScriptRoot\last-exe-sha.txt"
} else {
  $sha = 'MISSING'; $item = $null
}
@(
  "tag: $tag"
  "cargo build --release exit: $exit"
  "exe: $exe"
  "exe mtime: $($item.LastWriteTime)"
  "exe bytes: $($item.Length)"
  "exe sha256: $sha"
  "errors: $(@(Get-Content $log | Select-String -Pattern '^error|^error\[').Count)"
  "warnings: $(@(Get-Content $log | Select-String -Pattern '^warning:').Count)"
  "built-at: $(Get-Date -Format 's')"
) | Set-Content -Encoding utf8 "$PSScriptRoot\last-build-summary.txt"
Write-Output "exit=$exit sha=$sha"
