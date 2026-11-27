# Generates the Inno Setup wizard background images (light and dark).
#
# WizardBackImageFile is the image behind the page content on every wizard page,
# and WizardBackImageFileDynamicDark is the one picked when Windows is in dark
# mode. Inno sizes that area at 497:360, so these are drawn at twice the
# recommended 596x432 and let Setup scale down; a 1x image blurs at 150% DPI.
#
# The look is a near-flat tint of the panel palette with the Nex mark at low
# contrast in the corner: the installer should read as the same product as the
# panel without competing with the wizard's own controls. Nothing here is
# generated at build time - the PNGs are committed, and this script exists so
# they can be regenerated rather than hand-edited.

param(
  [string]$OutDir = (Join-Path $PSScriptRoot '..\..\apps\assets\installer'),
  [string]$MarkPath = (Join-Path $PSScriptRoot '..\..\apps\assets\brand-redesign\512.png')
)

Add-Type -AssemblyName System.Drawing
$ErrorActionPreference = 'Stop'

$width = 1192
$height = 864

function New-Background {
  param(
    [string]$Path,
    [string]$TopColor,
    [string]$BottomColor,
    [double]$MarkOpacity
  )

  $bitmap = New-Object System.Drawing.Bitmap($width, $height, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
  $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
  $graphics.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::HighQuality
  $graphics.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic

  $rect = New-Object System.Drawing.Rectangle(0, 0, $width, $height)
  $brush = New-Object System.Drawing.Drawing2D.LinearGradientBrush(
    $rect,
    [System.Drawing.ColorTranslator]::FromHtml($TopColor),
    [System.Drawing.ColorTranslator]::FromHtml($BottomColor),
    90.0
  )
  $graphics.FillRectangle($brush, $rect)

  # The mark sits fully inside the frame at low opacity. Bled off the edge it
  # reads as a clipped image rather than a deliberate one.
  $mark = [System.Drawing.Image]::FromFile($MarkPath)
  $size = [int]($width * 0.22)
  $attributes = New-Object System.Drawing.Imaging.ImageAttributes
  $matrix = New-Object System.Drawing.Imaging.ColorMatrix
  $matrix.Matrix33 = [single]$MarkOpacity
  $attributes.SetColorMatrix($matrix)
  $margin = [int]($width * 0.13)
  $target = New-Object System.Drawing.Rectangle(
    ($width - $size - $margin),
    ($height - $size - $margin),
    $size,
    $size
  )
  $graphics.DrawImage(
    $mark,
    $target,
    0, 0, $mark.Width, $mark.Height,
    [System.Drawing.GraphicsUnit]::Pixel,
    $attributes
  )

  $mark.Dispose()
  $attributes.Dispose()
  $brush.Dispose()
  $graphics.Dispose()

  New-Item -ItemType Directory -Force -Path (Split-Path -Parent $Path) | Out-Null
  $bitmap.Save($Path, [System.Drawing.Imaging.ImageFormat]::Png)
  $bitmap.Dispose()
  Write-Host "wrote $Path"
}

# Panel palette: --bg is #121212 dark and #fafafa light in style.css.
New-Background -Path (Join-Path $OutDir 'back-dark.png')  -TopColor '#181818' -BottomColor '#101010' -MarkOpacity 0.13
New-Background -Path (Join-Path $OutDir 'back-light.png') -TopColor '#FBFBFC' -BottomColor '#F1F1F3' -MarkOpacity 0.10
