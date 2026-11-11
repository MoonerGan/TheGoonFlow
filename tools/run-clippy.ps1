# one-shot: dev env + cargo clippy, log to tools\cargo-clippy-<tag>.log
# Why this exists: since #52 goon-core builds image's avif-native feature, and
# dav1d-sys is a system-deps binding. Without tools\dev-env.ps1 sourced first,
# cargo dies in dav1d-sys' build script with a pkg-config stack trace instead of
# a usable answer. Same reason run-test.ps1 and run-release.ps1 exist: bare cargo
# is a trap on Windows.
param([string]$tag = 'm1')
$ErrorActionPreference = 'Continue'
& "$PSScriptRoot\dev-env.ps1"
Set-Location "$PSScriptRoot\.."
$log = "$PSScriptRoot\cargo-clippy-$tag.log"
cargo clippy --workspace --all-targets 2>&1 | Out-File -Encoding utf8 $log
$exit = $LASTEXITCODE
Write-Output "exit=$exit log=$log"
Get-Content $log | Select-String -Pattern '^(error|warning)(\[|:)' | Select-Object -First 40

$warn  = @(Get-Content $log | Select-String -Pattern '^warning: ' | ForEach-Object { $_.Line })
$errs  = @(Get-Content $log | Select-String -Pattern '^(error|error\[)' | ForEach-Object { $_.Line })
@(
  "tag: $tag"
  "cargo clippy --workspace --all-targets exit: $exit"
  "warning lines: $($warn.Count)"
  $warn
  "compile errors: $(if ($errs.Count) { $errs[0] } else { 'none' })"
  "log: $log"
) | Set-Content -Encoding utf8 "$PSScriptRoot\last-clippy-summary.txt"
