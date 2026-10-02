# Non-invasive panel checks: no keyboard or mouse input is injected, the user's foreground window
# is only observed. Usable on a CI runner with an interactive desktop.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File tools\platform-probes\panel-invariants.ps1 [-Exe <KwikPaste.exe>] [-Rounds 20]
#
# Checks:
#   1. Window styles after setup: WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST, WS_THICKFRAME, no WS_EX_APPWINDOW.
#   2. Cross-process WM_MOUSEACTIVATE on the panel answers MA_NOACTIVATE (3).
#   3. Rounds x (show, hide) through the single-instance path: after each show the window rect equals
#      the rect the app wrote, the content rect equals the independently computed target, and the
#      foreground window never changes.
#   4. The minimum track size (WM_GETMINMAXINFO, cross-process) is 360x600 logical x DPI x text scale
#      plus the resize border, clamped to the work area.
#   5. PHANTOM_ACTIVATIONS == 0 and no WM_MOUSEACTIVATE override (patch W0002 in effect).
#
# -TextScale 1.5 runs the app with KP_TEXT_SCALE (honoured only in selftest mode) to check the text
# size compensation without touching the system setting. While the panel is shown the keyboard hook
# is live, so run it while nobody is typing (-IdleSeconds); a CI runner has no user.
param(
    [string]$Exe = '',
    [int]$Rounds = 20,
    [int]$IdleSeconds = 0,
    [double]$TextScale = 0
)

. "$PSScriptRoot\common.ps1"
$Exe = Resolve-AppExe $Exe
$results = New-ResultDir 'panel-invariants'
$report = New-Object System.Collections.Generic.List[string]
function Note([string]$Line) { $report.Add($Line); Write-Host $Line }

Assert-Desktop -IdleSeconds $IdleSeconds
$app = $null
$failures = 0
$tsf = if ($TextScale -gt 0) { $TextScale } else { [Probe]::TextScaleFactor() }
try {
    if ($TextScale -gt 0) { $env:KP_TEXT_SCALE = "$TextScale" }
    $app = Start-ProbeApp $Exe $results
    Remove-Item Env:KP_TEXT_SCALE -ErrorAction SilentlyContinue
    Note "text scale $tsf"
    $panel = $app.Hwnd
    Note ("app pid {0} panel hwnd 0x{1:X} exe {2}" -f $app.Process.Id, $panel.ToInt64(), $Exe)

    $style = [Probe]::Style($panel)
    $ex = [Probe]::ExStyle($panel)
    $stylesOk = (($ex -band 0x08000000) -ne 0) -and (($ex -band 0x80) -ne 0) -and (($ex -band 0x8) -ne 0) -and
        (($ex -band 0x40000) -eq 0) -and (($style -band 0x40000) -ne 0)
    Note ("styles: style=0x{0:X} exstyle=0x{1:X} -> {2}" -f $style, $ex, $(if ($stylesOk) { 'ok' } else { 'FAIL' }))
    if (-not $stylesOk) { $failures++ }

    $reply = [Probe]::MouseActivateReply($panel)
    Note "cross-process WM_MOUSEACTIVATE -> $reply (expected 3 = MA_NOACTIVATE)"
    if ($reply -ne 3) { $failures++ }

    $rectOk = 0; $clientOk = 0; $foregroundOk = 0; $shownCount = 0
    for ($round = 1; $round -le $Rounds; $round++) {
        $foreground = [Probe]::GetForegroundWindow()
        Send-ProbeCommand $Exe '--selftest-show'
        $shown = Wait-ProbeEvent 'shown' 5000
        if ($null -eq $shown) { Note "round ${round}: no shown event"; $failures++; continue }
        $shownCount++
        $window = [Probe]::WindowRect($panel)
        $client = [Probe]::ClientRectOnScreen($panel)
        $target = [int[]]$shown.target_outer
        # Nothing moves the cursor here, so the target can be recomputed from where it is now.
        $cursor = [Probe]::CursorOrPrimaryCenter()
        $expected = [Probe]::ExpectedClient($cursor.X, $cursor.Y, $tsf)
        if ($round -eq 1) {
            $insets = @(($client[0] - $window[0]), ($client[1] - $window[1]), ($window[2] - $client[2]), ($window[3] - $client[3]))
            $work = [System.Windows.Forms.Screen]::FromHandle($panel).WorkingArea
            $scale = [double]$shown.dpi / 96.0
            $minWidth = [math]::Min([math]::Round(360 * $tsf * $scale, [MidpointRounding]::AwayFromZero), $work.Width - $insets[0] - $insets[2]) + $insets[0] + $insets[2]
            $minHeight = [math]::Min([math]::Round(600 * $tsf * $scale, [MidpointRounding]::AwayFromZero), $work.Height - $insets[1] - $insets[3]) + $insets[1] + $insets[3]
            $minTrack = [Probe]::MinTrackSize($panel)
            $minOk = $minTrack[0] -eq $minWidth -and $minTrack[1] -eq $minHeight
            Note ("min track size {0}x{1}, expected {2}x{3} -> {4}" -f $minTrack[0], $minTrack[1], $minWidth, $minHeight, $(if ($minOk) { 'ok' } else { 'FAIL' }))
            if (-not $minOk) { $failures++ }
        }
        if ([Probe]::Same($window, $target)) { $rectOk++ } else { Note "round ${round}: window $([Probe]::Rect($window)) != written $([Probe]::Rect($target))" }
        if ([Probe]::Same($client, $expected) -and [Probe]::Same($client, [int[]]$shown.target_client)) { $clientOk++ } else {
            Note "round ${round}: client $([Probe]::Rect($client)) app target $([Probe]::Rect([int[]]$shown.target_client)) expected $([Probe]::Rect($expected))"
        }
        $after = [Probe]::GetForegroundWindow()
        if ($after -eq $foreground -and $shown.foreground -eq $foreground.ToInt64()) { $foregroundOk++ } else {
            Note ("round {0}: foreground changed 0x{1:X} -> 0x{2:X}" -f $round, $foreground.ToInt64(), $after.ToInt64())
        }

        Send-ProbeCommand $Exe '--selftest-hide'
        $hidden = Wait-ProbeEvent 'hidden' 5000
        if ($null -eq $hidden -or $hidden.visible) { Note "round ${round}: panel did not hide"; $failures++ }
    }
    Note "shown $shownCount/$Rounds; window rect == written rect $rectOk/$Rounds; content rect == target $clientOk/$Rounds; foreground unchanged $foregroundOk/$Rounds"
    if ($rectOk -ne $Rounds -or $clientOk -ne $Rounds -or $foregroundOk -ne $Rounds) { $failures++ }

    $last = Get-Content $app.Log | Select-Object -Last 1 | ConvertFrom-Json
    Note "phantom activations $($last.phantom_activations); WM_MOUSEACTIVATE replies $($last.mouse_activate_replies -join ','); overrides $($last.mouse_activate_overrides)"
    if ($last.phantom_activations -ne 0 -or $last.mouse_activate_overrides -ne 0) { $failures++ }
} finally {
    Stop-ProbeApp $app $Exe
    if ($null -ne $app) { Note "app exit code $($app.Process.ExitCode)" }
    $report | Set-Content (Join-Path $results 'summary.txt') -Encoding UTF8
}

if ($failures -gt 0) { Write-Host "FAILED ($failures)"; exit 1 }
Write-Host 'PASSED'
