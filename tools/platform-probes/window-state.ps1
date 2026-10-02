# Panel geometry persistence (core's window state, label "clipboard").
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File tools\platform-probes\window-state.ps1 [-Exe <KwikPaste.exe>]
#
# Uses only the probe instance's own data directory
# (%LOCALAPPDATA%\com.fastthree.kwikpaste.native-dev.selftest-platform\dev\state), which it clears first.
# No keyboard or mouse input is injected; the panel is shown and hidden through the single-instance path
# and resized with SetWindowPos from this script.
#
# 1. 1.x migration: a 1.x window-state.json (physical pixels) is converted on startup into
#    window-state.gpui.json (logical pixels with the monitor scale); with clipboard.window.position =
#    remember the panel's outer top-left and content size match the 1.x values.
# 2. A bigger size set while shown is saved on hide and used again on the next show.
# 3. After a restart the panel comes back at the same rect (remember).
# 4. center: the content is centered in the work area of the monitor under the cursor.
# 5. A remembered position on no monitor any more falls back to the center (1.x behavior).
param(
    [string]$Exe = ''
)

. "$PSScriptRoot\common.ps1"
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class Resize {
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int w, int hgt, uint flags);
}
"@

$Exe = Resolve-AppExe $Exe
$results = New-ResultDir 'window-state'
$report = New-Object System.Collections.Generic.List[string]
function Note([string]$Line) { $report.Add($Line); Write-Host $Line }
$failures = New-Object System.Collections.Generic.List[string]
function Check([string]$Name, [bool]$Passed, [string]$Detail = '') {
    Note ("  {0}: {1}{2}" -f $Name, $(if ($Passed) { 'ok' } else { 'FAIL' }), $(if ($Detail) { " ($Detail)" } else { '' }))
    if (-not $Passed) { $failures.Add($Name) }
}
function Near([int]$A, [int]$B) { return [math]::Abs($A - $B) -le 1 }

$stateDir = Join-Path $env:LOCALAPPDATA 'com.fastthree.kwikpaste.native-dev.selftest-platform\dev\state'
$nativeFile = Join-Path $stateDir 'window-state.gpui.json'
$legacyFile = Join-Path $stateDir 'window-state.json'

function Show-Panel {
    Send-ProbeCommand $Exe '--selftest-show'
    $shown = Wait-ProbeEvent 'shown' 5000
    if ($null -eq $shown) { throw 'The panel did not show.' }
    [Probe]::Pump(150)
    return [pscustomobject]@{ Window = [Probe]::WindowRect($panel); Client = [Probe]::ClientRectOnScreen($panel); Dpi = [int]$shown.dpi }
}
function Hide-Panel {
    Send-ProbeCommand $Exe '--selftest-hide'
    [void](Wait-ProbeEvent 'hidden' 5000)
    [Probe]::Pump(300)
}
function Set-Position([string]$Position) {
    Send-ProbeCommand $Exe ('--selftest-settings={"clipboard":{"window":{"position":"' + $Position + '"}}}')
    [Probe]::Pump(300)
}
function Read-Native { return (Get-Content $nativeFile -Raw | ConvertFrom-Json).clipboard }
# Each launch logs into its own folder: the probe log of an earlier launch would answer the wait for "ready".
function New-RunDir([int]$Run) { return (New-Item -ItemType Directory -Force -Path (Join-Path $results "run$Run")).FullName }

Assert-Desktop -IdleSeconds 0
$app = $null
try {
    if (Test-Path $stateDir) { Remove-Item (Join-Path $stateDir '*') -Force }
    New-Item -ItemType Directory -Force -Path $stateDir | Out-Null
    $work = [Probe]::PrimaryWorkArea()
    $legacy = @{ x = $work[0] + 300; y = $work[1] + 200; width = 800; height = 1000 }
    @{ clipboard = $legacy } | ConvertTo-Json | Set-Content $legacyFile -Encoding ASCII
    Note "1.x state: $($legacy | ConvertTo-Json -Compress)"

    Note '1. migration from 1.x'
    $app = Start-ProbeApp $Exe (New-RunDir 1) -KeepState
    $panel = $app.Hwnd
    Check 'native state file written at startup' (Test-Path $nativeFile)
    $native = Read-Native
    $scale = [double]$native.scale
    Note "  native state: $($native | ConvertTo-Json -Compress)"
    Check 'converted to logical pixels' ((Near ([int]($native.x * $scale)) $legacy.x) -and (Near ([int]($native.width * $scale)) $legacy.width))
    Check '1.x file left untouched' ((Get-Content $legacyFile -Raw | ConvertFrom-Json).clipboard.width -eq $legacy.width)
    Set-Position 'remember'
    $shown = Show-Panel
    Note "  shown: window $([Probe]::Rect($shown.Window)) client $([Probe]::Rect($shown.Client))"
    Check 'outer top-left at the 1.x position' ((Near $shown.Window[0] $legacy.x) -and (Near $shown.Window[1] $legacy.y))
    Check 'content size from the 1.x state' ((Near ($shown.Client[2] - $shown.Client[0]) $legacy.width) -and (Near ($shown.Client[3] - $shown.Client[1]) $legacy.height))

    Note '2. a bigger size is saved on hide'
    $biggerWidth = $shown.Window[2] - $shown.Window[0] + 120
    $biggerHeight = $shown.Window[3] - $shown.Window[1] + 80
    [void][Resize]::SetWindowPos($panel, [IntPtr]::Zero, $shown.Window[0], $shown.Window[1], $biggerWidth, $biggerHeight, 0x0014)
    [Probe]::Pump(200)
    $resized = [Probe]::ClientRectOnScreen($panel)
    Hide-Panel
    $native = Read-Native
    Note "  saved: $($native | ConvertTo-Json -Compress)"
    Check 'saved content size' ((Near ([int]($native.width * $native.scale)) ($resized[2] - $resized[0])) -and (Near ([int]($native.height * $native.scale)) ($resized[3] - $resized[1])))
    $again = Show-Panel
    Check 'shown again with the saved rect' ([Probe]::Same($again.Client, $resized)) "client $([Probe]::Rect($again.Client)) expected $([Probe]::Rect($resized))"
    Hide-Panel

    Note '3. after a restart'
    Stop-ProbeApp $app $Exe
    $app = Start-ProbeApp $Exe (New-RunDir 2) -KeepState
    $panel = $app.Hwnd
    $restarted = Show-Panel
    Check 'same rect after a restart' ([Probe]::Same($restarted.Client, $resized)) "client $([Probe]::Rect($restarted.Client)) expected $([Probe]::Rect($resized))"
    Hide-Panel

    Note '4. center'
    Set-Position 'center'
    $cursor = [Probe]::CursorOrPrimaryCenter()
    $centered = Show-Panel
    $screen = [System.Windows.Forms.Screen]::FromPoint((New-Object System.Drawing.Point($cursor.X, $cursor.Y))).WorkingArea
    $dx = ($centered.Client[0] - $screen.Left) - ($screen.Right - $centered.Client[2])
    $dy = ($centered.Client[1] - $screen.Top) - ($screen.Bottom - $centered.Client[3])
    Check 'centered in the cursor monitor work area' (([math]::Abs($dx) -le 1) -and ([math]::Abs($dy) -le 1)) "client $([Probe]::Rect($centered.Client)) work $screen"
    Hide-Panel

    Note '5. remembered position on no monitor'
    Stop-ProbeApp $app $Exe
    $native = Read-Native
    $native.x = -100000
    $native.y = -100000
    @{ clipboard = $native } | ConvertTo-Json | Set-Content $nativeFile -Encoding ASCII
    $app = Start-ProbeApp $Exe (New-RunDir 3) -KeepState
    $panel = $app.Hwnd
    Set-Position 'remember'
    $cursor = [Probe]::CursorOrPrimaryCenter()
    $fallback = Show-Panel
    $screen = [System.Windows.Forms.Screen]::FromPoint((New-Object System.Drawing.Point($cursor.X, $cursor.Y))).WorkingArea
    $dx = ($fallback.Client[0] - $screen.Left) - ($screen.Right - $fallback.Client[2])
    Check 'falls back to the center of the cursor monitor' ([math]::Abs($dx) -le 1) "client $([Probe]::Rect($fallback.Client)) work $screen"
    Hide-Panel
    Set-Position 'followCursor'
} catch {
    $failures.Add("ERROR: $($_.Exception.Message)")
    Note "ERROR: $($_.Exception.Message)"
} finally {
    Stop-ProbeApp $app $Exe
    Remove-Item $nativeFile, $legacyFile -ErrorAction SilentlyContinue
    $report | Set-Content (Join-Path $results 'summary.txt') -Encoding UTF8
    Write-Host "results: $results"
}

if ($failures.Count -gt 0) { Write-Host "FAILED: $($failures -join '; ')"; exit 1 }
Write-Host 'PASSED'
