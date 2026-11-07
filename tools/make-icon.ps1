# Generates apps/desktop/src-tauri/icons/{icon.png,icon.ico}
# Simple dark tile with a green "F". Regenerate anytime: powershell -File tools/make-icon.ps1
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing

$root = Split-Path -Parent $PSScriptRoot
$iconsDir = Join-Path $root 'apps\desktop\src-tauri\icons'
New-Item -ItemType Directory -Force -Path $iconsDir | Out-Null

$size = 256
$bmp = New-Object System.Drawing.Bitmap $size, $size
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.TextRenderingHint = [System.Drawing.Text.TextRenderingHint]::AntiAlias
$g.Clear([System.Drawing.Color]::FromArgb(15, 17, 21))

$brush = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::FromArgb(74, 222, 128))
$font = New-Object System.Drawing.Font('Segoe UI', 150, [System.Drawing.FontStyle]::Bold)
$g.DrawString('F', $font, $brush, 60, 24)
$font.Dispose(); $brush.Dispose(); $g.Dispose()

$pngPath = Join-Path $iconsDir 'icon.png'
$bmp.Save($pngPath, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()

# PNG-in-ICO container (single 256x256 entry; 0 in width/height bytes means 256)
$pngBytes = [System.IO.File]::ReadAllBytes($pngPath)
$ms = New-Object System.IO.MemoryStream
$bw = New-Object System.IO.BinaryWriter($ms)
$bw.Write([UInt16]0)                 # reserved
$bw.Write([UInt16]1)                 # type: icon
$bw.Write([UInt16]1)                 # entry count
$bw.Write([Byte]0)                   # width 0 => 256
$bw.Write([Byte]0)                   # height 0 => 256
$bw.Write([Byte]0)                   # palette
$bw.Write([Byte]0)                   # reserved
$bw.Write([UInt16]1)                 # planes
$bw.Write([UInt16]32)                # bpp
$bw.Write([UInt32]$pngBytes.Length)  # bytes in resource
$bw.Write([UInt32]22)                # offset (6 header + 16 entry)
$bw.Write($pngBytes)
[System.IO.File]::WriteAllBytes((Join-Path $iconsDir 'icon.ico'), $ms.ToArray())
$ms.Dispose(); $bw.Dispose()

Write-Output "icons written to $iconsDir"
