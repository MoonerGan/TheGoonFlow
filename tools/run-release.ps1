# one-shot: dev env + release build, log to tools\last-build-output.log.
# Stops a running GoonFlow first (cargo cannot replace a live exe), then writes
# tools\last-build-summary.txt with the exe's mtime, size and sha256 so a report
# can say exactly which bytes were tested. Verify with tools\verify-artifact.ps1.
#
# Why the ceremony below exists: a tagged run once printed "exit=0 sha=..." for a
# build that never happened. The build log was still held by another release run,
# Out-File threw, cargo never started, and $LASTEXITCODE/Get-FileHash happily
# reported the *previous* build's numbers. A summary line that cannot be trusted
# is worse than no summary, so every step that could fake a success is checked.
# Usage: powershell -ExecutionPolicy Bypass -File tools\run-release.ps1 -tag m11
param([string]$tag = 'dev')
$ErrorActionPreference = 'Continue'
& "$PSScriptRoot\dev-env.ps1"
Set-Location "$PSScriptRoot\.."
$log = "$PSScriptRoot\last-build-output.log"
$exe = "$PSScriptRoot\..\target\release\GoonFlow.exe"

# One release build at a time: two of them fight over target AND over this log,
# and the loser writes numbers describing someone else's exe.
$lockPath = "$PSScriptRoot\last-build.lock"
try {
    $lock = [System.IO.File]::Open($lockPath, 'CreateNew', 'Write', 'None')
} catch {
    Write-Output "ABORT: another release build holds $lockPath"
    Write-Output "       (remove that file only once you are sure no build is running)"
    exit 2
}
try {
    # Take the log now, so a locked log fails BEFORE cargo is invoked rather than
    # swallowing the build output.
    $probe = [System.IO.File]::Open($log, 'Create', 'Write', 'None')
    $probe.Close()
} catch {
    $lock.Close()
    Remove-Item $lockPath -Force -ErrorAction SilentlyContinue
    Write-Output "ABORT: cannot open $log for writing: $($_.Exception.Message)"
    exit 2
}

try {
    $pre = $null
    if (Test-Path $exe) {
        $preItem = Get-Item $exe
        $pre = "$($preItem.LastWriteTime.ToString('s'))/$($preItem.Length)"
    }

    $running = @(Get-Process GoonFlow -ErrorAction SilentlyContinue)
    if ($running.Count) {
        Write-Output "stopping running GoonFlow (PID $($running.Id -join ','))"
        $running | ForEach-Object { $_.Kill() }
        Start-Sleep -Seconds 2
    }

    cargo build --release --workspace --message-format short 2>&1 | Out-File -Encoding utf8 $log
    $exit = $LASTEXITCODE

    $lines = @(Get-Content $log -ErrorAction SilentlyContinue)
    # cargo always ends with "Finished `release` profile"; no such line means the
    # build did not run to completion, so nothing below may be reported as new.
    $finished = @($lines | Select-String -Pattern 'Finished `release` profile').Count
    $buildRan = ($lines.Count -gt 0) -and ($finished -gt 0)

    if (Test-Path $exe) {
        $sha = (Get-FileHash $exe -Algorithm SHA256).Hash
        $item = Get-Item $exe
        $sha | Set-Content -Encoding utf8 "$PSScriptRoot\last-exe-sha.txt"
    } else {
        $sha = 'MISSING'; $item = $null
    }
    $post = if ($item) { "$($item.LastWriteTime.ToString('s'))/$($item.Length)" } else { 'none' }

    @(
        "tag: $tag"
        "cargo build --release exit: $exit"
        "build ran to completion: $(if ($buildRan) { 'yes' } else { 'NO - the numbers below are the previous build' })"
        "log lines: $($lines.Count)"
        "exe: $exe"
        "exe before: $($pre)"
        "exe after:  $($post)"
        "exe rebuilt: $(if ($pre -and $post -ne $pre) { 'yes' } elseif ($pre) { 'no (cargo had nothing to do)' } else { 'new' })"
        "exe mtime: $($item.LastWriteTime)"
        "exe bytes: $($item.Length)"
        "exe sha256: $sha"
        "errors: $(@($lines | Select-String -Pattern '^error|^error\[').Count)"
        "warnings: $(@($lines | Select-String -Pattern '^warning:').Count)"
        "built-at: $(Get-Date -Format 's')"
    ) | Set-Content -Encoding utf8 "$PSScriptRoot\last-build-summary.txt"

    Write-Output "exit=$exit buildRan=$buildRan sha=$sha"
    if (-not $buildRan) { exit 3 }
} finally {
    $lock.Close()
    Remove-Item $lockPath -Force -ErrorAction SilentlyContinue
}
