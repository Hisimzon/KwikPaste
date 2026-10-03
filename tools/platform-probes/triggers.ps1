# Win+V takeover and the mouse-button trigger (shortcuts.winV / shortcuts.mouseTrigger, as 1.x).
#
#   powershell -STA -NoProfile -ExecutionPolicy Bypass -File tools\platform-probes\triggers.ps1 [-Exe <KwikPaste.exe>]
#
# Input goes only to this script's form (the cursor stays over it). The probe app accepts injected
# V presses carrying the probe marker (dwExtraInfo "KPPR"); real Win+V handling skips injected keys.
# 1. Win+V on: Win+V toggles the panel twice (show, hide); the V never reaches the form, the form stays
#    the foreground window (no Start menu, no clipboard history). Off: the hook thread stops. The system
#    clipboard-history settings (Clipboard\EnableClipboardHistory, Explorer\Advanced\DisabledHotkeys) are
#    read before and after and must be unchanged (1.x and 2.0 never write them).
# 2. Mouse trigger: Back (XBUTTON1) click shows and hides the panel and never reaches the form; Middle
#    click toggles; a middle drag past the threshold goes back to the form (button replayed) without a
#    toggle; Disabled passes the middle click through.
param(
    [string]$Exe = '',
    [int]$IdleSeconds = 10
)

. "$PSScriptRoot\common.ps1"
Add-Type -ReferencedAssemblies System.Windows.Forms, System.Drawing @'
using System;
using System.Runtime.InteropServices;
using System.Windows.Forms;

/// Counts the mouse buttons that reach it.
public class MouseTarget : Form {
    public int Middle, XButton1, Keys;
    public MouseTarget() {
        Text = "kwikpaste-probe-trigger-target";
        StartPosition = FormStartPosition.Manual;
        KeyPreview = true;
        MouseDown += (s, e) => { if (e.Button == MouseButtons.Middle) Middle++; if (e.Button == MouseButtons.XButton1) XButton1++; };
        KeyDown += (s, e) => { if (e.KeyCode == System.Windows.Forms.Keys.V) Keys++; };
    }
}

public static class TriggerInput {
    [StructLayout(LayoutKind.Sequential)] struct MOUSEINPUT { public int dx, dy; public uint mouseData, dwFlags, time; public IntPtr dwExtraInfo; }
    [StructLayout(LayoutKind.Sequential)] struct KEYBDINPUT { public ushort wVk, wScan; public uint dwFlags, time; public IntPtr dwExtraInfo; }
    [StructLayout(LayoutKind.Explicit)] struct UNION { [FieldOffset(0)] public MOUSEINPUT mi; [FieldOffset(0)] public KEYBDINPUT ki; }
    [StructLayout(LayoutKind.Sequential)] struct INPUT { public uint type; public UNION u; }
    [DllImport("user32.dll")] static extern uint SendInput(uint n, INPUT[] inputs, int size);
    const long PROBE_MARKER = 0x4B505052;
    static void Send(INPUT input) { SendInput(1, new[] { input }, Marshal.SizeOf(typeof(INPUT))); }
    static void Key(ushort vk, bool up, bool marked) {
        var input = new INPUT { type = 1 };
        input.u.ki.wVk = vk;
        input.u.ki.dwFlags = up ? 2u : 0u;
        if (marked) input.u.ki.dwExtraInfo = (IntPtr)PROBE_MARKER;
        Send(input);
    }
    public static void Mouse(uint flags, uint data) {
        var input = new INPUT { type = 0 };
        input.u.mi.dwFlags = flags;
        input.u.mi.mouseData = data;
        Send(input);
    }
    /// Win down, V down/up (probe-marked), Win up.
    public static void WinV() {
        Key(0x5B, false, false); System.Threading.Thread.Sleep(30);
        Key(0x56, false, true); System.Threading.Thread.Sleep(30);
        Key(0x56, true, true); System.Threading.Thread.Sleep(30);
        Key(0x5B, true, false);
    }
}
'@

$Exe = Resolve-AppExe $Exe
$results = New-ResultDir 'triggers'
$report = New-Object System.Collections.Generic.List[string]
function Note([string]$Line) { $report.Add($Line); Write-Host $Line }
$failures = New-Object System.Collections.Generic.List[string]
function Check([string]$Name, [bool]$Passed, [string]$Detail = '') {
    Note ("  {0}: {1}{2}" -f $Name, $(if ($Passed) { 'ok' } else { 'FAIL' }), $(if ($Detail) { " ($Detail)" } else { '' }))
    if (-not $Passed) { $failures.Add($Name) }
}
function Get-SystemClipboardSettings {
    $history = (Get-ItemProperty 'HKCU:\Software\Microsoft\Clipboard' -ErrorAction SilentlyContinue).EnableClipboardHistory
    $hotkeys = (Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced' -ErrorAction SilentlyContinue).DisabledHotkeys
    return "EnableClipboardHistory=$history DisabledHotkeys=$hotkeys"
}
function Set-Setting([string]$Json) { Send-ProbeCommand $Exe "--selftest-settings=$Json"; [Probe]::Pump(600) }
function Wait-Toggle([string]$Kind) { return $null -ne (Wait-ProbeEvent $Kind 2000) }

$MOUSE_XDOWN = 0x0080; $MOUSE_XUP = 0x0100; $MOUSE_MDOWN = 0x0020; $MOUSE_MUP = 0x0040

Assert-Desktop -IdleSeconds $IdleSeconds -NeedsInput
$userForeground = [Probe]::GetForegroundWindow()
$userCursor = New-Object Probe+POINT; [void][Probe]::GetCursorPos([ref]$userCursor)
$before = Get-SystemClipboardSettings
Note "system clipboard settings before: $before"
$app = $null; $target = $null
try {
    $app = Start-ProbeApp $Exe $results
    $panel = $app.Hwnd
    $work = [Probe]::PrimaryWorkArea()
    $target = New-Object MouseTarget
    $target.Bounds = New-Object System.Drawing.Rectangle(($work[0] + 80), ($work[1] + 80), 400, 300)
    $target.Show()
    [Probe]::Pump(300)
    $center = $target.PointToScreen((New-Object System.Drawing.Point(200, 150)))
    if (-not [Probe]::ClickIntoForegroundAt($target.Handle, $center.X, $center.Y)) { throw 'The target form did not take the foreground.' }

    Note '1. Win+V'
    Set-Setting '{"shortcuts":{"winV":true}}'
    Clear-ProbeEvents
    $keysBefore = $target.Keys
    [Probe]::RequireForegroundOf($target.Handle, $panel, 'Win+V')
    [TriggerInput]::WinV(); $shown = Wait-Toggle 'shown'; [Probe]::Pump(300)
    $fgAfterShow = [Probe]::GetForegroundWindow()
    [Probe]::RequireForegroundOf($target.Handle, $panel, 'Win+V')
    [TriggerInput]::WinV(); $hidden = Wait-Toggle 'hidden'; [Probe]::Pump(300)
    Check 'Win+V shows, then hides the panel' ($shown -and $hidden)
    Check 'the V never reached the form' ($target.Keys -eq $keysBefore) "keys at the form: $($target.Keys - $keysBefore)"
    Check 'the form stayed in front (no Start menu, no clipboard history)' ($fgAfterShow -eq $target.Handle -and [Probe]::GetForegroundWindow() -eq $target.Handle) ("foreground 0x{0:X}" -f [Probe]::GetForegroundWindow().ToInt64())
    Set-Setting '{"shortcuts":{"winV":false}}'
    $log = (Get-Content $app.Stderr -Encoding UTF8 -ErrorAction SilentlyContinue) -join "`n"
    Check 'turning it off stops the hook' ($log -match 'Win\+V takeover off')

    Note '2. mouse trigger'
    [void][Probe]::MoveTo($center.X, $center.Y)
    Set-Setting '{"shortcuts":{"mouseTrigger":"back"}}'
    Clear-ProbeEvents
    $xBefore = $target.XButton1
    [TriggerInput]::Mouse($MOUSE_XDOWN, 1); [Probe]::Pump(40); [TriggerInput]::Mouse($MOUSE_XUP, 1)
    $shown = Wait-Toggle 'shown'; [Probe]::Pump(200)
    [TriggerInput]::Mouse($MOUSE_XDOWN, 1); [Probe]::Pump(40); [TriggerInput]::Mouse($MOUSE_XUP, 1)
    $hidden = Wait-Toggle 'hidden'; [Probe]::Pump(200)
    Check 'Back button shows, then hides the panel; the form never sees it' ($shown -and $hidden -and $target.XButton1 -eq $xBefore) "XButton1 at the form: $($target.XButton1 - $xBefore)"

    Set-Setting '{"shortcuts":{"mouseTrigger":"middle"}}'
    Clear-ProbeEvents
    $mBefore = $target.Middle
    if ([Probe]::RootAt($center.X, $center.Y) -ne $target.Handle) { throw 'ABORT: the cursor is not over the target form' }
    [TriggerInput]::Mouse($MOUSE_MDOWN, 0); [Probe]::Pump(40); [TriggerInput]::Mouse($MOUSE_MUP, 0)
    $shown = Wait-Toggle 'shown'; [Probe]::Pump(200)
    Check 'Middle click toggles the panel; the form never sees it' ($shown -and $target.Middle -eq $mBefore)
    Send-ProbeCommand $Exe '--selftest-hide'; [void](Wait-ProbeEvent 'hidden' 2000)
    Clear-ProbeEvents
    [void][Probe]::MoveTo($center.X, $center.Y)
    [TriggerInput]::Mouse($MOUSE_MDOWN, 0); [Probe]::Pump(40)
    for ($i = 1; $i -le 6; $i++) { [void][Probe]::MoveTo($center.X + 8 * $i, $center.Y); [Probe]::Pump(20) }
    [TriggerInput]::Mouse($MOUSE_MUP, 0); [Probe]::Pump(300)
    $toggled = Wait-Toggle 'shown'
    Check 'a middle drag goes back to the form, no toggle' (-not $toggled -and $target.Middle -eq $mBefore + 1) "middle presses at the form: $($target.Middle - $mBefore)"

    Set-Setting '{"shortcuts":{"mouseTrigger":"disabled"}}'
    Clear-ProbeEvents
    [void][Probe]::MoveTo($center.X, $center.Y)
    $mBefore = $target.Middle
    [TriggerInput]::Mouse($MOUSE_MDOWN, 0); [Probe]::Pump(40); [TriggerInput]::Mouse($MOUSE_MUP, 0); [Probe]::Pump(300)
    Check 'disabled: the middle click reaches the form' ($target.Middle -eq $mBefore + 1 -and -not (Wait-Toggle 'shown'))
} catch {
    $failures.Add("ERROR: $($_.Exception.Message)")
    Note "ERROR: $($_.Exception.Message)"
} finally {
    try { if ($null -ne $app -and -not $app.Process.HasExited) { Set-Setting '{"shortcuts":{"winV":false,"mouseTrigger":"disabled"}}' } } catch {}
    Stop-ProbeApp $app $Exe
    if ($null -ne $target) { $target.Close() }
    $after = Get-SystemClipboardSettings
    Note "system clipboard settings after:  $after"
    Check 'system clipboard-history settings unchanged' ($before -eq $after)
    [void][Probe]::SetCursorPos($userCursor.X, $userCursor.Y)
    if ($userForeground -ne [IntPtr]::Zero) { [void][Probe]::SetForegroundWindow($userForeground) }
    $report | Set-Content (Join-Path $results 'summary.txt') -Encoding UTF8
    Write-Host "results: $results"
}

if ($failures.Count -gt 0) { Write-Host "FAILED: $($failures -join '; ')"; exit 1 }
Write-Host 'PASSED'
