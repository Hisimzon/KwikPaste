# Shared functions for the platform probe scripts. Dot-source it: . "$PSScriptRoot\common.ps1"
#
# The app under test runs in selftest mode: KWIKPASTE_SELFTEST=1 plus --selftest-platform, which
# gives it the identifier com.fastthree.kwikpaste.native-dev.selftest-platform (never the installed 1.x
# one, nor other selftest runs such as --selftest-smoke that may start at the same time)
# and makes it append one JSON line per ready/shown/hidden/quit event to $env:KWIKPASTE_PROBE_LOG.
# Commands reach it through the real single-instance path: a second launch with --selftest-show,
# --selftest-hide, --selftest-toggle or --selftest-quit hands its arguments to the running app.

$ErrorActionPreference = 'Stop'

if (-not ('Probe' -as [type])) {
    Add-Type -ReferencedAssemblies System.Windows.Forms, System.Drawing -Path (Join-Path $PSScriptRoot 'Probe.cs'), (Join-Path $PSScriptRoot 'Ime.cs'), (Join-Path $PSScriptRoot 'Clip.cs')
}
[Probe]::Init()
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes

$script:RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path

function Resolve-AppExe([string]$Exe) {
    if ($Exe -eq '') { $Exe = Join-Path $script:RepoRoot 'target\release\KwikPaste.exe' }
    if (-not (Test-Path $Exe)) { throw "KwikPaste.exe not found at $Exe; build it with cargo build --release -p kwikpaste-app" }
    return (Resolve-Path $Exe).Path
}

function New-ResultDir([string]$Name) {
    $stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
    $dir = Join-Path $PSScriptRoot "results\$Name-$stamp"
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    return $dir
}

# Refuses to start when the desktop cannot be driven or the user is active.
function Assert-Desktop([int]$IdleSeconds, [switch]$NeedsInput) {
    if (Get-Process LogonUI -ErrorAction SilentlyContinue) { throw 'The screen is locked (LogonUI is running).' }
    if ($NeedsInput -and -not [Probe]::InputDesktopAvailable()) { throw 'This process is not on the input desktop.' }
    if (-not [Probe]::DevHotkeyFree()) { throw 'Ctrl+Alt+Shift+F9 is already registered by another program (a dev instance still running?).' }
    $deadline = (Get-Date).AddMinutes(5)
    while ([Probe]::IdleMs() -lt ($IdleSeconds * 1000)) {
        if ((Get-Date) -gt $deadline) { throw "The user has not been idle for $IdleSeconds s within 5 minutes." }
        Start-Sleep -Seconds 1
    }
}

# The probe instance's own data directory (never the installed app's).
$script:ProbeDataDir = Join-Path $env:LOCALAPPDATA 'com.fastthree.kwikpaste.native-dev.selftest-platform\dev'

# Starts the probe app. Its saved panel geometry is cleared first, so every script starts from the
# default size at the cursor; -KeepState keeps it (window-state.ps1 checks persistence across launches).
function Start-ProbeApp([string]$Exe, [string]$ResultDir, [switch]$KeepState, [string[]]$Arguments = @()) {
    if (-not $KeepState) {
        foreach ($name in 'window-state.gpui.json', 'window-state.json') {
            Remove-Item (Join-Path $script:ProbeDataDir "state\$name") -ErrorAction SilentlyContinue
        }
    }
    $log = Join-Path $ResultDir 'probe.jsonl'
    $stderr = Join-Path $ResultDir 'app-stderr.txt'
    [Probe]::ResetLog()
    $script:ProbeEvents.Clear()
    $env:KWIKPASTE_SELFTEST = '1'
    $env:KWIKPASTE_PROBE_LOG = $log
    $process = Start-Process -FilePath $Exe -ArgumentList (@('--selftest-platform') + $Arguments) -PassThru -RedirectStandardError $stderr -NoNewWindow
    $null = $process.Handle
    $script:ProbeLog = $log
    $script:SecondStderr = Join-Path $ResultDir 'second-launch-stderr.txt'
    $ready = Wait-ProbeEvent 'ready' 20000
    if ($null -eq $ready) { throw "The app did not report ready; see $stderr" }
    return [pscustomobject]@{ Process = $process; Hwnd = [IntPtr][long]$ready.hwnd; Ready = $ready; Log = $log; Stderr = $stderr }
}

# Hands a --selftest-* command to the running app through a second launch. Double quotes (JSON in
# --selftest-settings=) are escaped for the C runtime's command-line parser.
function Send-ProbeCommand([string]$Exe, [string]$Command) {
    $env:KWIKPASTE_SELFTEST = '1'
    $argument = $Command.Replace('"', '\"')
    $second = Start-Process -FilePath $Exe -ArgumentList $argument -PassThru -NoNewWindow -RedirectStandardError $script:SecondStderr
    $null = $second.Handle
    if (-not $second.WaitForExit(15000)) { $second.Kill(); throw "The second launch with $Command did not exit." }
    if ($second.ExitCode -ne 0) { throw "The second launch with $Command exited with $($second.ExitCode); see $script:SecondStderr" }
}

$script:ProbeEvents = New-Object System.Collections.ArrayList

# Reads newly appended probe events into the pending queue.
function Read-ProbeEvents {
    foreach ($line in [Probe]::ReadNewLines($script:ProbeLog)) {
        [void]$script:ProbeEvents.Add(($line | ConvertFrom-Json))
    }
}

# Takes the oldest pending probe event of the given kind, waiting for it (pumping WinForms messages)
# up to TimeoutMs. Events of other kinds stay queued.
function Wait-ProbeEvent([string]$Kind, [int]$TimeoutMs = 3000) {
    $watch = [Diagnostics.Stopwatch]::StartNew()
    while ($true) {
        Read-ProbeEvents
        for ($i = 0; $i -lt $script:ProbeEvents.Count; $i++) {
            $event = $script:ProbeEvents[$i]
            if ($event.event -eq $Kind) { $script:ProbeEvents.RemoveAt($i); return $event }
        }
        if ($watch.ElapsedMilliseconds -ge $TimeoutMs) { return $null }
        [Probe]::Pump(5)
    }
}

# Takes every pending probe event of the given kind (after a final read).
function Get-ProbeEvents([string]$Kind) {
    Read-ProbeEvents
    $taken = @($script:ProbeEvents | Where-Object { $_.event -eq $Kind })
    foreach ($event in $taken) { $script:ProbeEvents.Remove($event) }
    return $taken
}

# Drops every pending probe event.
function Clear-ProbeEvents {
    Read-ProbeEvents
    $script:ProbeEvents.Clear()
}

# Windows 11 Notepad restores its last session (tabs, unsaved text) from its LocalState folder, and a
# Notepad window the probe opens rewrites that session when it closes. A probe that opens Notepad saves
# the session first and puts it back after its own Notepad window has closed, so the user's session is
# exactly as before. It refuses when Notepad is already running: those windows are the user's.
$script:NotepadState = Join-Path $env:LOCALAPPDATA 'Packages\Microsoft.WindowsNotepad_8wekyb3d8bbwe\LocalState'
$script:NotepadBackup = Join-Path $env:TEMP 'kwikpaste-probe-notepad-session'

function Save-NotepadSession {
    if (Get-Process Notepad -ErrorAction SilentlyContinue) { throw 'Notepad is already running; close it first (the probe must not touch the user''s Notepad).' }
    if (Test-Path $script:NotepadBackup) { throw "An earlier Notepad session backup is still at $script:NotepadBackup; restore it with Restore-NotepadSession first." }
    New-Item -ItemType Directory -Force -Path $script:NotepadBackup | Out-Null
    foreach ($name in 'TabState', 'WindowState') {
        $source = Join-Path $script:NotepadState $name
        if (Test-Path $source) { Copy-Item $source (Join-Path $script:NotepadBackup $name) -Recurse }
    }
}

function Restore-NotepadSession {
    if (-not (Test-Path $script:NotepadBackup)) { return }
    $watch = [Diagnostics.Stopwatch]::StartNew()
    while ((Get-Process Notepad -ErrorAction SilentlyContinue) -and $watch.ElapsedMilliseconds -lt 15000) { Start-Sleep -Milliseconds 200 }
    if (Get-Process Notepad -ErrorAction SilentlyContinue) {
        Write-Warning "Notepad is still running; its session backup stays at $script:NotepadBackup"
        return
    }
    foreach ($name in 'TabState', 'WindowState') {
        $target = Join-Path $script:NotepadState $name
        if (Test-Path $target) { Remove-Item $target -Recurse -Force }
        $saved = Join-Path $script:NotepadBackup $name
        if (Test-Path $saved) { Copy-Item $saved $target -Recurse }
    }
    Remove-Item $script:NotepadBackup -Recurse -Force
}

# Closes a window the probe opened; when it asks to save (Paint, Notepad), answers "don't save" through
# UI Automation. Only for the probe's own documents.
function Close-WithoutSaving([IntPtr]$Window) {
    [void][Probe]::PostMessageW($Window, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)
    $dontSave = [string][char]0x4E0D + [char]0x4FDD + [char]0x5B58
    $buttons = New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::ControlTypeProperty, [System.Windows.Automation.ControlType]::Button)
    $watch = [Diagnostics.Stopwatch]::StartNew()
    while ([Probe]::IsWindow($Window) -and $watch.ElapsedMilliseconds -lt 6000) {
        [Probe]::Pump(400)
        if (-not [Probe]::IsWindow($Window)) { break }
        foreach ($button in [System.Windows.Automation.AutomationElement]::FromHandle($Window).FindAll([System.Windows.Automation.TreeScope]::Descendants, $buttons)) {
            $name = $button.Current.Name
            if ($name -eq $dontSave -or $name -match "^Don.?t save$") {
                $button.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern).Invoke()
                Write-Host "  answered '$name' in 0x$('{0:X}' -f $Window.ToInt64())"
                break
            }
        }
    }
}

# Opens our WinForms target and makes it the foreground window with its text box focused.
function New-TargetForm {
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
    if ([Probe]::GetForegroundWindow() -ne $form.Handle) { $form.Close(); throw 'The probe form could not take the foreground.' }
    [void]$form.Box.Focus()
    $form.Box.Text = 'probe'
    $form.Box.SelectionStart = $form.Box.Text.Length
    [void]$form.Drain()
    return $form
}

function Stop-ProbeApp($App, [string]$Exe) {
    if ($null -eq $App) { return }
    if (-not $App.Process.HasExited) {
        try { Send-ProbeCommand $Exe '--selftest-quit' } catch { Write-Warning $_ }
        if (-not $App.Process.WaitForExit(10000)) { $App.Process.Kill(); Write-Warning 'The app did not quit; killed it.' }
    }
}

function Get-Percentile([double[]]$Values, [double]$P) {
    if ($Values.Count -eq 0) { return [double]::NaN }
    $sorted = $Values | Sort-Object
    $rank = [math]::Ceiling($P / 100 * $sorted.Count) - 1
    return $sorted[[math]::Max(0, [math]::Min($sorted.Count - 1, $rank))]
}

function Format-Ms([double]$Value) { return '{0:N2}' -f $Value }
