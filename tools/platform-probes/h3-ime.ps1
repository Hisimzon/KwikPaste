# H3 (Windows input methods) with Microsoft Pinyin and real key input.
#
#   powershell -STA -NoProfile -ExecutionPolicy Bypass -File tools\platform-probes\h3-ime.ps1 [-Exe <KwikPaste.exe>]
#
# Switches the session's keyboard input method to Microsoft Pinyin for the run (what Win+Space does)
# and always switches back to the previous one at the end. Same safety rules as h4-panel.ps1: waits
# for the user to be idle, uses only its own WinForms target and the panel, restores cursor and
# foreground. Screenshots of the panel region land in the result folder (they can show whatever is
# behind the panel, so they stay local; results/ is not committed).
#
# 1. Non-editing: the target is composing "zhong". Showing the panel, hovering and clicking it,
#    a hook key (Down) and hiding it must all leave the target's composition untouched; Space then
#    commits the first candidate (U+4E2D) in the target.
# 2. Editing: a click on the panel's search box enters editing; typing "nihao" composes in the panel,
#    the candidate window appears right below the search box (screenshot + changed pixels), Space commits
#    ni hao (U+4F60 U+597D); "shijie" then composes after it (gpui-kit#3286 would put the candidate
#    window back at the input's origin), Space commits shi jie (U+4E16 U+754C). Esc leaves editing and the target is foreground again.
param(
    [string]$Exe = '',
    [int]$IdleSeconds = 30
)

. "$PSScriptRoot\common.ps1"
$Exe = Resolve-AppExe $Exe
$results = New-ResultDir 'h3-ime'
$report = New-Object System.Collections.Generic.List[string]
function Note([string]$Line) { $report.Add($Line); Write-Host $Line }
$failures = New-Object System.Collections.Generic.List[string]
# Expected text, as code points: the script stays ASCII so Windows PowerShell 5.1 reads it in any code page.
$zhong = [string][char]0x4E2D
$nihao = [string][char]0x4F60 + [char]0x597D
$shijie = [string][char]0x4E16 + [char]0x754C
function Check([string]$Name, [bool]$Passed, [string]$Detail = '') {
    Note ("  {0}: {1}{2}" -f $Name, $(if ($Passed) { 'ok' } else { 'FAIL' }), $(if ($Detail) { " ($Detail)" } else { '' }))
    if (-not $Passed) { $failures.Add($Name) }
}

function Get-Cursor { $p = New-Object Probe+POINT; [void][Probe]::GetCursorPos([ref]$p); return $p }

function Get-ImeState {
    Send-ProbeCommand $Exe '--selftest-ime-state'
    return (Wait-ProbeEvent 'ime' 3000)
}

# The band of the panel right below the search box (the probe view only has a static caption there).
# The candidate window is found by the pixels that change in it while composing: on Windows 11 it is
# drawn by TextInputHost and is neither an enumerable window nor hit-testable through UI Automation.
function Get-Band {
    $client = [Probe]::ClientRectOnScreen($panel)
    return [Screens]::Grab($client[0], $client[1] + [int](48 * $scale), $client[2] - $client[0], [int](52 * $scale))
}

# Changed box [left, top, right, bottom] in the band since $Before (x from the panel's left edge), or $null.
function Find-Candidate($Before) {
    $after = Get-Band
    $box = [Screens]::ChangedBox($Before, $after, 30)
    $after.Dispose()
    $Before.Dispose()
    if ($null -eq $box) { return $null }
    return ,$box
}

function Save-PanelShot([string]$Name) {
    $client = [Probe]::ClientRectOnScreen($panel)
    $path = Join-Path $results "$Name.png"
    $ok = [Screens]::Shot($path, $client[0] - 20, $client[1] - 20, ($client[2] - $client[0]) + 480, [math]::Min(($client[3] - $client[1]) + 40, 700))
    Note "  screenshot $Name.png saved: $ok"
}

if (-not [System.Threading.Thread]::CurrentThread.GetApartmentState().Equals([System.Threading.ApartmentState]::STA)) {
    throw 'Run this script with powershell -STA.'
}
Assert-Desktop -IdleSeconds $IdleSeconds -NeedsInput
$userForeground = [Probe]::GetForegroundWindow()
$userCursor = Get-Cursor
$originalTip = [Tsf]::ActiveProfile()
Note ("input method at start: {0}" -f [Tsf]::Describe($originalTip))

$app = $null
$form = $null
$switched = $false
try {
    $app = Start-ProbeApp $Exe $results
    $panel = $app.Hwnd
    $form = New-TargetForm

    if (-not [Tsf]::IsMsPinyin($originalTip)) {
        $hr = [Tsf]::Activate([Tsf]::MsPinyinClsid, [Tsf]::MsPinyinProfile, 0x0804)
        [Probe]::Pump(500)
        $switched = $true
        Note ("switched to Microsoft Pinyin: hr=0x{0:X}, now {1}" -f $hr, [Tsf]::Describe([Tsf]::ActiveProfile()))
    }
    if (-not [Tsf]::IsMsPinyin([Tsf]::ActiveProfile())) { throw 'Microsoft Pinyin is not available; H3 not tested.' }

    Note '1. target composition while the panel is not editing'
    [void][TargetIme]::SetNative($form.Box.Handle)
    [Probe]::TypeLetters($form, $panel, 'zhong')
    [Probe]::Pump(400)
    $composition = [TargetIme]::Composition($form.Box.Handle)
    Check 'target composes zhong' ($composition -replace "'", '' -eq 'zhong') "composition '$composition'"

    Send-ProbeCommand $Exe '--selftest-show'
    [void](Wait-ProbeEvent 'shown' 3000)
    [Probe]::Pump(200)
    $client = [Probe]::ClientRectOnScreen($panel)
    $steps = [ordered]@{}
    $steps['panel shown'] = [TargetIme]::Composition($form.Box.Handle)
    $x = [int](($client[0] + $client[2]) / 2); $y = [int](($client[1] + $client[3]) / 2)
    [void][Probe]::MoveTo($x, $y); [Probe]::Pump(200)
    $steps['hover'] = [TargetIme]::Composition($form.Box.Handle)
    [Probe]::Click($form, $panel); [Probe]::Pump(200)
    $steps['click'] = [TargetIme]::Composition($form.Box.Handle)
    [Probe]::Tap($form, $panel, 0x28); [Probe]::Pump(200)
    $steps['hook key Down'] = [TargetIme]::Composition($form.Box.Handle)
    Send-ProbeCommand $Exe '--selftest-hide'
    [void](Wait-ProbeEvent 'hidden' 3000)
    [Probe]::Pump(200)
    $steps['panel hidden'] = [TargetIme]::Composition($form.Box.Handle)
    foreach ($step in $steps.Keys) {
        Check "target composition kept: $step" ($steps[$step] -eq $composition) "'$($steps[$step])'"
    }
    Check 'target kept the foreground' ([Probe]::GetForegroundWindow() -eq $form.Handle)
    [Probe]::Tap($form, $panel, 0x20)
    [Probe]::Pump(600)
    Note "  target after Space: composition '$([TargetIme]::Composition($form.Box.Handle))', events: $($form.Drain())"
    Check 'Space commits zhong in the target' ($form.Box.Text -eq ('probe' + $zhong)) "text '$($form.Box.Text)'"
    $form.Box.Text = 'probe'
    $form.Box.SelectionStart = 5

    Note '2. editing in the panel'
    # Show the panel near the primary screen's top-left corner: the candidate bar is wide, and next to
    # a screen's right edge the input method shifts it left, away from the composition.
    $work = [Probe]::PrimaryWorkArea()
    [void][Probe]::MoveTo($work[0] + 60, $work[1] + 60)
    [Probe]::Pump(100)
    Send-ProbeCommand $Exe '--selftest-show'
    [void](Wait-ProbeEvent 'shown' 3000)
    $client = [Probe]::ClientRectOnScreen($panel)
    $scale = [double]$app.Ready.dpi / 96.0
    [void][Probe]::MoveTo([int]($client[0] + 120 * $scale), [int]($client[1] + 28 * $scale))
    [Probe]::Pump(120)
    [Probe]::Click($form, $panel)
    $editing = Wait-ProbeEvent 'editing' 2000
    [Probe]::Pump(300)
    Check 'click on the search box enters editing' ($null -ne $editing -and $editing.entered) $editing.error
    Check 'panel is the foreground window' ([Probe]::GetForegroundWindow() -eq $panel)
    # Microsoft Pinyin starts a new input context in English mode and applies a mode change a moment
    # later, so switch and read it back until the Chinese (native) bit is set.
    $chinese = $false
    for ($attempt = 1; $attempt -le 5 -and -not $chinese; $attempt++) {
        Send-ProbeCommand $Exe '--selftest-ime-native'
        [void](Wait-ProbeEvent 'ime_native' 3000)
        [Probe]::Pump(300)
        $state = Get-ImeState
        $chinese = ($null -ne $state) -and $state.attached -and (([int]$state.conversion -band 1) -ne 0)
    }
    Check 'panel input method is in Chinese mode' $chinese "conversion $($state.conversion), attached $($state.attached)"

    $before = Get-Band
    [Probe]::TypeLetters($form, $panel, 'nihao')
    [Probe]::Pump(600)
    $state = Get-ImeState
    $first = Find-Candidate $before
    Note "  composing nihao: composition '$($state.composition)', GPUI candidate form $($state.candidate -join ','), changed band $($first -join ',')"
    Save-PanelShot 'ime-1-nihao'
    Check 'panel composes nihao' (($state.composition -replace "'", '') -eq 'nihao') "'$($state.composition)'"
    # The candidate window starts at the composition, inside the search box's text area.
    $atInput = ($null -ne $first) -and $first[0] -ge 12 * $scale -and $first[0] -le 200 * $scale
    Check 'candidate window appears right below the search box, at the composition' $atInput "changed band $($first -join ','), scale $scale"

    [Probe]::Tap($form, $panel, 0x20)
    [Probe]::Pump(400)
    $inputs = @(Get-ProbeEvents 'input')
    $value = if ($inputs.Count -gt 0) { $inputs[-1].value } else { '' }
    Check 'Space commits nihao in the panel' ($value -eq $nihao) "value '$value'"

    $before = Get-Band
    [Probe]::TypeLetters($form, $panel, 'shijie')
    [Probe]::Pump(600)
    $state = Get-ImeState
    $second = Find-Candidate $before
    Note "  composing shijie: composition '$($state.composition)', GPUI candidate form $($state.candidate -join ','), changed band $($second -join ',')"
    Save-PanelShot 'ime-2-shijie'
    # Two committed characters are wider than 20 logical px: a candidate window back at the input's
    # origin would start where the first one did (gpui-kit#3286).
    $moved = ($null -ne $first) -and ($null -ne $second) -and $second[0] -gt $first[0] + 20 * $scale
    Check 'second candidate window follows the caret (gpui-kit#3286 not reproduced)' $moved "first left $($first -join ','), second $($second -join ',')"

    [Probe]::Tap($form, $panel, 0x20)
    [Probe]::Pump(400)
    $inputs = @(Get-ProbeEvents 'input')
    $value = if ($inputs.Count -gt 0) { $inputs[-1].value } else { '' }
    Check 'Space commits shijie after nihao' ($value -eq ($nihao + $shijie)) "value '$value'"

    [Probe]::Tap($form, $panel, 0x1B)
    $ended = Wait-ProbeEvent 'editing_ended' 2000
    [Probe]::Pump(300)
    Check 'Esc leaves editing and gives the foreground back to the target' ($null -ne $ended -and [Probe]::GetForegroundWindow() -eq $form.Handle)
    Check 'target received no Alt and never entered menu mode' ($form.MenuActivations -eq 0 -and $form.KeyMenuCommands -eq 0) "menu $($form.MenuActivations), SC_KEYMENU $($form.KeyMenuCommands)"
    Send-ProbeCommand $Exe '--selftest-hide'
    [void](Wait-ProbeEvent 'hidden' 3000)
} catch {
    $failures.Add("ERROR: $($_.Exception.Message)")
    Note "ERROR: $($_.Exception.Message)"
} finally {
    if ($switched) {
        $hr = [Tsf]::Activate($originalTip.clsid, $originalTip.guidProfile, $originalTip.langid)
        [Probe]::Pump(300)
        Note ("input method restored: hr=0x{0:X}, now {1}" -f $hr, [Tsf]::Describe([Tsf]::ActiveProfile()))
    }
    try { if ($null -ne $app -and [Probe]::IsWindowVisible($app.Hwnd)) { Send-ProbeCommand $Exe '--selftest-hide' } } catch {}
    Stop-ProbeApp $app $Exe
    if ($null -ne $form) { $form.Close(); [Probe]::Pump(200) }
    [void][Probe]::SetCursorPos($userCursor.X, $userCursor.Y)
    if ($userForeground -ne [IntPtr]::Zero) { [void][Probe]::SetForegroundWindow($userForeground) }
    $report | Set-Content (Join-Path $results 'summary.txt') -Encoding UTF8
    Write-Host "results: $results"
}

if ($failures.Count -gt 0) { Write-Host "FAILED: $($failures -join '; ')"; exit 1 }
Write-Host 'PASSED'
