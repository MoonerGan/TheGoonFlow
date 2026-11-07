# Imports the MSVC x64 build environment (cl/nmake/LIB/INCLUDE) into the
# current PowerShell session, plus cargo/node tool paths. Use for cargo builds
# that vendor C dependencies (SQLCipher + OpenSSL need more than cc auto-detect).
# Usage: powershell -ExecutionPolicy Bypass -File tools\dev-env.ps1; cargo build ...
$ErrorActionPreference = 'Stop'

$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$vs = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if (-not $vs) { throw "VS Build Tools with VC tools not found" }
$vcvars = Join-Path $vs 'VC\Auxiliary\Build\vcvars64.bat'

cmd /c "`"$vcvars`" && set" | ForEach-Object {
    if ($_ -match '^([^=]+)=(.*)$') { Set-Item -Path "env:$($Matches[1])" -Value $Matches[2] }
}

$env:Path = "$env:USERPROFILE\.cargo\bin;$env:ProgramFiles\nodejs;C:\Strawberry\perl\bin;C:\Strawberry\c\bin;C:\nasm;$env:Path"

$report = foreach ($c in 'cl.exe','perl','nasm') { "$c=$(if (Get-Command $c -ErrorAction SilentlyContinue) { 'yes' } else { 'MISSING' })" }
Write-Output "dev env ready: $($report -join ', ')"
