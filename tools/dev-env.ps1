# Imports the MSVC x64 build environment (cl/nmake/LIB/INCLUDE) into the
# current PowerShell session, plus cargo/node tool paths. Use for cargo builds
# that vendor C dependencies (SQLCipher + OpenSSL need more than cc auto-detect).
# Also points system-deps at the static dav1d used by AVIF decoding (#52).
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

# dav1d (#52): goon-core turns on image's avif-native feature, which links dav1d
# through dav1d-sys - a system-deps binding with no bundled C source. The probe
# fails here (only Strawberry Perl's pkg-config.bat, which Rust cannot exec) and
# the build-from-source fallback needs meson + ninja (meson is not installed), so
# hand system-deps the prebuilt STATIC dav1d. Static only: the exe is one file.
$dav1dRoot = if ($env:GOON_DAV1D_DIR) { $env:GOON_DAV1D_DIR } else { Join-Path $env:LOCALAPPDATA 'goonflow-build\dav1d-x64-static' }
$dav1d = (Test-Path (Join-Path $dav1dRoot 'lib\dav1d.lib')) -and (Test-Path (Join-Path $dav1dRoot 'include\dav1d\dav1d.h'))
if ($dav1d) {
    $dav1dVars = [ordered]@{
        SYSTEM_DEPS_DAV1D_NO_PKG_CONFIG  = '1'
        SYSTEM_DEPS_DAV1D_LIB            = 'dav1d'
        SYSTEM_DEPS_DAV1D_SEARCH_NATIVE  = (Join-Path $dav1dRoot 'lib')
        SYSTEM_DEPS_DAV1D_SEARCH_INCLUDE = (Join-Path $dav1dRoot 'include')
        SYSTEM_DEPS_DAV1D_LINK           = 'static'
    }
    foreach ($name in $dav1dVars.Keys) {
        if (-not (Test-Path "env:$name")) { Set-Item -Path "env:$name" -Value $dav1dVars[$name] }
    }
}

$report = foreach ($c in 'cl.exe','perl','nasm') { "$c=$(if (Get-Command $c -ErrorAction SilentlyContinue) { 'yes' } else { 'MISSING' })" }
$report += "dav1d=$(if ($dav1d) { 'static' } else { 'missing' })"
Write-Output "dev env ready: $($report -join ', ')"

if (-not $dav1d) {
    Write-Output @(
        ''
        '!!! dav1d MISSING. The AVIF feature (#52) makes dav1d a hard build'
        '!!! requirement: dav1d-sys probes dav1d >= 1.3.0 with pkg-config, that'
        '!!! probe fails on this machine, and its build-from-source fallback needs'
        '!!! meson + ninja; meson is not installed here. Three fixes - pick one:'
        "!!!   checked for lib\dav1d.lib + include\dav1d\dav1d.h under: $dav1dRoot"
        '!!!   (i)   vcpkg install dav1d:x64-windows-static, then copy'
        '!!!         installed\x64-windows-static\include\dav1d and'
        '!!!         installed\x64-windows-static\lib\dav1d.lib into'
        "!!!           $((Join-Path $env:LOCALAPPDATA 'goonflow-build\dav1d-x64-static'))"
        '!!!         keeping the include\ + lib\ folder layout'
        '!!!   (ii)  set GOON_DAV1D_DIR to any dav1d >= 1.3 install that has an'
        '!!!         include\ folder and a lib\ folder next to each other'
        '!!!   (iii) set the five SYSTEM_DEPS_DAV1D_* vars yourself:'
        '!!!         SYSTEM_DEPS_DAV1D_NO_PKG_CONFIG=1, SYSTEM_DEPS_DAV1D_LIB=dav1d,'
        '!!!         SYSTEM_DEPS_DAV1D_SEARCH_NATIVE=<root>\lib,'
        '!!!         SYSTEM_DEPS_DAV1D_SEARCH_INCLUDE=<root>\include,'
        '!!!         SYSTEM_DEPS_DAV1D_LINK=static'
        '!!! A SHARED dav1d (the plain x64-windows triplet) is NOT acceptable - the'
        '!!! exe must stay single-file with no DLLs beside it. Check after a release'
        '!!! build: dumpbin /dependents target\release\GoonFlow.exe must list no'
        '!!! dav1d*.dll.'
        ''
    )
}
