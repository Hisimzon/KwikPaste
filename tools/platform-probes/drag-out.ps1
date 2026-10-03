# H5 drag-out: drag from the never-activating panel into a WinForms drop target this script owns.
#
#   powershell -STA -NoProfile -ExecutionPolicy Bypass -File tools\platform-probes\drag-out.ps1 [-Exe <KwikPaste.exe>] [-Rounds 3] [-EscRounds 5]
#
# The probe app's platform view has a drag source (the middle of the panel) wired like a list card
# (DragTracker + drag_out::start). Real mouse input (SendInput) presses there, moves past the system
# drag threshold, carries the drag over to the target and releases. The app only lets drops land on its
# own windows and on this script's process (KWIKPASTE_DRAG_ALLOW), and cancels any drag after 15 s. No
# clipboard is involved, so the installed app keeps running.
#
# 1. Plain text, text + HTML, text + RTF, a PNG file (image records drag their original file) and two
#    files reach the target with the right formats and content; DoDragDrop reports Dropped (effect COPY).
# 2. Self-drop: released on the panel itself, the drop is refused (effect NONE); GPUI sees no FileDrop and
#    no click; a right click on the source row afterwards is not turned into a ghost left click.
# 3. Esc while dragging cancels the drag (the keyboard hook swallows Esc); key to DoDragDrop returning
#    must be <= 100 ms; the target receives neither the drop nor the Esc.
# Throughout: the panel is never activated (WM_ACTIVATE count unchanged), the target stays the
# foreground window. Word / WPS are not installed on this machine and are not tested.
param(
    [string]$Exe = '',
    [int]$Rounds = 3,
    [int]$EscRounds = 5,
    [int]$IdleSeconds = 30
)

. "$PSScriptRoot\common.ps1"
Add-Type -ReferencedAssemblies System.Windows.Forms, System.Drawing @'
using System;
using System.Diagnostics;
using System.Drawing;
using System.Runtime.InteropServices;
using System.Windows.Forms;

/// The drop target: accepts any drop as Copy and records what arrived.
public class DropTarget : Form {
    public int Enters, Drops, EscapeKeys;
    public string[] Formats = new string[0];
    public string Unicode, Html, Rtf;
    public string[] Files = new string[0];
    public DropTarget() {
        Text = "kwikpaste-probe-drop-target";
        StartPosition = FormStartPosition.Manual;
        AllowDrop = true;
        KeyPreview = true;
        var label = new Label();
        label.Dock = DockStyle.Fill;
        label.Text = "drop here";
        label.AllowDrop = false;
        Controls.Add(label);
        DragEnter += (s, e) => { Enters++; e.Effect = Accept(e); };
        DragOver += (s, e) => { e.Effect = Accept(e); };
        DragDrop += (s, e) => {
            Drops++;
            Formats = e.Data.GetFormats();
            Unicode = e.Data.GetData(DataFormats.UnicodeText) as string;
            Html = e.Data.GetData(DataFormats.Html) as string;
            Rtf = e.Data.GetData(DataFormats.Rtf) as string;
            Files = e.Data.GetData(DataFormats.FileDrop) as string[] ?? new string[0];
        };
        KeyDown += (s, e) => { if (e.KeyCode == Keys.Escape) EscapeKeys++; };
    }
    static DragDropEffects Accept(DragEventArgs e) {
        return (e.AllowedEffect & DragDropEffects.Copy) != 0 ? DragDropEffects.Copy : DragDropEffects.None;
    }
    public void Reset() {
        Enters = 0; Drops = 0; EscapeKeys = 0;
        Formats = new string[0]; Unicode = null; Html = null; Rtf = null; Files = new string[0];
    }
}

/// Raw mouse buttons and the Esc key through SendInput (moves go through Probe.MoveTo).
public static class DragInput {
    [StructLayout(LayoutKind.Sequential)] struct MOUSEINPUT { public int dx, dy; public uint mouseData, dwFlags, time; public IntPtr dwExtraInfo; }
    [StructLayout(LayoutKind.Sequential)] struct KEYBDINPUT { public ushort wVk, wScan; public uint dwFlags, time; public IntPtr dwExtraInfo; }
    [StructLayout(LayoutKind.Explicit)] struct UNION { [FieldOffset(0)] public MOUSEINPUT mi; [FieldOffset(0)] public KEYBDINPUT ki; }
    [StructLayout(LayoutKind.Sequential)] struct INPUT { public uint type; public UNION u; }
    [DllImport("user32.dll")] static extern uint SendInput(uint n, INPUT[] inputs, int size);
    static void Mouse(uint flags) {
        var input = new INPUT { type = 0 };
        input.u.mi.dwFlags = flags;
        SendInput(1, new[] { input }, Marshal.SizeOf(typeof(INPUT)));
    }
    static void Key(ushort vk, bool up) {
        var input = new INPUT { type = 1 };
        input.u.ki.wVk = vk;
        input.u.ki.dwFlags = up ? 2u : 0u;
        SendInput(1, new[] { input }, Marshal.SizeOf(typeof(INPUT)));
    }
    public static void LeftDown() { Mouse(0x0002); }
    public static void LeftUp() { Mouse(0x0004); }
    public static void RightClick() { Mouse(0x0008); System.Threading.Thread.Sleep(60); Mouse(0x0010); }
    /// Presses and releases Esc; returns the Stopwatch timestamp taken right before the press.
    public static long Escape() {
        long at = Stopwatch.GetTimestamp();
        Key(0x1B, false);
        Key(0x1B, true);
        return at;
    }
    public static bool IsLeftDown() { return (GetAsyncKeyState(0x01) & 0x8000) != 0; }
    [DllImport("user32.dll")] static extern short GetAsyncKeyState(int vk);
}
'@

$Exe = Resolve-AppExe $Exe
$results = New-ResultDir 'drag-out'
$report = New-Object System.Collections.Generic.List[string]
function Note([string]$Line) { $report.Add($Line); Write-Host $Line }
$failures = New-Object System.Collections.Generic.List[string]
function Expect([string]$Name, [int]$Passed, [int]$Total, [string]$Detail = '') {
    Note ("  {0}: {1}/{2}{3}" -f $Name, $Passed, $Total, $(if ($Detail) { " ($Detail)" } else { '' }))
    if ($Passed -ne $Total) { $failures.Add($Name) }
}
function Ms([long]$From, [long]$To, [double]$PerSecond) { return ($To - $From) * 1000.0 / $PerSecond }

# Test files: a PNG (an image record drags its original file) and two plain files.
$fileDir = (New-Item -ItemType Directory -Force -Path (Join-Path $results 'files')).FullName
$png = Join-Path $fileDir 'kp-drag.png'
$bitmap = New-Object System.Drawing.Bitmap 64, 40
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
$graphics.Clear([System.Drawing.Color]::FromArgb(255, 0, 200)); $graphics.Dispose()
$bitmap.Save($png, [System.Drawing.Imaging.ImageFormat]::Png); $bitmap.Dispose()
$fileA = Join-Path $fileDir 'kp-drag-a.txt'; Set-Content $fileA 'a' -Encoding ASCII
$fileB = Join-Path $fileDir 'kp-drag-b.txt'; Set-Content $fileB 'b' -Encoding ASCII
$stamp = Get-Date -Format 'HHmmss'

function Json($Value) { return ($Value | ConvertTo-Json -Compress) }
$cases = @(
    [pscustomobject]@{ Name = 'plain text'; Payload = (Json @{ plain = "kp-plain-$stamp" }); Check = { param($t) $t.Unicode -eq "kp-plain-$stamp" -and $t.Html -eq $null -and $t.Rtf -eq $null } },
    [pscustomobject]@{ Name = 'text + HTML'; Payload = (Json @{ plain = "kp-html-$stamp"; html = "<b>kp-html-$stamp</b>" }); Check = { param($t) $t.Unicode -eq "kp-html-$stamp" -and $t.Html -like "*<!--StartFragment--><b>kp-html-$stamp</b><!--EndFragment-->*" } },
    [pscustomobject]@{ Name = 'text + RTF'; Payload = (Json @{ plain = "kp-rtf-$stamp"; rtf = "{\rtf1\ansi {\b kp-rtf-$stamp}}" }); Check = { param($t) $t.Unicode -eq "kp-rtf-$stamp" -and $t.Rtf -eq "{\rtf1\ansi {\b kp-rtf-$stamp}}" } },
    [pscustomobject]@{ Name = 'PNG file'; Payload = (Json @{ files = @($png) }); Check = { param($t) $t.Files.Count -eq 1 -and $t.Files[0] -eq $png } },
    [pscustomobject]@{ Name = 'two files'; Payload = (Json @{ files = @($fileA, $fileB) }); Check = { param($t) $t.Files.Count -eq 2 -and $t.Files -contains $fileA -and $t.Files -contains $fileB } }
)

Assert-Desktop -IdleSeconds $IdleSeconds -NeedsInput
$userForeground = [Probe]::GetForegroundWindow()
$userCursor = New-Object Probe+POINT; [void][Probe]::GetCursorPos([ref]$userCursor)

$app = $null; $target = $null
try {
    $env:KWIKPASTE_DRAG_ALLOW = "$PID"
    $app = Start-ProbeApp $Exe $results
    $panel = $app.Hwnd
    Note ("probe app pid {0}, panel 0x{1:X}; drops allowed on pids {2},{0}" -f $app.Process.Id, $panel.ToInt64(), $PID)

    # The panel appears at the cursor. Show it once to learn where, hide it, put the target so it
    # overlaps the panel's right edge (a drag never crosses anything but the panel and the target),
    # make the target the foreground window, then show the panel at the same place again.
    $work = [Probe]::PrimaryWorkArea()
    function Show-Panel {
        [void][Probe]::MoveTo($work[0] + 120, $work[1] + 120)
        Send-ProbeCommand $Exe '--selftest-show'
        if ($null -eq (Wait-ProbeEvent 'shown' 3000)) { throw 'The panel did not show.' }
        [Probe]::Pump(300)
        return [Probe]::ClientRectOnScreen($panel)
    }
    $client = Show-Panel
    Send-ProbeCommand $Exe '--selftest-hide'
    [void](Wait-ProbeEvent 'hidden' 3000)

    $target = New-Object DropTarget
    $target.Bounds = New-Object System.Drawing.Rectangle(($client[2] - 20), $client[1], 480, 360)
    $target.Show()
    [Probe]::Pump(300)
    $drop = $target.PointToScreen((New-Object System.Drawing.Point(300, 180)))
    if (-not [Probe]::ClickIntoForegroundAt($target.Handle, $drop.X, $drop.Y)) { throw 'The drop target did not take the foreground.' }

    $again = Show-Panel
    if (-not [Probe]::Same($client, $again)) { throw "The panel moved: $([Probe]::Rect($client)) then $([Probe]::Rect($again))." }
    $source = New-Object Probe+POINT
    $source.X = [int](($client[0] + $client[2]) / 2); $source.Y = [int]($client[1] + ($client[3] - $client[1]) * 0.55)
    $selfDrop = New-Object Probe+POINT
    $selfDrop.X = $source.X - 60; $selfDrop.Y = $source.Y + 30
    Note "panel client $([Probe]::Rect($client)), drag source $($source.X),$($source.Y), drop point $($drop.X),$($drop.Y)"
    if ([Probe]::RootAt($source.X, $source.Y) -ne $panel) { throw 'The drag source point is not over the panel.' }
    if ([Probe]::GetForegroundWindow() -ne $target.Handle) { throw 'The drop target is not the foreground window.' }

    # Presses on the source, moves past the drag threshold and waits for DoDragDrop to start.
    function Begin-Drag {
        if ([Probe]::GetForegroundWindow() -ne $target.Handle) { throw "ABORT: foreground is 0x$('{0:X}' -f [Probe]::GetForegroundWindow().ToInt64()), not the drop target" }
        [void][Probe]::MoveTo($source.X, $source.Y)
        [Probe]::Pump(80)
        if ([Probe]::RootAt($source.X, $source.Y) -ne $panel) { throw 'ABORT: the drag source point is not over the panel' }
        Clear-ProbeEvents
        [DragInput]::LeftDown()
        [Probe]::Pump(80)
        for ($i = 1; $i -le 4; $i++) { [void][Probe]::MoveTo($source.X + 4 * $i, $source.Y); [Probe]::Pump(20) }
        $started = Wait-ProbeEvent 'drag_started' 3000
        if ($null -eq $started) { [DragInput]::LeftUp(); throw 'The drag did not start.' }
        return $started
    }

    # Moves the cursor in steps (the OLE loop tracks it) to (x, y).
    function Move-Drag([int]$X, [int]$Y, [int]$Steps = 12) {
        $from = New-Object Probe+POINT; [void][Probe]::GetCursorPos([ref]$from)
        for ($i = 1; $i -le $Steps; $i++) {
            [void][Probe]::MoveTo([int]($from.X + ($X - $from.X) * $i / $Steps), [int]($from.Y + ($Y - $from.Y) * $i / $Steps))
            [Probe]::Pump(25)
        }
        [Probe]::Pump(150)
    }

    Note '1. drops into the WinForms target'
    foreach ($case in $cases) {
        Send-ProbeCommand $Exe "--selftest-drag-payload=$($case.Payload)"
        if ($null -eq (Wait-ProbeEvent 'drag_payload' 3000)) { throw "The payload for $($case.Name) was not taken." }
        $count = @{ Dropped = 0; Content = 0; Foreground = 0; NoActivation = 0 }
        for ($round = 1; $round -le $Rounds; $round++) {
            $target.Reset()
            $started = Begin-Drag
            Move-Drag $drop.X $drop.Y
            if ([Probe]::RootAt($drop.X, $drop.Y) -ne $target.Handle) { throw 'ABORT: the drop point is not over the target' }
            [DragInput]::LeftUp()
            $finished = Wait-ProbeEvent 'drag_finished' 5000
            [Probe]::Pump(300)
            $label = "$($case.Name) #$round"
            if ($null -ne $finished -and $finished.result -eq 'Dropped' -and $finished.effect -eq 1 -and $target.Drops -eq 1) { $count.Dropped++ } else { Note "  ${label}: result $($finished.result) effect $($finished.effect), target drops $($target.Drops)" }
            if (& $case.Check $target) { $count.Content++ } else { Note "  ${label}: formats [$($target.Formats -join ', ')] unicode '$($target.Unicode)' files [$($target.Files -join ';')]" }
            if ([Probe]::GetForegroundWindow() -eq $target.Handle -and $null -ne $finished -and [long]$finished.foreground -eq $target.Handle.ToInt64()) { $count.Foreground++ }
            if ($null -ne $finished -and $finished.activations -eq $started.activations) { $count.NoActivation++ } else { Note "  ${label}: panel activations $($started.activations) -> $($finished.activations)" }
        }
        Note "  $($case.Name): formats offered [$($target.Formats -join ', ')]"
        Expect "$($case.Name): Dropped, effect COPY, one drop" $count.Dropped $Rounds
        Expect "$($case.Name): formats and content" $count.Content $Rounds
        Expect "$($case.Name): target stayed the foreground" $count.Foreground $Rounds
        Expect "$($case.Name): panel never activated" $count.NoActivation $Rounds
    }

    Note '2. self-drop onto the panel'
    foreach ($case in @($cases[1], $cases[3])) {
        Send-ProbeCommand $Exe "--selftest-drag-payload=$($case.Payload)"
        [void](Wait-ProbeEvent 'drag_payload' 3000)
        $count = @{ Refused = 0; Quiet = 0; NoGhost = 0 }
        for ($round = 1; $round -le $Rounds; $round++) {
            $target.Reset()
            [void](Begin-Drag)
            Move-Drag $selfDrop.X $selfDrop.Y 6
            if ([Probe]::RootAt($selfDrop.X, $selfDrop.Y) -ne $panel) { throw 'ABORT: the self-drop point is not over the panel' }
            [DragInput]::LeftUp()
            $finished = Wait-ProbeEvent 'drag_finished' 5000
            [Probe]::Pump(300)
            if ($null -ne $finished -and $finished.result -eq 'Refused' -and $finished.effect -eq 0) { $count.Refused++ } else { Note "  self-drop $($case.Name) #${round}: result $($finished.result) effect $($finished.effect)" }
            $leaked = @(Get-ProbeEvents 'file_drop') + @(Get-ProbeEvents 'click')
            if ($leaked.Count -eq 0 -and $target.Drops -eq 0) { $count.Quiet++ } else { Note "  self-drop $($case.Name) #${round}: $($leaked | ForEach-Object { $_.event + ' ' + $_.detail })" }
            # A right click on the source: its button-up must not complete the drag's left press.
            [void][Probe]::MoveTo($source.X, $source.Y)
            [Probe]::Pump(80)
            [DragInput]::RightClick()
            [Probe]::Pump(300)
            $rightUp = @(Get-ProbeEvents 'mouse_up' | Where-Object { $_.detail -eq 'right' })
            $ghost = @(Get-ProbeEvents 'click')
            if ($rightUp.Count -ge 1 -and $ghost.Count -eq 0) { $count.NoGhost++ } else { Note "  self-drop $($case.Name) #${round}: right ups $($rightUp.Count), clicks $($ghost.Count)" }
        }
        Expect "self-drop $($case.Name): refused (effect NONE)" $count.Refused $Rounds
        Expect "self-drop $($case.Name): no FileDrop, no click in GPUI, nothing at the target" $count.Quiet $Rounds
        Expect "self-drop $($case.Name): right click afterwards is no ghost left click" $count.NoGhost $Rounds
    }

    Note '3. Esc cancels'
    Send-ProbeCommand $Exe "--selftest-drag-payload=$($cases[0].Payload)"
    [void](Wait-ProbeEvent 'drag_payload' 3000)
    $latency = New-Object System.Collections.Generic.List[double]
    $count = @{ Cancelled = 0; Clean = 0; Visible = 0 }
    for ($round = 1; $round -le $EscRounds; $round++) {
        $target.Reset()
        [void](Begin-Drag)
        Move-Drag $drop.X $drop.Y
        [Probe]::Pump(200)
        $escAt = [DragInput]::Escape()
        $finished = Wait-ProbeEvent 'drag_finished' 3000
        [DragInput]::LeftUp()
        [Probe]::Pump(300)
        if ($null -ne $finished -and $finished.result -eq 'Cancelled' -and $null -ne $finished.cancel_requested) {
            $count.Cancelled++
            $latency.Add((Ms $escAt ([long]$finished.ticks) ([double]$finished.ticks_per_second)))
        } else { Note "  esc #${round}: result $($finished.result) cancel $($finished.cancel_requested)" }
        if ($target.Drops -eq 0 -and $target.EscapeKeys -eq 0) { $count.Clean++ } else { Note "  esc #${round}: target drops $($target.Drops), Esc keys $($target.EscapeKeys)" }
        if ([Probe]::IsWindowVisible($panel)) { $count.Visible++ }
    }
    Expect 'Esc: drag cancelled' $count.Cancelled $EscRounds
    Expect 'Esc: no drop and no Esc at the target' $count.Clean $EscRounds
    Expect 'Esc: panel stays visible' $count.Visible $EscRounds
    if ($latency.Count -gt 0) {
        $max = ($latency | Measure-Object -Maximum).Maximum
        Note ("  Esc -> DoDragDrop returned: p50 {0} ms, max {1} ms" -f (Format-Ms (Get-Percentile $latency 50)), (Format-Ms $max))
        Expect 'Esc: cancelled within 100 ms' ([int]($max -le 100)) 1
    }
    Note '  Word / WPS: not installed on this machine, not tested'
} catch {
    $failures.Add("ERROR: $($_.Exception.Message)")
    Note "ERROR: $($_.Exception.Message)"
} finally {
    if ([DragInput]::IsLeftDown()) { [DragInput]::LeftUp() }
    try { if ($null -ne $app -and [Probe]::IsWindowVisible($app.Hwnd)) { Send-ProbeCommand $Exe '--selftest-hide' } } catch {}
    Stop-ProbeApp $app $Exe
    if ($null -ne $target) { $target.Close(); [Probe]::Pump(200) }
    Remove-Item Env:KWIKPASTE_DRAG_ALLOW -ErrorAction SilentlyContinue
    [void][Probe]::SetCursorPos($userCursor.X, $userCursor.Y)
    if ($userForeground -ne [IntPtr]::Zero) { [void][Probe]::SetForegroundWindow($userForeground) }
    $report | Set-Content (Join-Path $results 'summary.txt') -Encoding UTF8
    Write-Host "results: $results"
}

if ($failures.Count -gt 0) { Write-Host "FAILED: $($failures -join '; ')"; exit 1 }
Write-Host 'PASSED'
