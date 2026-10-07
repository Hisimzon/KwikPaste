# Steps around the real-clipboard probes, so test content never lands in the user's history.
#
#   powershell -STA -NoProfile -ExecutionPolicy Bypass -File tools\platform-probes\installed-app.ps1 -Action Prepare
#   ... run real-clipboard.ps1 / paste.ps1 ...
#   powershell -STA -NoProfile -ExecutionPolicy Bypass -File tools\platform-probes\installed-app.ps1 -Action Restore
#
# The installed app is the user's everyday KwikPaste (2.0 since the overwrite install; 1.x before). Both
# put the same tray-icon window and menu up, so the same steps quit either one.
#
# Prepare: waits for the user to be idle, saves the clipboard (every memory format, so images and files
#          survive too) and the autostart (HKCU Run) value,
#          then quits the installed KwikPaste through its own tray menu (Exit), like the user would; it
#          never kills the process.
# Restore: puts the clipboard back, deletes the backup, starts the installed app again with
#          --auto-launch (it starts silently in the tray), waits until it runs and checks the autostart
#          value is unchanged.
# Both refuse while %TEMP%\kwikpaste-installed-app.lock exists: the packaging session is then installing
# or checking the installed app.
param(
    [Parameter(Mandatory = $true)][ValidateSet('Prepare', 'Restore')][string]$Action,
    [string]$Installed = "$env:ProgramFiles\KwikPaste\KwikPaste.exe",
    [int]$IdleSeconds = 30
)

. "$PSScriptRoot\common.ps1"
Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class InstalledTray {
    public delegate bool EnumProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc cb, IntPtr l);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] static extern IntPtr SendMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] static extern int GetMenuItemCount(IntPtr menu);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetMenuStringW(IntPtr menu, uint item, StringBuilder s, int n, uint flags);
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
    public static string[] Items(IntPtr menuWindow) {
        IntPtr menu = SendMessageW(menuWindow, 0x01E1, IntPtr.Zero, IntPtr.Zero);
        int count = GetMenuItemCount(menu);
        var items = new string[Math.Max(0, count)];
        for (int i = 0; i < count; i++) { var sb = new StringBuilder(128); GetMenuStringW(menu, (uint)i, sb, 128, 0x400); items[i] = sb.ToString(); }
        return items;
    }
}
"@

$backup = Join-Path $env:TEMP 'kwikpaste-probe-clipboard-backup.json'
# Not restored: GDI-handle formats (bitmap, metafiles, palette) are not memory blocks and Windows
# synthesizes them from the restored DIB / text; the OLE markers point into the previous owner's
# process and would make the restored content look like a dead OLE data object.
$skippedFormats = @('#2', '#3', '#9', '#14', 'DataObject', 'Ole Private Data')
$runBackup = Join-Path $env:TEMP 'kwikpaste-probe-run-value.txt'
$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
# Exit menu item of the tray (kwikpaste-core's tray strings, same as 1.x) in zh-CN and en-US; zh-CN as
# code points so the script stays ASCII.
$exitLabels = @(([string][char]0x9000 + [char]0x51FA + [char]0x5E94 + [char]0x7528), 'Exit')
$lock = Join-Path $env:TEMP 'kwikpaste-installed-app.lock'
if (Test-Path $lock) { throw "$lock exists: the installed app is being installed or checked by another session; try again later." }

function Get-Installed {
    return @(Get-Process KwikPaste -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $Installed })
}

function Get-RunValue {
    $value = (Get-ItemProperty $runKey -ErrorAction SilentlyContinue).KwikPaste
    if ($null -eq $value) { return '<none>' }
    return $value
}

if ($Action -eq 'Prepare') {
    Assert-Desktop -IdleSeconds $IdleSeconds -NeedsInput
    $saved = [ordered]@{}
    foreach ($format in [Clip]::Formats()) {
        if ($skippedFormats -contains $format) { continue }
        $bytes = [Clip]::Get($format)
        if ($null -ne $bytes) { $saved[$format] = [Convert]::ToBase64String($bytes) }
    }
    [System.IO.File]::WriteAllText($backup, ($saved | ConvertTo-Json -Compress), [System.Text.Encoding]::UTF8)
    [System.IO.File]::WriteAllText($runBackup, (Get-RunValue), [System.Text.Encoding]::UTF8)
    Write-Host "clipboard saved ($($saved.Count) formats); autostart: $(Get-RunValue)"

    $running = Get-Installed
    if ($running.Count -eq 0) { Write-Host 'the installed app is not running'; exit 0 }
    # Open the handles now: ExitCode is only readable when it was opened before the exit.
    foreach ($candidate in $running) { $null = $candidate.Handle }
    $userForeground = [Probe]::GetForegroundWindow()
    # 2.0 runs as two processes; the tray belongs to the one with the UI.
    $process = $null; $tray = [IntPtr]::Zero
    foreach ($candidate in $running) {
        $tray = [InstalledTray]::Find([uint32]$candidate.Id, 'tray_icon_app', $false)
        if ($tray -ne [IntPtr]::Zero) { $process = $candidate; break }
    }
    if ($tray -eq [IntPtr]::Zero) { throw 'The installed app has no tray window.' }
    [void][InstalledTray]::PostMessageW($tray, 6002, [IntPtr]::Zero, [IntPtr]0x0205)
    $menu = [IntPtr]::Zero
    $watch = [Diagnostics.Stopwatch]::StartNew()
    while ($menu -eq [IntPtr]::Zero -and $watch.ElapsedMilliseconds -lt 3000) {
        Start-Sleep -Milliseconds 50
        $menu = [InstalledTray]::Find([uint32]$process.Id, '#32768', $true)
    }
    if ($menu -eq [IntPtr]::Zero) { throw 'The tray menu of the installed app did not open.' }
    $items = [InstalledTray]::Items($menu)
    $index = -1
    foreach ($label in $exitLabels) { if ($index -lt 0) { $index = [array]::IndexOf($items, $label) } }
    Write-Host "tray menu: $($items -join ' / ')"
    if ($index -lt 0) {
        [void][InstalledTray]::PostMessageW($menu, 0x0100, [IntPtr]0x1B, [IntPtr]::Zero)
        throw 'No exit item in the tray menu.'
    }
    for ($i = 0; $i -le $index; $i++) { [void][InstalledTray]::PostMessageW($menu, 0x0100, [IntPtr]0x28, [IntPtr]::Zero); Start-Sleep -Milliseconds 80 }
    [void][InstalledTray]::PostMessageW($menu, 0x0100, [IntPtr]0x0D, [IntPtr]::Zero)
    foreach ($candidate in $running) { if (-not $candidate.WaitForExit(10000)) { throw 'The installed app did not exit.' } }
    if ($userForeground -ne [IntPtr]::Zero) { [void][Probe]::SetForegroundWindow($userForeground) }
    Write-Host "the installed app exited through its tray menu (exit code $($process.ExitCode))"
    exit 0
}

# Restore
$formats = New-Object 'System.Collections.Generic.Dictionary[string,byte[]]'
if (Test-Path $backup) {
    $saved = [System.IO.File]::ReadAllText($backup, [System.Text.Encoding]::UTF8) | ConvertFrom-Json
    foreach ($property in $saved.PSObject.Properties) { $formats[$property.Name] = [Convert]::FromBase64String($property.Value) }
}
if ($formats.Count -gt 0) { [Clip]::Set($formats) } else { [Clip]::SetText('') }
if (Test-Path $backup) { Remove-Item $backup }
Write-Host "clipboard restored ($($formats.Count) formats)"
if ((Get-Installed).Count -eq 0) {
    Start-Process -FilePath $Installed -ArgumentList '--auto-launch'
    $watch = [Diagnostics.Stopwatch]::StartNew()
    while ((Get-Installed).Count -eq 0 -and $watch.ElapsedMilliseconds -lt 10000) { Start-Sleep -Milliseconds 200 }
    Start-Sleep -Seconds 2
}
$restarted = (Get-Installed).Count -gt 0
Write-Host "installed app running: $restarted"
$failed = -not $restarted
if (Test-Path $runBackup) {
    $before = [System.IO.File]::ReadAllText($runBackup, [System.Text.Encoding]::UTF8)
    Remove-Item $runBackup
    $after = Get-RunValue
    Write-Host "autostart before: $before"
    Write-Host "autostart after:  $after"
    if ($before -ne $after) { Write-Host 'AUTOSTART CHANGED'; $failed = $true }
}
if ($failed) { Write-Host 'FAILED'; exit 1 }
