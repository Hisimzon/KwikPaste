# Panic hook, crash restart and the vsync watchdog (appendix C section 8). The probe app runs with
# --selftest-crash-restart, so it follows the release restart policy, in its own data directory. No
# keyboard or mouse input is injected and the panel is never shown.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File tools\platform-probes\crash-restart.ps1 [-Exe <KwikPaste.exe>]
#
# 1. --selftest-panic=thread: a worker thread panics. The panic is logged (thread, location, payload),
#    last-crash.json records it, and the main thread restarts in order: the old process exits with 70,
#    a child with --relaunched-after-crash 1 becomes the only instance.
# 2. --selftest-panic=main: the main thread panics; the process dies (0xC0000409 when it happens inside
#    a window procedure). The hook already started relaunch 2, which waits for the single instance
#    mutex and takes over.
# 3. A third crash within 10 minutes restarts in degraded mode (GPUI_DISABLE_DIRECT_COMPOSITION=1,
#    reduce motion).
# 4. A fourth crash is not restarted; last-crash.json has gaveUp. The next manual launch logs it and
#    clears the mark.
# 5. Watchdog: --selftest-vsync-dead makes the vsync thread look dead; the next show request restarts
#    the app instead of showing the panel.
param(
    [string]$Exe = ''
)

. "$PSScriptRoot\common.ps1"
$Exe = Resolve-AppExe $Exe
$results = New-ResultDir 'crash-restart'
$report = New-Object System.Collections.Generic.List[string]
function Note([string]$Line) { $report.Add($Line); Write-Host $Line }
$failures = New-Object System.Collections.Generic.List[string]
function Check([string]$Name, [bool]$Passed, [string]$Detail = '') {
    Note ("  {0}: {1}{2}" -f $Name, $(if ($Passed) { 'ok' } else { 'FAIL' }), $(if ($Detail) { " ($Detail)" } else { '' }))
    if (-not $Passed) { $failures.Add($Name) }
}

$crashFile = Join-Path $script:ProbeDataDir 'state\last-crash.json'
$script:Run = 0

function Get-Instances { return @(Get-Process KwikPaste -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $Exe }) }

function Start-CrashApp {
    $script:Run++
    $dir = (New-Item -ItemType Directory -Force -Path (Join-Path $results "run$($script:Run)")).FullName
    $app = Start-ProbeApp $Exe $dir -Arguments @('--selftest-crash-restart')
    $health = Wait-ProbeEvent 'health' 3000
    return [pscustomobject]@{ Process = $app.Process; Stderr = $app.Stderr; Health = $health }
}

function Read-Crashes {
    if (-not (Test-Path $crashFile)) { return $null }
    return Get-Content $crashFile -Raw -Encoding UTF8 | ConvertFrom-Json
}

function Read-Stderr([string]$Path) { return (Get-Content $Path -Encoding UTF8 -ErrorAction SilentlyContinue) -join "`n" }

# Sends a crash command to the current instance and waits for it to exit and (unless -NoRestart) for
# the restarted child to report ready. Returns the child process (or $null).
function Invoke-Crash([System.Diagnostics.Process]$Current, [string]$Command, [string]$Label, [int]$Relaunch, [bool]$Degraded, [int[]]$ExitCodes, [switch]$NoRestart) {
    Note $Label
    Clear-ProbeEvents
    $watch = [Diagnostics.Stopwatch]::StartNew()
    Send-ProbeCommand $Exe $Command
    $exited = $Current.WaitForExit(20000)
    $exitMs = $watch.ElapsedMilliseconds
    $code = if ($exited) { $Current.ExitCode } else { 'still running' }
    Check 'the crashed process exited' ($exited -and $ExitCodes -contains $Current.ExitCode) ("exit code {0} (0x{1:X8}) after {2} ms" -f $code, $(if ($exited) { $Current.ExitCode } else { 0 }), $exitMs)
    if ($NoRestart) {
        $ready = Wait-ProbeEvent 'ready' 8000
        Check 'no restart' ($null -eq $ready -and (Get-Instances).Count -eq 0) "instances $((Get-Instances).Count)"
        return $null
    }
    $ready = Wait-ProbeEvent 'ready' 20000
    $readyMs = $watch.ElapsedMilliseconds
    if ($null -eq $ready) { Check 'restarted child ready' $false; return $null }
    $child = Get-Process -Id ([int]$ready.pid) -ErrorAction SilentlyContinue
    if ($null -ne $child) { $null = $child.Handle }
    $health = $script:ProbeEvents | Where-Object { $_.event -eq 'health' -and $_.pid -eq $ready.pid } | Select-Object -First 1
    if ($null -eq $health) { $health = Wait-ProbeEvent 'health' 1000 }
    Check "restarted as relaunch #$Relaunch$(if ($Degraded) { ' (degraded)' })" ($null -ne $health -and [int]$health.relaunch -eq $Relaunch -and [bool]$health.degraded -eq $Degraded -and [bool]$health.direct_composition_disabled -eq $Degraded) ("pid {0}, ready {1} ms after the command, relaunch {2}, degraded {3}, DirectComposition disabled {4}" -f $ready.pid, $readyMs, $health.relaunch, $health.degraded, $health.direct_composition_disabled)
    $instances = Get-Instances
    Check 'exactly one instance' ($instances.Count -eq 1 -and $instances[0].Id -eq [int]$ready.pid) "instances: $(($instances | ForEach-Object { $_.Id }) -join ',')"
    return $child
}

$current = $null
try {
    if ((Get-Instances).Count -gt 0) { throw "$Exe is already running; quit it first." }
    Remove-Item $crashFile -ErrorAction SilentlyContinue

    $app = Start-CrashApp
    $stderr = $app.Stderr
    $current = $app.Process
    Note "probe app pid $($current.Id), relaunch $($app.Health.relaunch)"

    $current = Invoke-Crash $current '--selftest-panic=thread' '1. worker thread panic' 1 $false @(70)
    $log = Read-Stderr $stderr
    Check 'panic logged with thread, location and payload' ($log -match "panic on thread 'selftest-panic' at [^\n]*instance\.rs:\d+:\d+: selftest panic on a worker thread")
    Check 'restart decision logged' ($log -match 'crash #1 within 10 minutes: restart' -and $log -match 'restarted as pid \d+ \(relaunch #1\)')
    $crashes = Read-Crashes
    Check 'last-crash.json records the crash' ($null -ne $crashes -and $crashes.crashes.Count -eq 1 -and $crashes.crashes[0].thread -eq 'selftest-panic' -and $crashes.crashes[0].action -eq 'restart' -and -not $crashes.gaveUp)

    if ($null -ne $current) {
        $current = Invoke-Crash $current '--selftest-panic=main' '2. main thread panic' 2 $false @(-1073740791, 101)
        Check 'main thread panic logged' ((Read-Stderr $stderr) -match "panic on thread 'main' at [^\n]*: selftest panic on the main thread")
    }
    if ($null -ne $current) {
        $current = Invoke-Crash $current '--selftest-panic=thread' '3. third crash: degraded restart' 3 $true @(70)
    }
    if ($null -ne $current) {
        [void](Invoke-Crash $current '--selftest-panic=main' '4. fourth crash within 10 minutes: no restart' 0 $false @(-1073740791, 101) -NoRestart)
        $current = $null
        $crashes = Read-Crashes
        $actions = ($crashes.crashes | ForEach-Object { $_.action }) -join ','
        Check 'last-crash.json: restart, restart, restart-degraded, give-up; gaveUp set' ($actions -eq 'restart,restart,restart-degraded,give-up' -and $crashes.gaveUp) $actions
        Note "  last-crash.json: $((Get-Content $crashFile -Raw) -replace '\s+', ' ')"
    }

    Note '5. manual launch after giving up'
    $app = Start-CrashApp
    $current = $app.Process
    Check 'starts normally (relaunch 0)' ([int]$app.Health.relaunch -eq 0)
    Check 'logs the earlier give-up and clears the mark' ((Read-Stderr $app.Stderr) -match 'was not restarted' -and -not (Read-Crashes).gaveUp)
    Stop-ProbeApp ([pscustomobject]@{ Process = $current }) $Exe
    $current = $null
    Remove-Item $crashFile -ErrorAction SilentlyContinue

    Note '6. watchdog: dead vsync thread'
    $app = Start-CrashApp
    $current = $app.Process
    Send-ProbeCommand $Exe '--selftest-vsync-dead'
    $current = Invoke-Crash $current '--selftest-show' '  show request with the vsync thread dead' 1 $false @(70)
    Check 'the panel was not shown' ($null -eq ($script:ProbeEvents | Where-Object { $_.event -eq 'shown' }))
    Check 'watchdog reason logged' ((Read-Stderr $app.Stderr) -match 'panel show: the vsync thread is not running')
} catch {
    $failures.Add("ERROR: $($_.Exception.Message)")
    Note "ERROR: $($_.Exception.Message)"
} finally {
    foreach ($process in Get-Instances) {
        try { Send-ProbeCommand $Exe '--selftest-quit' } catch {}
        if (-not $process.WaitForExit(10000)) { $process.Kill(); Write-Warning "killed $($process.Id)" }
    }
    if (Test-Path $crashFile) { Copy-Item $crashFile (Join-Path $results 'last-crash.json'); Remove-Item $crashFile }
    $report | Set-Content (Join-Path $results 'summary.txt') -Encoding UTF8
    Write-Host "results: $results"
}

if ($failures.Count -gt 0) { Write-Host "FAILED: $($failures -join '; ')"; exit 1 }
Write-Host 'PASSED'
