# Idle wakeups with the panel hidden vs visible: checks that GPUI's VSyncProvider thread parks while
# no window is visible (vendored patch W0001). No keyboard or mouse input is injected; the panel is
# shown and hidden through the single-instance path and appears at the current cursor position.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File tools\platform-probes\vsync-idle.ps1 [-Exe <KwikPaste.exe>] [-Seconds 10] [-Reps 3]
#
# Phases, each measured Reps x Seconds: A hidden and never shown; B visible (static, nothing animates);
# C hidden again after B. Wakeups are context-switch deltas of the app's own threads, read from two
# WMI snapshots around each window (nothing samples inside the window); CPU is the process's
# TotalProcessorTime delta.
param(
    [string]$Exe = '',
    [int]$Seconds = 10,
    [int]$Reps = 3,
    [int]$IdleSeconds = 30
)

. "$PSScriptRoot\common.ps1"
$Exe = Resolve-AppExe $Exe
$results = New-ResultDir 'vsync-idle'
$report = New-Object System.Collections.Generic.List[string]
function Note([string]$Line) { $report.Add($Line); Write-Host $Line }

function Get-ThreadCounters([int]$ProcessId) {
    $map = @{}
    foreach ($thread in Get-CimInstance Win32_PerfRawData_PerfProc_Thread -Filter "IDProcess=$ProcessId") {
        $map[[uint32]$thread.IDThread] = [double]$thread.ContextSwitchesPersec
    }
    return $map
}

function Measure-Phase($App, [string]$Label) {
    $rows = @()
    for ($rep = 1; $rep -le $Reps; $rep++) {
        $App.Process.Refresh()
        $cpu0 = $App.Process.TotalProcessorTime
        $watch = [Diagnostics.Stopwatch]::StartNew()
        $before = Get-ThreadCounters $App.Process.Id
        $start = $watch.Elapsed.TotalSeconds
        Start-Sleep -Seconds $Seconds
        $end = $watch.Elapsed.TotalSeconds
        $after = Get-ThreadCounters $App.Process.Id
        $finish = $watch.Elapsed.TotalSeconds
        $App.Process.Refresh()
        $cpu = ($App.Process.TotalProcessorTime - $cpu0).TotalMilliseconds
        # Each snapshot is taken at the middle of its WMI call.
        $elapsed = (($end + $finish) / 2) - ($start / 2)

        $total = 0.0; $vsync = 0.0; $busiest = @()
        foreach ($tid in $after.Keys) {
            if (-not $before.ContainsKey($tid)) { continue }
            $rate = ($after[$tid] - $before[$tid]) / $elapsed
            $total += $rate
            $name = [Probe]::ThreadName($tid)
            if ($name -eq 'VSyncProvider') { $vsync += $rate }
            if ($rate -ge 0.5) { $busiest += ('{0}({1})={2:N1}' -f $(if ($name) { $name } else { 'tid' }), $tid, $rate) }
        }
        $row = [pscustomobject]@{ Phase = $Label; Rep = $rep; Seconds = [math]::Round($elapsed, 2); VSyncPerSec = [math]::Round($vsync, 1); ProcessPerSec = [math]::Round($total, 1); CpuPercentOfCore = [math]::Round($cpu / 10 / $elapsed, 3) }
        $rows += $row
        Note ("{0} rep {1}: {2:N1} s  VSyncProvider {3:N1}/s  process {4:N1}/s  cpu {5:N3}% of one core  threads waking: {6}" -f $Label, $rep, $elapsed, $vsync, $total, $row.CpuPercentOfCore, ($busiest -join ' '))
    }
    return $rows
}

function Median([double[]]$Values) { return Get-Percentile $Values 50 }

Assert-Desktop -IdleSeconds $IdleSeconds
$app = $null
$all = @()
try {
    $app = Start-ProbeApp $Exe $results
    Note ("app pid {0} panel hwnd 0x{1:X} exe {2}" -f $app.Process.Id, $app.Hwnd.ToInt64(), $Exe)
    Start-Sleep -Seconds 3
    $all += Measure-Phase $app 'A hidden, never shown'

    Send-ProbeCommand $Exe '--selftest-show'
    if ($null -eq (Wait-ProbeEvent 'shown' 5000)) { throw 'The panel did not show.' }
    Start-Sleep -Seconds 2
    $all += Measure-Phase $app 'B visible'

    Send-ProbeCommand $Exe '--selftest-hide'
    if ($null -eq (Wait-ProbeEvent 'hidden' 5000)) { throw 'The panel did not hide.' }
    Start-Sleep -Seconds 2
    $all += Measure-Phase $app 'C hidden after shown'
} finally {
    Stop-ProbeApp $app $Exe
    if ($null -ne $app) { Note "app exit code $($app.Process.ExitCode)" }
}

Note ''
Note 'median per phase:'
foreach ($group in $all | Group-Object Phase) {
    Note ('  {0,-22} VSyncProvider {1,7:N1}/s  process {2,7:N1}/s  cpu {3:N3}% of one core' -f $group.Name,
        (Median ($group.Group | ForEach-Object VSyncPerSec)), (Median ($group.Group | ForEach-Object ProcessPerSec)),
        (Median ($group.Group | ForEach-Object CpuPercentOfCore)))
}
$all | Export-Csv (Join-Path $results 'wakeups.csv') -NoTypeInformation -Encoding UTF8
$report | Set-Content (Join-Path $results 'summary.txt') -Encoding UTF8
