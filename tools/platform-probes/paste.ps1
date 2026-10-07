# H4 paste: Enter in the panel pastes back into the app that was in front, exactly once.
#
#   powershell -STA -NoProfile -ExecutionPolicy Bypass -File tools\platform-probes\installed-app.ps1 -Action Prepare
#   powershell -STA -NoProfile -ExecutionPolicy Bypass -File tools\platform-probes\paste.ps1 [-Exe <KwikPaste.exe>] [-Rounds 20] [-Targets form,rich] [-UiPanel] [-QuickPaste] [-PastePlain]
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
# 300 ms, and held 2.6 s, past the 2 s wait, which must not paste). -PastePlain checks the global plain
# paste hotkey (shortcuts.pastePlain = Ctrl+Alt+F11) with rich text, files and an image-only clipboard
# into the RichTextBox; -Targets none skips the Enter rounds.
param(
    [string]$Exe = '',
    [int]$Rounds = 20,
    [string]$Targets = 'form,rich',
    [int]$IdleSeconds = 30,
    [switch]$UiPanel,
    [switch]$QuickPaste,
    [switch]$PastePlain,
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
        if ($target -eq 'none') { continue }
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

    if ($PastePlain) {
        Note 'plain paste Ctrl+Alt+F11 into the RichTextBox'
        # The targets are PowerShell windows; a listed pause app would release every hotkey.
        Send-ProbeCommand $Exe '--selftest-settings={"shortcuts":{"pastePlain":"Control+Alt+F11","pauseAppIds":[]}}'
        [Probe]::Pump(500)
        if ($null -eq $rich) {
            $rich = New-Object RichTarget
            $work = [Probe]::PrimaryWorkArea()
            $rich.Bounds = New-Object System.Drawing.Rectangle(($work[0] + 700), ($work[1] + 60), 560, 240)
            $rich.Show()
            [Probe]::Pump(400)
        }
        $center = $rich.Box.PointToScreen((New-Object System.Drawing.Point(200, 100)))
        if (-not [Probe]::ClickIntoForegroundAt($rich.Handle, $center.X, $center.Y)) { throw 'the RichTextBox window did not take the foreground' }
        $VK_F11 = [uint16]0x7A
        function Invoke-PlainPaste {
            [void](Get-ProbeEvents 'pasted')
            [void](Get-ProbeEvents 'clipboard')
            [Probe]::ChordFor($rich.Handle, $panel, [uint16[]]@($VK_CONTROL, $VK_MENU), $VK_F11, 300)
            $pasted = Wait-ProbeEvent 'pasted' 3000
            [Probe]::Pump(600)
            return $pasted
        }

        # Rich text (text + RTF + HTML): pasted unformatted, the clipboard keeps only the text, no new history item.
        $token = "kp-$stamp-plain"
        $rich.Box.Clear()
        $formats = New-Object 'System.Collections.Generic.Dictionary[string,byte[]]'
        $formats['#13'] = [Clip]::Text($token)
        $formats['Rich Text Format'] = [Clip]::Ascii('{\rtf1\ansi{\fonttbl\f0\fswiss Arial;}\f0\b ' + $token + '\b0\par}')
        $formats['HTML Format'] = [Clip]::Html("<b>$token</b>")
        [Clip]::Set($formats)
        if ($null -eq (Wait-Captured $token)) { Note '  the rich token was not stored' }
        $pasted = Invoke-PlainPaste
        $rich.Box.SelectAll()
        $bold = $rich.Box.SelectionFont -ne $null -and $rich.Box.SelectionFont.Bold
        $left = [Clip]::Formats() | Where-Object { $_ -eq 'Rich Text Format' -or $_ -eq 'HTML Format' }
        $recorded = @(Get-ProbeEvents 'clipboard')
        Expect 'plain paste injected for rich text' ([int]($null -ne $pasted -and $pasted.kind -eq 'plain')) 1
        Expect 'rich text pasted once, unformatted' ([int]($rich.Box.Text.TrimEnd() -eq $token -and -not $bold)) 1
        Expect 'clipboard left as plain text' ([int]($null -eq $left -and [Clip]::GetText() -eq $token)) 1
        Expect 'no history item for the stripped write' ([int]($recorded.Count -eq 0)) 1

        # Files: the paths are pasted as text.
        $file = Join-Path $env:WINDIR 'win.ini'
        $rich.Box.Clear()
        $formats = New-Object 'System.Collections.Generic.Dictionary[string,byte[]]'
        $formats['#15'] = [Clip]::Files(@($file))
        [Clip]::Set($formats)
        [Probe]::Pump(600)
        $pasted = Invoke-PlainPaste
        Expect 'files pasted as their path' ([int]($null -ne $pasted -and $rich.Box.Text.TrimEnd() -eq $file)) 1

        # Image only: nothing to paste.
        $rich.Box.Clear()
        $formats = New-Object 'System.Collections.Generic.Dictionary[string,byte[]]'
        $formats['#8'] = [Clip]::Dib(16, 16, [System.Drawing.Color]::Red, [System.Drawing.Color]::Blue)
        [Clip]::Set($formats)
        [Probe]::Pump(600)
        $pasted = Invoke-PlainPaste
        Expect 'image-only clipboard: no paste' ([int]($null -eq $pasted -and $rich.Box.Text -eq '')) 1
        Send-ProbeCommand $Exe '--selftest-settings={"shortcuts":{"pastePlain":""}}'
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
