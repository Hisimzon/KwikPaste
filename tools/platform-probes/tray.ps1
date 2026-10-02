# Tray icon and menu, driven through tray-icon's own window messages instead of real clicks on the
# taskbar.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File tools\platform-probes\tray.ps1 [-Exe <KwikPaste.exe>]
#
# 1. A left button up on the icon (tray-icon's callback message 6002 with WM_LBUTTONUP) shows the panel.
# 2. A right button up opens the menu at the real cursor; its items are read through MN_GETHMENU and
#    must be the 1.x items from kwikpaste-core's tray strings in the default language zh-CN
#    (Preference, Exit). Choosing Preference shows the panel until the preference window exists.
# 3. Opening the menu again and choosing Exit makes the app exit with code 0.
# Opening a tray menu takes the foreground, like any tray menu; the script puts it back afterwards.
param(
    [string]$Exe = '',
    [int]$IdleSeconds = 30
)

. "$PSScriptRoot\common.ps1"
Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class Tray {
    public delegate bool EnumProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern IntPtr SendMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern int GetMenuItemCount(IntPtr menu);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetMenuStringW(IntPtr menu, uint item, StringBuilder s, int n, uint flags);
    public static IntPtr Find(uint pid, string cls, bool visibleOnly) {
        IntPtr found = IntPtr.Zero;
        EnumWindows((h, l) => {
            uint p; GetWindowThreadProcessId(h, out p);
            if (p != pid || (visibleOnly && !IsWindowVisible(h))) return true;
            var sb = new StringBuilder(256); GetClassNameW(h, sb, 256);
            if (sb.ToString() == cls) { found = h; return false; }
            return true;
        }, IntPtr.Zero);
        return found;
    }
    public static string Items(IntPtr menuWindow) {
        IntPtr menu = SendMessageW(menuWindow, 0x01E1, IntPtr.Zero, IntPtr.Zero);
        int count = GetMenuItemCount(menu);
        var parts = new string[Math.Max(0, count)];
        for (int i = 0; i < count; i++) { var sb = new StringBuilder(128); GetMenuStringW(menu, (uint)i, sb, 128, 0x400); parts[i] = sb.ToString(); }
        return String.Join(" / ", parts);
    }
}
"@

$Exe = Resolve-AppExe $Exe
$results = New-ResultDir 'tray'
$report = New-Object System.Collections.Generic.List[string]
function Note([string]$Line) { $report.Add($Line); Write-Host $Line }

function Open-Menu($TrayWindow, [uint32]$ProcessId) {
    [void][Tray]::PostMessageW($TrayWindow, 6002, [IntPtr]::Zero, [IntPtr]0x0205)
    $watch = [Diagnostics.Stopwatch]::StartNew()
    while ($watch.ElapsedMilliseconds -lt 3000) {
        $menu = [Tray]::Find($ProcessId, '#32768', $true)
        if ($menu -ne [IntPtr]::Zero) { return $menu }
        Start-Sleep -Milliseconds 50
    }
    throw 'The tray menu did not open.'
}

function Choose($Menu, [int]$Position) {
    for ($i = 0; $i -le $Position; $i++) { [void][Tray]::PostMessageW($Menu, 0x0100, [IntPtr]0x28, [IntPtr]::Zero); Start-Sleep -Milliseconds 80 }
    [void][Tray]::PostMessageW($Menu, 0x0100, [IntPtr]0x0D, [IntPtr]::Zero)
}

Assert-Desktop -IdleSeconds $IdleSeconds -NeedsInput
$userForeground = [Probe]::GetForegroundWindow()
$app = $null
$failures = 0
try {
    $app = Start-ProbeApp $Exe $results
    $processId = [uint32]$app.Process.Id
    $trayWindow = [Tray]::Find($processId, 'tray_icon_app', $false)
    if ($trayWindow -eq [IntPtr]::Zero) { throw 'No tray_icon_app window: the tray icon was not created.' }

    [void][Tray]::PostMessageW($trayWindow, 6002, [IntPtr]::Zero, [IntPtr]0x0202)
    $shown = Wait-ProbeEvent 'shown' 3000
    Note "left click -> shown by $($shown.source)"
    if ($null -eq $shown -or $shown.source -ne 'tray') { $failures++ }
    Send-ProbeCommand $Exe '--selftest-hide'
    [void](Wait-ProbeEvent 'hidden' 3000)

    $menu = Open-Menu $trayWindow $processId
    $items = [Tray]::Items($menu)
    Note "menu items: $items"
    # zh-CN "Preference" / "Exit app", as code points so the script stays ASCII.
    $expected = "$([char]0x504F)$([char]0x597D)$([char]0x8BBE)$([char]0x7F6E) / $([char]0x9000)$([char]0x51FA)$([char]0x5E94)$([char]0x7528)"
    if ($items -ne $expected) { $failures++ }
    Choose $menu 0
    $shown = Wait-ProbeEvent 'shown' 3000
    Note "menu 'preference' -> shown by $($shown.source)"
    if ($null -eq $shown -or $shown.source -ne 'tray') { $failures++ }
    Send-ProbeCommand $Exe '--selftest-hide'
    [void](Wait-ProbeEvent 'hidden' 3000)

    $menu = Open-Menu $trayWindow $processId
    Choose $menu 1
    $exited = $app.Process.WaitForExit(10000)
    Note "menu 'exit' -> exited $exited with code $($app.Process.ExitCode)"
    if (-not $exited -or $app.Process.ExitCode -ne 0) { $failures++ }
} catch {
    $failures++
    Note "ERROR: $($_.Exception.Message)"
} finally {
    Stop-ProbeApp $app $Exe
    if ($userForeground -ne [IntPtr]::Zero) { [void][Probe]::SetForegroundWindow($userForeground) }
    $report | Set-Content (Join-Path $results 'summary.txt') -Encoding UTF8
}

if ($failures -gt 0) { Write-Host "FAILED ($failures)"; exit 1 }
Write-Host 'PASSED'
