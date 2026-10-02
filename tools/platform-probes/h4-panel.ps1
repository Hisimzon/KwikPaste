# H4 subset with real input: the panel must never take the foreground or the focus.
#
#   powershell -STA -NoProfile -ExecutionPolicy Bypass -File tools\platform-probes\h4-panel.ps1 [-Exe <KwikPaste.exe>] [-Rounds 20]
#
# Uses the real mouse and keyboard (SendInput). It waits until the user has been idle for
# -IdleSeconds, opens its own WinForms target window and makes it the foreground app, and only
# injects input while that window (or the panel) is the foreground window; the mouse is only pressed
# over the panel. At the end it closes everything it opened and restores the cursor and foreground.
#
# A. Hotkey (Ctrl+Alt+Shift+F9), Rounds x: with the target focused, the hotkey shows the panel at
#    the cursor; the foreground window and the target's focused control stay the same, F9 never
#    reaches the target and its menu never activates; GetWindowRect equals the rect the app wrote and
#    the content rect equals the independently computed target (cursor monitor, work area, DPI,
#    text scale). The hotkey again hides it, again without touching the foreground.
#    The cursor visits points on every monitor, including corners that force clamping.
# B. Click, Rounds x: show by hotkey, click the middle of the panel; the foreground and focus stay,
#    and the panel's WM_MOUSEACTIVATE answer was MA_NOACTIVATE (3) exactly once per click.
# C. Frame resize, DragRounds x: drag the bottom-right resize border out and back; the panel's own
#    capture loop resizes it (DefWindowProc's modal loop would activate it) and the foreground stays.
# PHANTOM_ACTIVATIONS must stay 0. Hotkey-to-first-frame latency is reported as p50/p95 for every
# show: in-app (WM_HOTKEY received -> first frame) and end to end (F9 injected -> first frame).
param(
    [string]$Exe = '',
    [int]$Rounds = 20,
    [int]$DragRounds = 5,
    [int]$IdleSeconds = 30
)

. "$PSScriptRoot\common.ps1"
$Exe = Resolve-AppExe $Exe
$results = New-ResultDir 'h4-panel'
$report = New-Object System.Collections.Generic.List[string]
function Note([string]$Line) { $report.Add($Line); Write-Host $Line }

$frequency = [Diagnostics.Stopwatch]::Frequency
$latencyApp = New-Object System.Collections.Generic.List[double]
$latencyE2e = New-Object System.Collections.Generic.List[double]
$rows = New-Object System.Collections.Generic.List[object]

function Get-Cursor { $p = New-Object Probe+POINT; [void][Probe]::GetCursorPos([ref]$p); return $p }

# Cursor points for scenario A: fractions of each monitor's work area, cycling through monitors.
function Get-TestPoints([int]$Count) {
    $fractions = @(@(0.10, 0.10), @(0.50, 0.45), @(0.97, 0.97), @(0.02, 0.70), @(0.75, 0.03))
    $screens = [System.Windows.Forms.Screen]::AllScreens
    $points = @()
    for ($i = 0; $i -lt $Count; $i++) {
        $area = $screens[$i % $screens.Count].WorkingArea
        $f = $fractions[[math]::Floor($i / $screens.Count) % $fractions.Count]
        $points += , @([int]($area.X + $area.Width * $f[0]), [int]($area.Y + $area.Height * $f[1]))
    }
    return $points
}

function Show-ByHotkey($Form, $Panel, [string]$Label) {
    $injected = [Probe]::DevHotkey($Form, $Panel)
    $shown = Wait-ProbeEvent 'shown' 3000
    if ($null -eq $shown) { throw "${Label}: the hotkey did not show the panel" }
    $latencyApp.Add([double]$shown.latency_ms)
    $latencyE2e.Add(([double]$shown.frame - $injected) * 1000.0 / $frequency)
    return $shown
}

function Hide-ByHotkey($Form, $Panel, [string]$Label) {
    [void][Probe]::DevHotkey($Form, $Panel)
    $hidden = Wait-ProbeEvent 'hidden' 3000
    if ($null -eq $hidden) { throw "${Label}: the hotkey did not hide the panel" }
    return $hidden
}

function Test-Target($Form, [string]$Label) {
    [Probe]::Pump(150)
    $foreground = [Probe]::GetForegroundWindow() -eq $Form.Handle
    $focus = [Probe]::GetFocus() -eq $Form.Box.Handle
    $leaks = ($Form.F9KeyDowns -eq 0) -and ($Form.MenuActivations -eq 0) -and ($Form.Box.Text -eq 'probe')
    if (-not ($foreground -and $focus -and $leaks)) {
        Note ("{0}: foreground={1} focus={2} F9={3} menu={4} text='{5}' events: {6}" -f $Label, $foreground, $focus, $Form.F9KeyDowns, $Form.MenuActivations, $Form.Box.Text, $Form.Drain())
    }
    [void]$Form.Drain()
    return [pscustomobject]@{ Foreground = $foreground; Focus = $focus; NoLeak = $leaks }
}

if (-not [System.Threading.Thread]::CurrentThread.GetApartmentState().Equals([System.Threading.ApartmentState]::STA)) {
    throw 'Run this script with powershell -STA.'
}
Assert-Desktop -IdleSeconds $IdleSeconds -NeedsInput
$userForeground = [Probe]::GetForegroundWindow()
$userCursor = Get-Cursor
Note ("user idle {0:N0} s; foreground 0x{1:X}; cursor {2},{3}; virtual screen {4}; text scale {5}" -f ([Probe]::IdleMs() / 1000), $userForeground.ToInt64(), $userCursor.X, $userCursor.Y, [Probe]::VirtualScreen(), [Probe]::TextScaleFactor())
foreach ($screen in [System.Windows.Forms.Screen]::AllScreens) { Note "monitor $($screen.DeviceName) bounds $($screen.Bounds) work $($screen.WorkingArea)" }

$app = $null
$form = $null
$failed = $false
try {
    $app = Start-ProbeApp $Exe $results
    $panel = $app.Hwnd
    Note ("app pid {0} panel hwnd 0x{1:X} exe {2}" -f $app.Process.Id, $panel.ToInt64(), $Exe)

    $form = New-Object TargetForm
    $primary = [System.Windows.Forms.Screen]::PrimaryScreen.WorkingArea
    $form.Bounds = New-Object System.Drawing.Rectangle(($primary.X + 60), ($primary.Y + 60), 560, 200)
    # Topmost only while it takes the foreground, so the click below lands on it.
    $form.TopMost = $true
    $form.Show()
    [Probe]::Pump(300)
    $form.Activate()
    [void]$form.Box.Focus()
    [Probe]::Pump(300)
    if ([Probe]::GetForegroundWindow() -ne $form.Handle) { [void][Probe]::SetForegroundWindow($form.Handle); [Probe]::Pump(300) }
    if ([Probe]::GetForegroundWindow() -ne $form.Handle) { [void][Probe]::ClickIntoForeground($form) }
    $form.TopMost = $false
    [Probe]::Pump(200)
    if ([Probe]::GetForegroundWindow() -ne $form.Handle) { throw 'The probe form could not take the foreground.' }
    [void]$form.Box.Focus()
    $form.Box.Text = 'probe'
    [void]$form.Drain()
    Note ("target form 0x{0:X} is the foreground; focused control is its text box: {1}" -f $form.Handle.ToInt64(), ([Probe]::GetFocus() -eq $form.Box.Handle))

    # ---------------------------------------------------------------- A. hotkey
    $countA = @{ Shown = 0; Foreground = 0; Focus = 0; NoLeak = 0; WindowRect = 0; ClientRect = 0; HideForeground = 0; Hidden = 0 }
    $points = Get-TestPoints $Rounds
    for ($round = 1; $round -le $Rounds; $round++) {
        $label = "A$round"
        $cursor = [Probe]::MoveTo($points[$round - 1][0], $points[$round - 1][1])
        [Probe]::Pump(60)
        $expected = [Probe]::ExpectedClient($cursor.X, $cursor.Y)
        $shown = Show-ByHotkey $form $panel $label
        $countA.Shown++
        $state = Test-Target $form "$label show"
        if ($state.Foreground) { $countA.Foreground++ }
        if ($state.Focus) { $countA.Focus++ }
        if ($state.NoLeak) { $countA.NoLeak++ }
        $window = [Probe]::WindowRect($panel)
        $client = [Probe]::ClientRectOnScreen($panel)
        $windowOk = [Probe]::Same($window, [int[]]$shown.target_outer)
        $clientOk = [Probe]::Same($client, $expected)
        if ($windowOk) { $countA.WindowRect++ } else { Note "${label}: window $([Probe]::Rect($window)) != written $([Probe]::Rect([int[]]$shown.target_outer))" }
        if ($clientOk) { $countA.ClientRect++ } else { Note "${label}: client $([Probe]::Rect($client)) != expected $([Probe]::Rect($expected)) (cursor $($cursor.X),$($cursor.Y))" }
        $rows.Add([pscustomobject]@{ Round = $label; CursorX = $cursor.X; CursorY = $cursor.Y; Expected = [Probe]::Rect($expected); Client = [Probe]::Rect($client); Window = [Probe]::Rect($window); Dpi = $shown.dpi; LatencyMs = $shown.latency_ms; E2eMs = $latencyE2e[$latencyE2e.Count - 1]; FrameInsideShow = $shown.frame_inside_show; Foreground = $state.Foreground; Focus = $state.Focus })

        $hidden = Hide-ByHotkey $form $panel $label
        $after = Test-Target $form "$label hide"
        if ($after.Foreground -and $after.Focus -and $after.NoLeak) { $countA.HideForeground++ }
        if (-not $hidden.visible -and -not [Probe]::IsWindowVisible($panel)) { $countA.Hidden++ }
    }

    # ---------------------------------------------------------------- B. click
    $countB = @{ Clicked = 0; Foreground = 0; Focus = 0; NoLeak = 0; NoActivate = 0 }
    $lastReplies = $null
    $start = $primary
    for ($round = 1; $round -le $Rounds; $round++) {
        $label = "B$round"
        [void][Probe]::MoveTo([int]($start.X + $start.Width * 0.35), [int]($start.Y + $start.Height * 0.25))
        [Probe]::Pump(60)
        $shown = Show-ByHotkey $form $panel $label
        if ($null -eq $lastReplies) { $lastReplies = [int[]]$shown.mouse_activate_replies }
        $client = [Probe]::ClientRectOnScreen($panel)
        $x = [int](($client[0] + $client[2]) / 2); $y = [int](($client[1] + $client[3]) / 2)
        [void][Probe]::MoveTo($x, $y)
        [Probe]::Pump(120)
        if ([Probe]::RootAt($x, $y) -ne $panel) { throw "${label}: the middle of the panel ($x,$y) is not the panel" }
        [Probe]::Click($form, $panel)
        $countB.Clicked++
        $state = Test-Target $form "$label click"
        if ($state.Foreground) { $countB.Foreground++ }
        if ($state.Focus) { $countB.Focus++ }
        if ($state.NoLeak) { $countB.NoLeak++ }

        $hidden = Hide-ByHotkey $form $panel $label
        $replies = [int[]]$hidden.mouse_activate_replies
        $delta = @(0, 1, 2, 3, 4 | ForEach-Object { $replies[$_] - $lastReplies[$_] })
        if ($delta[3] -eq 1 -and ($delta[0] + $delta[1] + $delta[2] + $delta[4]) -eq 0 -and $hidden.mouse_activate_overrides -eq 0) { $countB.NoActivate++ } else {
            Note "${label}: WM_MOUSEACTIVATE replies this round $($delta -join ','), overrides $($hidden.mouse_activate_overrides)"
        }
        $lastReplies = $replies
    }

    # ---------------------------------------------------------------- C. resize by the frame
    # Last on purpose: the panel keeps the size the user dragged to, which would change A's targets.
    $countC = @{ Resized = 0; Restored = 0; Foreground = 0 }
    for ($round = 1; $round -le $DragRounds; $round++) {
        $label = "C$round"
        [void][Probe]::MoveTo([int]($start.X + $start.Width * 0.35), [int]($start.Y + $start.Height * 0.25))
        [Probe]::Pump(60)
        [void](Show-ByHotkey $form $panel $label)
        $before = [Probe]::WindowRect($panel)
        $insets = [Probe]::ClientRectOnScreen($panel)
        # Bottom-right corner, inside the invisible resize border below and right of the content.
        $x = [int](($insets[2] + $before[2]) / 2); $y = [int](($insets[3] + $before[3]) / 2)
        [void][Probe]::MoveTo($x, $y)
        [Probe]::Pump(120)
        [Probe]::DragBy($panel, 60, 45, 6)
        $grown = [Probe]::WindowRect($panel)
        $grewOk = [math]::Abs(($grown[2] - $grown[0]) - ($before[2] - $before[0]) - 60) -le 1 -and [math]::Abs(($grown[3] - $grown[1]) - ($before[3] - $before[1]) - 45) -le 1 -and $grown[0] -eq $before[0] -and $grown[1] -eq $before[1]
        [Probe]::Pump(80)
        [Probe]::DragBy($panel, -60, -45, 6)
        $restored = [Probe]::WindowRect($panel)
        $state = Test-Target $form "$label drag"
        if ($grewOk) { $countC.Resized++ } else { Note "${label}: before $([Probe]::Rect($before)) after grow $([Probe]::Rect($grown))" }
        if ([Probe]::Same($restored, $before)) { $countC.Restored++ } else { Note "${label}: before $([Probe]::Rect($before)) after shrink $([Probe]::Rect($restored))" }
        if ($state.Foreground -and $state.Focus -and $state.NoLeak) { $countC.Foreground++ }
        [void](Hide-ByHotkey $form $panel $label)
    }

    $final = Get-Content $app.Log | Where-Object { $_ -match '"event":"hidden"' } | Select-Object -Last 1 | ConvertFrom-Json
    Note ''
    Note "A hotkey show: shown $($countA.Shown)/$Rounds; foreground unchanged $($countA.Foreground)/$Rounds; focus unchanged $($countA.Focus)/$Rounds; no F9/menu/text leak $($countA.NoLeak)/$Rounds"
    Note "A geometry: GetWindowRect == written rect $($countA.WindowRect)/$Rounds; content rect == independent target $($countA.ClientRect)/$Rounds"
    Note "A hotkey hide: hidden $($countA.Hidden)/$Rounds; foreground and focus unchanged $($countA.HideForeground)/$Rounds"
    Note "B click: clicked $($countB.Clicked)/$Rounds; foreground unchanged $($countB.Foreground)/$Rounds; focus unchanged $($countB.Focus)/$Rounds; no leak $($countB.NoLeak)/$Rounds; WM_MOUSEACTIVATE -> 3 exactly once $($countB.NoActivate)/$Rounds"
    Note "C frame resize (own capture loop): grew by the drag $($countC.Resized)/$DragRounds; back to the original rect $($countC.Restored)/$DragRounds; foreground and focus unchanged $($countC.Foreground)/$DragRounds"
    Note "PHANTOM_ACTIVATIONS $($final.phantom_activations); WM_MOUSEACTIVATE overrides $($final.mouse_activate_overrides); replies by value $($final.mouse_activate_replies -join ',')"
    Note ("hotkey -> first frame, in app ({0} shows): p50 {1} ms, p95 {2} ms, max {3} ms" -f $latencyApp.Count, (Format-Ms (Get-Percentile $latencyApp 50)), (Format-Ms (Get-Percentile $latencyApp 95)), (Format-Ms (($latencyApp | Measure-Object -Maximum).Maximum)))
    Note ("F9 injected -> first frame, end to end: p50 {0} ms, p95 {1} ms, max {2} ms" -f (Format-Ms (Get-Percentile $latencyE2e 50)), (Format-Ms (Get-Percentile $latencyE2e 95)), (Format-Ms (($latencyE2e | Measure-Object -Maximum).Maximum)))
    $insideShow = @(Get-Content $app.Log | Where-Object { $_ -match '"event":"shown"' } | ConvertFrom-Json | Where-Object { $_.frame_inside_show }).Count
    Note "first frame drawn inside ShowWindow: $insideShow of $($latencyApp.Count) shows"

    $allA = @($countA.Values | Where-Object { $_ -ne $Rounds }).Count -eq 0
    $allB = @($countB.Values | Where-Object { $_ -ne $Rounds }).Count -eq 0
    $allC = @($countC.Values | Where-Object { $_ -ne $DragRounds }).Count -eq 0
    $failed = -not ($allA -and $allB -and $allC -and $final.phantom_activations -eq 0 -and $final.mouse_activate_overrides -eq 0)
} catch {
    $failed = $true
    Note "ERROR: $($_.Exception.Message)"
} finally {
    try { if ($null -ne $app -and [Probe]::IsWindowVisible($app.Hwnd)) { Send-ProbeCommand $Exe '--selftest-hide' } } catch {}
    Stop-ProbeApp $app $Exe
    if ($null -ne $app) { Note "app exit code $($app.Process.ExitCode)" }
    if ($null -ne $form) { $form.Close(); [Probe]::Pump(200) }
    [void][Probe]::SetCursorPos($userCursor.X, $userCursor.Y)
    if ($userForeground -ne [IntPtr]::Zero) { [void][Probe]::SetForegroundWindow($userForeground) }
    $rows | Export-Csv (Join-Path $results 'rounds-a.csv') -NoTypeInformation -Encoding UTF8
    $latencyApp | Set-Content (Join-Path $results 'latency-app-ms.txt')
    $latencyE2e | Set-Content (Join-Path $results 'latency-e2e-ms.txt')
    $report | Set-Content (Join-Path $results 'summary.txt') -Encoding UTF8
    Write-Host "results: $results"
}

if ($failed) { Write-Host 'FAILED'; exit 1 }
Write-Host 'PASSED'
