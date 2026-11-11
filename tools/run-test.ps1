# one-shot: dev env + cargo test, log to tools\cargo-test-<tag>.log
# Also writes tools\last-test-summary.txt: pass/fail counts copied straight out of
# cargo's own output, so any test claim in a report can be checked against a file.
param([string]$tag = 'm56', [string]$filter = '')
$ErrorActionPreference = 'Continue'
& "$PSScriptRoot\dev-env.ps1"
Set-Location "$PSScriptRoot\.."
$log = "$PSScriptRoot\cargo-test-$tag.log"
if ($filter) {
  cargo test --workspace --all-targets -- --nocapture $filter 2>&1 | Out-File -Encoding utf8 $log
} else {
  cargo test --workspace --all-targets 2>&1 | Out-File -Encoding utf8 $log
}
$exit = $LASTEXITCODE
Write-Output "exit=$exit log=$log"
Get-Content $log | Select-String -Pattern '^(error|error\[|test result|failures:)' | Select-Object -First 40

$results = @(Get-Content $log | Select-String -Pattern '^test result:' | ForEach-Object { $_.Line })
$errors  = @(Get-Content $log | Select-String -Pattern '^(error|error\[)' | ForEach-Object { $_.Line })
@(
  "tag: $tag"
  "cargo test --workspace exit: $exit"
  "test result lines:"
  $results
  "compile errors: $(if ($errors.Count) { $errors[0] } else { 'none' })"
  "log: $log"
) | Set-Content -Encoding utf8 "$PSScriptRoot\last-test-summary.txt"
