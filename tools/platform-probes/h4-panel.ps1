# H4 subset with real input: the panel must never take the foreground or the focus, and the
# keyboard hook, outside-click hide and editing mode must not leak anything to the foreground app.
#
#   powershell -STA -NoProfile -ExecutionPolicy Bypass -File tools\platform-probes\h4-panel.ps1 [-Exe <KwikPaste.exe>] [-Rounds 20] [-Scenarios ABCDEFH]
#
# Uses the real mouse and keyboard (SendInput). It waits until the user has been idle for
# -IdleSeconds, opens its own WinForms target window and makes it the foreground app, and only
# injects input while that window (or the panel) is the foreground window; the mouse is only pressed
# over the panel or our own form. At the end it closes everything it opened and restores the cursor
# and the foreground window.
#
# A. Hotkey, Rounds x: the hotkey shows the panel at the cursor; foreground and focused control stay,
#    F9 never reaches the target and its menu never activates; GetWindowRect equals the rect the app
#    wrote and the content rect equals the independently computed target. The hotkey hides it again.
#    The cursor visits points on every monitor, including corners that force clamping.
# B. Click, Rounds x: click the middle of the panel; foreground and focus stay, and the panel answered
#    WM_MOUSEACTIVATE with MA_NOACTIVATE (3) exactly once per click.
# C. Frame resize, DragRounds x: drag the bottom-right resize border out and back; the panel's own
#    capture loop resizes it (DefWindowProc's modal loop would activate it) and the foreground stays.
# D. Outside click, Rounds x: with the panel shown, click the target's text box; the mouse hook hides
#    the panel, the click lands in the target, foreground and focus stay.
# E. Hook keys, Rounds x: Down, Up and Ctrl+K go to the panel through the keyboard hook (dispatched as
#    GPUI keystrokes), never to the target.
# F. Editing, Rounds x each: Ctrl+F (keyboard trigger: the marked Alt is swallowed by our own hook) and
#    a click on the panel's search box (mouse trigger) make the panel the foreground window; Esc gives
#    the foreground back to the target. The target must receive 0 Alt key downs and 0 SC_KEYMENU.
# H. Settings: switching shortcuts.openClipboard through core re-registers the hotkey (the old one
#    stops working, the new one shows the panel), and switching back restores it.
# PHANTOM_ACTIVATIONS must stay 0. Hotkey-to-first-frame latency is reported as p50/p95 for every
# show: in-app (WM_HOTKEY received -> first frame) and end to end (F9 injected -> first frame).
param(
    [string]$Exe = '',
    [int]$Rounds = 20,
    [int]$DragRounds = 5,
    # Letters of the scenarios to run (powershell -File cannot pass arrays).
    [string]$Scenarios = 'ABCDEFH',
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
$failures = New-Object System.Collections.Generic.List[string]
function Expect([string]$Name, [int]$Passed, [int]$Total) {
    Note ("  {0}: {1}/{2}" -f $Name, $Passed, $Total)
    if ($Passed -ne $Total) { $failures.Add($Name) }
}

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

function Move-ToSafePoint {
    $area = [System.Windows.Forms.Screen]::PrimaryScreen.WorkingArea
    [void][Probe]::MoveTo([int]($area.X + $area.Width * 0.35), [int]($area.Y + $area.Height * 0.25))
    [Probe]::Pump(60)
}

function Show-ByHotkey([string]$Label) {
    $injected = [Probe]::DevHotkey($form, $panel)
    $shown = Wait-ProbeEvent 'shown' 3000
    if ($null -eq $shown) { throw "${Label}: the hotkey did not show the panel" }
    $latencyApp.Add([double]$shown.latency_ms)
    $latencyE2e.Add(([double]$shown.frame - $injected) * 1000.0 / $frequency)
    return $shown
}

function Hide-ByHotkey([string]$Label) {
    [void][Probe]::DevHotkey($form, $panel)
    $hidden = Wait-ProbeEvent 'hidden' 3000
    if ($null -eq $hidden) { throw "${Label}: the hotkey did not hide the panel" }
    return $hidden
}

# The target keeps the foreground and its focused text box, and saw no F9, menu mode or typing.
# (Its Alt key downs are not checked here: the Ctrl+Alt+Shift+F9 chord itself presses Alt in the
# target. Scenario F checks Alt around editing, between two chords.)
function Test-Target([string]$Label) {
    [Probe]::Pump(150)
    $foreground = [Probe]::GetForegroundWindow() -eq $form.Handle
    $focus = [Probe]::GetFocus() -eq $form.Box.Handle
    $leaks = ($form.F9KeyDowns -eq 0) -and ($form.MenuActivations -eq 0) -and
        ($form.KeyMenuCommands -eq 0) -and ($form.Box.Text -eq 'probe')
    if (-not ($foreground -and $focus -and $leaks)) {
        Note ("{0}: foreground={1} focus={2} F9={3} Alt={4} SC_KEYMENU={5} menu={6} text='{7}' events: {8}" -f $Label, $foreground, $focus,
            $form.F9KeyDowns, $form.AltKeyDowns, $form.KeyMenuCommands, $form.MenuActivations, $form.Box.Text, $form.Drain())
    }
    [void]$form.Drain()
    return [pscustomobject]@{ Foreground = $foreground; Focus = $focus; NoLeak = $leaks; All = ($foreground -and $focus -and $leaks) }
}

# Gives the foreground back to the target after a failed round, so the next round can inject.
function Restore-Target {
    if ([Probe]::IsWindowVisible($panel)) { Send-ProbeCommand $Exe '--selftest-hide'; [void](Wait-ProbeEvent 'hidden' 3000) }
    if ([Probe]::GetForegroundWindow() -ne $form.Handle) { [void][Probe]::ClickIntoForeground($form) }
    $form.Box.Text = 'probe'
    $form.Box.SelectionStart = 5
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
try {
    $app = Start-ProbeApp $Exe $results
    $panel = $app.Hwnd
    Note ("app pid {0} panel hwnd 0x{1:X} exe {2}" -f $app.Process.Id, $panel.ToInt64(), $Exe)
    $form = New-TargetForm
    Note ("target form 0x{0:X} is the foreground; focused control is its text box: {1}" -f $form.Handle.ToInt64(), ([Probe]::GetFocus() -eq $form.Box.Handle))

    if ($Scenarios.Contains('A')) {
        Note 'A. hotkey'
        $count = @{ Foreground = 0; Focus = 0; NoLeak = 0; WindowRect = 0; ClientRect = 0; Hide = 0 }
        $points = Get-TestPoints $Rounds
        for ($round = 1; $round -le $Rounds; $round++) {
            $label = "A$round"
            $cursor = [Probe]::MoveTo($points[$round - 1][0], $points[$round - 1][1])
            [Probe]::Pump(60)
            $expected = [Probe]::ExpectedClient($cursor.X, $cursor.Y)
            $shown = Show-ByHotkey $label
            $state = Test-Target "$label show"
            if ($state.Foreground) { $count.Foreground++ }
            if ($state.Focus) { $count.Focus++ }
            if ($state.NoLeak) { $count.NoLeak++ }
            $window = [Probe]::WindowRect($panel)
            $client = [Probe]::ClientRectOnScreen($panel)
            if ([Probe]::Same($window, [int[]]$shown.target_outer)) { $count.WindowRect++ } else { Note "${label}: window $([Probe]::Rect($window)) != written $([Probe]::Rect([int[]]$shown.target_outer))" }
            if ([Probe]::Same($client, $expected)) { $count.ClientRect++ } else { Note "${label}: client $([Probe]::Rect($client)) != expected $([Probe]::Rect($expected)) (cursor $($cursor.X),$($cursor.Y))" }
            $rows.Add([pscustomobject]@{ Round = $label; CursorX = $cursor.X; CursorY = $cursor.Y; Expected = [Probe]::Rect($expected); Client = [Probe]::Rect($client); Window = [Probe]::Rect($window); Dpi = $shown.dpi; LatencyMs = $shown.latency_ms; E2eMs = $latencyE2e[$latencyE2e.Count - 1]; FrameInsideShow = $shown.frame_inside_show })
            $hidden = Hide-ByHotkey $label
            $after = Test-Target "$label hide"
            if ($after.All -and -not $hidden.visible) { $count.Hide++ }
        }
        Expect 'A hotkey show: foreground unchanged' $count.Foreground $Rounds
        Expect 'A hotkey show: focused control unchanged' $count.Focus $Rounds
        Expect 'A hotkey show: no F9/Alt/menu/typing leak' $count.NoLeak $Rounds
        Expect 'A GetWindowRect == written rect' $count.WindowRect $Rounds
        Expect 'A content rect == independent target' $count.ClientRect $Rounds
        Expect 'A hotkey hide: hidden, foreground and focus unchanged' $count.Hide $Rounds
    }

    if ($Scenarios.Contains('B')) {
        Note 'B. click inside the panel'
        $count = @{ Target = 0; NoActivate = 0 }
        $lastReplies = $null
        for ($round = 1; $round -le $Rounds; $round++) {
            $label = "B$round"
            Move-ToSafePoint
            $shown = Show-ByHotkey $label
            if ($null -eq $lastReplies) { $lastReplies = [int[]]$shown.mouse_activate_replies }
            $client = [Probe]::ClientRectOnScreen($panel)
            $x = [int](($client[0] + $client[2]) / 2); $y = [int](($client[1] + $client[3]) / 2)
            [void][Probe]::MoveTo($x, $y)
            [Probe]::Pump(120)
            if ([Probe]::RootAt($x, $y) -ne $panel) { throw "${label}: the middle of the panel ($x,$y) is not the panel" }
            [Probe]::Click($form, $panel)
            if ((Test-Target "$label click").All) { $count.Target++ }
            $hidden = Hide-ByHotkey $label
            $replies = [int[]]$hidden.mouse_activate_replies
            $delta = @(0, 1, 2, 3, 4 | ForEach-Object { $replies[$_] - $lastReplies[$_] })
            if ($delta[3] -eq 1 -and ($delta[0] + $delta[1] + $delta[2] + $delta[4]) -eq 0 -and $hidden.mouse_activate_overrides -eq 0) { $count.NoActivate++ } else {
                Note "${label}: WM_MOUSEACTIVATE replies this round $($delta -join ','), overrides $($hidden.mouse_activate_overrides)"
            }
            $lastReplies = $replies
        }
        Expect 'B click: foreground, focus unchanged, no leak' $count.Target $Rounds
        Expect 'B click: WM_MOUSEACTIVATE -> 3 exactly once' $count.NoActivate $Rounds
    }

    if ($Scenarios.Contains('C')) {
        Note 'C. resize by the frame'
        $count = @{ Resized = 0; Restored = 0; Target = 0 }
        for ($round = 1; $round -le $DragRounds; $round++) {
            $label = "C$round"
            Move-ToSafePoint
            [void](Show-ByHotkey $label)
            $before = [Probe]::WindowRect($panel)
            $content = [Probe]::ClientRectOnScreen($panel)
            # Bottom-right corner, inside the invisible resize border below and right of the content.
            $x = [int](($content[2] + $before[2]) / 2); $y = [int](($content[3] + $before[3]) / 2)
            [void][Probe]::MoveTo($x, $y)
            [Probe]::Pump(120)
            [Probe]::DragBy($panel, 60, 45, 6)
            $grown = [Probe]::WindowRect($panel)
            $grewOk = [math]::Abs(($grown[2] - $grown[0]) - ($before[2] - $before[0]) - 60) -le 1 -and [math]::Abs(($grown[3] - $grown[1]) - ($before[3] - $before[1]) - 45) -le 1 -and $grown[0] -eq $before[0] -and $grown[1] -eq $before[1]
            [Probe]::Pump(80)
            [Probe]::DragBy($panel, -60, -45, 6)
            $restored = [Probe]::WindowRect($panel)
            if ($grewOk) { $count.Resized++ } else { Note "${label}: before $([Probe]::Rect($before)) after grow $([Probe]::Rect($grown))" }
            if ([Probe]::Same($restored, $before)) { $count.Restored++ } else { Note "${label}: before $([Probe]::Rect($before)) after shrink $([Probe]::Rect($restored))" }
            if ((Test-Target "$label drag").All) { $count.Target++ }
            [void](Hide-ByHotkey $label)
        }
        Expect 'C grew by the drag' $count.Resized $DragRounds
        Expect 'C back to the original rect' $count.Restored $DragRounds
        Expect 'C foreground and focus unchanged' $count.Target $DragRounds
    }

    if ($Scenarios.Contains('D')) {
        Note 'D. click outside the panel'
        $count = @{ Hidden = 0; Target = 0 }
        for ($round = 1; $round -le $Rounds; $round++) {
            $label = "D$round"
            Move-ToSafePoint
            [void](Show-ByHotkey $label)
            $box = $form.Box.PointToScreen((New-Object System.Drawing.Point(($form.Box.Width - 20), ($form.Box.Height / 2))))
            [void][Probe]::MoveTo($box.X, $box.Y)
            [Probe]::Pump(100)
            [Probe]::Click($form, $panel)
            $hidden = Wait-ProbeEvent 'hidden' 2000
            if ($null -ne $hidden -and $hidden.source -eq 'outside-click' -and -not [Probe]::IsWindowVisible($panel)) { $count.Hidden++ } else {
                Note "${label}: panel not hidden by the outside click (event source $($hidden.source))"
                Restore-Target
            }
            if ((Test-Target "$label outside click").All) { $count.Target++ }
        }
        Expect 'D outside click hides the panel' $count.Hidden $Rounds
        Expect 'D foreground, focus unchanged, click landed in the target' $count.Target $Rounds
    }

    if ($Scenarios.Contains('E')) {
        Note 'E. keys through the hook'
        $count = @{ Dispatched = 0; NotInTarget = 0; Target = 0 }
        for ($round = 1; $round -le $Rounds; $round++) {
            $label = "E$round"
            Move-ToSafePoint
            [void](Show-ByHotkey $label)
            $before = @{}; foreach ($key in $form.KeyDowns.Keys) { $before[$key] = $form.KeyDowns[$key] }
            [void](Get-ProbeEvents 'hook_key')
            [Probe]::Tap($form, $panel, 0x28)
            [Probe]::Tap($form, $panel, 0x26)
            [Probe]::CtrlTap($form, $panel, 0x4B)
            [Probe]::Pump(150)
            $keys = @(Get-ProbeEvents 'hook_key' | Where-Object { $_.key -ne 'control' })
            $names = ($keys | ForEach-Object { if ($_.ctrl) { "ctrl-$($_.key)" } else { $_.key } }) -join ' '
            $handledArrows = @($keys | Where-Object { $_.key -in @('down', 'up') -and $_.handled }).Count
            if ($names -eq 'down up ctrl-k' -and $handledArrows -eq 2) { $count.Dispatched++ } else { Note "${label}: hook keys '$names' (arrows handled $handledArrows)" }
            $leaked = @('Down', 'Up', 'K' | Where-Object { $form.KeyDowns.ContainsKey([System.Windows.Forms.Keys]$_) -and $form.KeyDowns[[System.Windows.Forms.Keys]$_] -ne $before[[System.Windows.Forms.Keys]$_] })
            if ($leaked.Count -eq 0) { $count.NotInTarget++ } else { Note "${label}: the target received $($leaked -join ',')" }
            if ((Test-Target "$label keys").All) { $count.Target++ }
            [void](Hide-ByHotkey $label)
        }
        Expect 'E Down, Up, Ctrl+K dispatched to the panel' $count.Dispatched $Rounds
        Expect 'E none of them reached the target' $count.NotInTarget $Rounds
        Expect 'E foreground and focus unchanged' $count.Target $Rounds
    }

    if ($Scenarios.Contains('F')) {
        foreach ($trigger in @('keyboard', 'mouse')) {
            Note "F. editing by $trigger"
            $count = @{ Entered = 0; Foreground = 0; Swallowed = 0; Back = 0; NoAlt = 0; Phantom = 0 }
            $editMs = New-Object System.Collections.Generic.List[double]
            for ($round = 1; $round -le $Rounds; $round++) {
                $label = "F-$trigger$round"
                Move-ToSafePoint
                [void](Show-ByHotkey $label)
                [void](Test-Target "$label shown")
                $altBefore = $form.AltKeyDowns
                if ($trigger -eq 'keyboard') {
                    [Probe]::CtrlTap($form, $panel, 0x46)
                } else {
                    $client = [Probe]::ClientRectOnScreen($panel)
                    $scale = [double]$app.Ready.dpi / 96.0
                    # The search box sits at the top of the probe view (12 px padding, 32 px high).
                    [void][Probe]::MoveTo([int]($client[0] + 120 * $scale), [int]($client[1] + 28 * $scale))
                    [Probe]::Pump(120)
                    [Probe]::Click($form, $panel)
                }
                $editing = Wait-ProbeEvent 'editing' 2000
                [Probe]::Pump(100)
                if ($null -ne $editing -and $editing.entered -and $editing.trigger -eq $trigger) { $count.Entered++; $editMs.Add([double]$editing.elapsed_ms) } else {
                    Note "${label}: editing not entered ($($editing.error))"
                }
                if ([Probe]::GetForegroundWindow() -eq $panel) { $count.Foreground++ }
                if ($trigger -eq 'mouse' -or ($null -ne $editing -and $editing.marked_alt_swallowed)) { $count.Swallowed++ }
                if ([Probe]::GetForegroundWindow() -eq $panel) { [Probe]::Tap($form, $panel, 0x1B) } else { Send-ProbeCommand $Exe '--selftest-end-edit' }
                $ended = Wait-ProbeEvent 'editing_ended' 2000
                [Probe]::Pump(150)
                if ($null -ne $ended -and [Probe]::GetForegroundWindow() -eq $form.Handle -and [Probe]::GetFocus() -eq $form.Box.Handle) { $count.Back++ } else {
                    Note "${label}: foreground after Esc 0x$('{0:X}' -f [Probe]::GetForegroundWindow().ToInt64())"
                    Restore-Target
                }
                $altDuringEdit = $form.AltKeyDowns - $altBefore
                if ($altDuringEdit -eq 0 -and $form.KeyMenuCommands -eq 0 -and $form.MenuActivations -eq 0) { $count.NoAlt++ } else {
                    Note "${label}: target saw Alt $altDuringEdit, SC_KEYMENU $($form.KeyMenuCommands), menu $($form.MenuActivations): $($form.Drain())"
                }
                if ($null -ne $ended -and $ended.phantom_activations -eq 0) { $count.Phantom++ }
                [void]$form.Drain()
                if ([Probe]::IsWindowVisible($panel)) { [void](Hide-ByHotkey $label) }
            }
            Expect "F $trigger editing entered" $count.Entered $Rounds
            Expect "F $trigger panel became the foreground window" $count.Foreground $Rounds
            Expect "F $trigger marked Alt swallowed (keyboard only)" $count.Swallowed $Rounds
            Expect "F $trigger Esc gave foreground and focus back to the target" $count.Back $Rounds
            Expect "F $trigger target received 0 Alt / SC_KEYMENU / menu" $count.NoAlt $Rounds
            Expect "F $trigger no phantom activation" $count.Phantom $Rounds
            if ($editMs.Count -gt 0) {
                Note ("  F $trigger time to foreground: p50 {0} ms, p95 {1} ms, max {2} ms" -f (Format-Ms (Get-Percentile $editMs 50)), (Format-Ms (Get-Percentile $editMs 95)), (Format-Ms (($editMs | Measure-Object -Maximum).Maximum)))
            }
        }
    }

    if ($Scenarios.Contains('H')) {
        Note 'H. hotkey from settings'
        $alternate = 'Control+Alt+Shift+F8'
        if (-not [Probe]::RegisterHotKey([IntPtr]::Zero, 0x5151, (0x1 -bor 0x2 -bor 0x4 -bor 0x4000), 0x77)) { throw "$alternate is held by another program" }
        [void][Probe]::UnregisterHotKey([IntPtr]::Zero, 0x5151)
        Send-ProbeCommand $Exe "--selftest-settings={`"shortcuts`":{`"openClipboard`":`"$alternate`"}}"
        [Probe]::Pump(400)
        $oldKey = $true
        Move-ToSafePoint
        [void][Probe]::DevHotkey($form, $panel)
        if ($null -ne (Wait-ProbeEvent 'shown' 800)) { $oldKey = $false; Note 'H: the old hotkey still shows the panel'; [void](Hide-ByHotkey 'H') }
        # F9 without its registration reaches our own form: that is expected here, reset the counter.
        $form.F9KeyDowns = 0
        [void]$form.Drain()
        [Probe]::RequireForeground($form, $panel, 'the new hotkey')
        $newKey = $false
        [void][Probe]::DevHotkeyWith($form, $panel, 0x77)
        if ($null -ne (Wait-ProbeEvent 'shown' 2000)) { $newKey = $true; [void][Probe]::DevHotkeyWith($form, $panel, 0x77); [void](Wait-ProbeEvent 'hidden' 2000) }
        Send-ProbeCommand $Exe "--selftest-settings={`"shortcuts`":{`"openClipboard`":`"Alt+C`"}}"
        [Probe]::Pump(400)
        $restored = $false
        [void][Probe]::DevHotkey($form, $panel)
        if ($null -ne (Wait-ProbeEvent 'shown' 2000)) { $restored = $true; [void](Hide-ByHotkey 'H restore') }
        Expect 'H old hotkey released after the settings change' ([int]$oldKey) 1
        Expect 'H new hotkey shows the panel' ([int]$newKey) 1
        Expect 'H back to the development default (settings Alt+C -> Ctrl+Alt+Shift+F9)' ([int]$restored) 1
    }

    $final = Get-Content $app.Log | Where-Object { $_ -match '"event":"hidden"' } | Select-Object -Last 1 | ConvertFrom-Json
    Note ''
    Note "PHANTOM_ACTIVATIONS $($final.phantom_activations); WM_MOUSEACTIVATE overrides $($final.mouse_activate_overrides); replies by value $($final.mouse_activate_replies -join ',')"
    if ($final.phantom_activations -ne 0) { $failures.Add('phantom activations') }
    if ($final.mouse_activate_overrides -ne 0) { $failures.Add('WM_MOUSEACTIVATE overrides') }
    if ($latencyApp.Count -gt 0) {
        Note ("hotkey -> first frame, in app ({0} shows): p50 {1} ms, p95 {2} ms, max {3} ms" -f $latencyApp.Count, (Format-Ms (Get-Percentile $latencyApp 50)), (Format-Ms (Get-Percentile $latencyApp 95)), (Format-Ms (($latencyApp | Measure-Object -Maximum).Maximum)))
        Note ("F9 injected -> first frame, end to end: p50 {0} ms, p95 {1} ms, max {2} ms" -f (Format-Ms (Get-Percentile $latencyE2e 50)), (Format-Ms (Get-Percentile $latencyE2e 95)), (Format-Ms (($latencyE2e | Measure-Object -Maximum).Maximum)))
    }
} catch {
    $failures.Add("ERROR: $($_.Exception.Message)")
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

if ($failures.Count -gt 0) { Write-Host "FAILED: $($failures -join '; ')"; exit 1 }
Write-Host 'PASSED'
