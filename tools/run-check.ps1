# one-shot: dev env + cargo check (incl. tests), log to tools\cargo-check-<tag>.log
param([string]$tag = 'c0')
$ErrorActionPreference = 'Continue'
& "$PSScriptRoot\dev-env.ps1"
Set-Location "$PSScriptRoot\.."
$log = "$PSScriptRoot\cargo-check-$tag.log"
cargo check --workspace --all-targets 2>&1 | Out-File -Encoding utf8 $log
$exit = $LASTEXITCODE
Write-Output "exit=$exit log=$log"
Get-Content $log | Select-String -Pattern '^(error|error\[|warning: unused)' | Select-Object -First 30
