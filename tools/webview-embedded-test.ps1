# Is the UI actually inside the exe?
#
# tauri's own build script sets `dev = !custom_protocol` (tauri-2.12.5 build.rs:253), and
# a window built in `dev` mode loads `build.devUrl` - http://localhost:1420 - instead of
# the embedded assets. Such an exe starts, gets its window, answers the companion API and
# even reacts to the panic key, but the screen shows WebView2's "Hmmm... can't reach this
# page / ERR_CONNECTION_REFUSED". Nothing else in the artifact checks catches that, so this
# one asks the web engine what page it is really on:
#
#   * a good artifact serves itself from `http://tauri.localhost/` and the document title
#     is the app's ("Flow"),
#   * a broken one is sitting on `http://localhost:<devPort>` with the error page title.
#
# Exits 1 on the broken shape, and says what to fix.
$ErrorActionPreference = 'Continue'
$exe = 'C:\Source\TheGoonFlow\target\release\GoonFlow.exe'
if (-not (Test-Path $exe)) { "FAIL: no exe at $exe"; exit 1 }

# WebView2 only opens a DevTools port when the loader sees this variable, and it must be
# set before the process starts.
$port = $null
foreach ($p in 9223..9243) {
    $l = New-Object System.Net.Sockets.TcpListener([System.Net.IPAddress]::Loopback, $p)
    try { $l.Start(); $port = $p; $l.Stop(); break } catch { } finally { $l = $null }
}
if (-not $port) { "FAIL: no free loopback port to probe on"; exit 1 }
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=$port"

Get-Process GoonFlow -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 500
$p = Start-Process -FilePath $exe -PassThru -NoNewWindow

$page = $null
for ($i = 0; $i -lt 30 -and -not $page; $i++) {
    Start-Sleep -Milliseconds 700
    try {
        $r = Invoke-WebRequest -Uri "http://127.0.0.1:$port/json" -UseBasicParsing -TimeoutSec 3
        $all = ($r.Content | ConvertFrom-Json) | Where-Object { $_.type -eq 'page' }
        if ($all) { $page = $all | Select-Object -First 1 }
    } catch { }
}

Get-Process GoonFlow -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 800
Remove-Item Env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS -ErrorAction SilentlyContinue

if (-not $page) {
    "FAIL: the webview never reported a page target (devtools port $port silent)"
    exit 1
}
"url   = $($page.url)"
"title = $($page.title)"

if ($page.url -match '^https?://localhost:\d+') {
    ""
    "FAIL: the window is loading the vite dev server at $($page.url) - this exe does not"
    "carry its own UI. tauri compiled it in dev mode, which means the custom-protocol"
    "feature was off when it was built (apps/desktop/src-tauri/Cargo.toml [features])."
    "It only looks alive while something listens on that port."
    exit 1
}
if ($page.url -notmatch '^https?://tauri\.localhost') {
    ""
    "FAIL: unexpected webview origin: $($page.url)"
    exit 1
}
if ($page.title -notmatch 'Flow') {
    ""
    "FAIL: the page loaded from tauri.localhost but its title is not the app's: $($page.title)"
    exit 1
}

# Freshness: the window serving its own UI is not enough - it has to be the UI that is in
# apps\desktop\dist right now. tauri-build only watches the dist *directory*
# (tauri-build-2.7.4 codegen/context.rs:96), so a frontend rebuild does not reliably
# re-embed, and a stale exe looks perfectly current. Compare the hashed file names the
# page references with what the binary carries.
$index = 'C:\Source\TheGoonFlow\apps\desktop\dist\index.html'
if (-not (Test-Path $index)) {
    ""
    "FAIL: no $index - the frontend was never built, so this exe cannot carry today's UI"
    exit 1
}
$html = [System.IO.File]::ReadAllText($index)
$names = [regex]::Matches($html, '[A-Za-z0-9_-]+\.(js|css)') | ForEach-Object { $_.Value } | Sort-Object -Unique
if (-not $names) { "FAIL: dist/index.html references no assets"; exit 1 }
$txt = [System.Text.Encoding]::GetEncoding('ISO-8859-1').GetString([System.IO.File]::ReadAllBytes($exe))
$missing = @($names | Where-Object { -not $txt.Contains($_) })
"assets  = $($names -join ', ')"
if ($missing.Count -gt 0) {
    ""
    "FAIL: the exe does not embed today's frontend - missing: $($missing -join ', ')"
    "apps/desktop/dist was rebuilt after this exe; tools/run-release.ps1 builds the frontend"
    "and touches the dist directory so cargo re-embeds it."
    exit 1
}
""
"OK: the exe serves its own UI from tauri.localhost, from today's dist assets"
