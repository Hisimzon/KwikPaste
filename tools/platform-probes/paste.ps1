# H4 paste: Enter in the panel pastes back into the app that was in front, exactly once.
#
#   powershell -STA -NoProfile -ExecutionPolicy Bypass -File tools\platform-probes\installed-app.ps1 -Action Prepare
#   powershell -STA -NoProfile -ExecutionPolicy Bypass -File tools\platform-probes\paste.ps1 [-Exe <KwikPaste.exe>] [-Rounds 20] [-Targets form,rich] [-UiPanel] [-QuickPaste]
#   powershell -STA -NoProfile -ExecutionPolicy Bypass -File tools\platform-probes\installed-app.ps1 -Action Restore
#
# Real system clipboard (--selftest-real-clipboard): the installed app must not be running. Each round
# copies a unique token in the target (the watcher stores it as the newest item), shows the panel with
# the development hotkey, presses Enter (swallowed by the panel's keyboard hook) and checks the target's
# text grew by the token exactly once, the target kept the foreground and the panel hid.
#
# Targets are windows this script creates: form (a WinForms TextBox), rich (a WinForms RichTextBox
# in a second window). Apps that restore the user's session (Notepad, Edge, Office) are never used.
# Word is not installed on this machine. -UiPanel uses the real clipboard list (ListIntent::Paste) instead of the
# platform probe view. -QuickPaste also checks the global quick paste (Ctrl+Alt+1 with the modifiers held
# 300 ms, and held 2.6 s, past the 2 s wait, which must not paste).
param(
    [string]$Exe = '',
    [int]$Rounds = 20,
    [string]$Targets = 'form,rich',
    [int]$IdleSeconds = 30,
    [switch]$UiPanel,
    [switch]$QuickPaste,
    # Wait this long after the watcher stored the token before opening the panel (a user takes a moment).
    [int]$SettleMs = 0
)

. "$PSScriptRoot\common.ps1"
Add-Type -ReferencedAssemblies System.Windows.Forms, System.Drawing @'
using System.Drawing;
using System.Windows.Forms;
/// A second paste target: a RichTextBox in its own window.
public class RichTarget : Form {
    public RichTextBox Box;
    public RichTarget() {
        Text = "kwikpaste-probe-rich-target";
        StartPosition = FormStartPosition.Manual;
        Box = new RichTextBox();
        Box.Dock = DockStyle.Fill;
        Controls.Add(Box);
    }
}
'@
$Exe = Resolve-AppExe $Exe
$results = New-ResultDir 'paste'
$report = New-Object System.Collections.Generic.List[string]
function Note([string]$Line) { $report.Add($Line); Write-Host $Line }
$failures = New-Object System.Collections.Generic.List[string]
function Expect([string]$Name, [int]$Passed, [int]$Total) {
    Note ("  {0}: {1}/{2}" -f $Name, $Passed, $Total)
    if ($Passed -ne $Total) { $failures.Add($Name) }
}

$stamp = Get-Date -Format 'HHmmss'
$VK_CONTROL = [uint16]0x11
$VK_MENU = [uint16]0x12

function Wait-Captured([string]$Token, [int]$TimeoutMs = 3000) {
    $watch = [Diagnostics.Stopwatch]::StartNew()
    while ($watch.ElapsedMilliseconds -lt $TimeoutMs) {
        foreach ($event in @(Get-ProbeEvents 'clipboard')) { if ($event.item.content -eq $Token) { return $event } }
        [Probe]::Pump(30)
    }
    return $null
}

$running = @(Get-Process KwikPaste -ErrorAction SilentlyContinue | Where-Object { $_.Path -and $_.Path -notlike '*\target\*' })
if ($running.Count -gt 0) { throw "The installed KwikPaste is running ($($running[0].Path)); quit it first with installed-app.ps1 -Action Prepare." }
Assert-Desktop -IdleSeconds $IdleSeconds -NeedsInput
$userForeground = [Probe]::GetForegroundWindow()
$userCursor = New-Object Probe+POINT; [void][Probe]::GetCursorPos([ref]$userCursor)

$app = $null; $form = $null; $rich = $null
$arguments = @('--selftest-real-clipboard')
if ($UiPanel) { $arguments += '--selftest-ui-panel' }
try {
    $app = Start-ProbeApp $Exe $results -Arguments $arguments
    $panel = $app.Hwnd
    Note ("probe app pid {0}, panel 0x{1:X}, content: {2}" -f $app.Process.Id, $panel.ToInt64(), $(if ($UiPanel) { 'clipboard list' } else { 'platform probe view' }))
    # The development hotkey (Ctrl+Alt+Shift+F9) contains Alt+Shift, the Windows switch-input-language
    # chord, which can change the target's input language or move its focus. The paste rounds use a
    # hotkey without Shift instead; the default Alt+C has no Shift either.
    Send-ProbeCommand $Exe '--selftest-settings={"shortcuts":{"openClipboard":"Control+Alt+F10"}}'
    [Probe]::Pump(500)
    $form = New-TargetForm

    foreach ($target in $Targets.Split(',')) {
        Note "Enter pastes into $target"
        if ($target -eq 'form') {
            $window = $form.Handle
            [void][Probe]::ClickIntoForeground($form)
            $form.Box.SelectionStart = $form.Box.Text.Length
            $read = { $form.Box.Text }
        } elseif ($target -eq 'rich') {
            $rich = New-Object RichTarget
            $work = [Probe]::PrimaryWorkArea()
            $rich.Bounds = New-Object System.Drawing.Rectangle(($work[0] + 700), ($work[1] + 60), 560, 240)
            $rich.Show()
            [Probe]::Pump(400)
            $center = $rich.Box.PointToScreen((New-Object System.Drawing.Point(200, 100)))
            if (-not [Probe]::ClickIntoForegroundAt($rich.Handle, $center.X, $center.Y)) { Note '  the RichTextBox window did not take the foreground'; $failures.Add('rich'); continue }
            $window = $rich.Handle
            $read = { $rich.Box.Text }
        } else {
            throw "unknown target $target"
        }

        $count = @{ Captured = 0; Pasted = 0; Once = 0; Foreground = 0; Hidden = 0 }
        $elapsed = New-Object System.Collections.Generic.List[double]
        for ($round = 1; $round -le $Rounds; $round++) {
            $label = "$target$round"
            $token = "[kp-$stamp-$label]"
            $before = & $read
            [Clip]::SetText($token)
            if ($null -eq (Wait-Captured $token)) { Note "${label}: the watcher did not store the token"; continue }
            if ($SettleMs -gt 0) { [Probe]::Pump($SettleMs) }
            $count.Captured++
            [void](Get-ProbeEvents 'pasted')
            [Probe]::ChordFor($window, $panel, [uint16[]]@($VK_CONTROL, $VK_MENU), [uint16]0x79, 30)
            if ($null -eq (Wait-ProbeEvent 'shown' 3000)) { Note "${label}: the panel did not show"; continue }
            [Probe]::Pump(150)
            [Probe]::TapFor($window, $panel, 0x0D)
            $pasted = Wait-ProbeEvent 'pasted' 3000
            [Probe]::Pump(400)
            $after = & $read
            if ($null -ne $pasted) { $count.Pasted++; $elapsed.Add([double]$pasted.elapsed_ms) }
            $expected = $before + $token
            if ($after -eq $expected) { $count.Once++ } else { Note "${label}: text '$after' expected '$expected'" }
            if ([Probe]::GetForegroundWindow() -eq $window) { $count.Foreground++ } else { Note ("{0}: foreground 0x{1:X}" -f $label, [Probe]::GetForegroundWindow().ToInt64()) }
            if (-not [Probe]::IsWindowVisible($panel)) { $count.Hidden++ } else { Send-ProbeCommand $Exe '--selftest-hide'; [void](Wait-ProbeEvent 'hidden' 3000) }
        }
        Expect "$target token stored by the watcher" $count.Captured $Rounds
        Expect "$target paste injected" $count.Pasted $Rounds
        Expect "$target text grew by the token exactly once" $count.Once $Rounds
        Expect "$target kept the foreground" $count.Foreground $Rounds
        Expect "$target panel hidden after the paste" $count.Hidden $Rounds
        if ($elapsed.Count -gt 0) {
            Note ("  Enter -> paste injected: p50 {0} ms, p95 {1} ms, max {2} ms" -f (Format-Ms (Get-Percentile $elapsed 50)), (Format-Ms (Get-Percentile $elapsed 95)), (Format-Ms (($elapsed | Measure-Object -Maximum).Maximum)))
        }
    }

    if ($QuickPaste) {
        Note 'quick paste Ctrl+Alt+1 into the form'
        Send-ProbeCommand $Exe '--selftest-settings={"shortcuts":{"quickPaste":{"enabled":true,"modifiers":"controlAlt"}}}'
        [Probe]::Pump(500)
        [void][Probe]::ClickIntoForeground($form)
        $form.Box.SelectionStart = $form.Box.Text.Length
        $menuBefore = $form.MenuActivations
        $quick = @{ Once = 0; Pasted = 0 }
        $quickRounds = 5
        for ($round = 1; $round -le $quickRounds; $round++) {
            $token = "[kp-$stamp-quick$round]"
            $before = $form.Box.Text
            [Clip]::SetText($token)
            if ($null -eq (Wait-Captured $token)) { Note "quick${round}: not stored"; continue }
            [void](Get-ProbeEvents 'pasted')
            [Probe]::ChordFor($form.Handle, $panel, [uint16[]]@($VK_CONTROL, $VK_MENU), [uint16]0x31, 300)
            $pasted = Wait-ProbeEvent 'pasted' 3000
            [Probe]::Pump(400)
            if ($null -ne $pasted -and $pasted.kind -eq 'quick') { $quick.Pasted++ }
            if ($form.Box.Text -eq $before + $token) { $quick.Once++ } else { Note "quick${round}: text '$($form.Box.Text)'" }
        }
        Expect 'quick paste injected after the modifiers were released' $quick.Pasted $quickRounds
        Expect 'quick paste text grew by the newest item exactly once' $quick.Once $quickRounds

        $token = "[kp-$stamp-held]"
        $before = $form.Box.Text
        [Clip]::SetText($token)
        [void](Wait-Captured $token)
        [void](Get-ProbeEvents 'pasted')
        [Probe]::ChordFor($form.Handle, $panel, [uint16[]]@($VK_CONTROL, $VK_MENU), [uint16]0x31, 2600)
        $pasted = Wait-ProbeEvent 'pasted' 1500
        [Probe]::Pump(300)
        Expect 'modifiers held past 2 s: no paste, the item stays on the clipboard' ([int]($null -eq $pasted -and $form.Box.Text -eq $before -and [Clip]::GetText() -eq $token)) 1
        Expect 'the form never entered menu mode (Alt release masked)' ([int]($form.MenuActivations -eq $menuBefore)) 1
        Send-ProbeCommand $Exe '--selftest-settings={"shortcuts":{"quickPaste":{"enabled":false}}}'
    }
} catch {
    $failures.Add("ERROR: $($_.Exception.Message)")
    Note "ERROR: $($_.Exception.Message)"
} finally {
    try { if ($null -ne $app -and [Probe]::IsWindowVisible($app.Hwnd)) { Send-ProbeCommand $Exe '--selftest-hide' } } catch {}
    try { if ($null -ne $app -and -not $app.Process.HasExited) { Send-ProbeCommand $Exe '--selftest-settings={"shortcuts":{"openClipboard":"Alt+C"}}' } } catch {}
    Stop-ProbeApp $app $Exe
    if ($null -ne $rich) { $rich.Close() }
    if ($null -ne $form) { $form.Close(); [Probe]::Pump(200) }
    [void][Probe]::SetCursorPos($userCursor.X, $userCursor.Y)
    if ($userForeground -ne [IntPtr]::Zero) { [void][Probe]::SetForegroundWindow($userForeground) }
    $report | Set-Content (Join-Path $results 'summary.txt') -Encoding UTF8
    Write-Host "results: $results"
}

if ($failures.Count -gt 0) { Write-Host "FAILED: $($failures -join '; ')"; exit 1 }
Write-Host 'PASSED'
