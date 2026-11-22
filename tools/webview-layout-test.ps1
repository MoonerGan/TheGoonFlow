# The viewer's geometry, measured in a real session (see tools\viewer-layout-probe.js).
#
# 2026-11-22 report: "Fit still doesn't make the image actually fit to the screen
# size... certain images show both [Fit and 1:1] as selected even though the image
# resolution definitely doesn't fit to my screen size height or width wise". The
# arithmetic in lib\view-zoom.ts has unit tests, but a unit test cannot say what
# box the browser gives the picture, nor what the app's own readout claims about
# it. This does: a throwaway vault in %TEMP%, three generated pictures of exact
# pixel size, real clicks on the real buttons, and the numbers the engine drew.
$ErrorActionPreference = 'Continue'
& "$env:ProgramFiles\nodejs\node.exe" 'C:\Source\TheGoonFlow\tools\viewer-layout-probe.js' @args
exit $LASTEXITCODE
