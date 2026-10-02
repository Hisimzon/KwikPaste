# Update handoff host steps without installing anything.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File tools\platform-probes\handoff.ps1 [-Exe <KwikPaste.exe>]
#
# --selftest-handoff=3 makes the probe app run the HandoffHost steps the updater uses (stop input,
# remove the tray icon, release the single instance) through the same main-thread channel, wait 2 s
# and exit with code 3. Nothing is downloaded, installed or started. Checks before and after:
#   - the development panel hotkey (Ctrl+Alt+Shift+F9) is held, then free;
#   - the tray icon's tray_icon_app window exists, then is gone;
#   - the single-instance mutex and message window exist, then are gone (a new process could start);
#   - the app exits with code 3.
param(
    [string]$Exe = ''
)

. "$PSScriptRoot\common.ps1"
Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class Handoff {
    public delegate bool EnumProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc cb, IntPtr l);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern IntPtr FindWindowW(string cls, string title);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)] static extern IntPtr OpenMutexW(uint access, bool inherit, string name);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr h);
    public static bool HasWindowOfClass(uint pid, string cls) {
        bool found = false;
        EnumWindows((h, l) => {
            uint p; GetWindowThreadProcessId(h, out p);
            if (p != pid) return true;
            var sb = new StringBuilder(256); GetClassNameW(h, sb, 256);
            if (sb.ToString() == cls) { found = true; return false; }
            return true;
        }, IntPtr.Zero);
        return found;
    }
    public static bool MessageWindow(string identifier) {
        return FindWindowW(identifier + "-sic", identifier + "-siw") != IntPtr.Zero;
    }
    public static bool Mutex(string identifier) {
        IntPtr h = OpenMutexW(0x00100000, false, identifier + "-sim");
        if (h == IntPtr.Zero) return false;
        CloseHandle(h);
        return true;
    }
}
"@

$identifier = 'com.fastthree.kwikpaste.native-dev.selftest-platform'
$Exe = Resolve-AppExe $Exe
$results = New-ResultDir 'handoff'
$report = New-Object System.Collections.Generic.List[string]
function Note([string]$Line) { $report.Add($Line); Write-Host $Line }
$failures = New-Object System.Collections.Generic.List[string]
function Check([string]$Name, [bool]$Passed) {
    Note ("  {0}: {1}" -f $Name, $(if ($Passed) { 'ok' } else { 'FAIL' }))
    if (-not $Passed) { $failures.Add($Name) }
}

function Get-State($ProcessId) {
    return [pscustomobject]@{
        HotkeyHeld = -not [Probe]::DevHotkeyFree()
        Tray = [Handoff]::HasWindowOfClass($ProcessId, 'tray_icon_app')
        MessageWindow = [Handoff]::MessageWindow($identifier)
        Mutex = [Handoff]::Mutex($identifier)
    }
}

Assert-Desktop -IdleSeconds 0
$app = $null
try {
    $app = Start-ProbeApp $Exe $results
    $processId = [uint32]$app.Process.Id
    $before = Get-State $processId
    Note "before: $before"
    Check 'before: hotkey held' $before.HotkeyHeld
    Check 'before: tray icon window exists' $before.Tray
    Check 'before: single-instance message window exists' $before.MessageWindow
    Check 'before: single-instance mutex exists' $before.Mutex

    Send-ProbeCommand $Exe '--selftest-handoff=3'
    $handoff = Wait-ProbeEvent 'handoff' 5000
    Check 'host steps finished' ($null -ne $handoff)
    [Probe]::Pump(300)
    $after = Get-State $processId
    Note "after the host steps: $after (process alive: $(-not $app.Process.HasExited))"
    Check 'after: hotkey released' (-not $after.HotkeyHeld)
    Check 'after: tray icon removed' (-not $after.Tray)
    Check 'after: single-instance message window destroyed' (-not $after.MessageWindow)
    Check 'after: single-instance mutex released' (-not $after.Mutex)
    Check 'process still running during the pause' (-not $app.Process.HasExited)

    $exited = $app.Process.WaitForExit(10000)
    Check 'process exited' $exited
    if ($exited) { Check "exit code 3 (got $($app.Process.ExitCode))" ($app.Process.ExitCode -eq 3) }
} catch {
    $failures.Add("ERROR: $($_.Exception.Message)")
    Note "ERROR: $($_.Exception.Message)"
} finally {
    if ($null -ne $app -and -not $app.Process.HasExited) { Stop-ProbeApp $app $Exe }
    $report | Set-Content (Join-Path $results 'summary.txt') -Encoding UTF8
    Write-Host "results: $results"
}

if ($failures.Count -gt 0) { Write-Host "FAILED: $($failures -join '; ')"; exit 1 }
Write-Host 'PASSED'
