#Requires -Version 7
# Generates icon.ico (16-256 px) and icon.png (256 px), plus icon-16/32.png previews.
param([Parameter(Mandatory)][string]$OutDir)
Add-Type -AssemblyName System.Drawing
$ErrorActionPreference = 'Stop'

function RoundRect([float]$x, [float]$y, [float]$w, [float]$h, [float]$r) {
    $p = New-Object System.Drawing.Drawing2D.GraphicsPath
    $d = 2 * $r
    $p.AddArc($x, $y, $d, $d, 180, 90)
    $p.AddArc($x + $w - $d, $y, $d, $d, 270, 90)
    $p.AddArc($x + $w - $d, $y + $h - $d, $d, $d, 0, 90)
    $p.AddArc($x, $y + $h - $d, $d, $d, 90, 90)
    $p.CloseFigure()
    $p
}

function Draw([int]$size) {
    $bmp = New-Object System.Drawing.Bitmap $size, $size, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = 'AntiAlias'
    $g.PixelOffsetMode = 'HighQuality'
    $g.ScaleTransform($size / 256, $size / 256)
    $small = $size -le 24
    $C = { param($hex) [System.Drawing.ColorTranslator]::FromHtml($hex) }

    # sheet
    $sheet = RoundRect 36 20 168 216 18
    $g.FillPath((New-Object System.Drawing.SolidBrush (& $C '#FFFFFF')), $sheet)
    # header band, clipped to sheet
    $g.SetClip($sheet)
    $g.FillRectangle((New-Object System.Drawing.SolidBrush (& $C '#2F6FDE')), 36, 20, 168, ($small ? 64 : 56))
    $g.ResetClip()
    $g.DrawPath((New-Object System.Drawing.Pen (& $C '#5B6573'), ($small ? 14 : 8)), $sheet)

    # ruled lines
    $linePen = New-Object System.Drawing.Pen (& $C '#AEB8C4'), ($small ? 18 : 10)
    $linePen.StartCap = $linePen.EndCap = 'Round'
    $ys = $small ? @(130, 186) : @(108, 140, 172, 204)
    foreach ($y in $ys) { $g.DrawLine($linePen, 64, $y, ($small ? 176 : 150), $y) }

    if (-not $small) {
        # pencil, rotated so the tip points down-left onto the sheet
        $g.TranslateTransform(182, 150)
        $g.RotateTransform(45)
        $wood = & $C '#F3D3A6'
        $g.FillRectangle((New-Object System.Drawing.SolidBrush (& $C '#E8743B')), -15, -60, 30, 86)        # body
        $g.FillRectangle((New-Object System.Drawing.SolidBrush (& $C '#C4561F')), 5, -60, 10, 86)          # shade
        $g.FillRectangle((New-Object System.Drawing.SolidBrush (& $C '#9AA3AE')), -15, -74, 30, 14)         # ferrule
        $g.FillPath((New-Object System.Drawing.SolidBrush (& $C '#F28B9B')), (RoundRect -15 -88 30 18 6))  # eraser
        $g.FillPolygon((New-Object System.Drawing.SolidBrush $wood), [System.Drawing.PointF[]]@(
            [System.Drawing.PointF]::new(-15, 26), [System.Drawing.PointF]::new(15, 26), [System.Drawing.PointF]::new(0, 60)))
        $g.FillPolygon((New-Object System.Drawing.SolidBrush (& $C '#2B2B2B')), [System.Drawing.PointF[]]@(
            [System.Drawing.PointF]::new(-6, 46), [System.Drawing.PointF]::new(6, 46), [System.Drawing.PointF]::new(0, 60)))
    }
    $g.Dispose()
    $ms = New-Object System.IO.MemoryStream
    $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
    , $ms.ToArray()
}

$sizes = 16, 24, 32, 48, 64, 128, 256
$pngs = $sizes | % { , (Draw $_) }

# ICO container with PNG-compressed entries
$out = New-Object System.IO.MemoryStream
$w = New-Object System.IO.BinaryWriter $out
$w.Write([uint16]0); $w.Write([uint16]1); $w.Write([uint16]$sizes.Count)
$offset = 6 + 16 * $sizes.Count
for ($i = 0; $i -lt $sizes.Count; $i++) {
    $s = $sizes[$i] % 256   # 256 is stored as 0
    $w.Write([byte]$s); $w.Write([byte]$s); $w.Write([byte]0); $w.Write([byte]0)
    $w.Write([uint16]1); $w.Write([uint16]32)
    $w.Write([uint32]$pngs[$i].Length); $w.Write([uint32]$offset)
    $offset += $pngs[$i].Length
}
foreach ($p in $pngs) { $w.Write($p) }
$w.Flush()

New-Item -ItemType Directory -Force $OutDir | Out-Null
[System.IO.File]::WriteAllBytes((Join-Path $OutDir 'icon.ico'), $out.ToArray())
[System.IO.File]::WriteAllBytes((Join-Path $OutDir 'icon.png'), $pngs[-1])
[System.IO.File]::WriteAllBytes((Join-Path $OutDir 'icon-32.png'), $pngs[2])
[System.IO.File]::WriteAllBytes((Join-Path $OutDir 'icon-16.png'), $pngs[0])
"ok: $($out.Length) bytes"
