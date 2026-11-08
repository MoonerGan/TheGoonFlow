# one-shot: dev env + cargo test, log to tools\cargo-test-<tag>.log
param([string]$tag = 'm56', [string]$filter = '')
$ErrorActionPreference = 'Continue'
& "$PSScriptRoot\dev-env.ps1"
Set-Location "$PSScriptRoot\.."
$log = "$PSScriptRoot\cargo-test-$tag.log"
if ($filter) {
  cargo test --workspace -- --nocapture $filter 2>&1 | Out-File -Encoding utf8 $log
} else {
  cargo test --workspace 2>&1 | Out-File -Encoding utf8 $log
}
Write-Output "exit=$LASTEXITCODE log=$log"
Get-Content $log | Select-String -Pattern '^(error|error\[|test result|failures:)' | Select-Object -First 40
