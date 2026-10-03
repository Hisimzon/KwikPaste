# GPU device loss (patch 0003): simulated losses go through GPUI's real recovery path (new DXGI
# factory and device, DirectWrite GPU state, every window renderer). No input is injected.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File tools\platform-probes\device-loss.ps1 [-Exe <KwikPaste.exe>]
#
# With the panel visible:
# 1. a loss that recovers on the first attempt: recovered, and the panel draws again;
# 2. a loss whose first 3 device recreations fail: recovered on the 4th scheduled attempt (500 ms);
# 3. a loss that never recovers: after the schedule (8 s) the status reads failing, no panic, and the
#    watchdog restarts the app in an orderly way (exit code 70, relaunch #1).
param(
    [string]$Exe = ''
)

. "$PSScriptRoot\common.ps1"
$Exe = Resolve-AppExe $Exe
$results = New-ResultDir 'device-loss'
$report = New-Object System.Collections.Generic.List[string]
function Note([string]$Line) { $report.Add($Line); Write-Host $Line }
$failures = New-Object System.Collections.Generic.List[string]
function Check([string]$Name, [bool]$Passed, [string]$Detail = '') {
    Note ("  {0}: {1}{2}" -f $Name, $(if ($Passed) { 'ok' } else { 'FAIL' }), $(if ($Detail) { " ($Detail)" } else { '' }))
    if (-not $Passed) { $failures.Add($Name) }
}
$crashFile = Join-Path $script:ProbeDataDir 'state\last-crash.json'

$app = $null
try {
    Remove-Item $crashFile -ErrorAction SilentlyContinue
    $app = Start-ProbeApp $Exe $results -Arguments @('--selftest-crash-restart')
    Send-ProbeCommand $Exe '--selftest-show'
    if ($null -eq (Wait-ProbeEvent 'shown' 3000)) { throw 'The panel did not show.' }
    [Probe]::Pump(500)

    foreach ($case in @(@{ Fail = 0; Label = '1. recovers on the first attempt'; MinMs = 0 }, @{ Fail = 3; Label = '2. first 3 recreations fail'; MinMs = 450 })) {
        Note $case.Label
        Send-ProbeCommand $Exe "--selftest-device-lost=$($case.Fail)"
        $event = Wait-ProbeEvent 'device_recovered' 15000
        Check 'recovered without a panic, panel drew again' ($null -ne $event -and [int]$event.frames_after -ge 1 -and [int]$event.recovery_ms -ge $case.MinMs -and -not $app.Process.HasExited) $(if ($null -ne $event) { "recovery $($event.recovery_ms) ms, losses $($event.losses), recoveries $($event.recoveries), frames after $($event.frames_after)" } else { 'no event' })
    }

    Note '3. never recovers'
    Clear-ProbeEvents
    $watch = [Diagnostics.Stopwatch]::StartNew()
    Send-ProbeCommand $Exe '--selftest-device-lost=1000'
    $failing = Wait-ProbeEvent 'device_failing' 20000
    Check 'reported failing after the schedule, process still alive' ($null -ne $failing -and -not $app.Process.HasExited) "$($watch.ElapsedMilliseconds) ms"
    $exited = $app.Process.WaitForExit(15000)
    $ready = Wait-ProbeEvent 'ready' 15000
    $health = if ($null -ne $ready) { Wait-ProbeEvent 'health' 3000 } else { $null }
    Check 'watchdog restarted the app in order' ($exited -and $app.Process.ExitCode -eq 70 -and $null -ne $health -and [int]$health.relaunch -eq 1) ("exit {0}, relaunch {1}, {2} ms after the loss" -f $(if ($exited) { $app.Process.ExitCode } else { 'running' }), $health.relaunch, $watch.ElapsedMilliseconds)
    $log = (Get-Content $app.Stderr -Encoding UTF8 -ErrorAction SilentlyContinue) -join "`n"
    Check 'logged the attempts and the reason' ($log -match 'device-loss recovery attempt 8 failed' -and $log -match 'could not be recovered')
} catch {
    $failures.Add("ERROR: $($_.Exception.Message)")
    Note "ERROR: $($_.Exception.Message)"
} finally {
    foreach ($process in @(Get-Process KwikPaste -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $Exe })) {
        try { Send-ProbeCommand $Exe '--selftest-quit' } catch {}
        if (-not $process.WaitForExit(10000)) { $process.Kill() }
    }
    if (Test-Path $crashFile) { Copy-Item $crashFile (Join-Path $results 'last-crash.json'); Remove-Item $crashFile }
    $report | Set-Content (Join-Path $results 'summary.txt') -Encoding UTF8
    Write-Host "results: $results"
}

if ($failures.Count -gt 0) { Write-Host "FAILED: $($failures -join '; ')"; exit 1 }
Write-Host 'PASSED'
