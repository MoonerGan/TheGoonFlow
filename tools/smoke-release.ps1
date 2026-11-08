# Smoke-test the release exe: launch it, find the companion port it bound,
# verify /api/handshake, then close it again.
$ErrorActionPreference = 'Continue'
$exe = "C:\Source\TheGoonFlow\target\release\GoonFlow.exe"
Start-Process $exe
Start-Sleep -Seconds 6
$proc = Get-Process GoonFlow -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $proc) { Write-Output "FAIL: process not running"; exit 1 }
$conns = Get-NetTCPConnection -OwningProcess $proc.Id -State Listen -ErrorAction SilentlyContinue
if (-not $conns) { Write-Output "FAIL: no listener found"; Stop-Process -Id $proc.Id; exit 1 }
$port = ($conns | Where-Object { $_.LocalAddress -eq '127.0.0.1' } | Select-Object -First 1).LocalPort
Write-Output "companion port: $port"
$body = curl.exe -s --max-time 5 "http://127.0.0.1:$port/api/handshake"
Write-Output "handshake: $body"
Stop-Process -Id $proc.Id -ErrorAction SilentlyContinue
if ($body -match 'TheGoonFlow') { Write-Output "SMOKE OK" } else { Write-Output "SMOKE FAIL" }
