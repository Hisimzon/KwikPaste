# Panic hook, native crash handler, crash restart and the vsync watchdog (appendix C section 8). The
# probe app runs with --selftest-crash-restart, so it follows the release restart policy, in its own
# data directory. No keyboard or mouse input is injected and the panel is never shown. Works with a
# panic = "unwind" and a panic = "abort" build (pass -Abort for the latter: every panic then ends the
# process with 0xC0000409).
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File tools\platform-probes\crash-restart.ps1 [-Exe <KwikPaste.exe>] [-Abort]
#
# 1. --selftest-panic=thread: a worker thread panics. The panic is logged (thread, phase, location,
#    payload), last-crash.json records it, and a child with --relaunched-after-crash 1 becomes the only
#    instance (unwind: orderly restart from the main thread, exit code 70; abort: started from the hook).
# 2. --selftest-panic=main: the main thread panics; the hook starts relaunch 2, which takes over the
#    single instance mutex once the crashed process is gone.
# 3. --selftest-panic=native: an unhandled access violation; the unhandled-exception filter logs it and
#    starts relaunch 3, in degraded mode (GPUI_DISABLE_DIRECT_COMPOSITION=1, reduce motion).
# 4. A fourth crash within 10 minutes is not restarted; last-crash.json has gaveUp.
# 5. The next manual launch logs the give-up and clears the mark. It is then killed; the launch after it
#    records the unclean exit (no restart, but it counts).
# 6. Watchdog: --selftest-vsync-dead makes the vsync thread look dead; the next show request restarts
#    the app instead of showing the panel.
# 7. Poison input at startup (KWIKPASTE_SELFTEST_PANIC_AT_STARTUP=1, inherited by every child): crash 1
#    restarts normally, crash 2 is the second startup crash in a row and restarts degraded, crash 3
#    restarts degraded, crash 4 gives up; every record says phase startup.
param(
    [string]$Exe = '',
    [switch]$Abort
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

$FASTFAIL = -1073740791      # 0xC0000409
$ACCESS_VIOLATION = -1073741819   # 0xC0000005
$threadExit = if ($Abort) { @($FASTFAIL) } else { @(70) }
$mainExit = if ($Abort) { @($FASTFAIL) } else { @($FASTFAIL, 101) }
$crashFile = Join-Path $script:ProbeDataDir 'state\last-crash.json'
$runningFile = Join-Path $script:ProbeDataDir 'state\running.json'
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

function Wait-NoInstances([int]$TimeoutMs) {
    $watch = [Diagnostics.Stopwatch]::StartNew()
    while ((Get-Instances).Count -gt 0 -and $watch.ElapsedMilliseconds -lt $TimeoutMs) { [Probe]::Pump(200) }
    return (Get-Instances).Count -eq 0
}

# Sends a crash command to the current instance and waits for it to exit and (unless -NoRestart) for
# the restarted child to report ready. Returns the child process (or $null).
function Invoke-Crash([System.Diagnostics.Process]$Current, [string]$Command, [string]$Label, [int]$Relaunch, [bool]$Degraded, [int[]]$ExitCodes, [switch]$NoRestart) {
    Note $Label
    Clear-ProbeEvents
    $watch = [Diagnostics.Stopwatch]::StartNew()
    Send-ProbeCommand $Exe $Command
    $exited = $Current.WaitForExit(20000)
    $exitMs = $watch.ElapsedMilliseconds
    $code = if ($exited) { $Current.ExitCode } else { 0 }
    Check 'the crashed process exited' ($exited -and $ExitCodes -contains $code) ("exit code {0} (0x{1:X8}) after {2} ms" -f $code, $code, $exitMs)
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
    Remove-Item $crashFile, $runningFile -ErrorAction SilentlyContinue
    Remove-Item Env:KWIKPASTE_SELFTEST_PANIC_AT_STARTUP -ErrorAction SilentlyContinue

    $app = Start-CrashApp
    $stderr = $app.Stderr
    $current = $app.Process
    Note "probe app pid $($current.Id), relaunch $($app.Health.relaunch), build: $(if ($Abort) { 'panic = abort' } else { 'panic = unwind' })"

    $current = Invoke-Crash $current '--selftest-panic=thread' '1. worker thread panic' 1 $false $threadExit
    $log = Read-Stderr $stderr
    Check 'panic logged with thread, phase, location and payload' ($log -match "panic on thread 'selftest-panic' in phase idle at [^\n]*instance\.rs:\d+:\d+: selftest panic on a worker thread")
    Check 'restart decision logged' ($log -match 'crash #1 within 10 minutes in phase idle: restart' -and $log -match 'restarted as pid \d+ \(relaunch #1\)')
    $crashes = Read-Crashes
    Check 'last-crash.json records the crash with its phase' ($null -ne $crashes -and $crashes.crashes.Count -eq 1 -and $crashes.crashes[0].thread -eq 'selftest-panic' -and $crashes.crashes[0].phase -eq 'idle' -and $crashes.crashes[0].action -eq 'restart')

    if ($null -ne $current) {
        $current = Invoke-Crash $current '--selftest-panic=main' '2. main thread panic' 2 $false $mainExit
        Check 'main thread panic logged' ((Read-Stderr $stderr) -match "panic on thread 'main' in phase idle at [^\n]*: selftest panic on the main thread")
    }
    if ($null -ne $current) {
        $current = Invoke-Crash $current '--selftest-panic=native' '3. native crash (access violation): degraded restart' 3 $true @($ACCESS_VIOLATION)
        Check 'native crash logged' ((Read-Stderr $stderr) -match 'native exception 0xC0000005 at 0x[0-9A-F]+ on thread ''main'' in phase idle')
    }
    if ($null -ne $current) {
        [void](Invoke-Crash $current '--selftest-panic=main' '4. fourth crash within 10 minutes: no restart' 0 $false $mainExit -NoRestart)
        $current = $null
        $crashes = Read-Crashes
        $actions = ($crashes.crashes | ForEach-Object { $_.action }) -join ','
        Check 'last-crash.json: restart, restart, restart-degraded, give-up; gaveUp set' ($actions -eq 'restart,restart,restart-degraded,give-up' -and $crashes.gaveUp) $actions
    }

    Note '5. manual launch after giving up, then a kill'
    $app = Start-CrashApp
    $current = $app.Process
    Check 'starts normally (relaunch 0)' ([int]$app.Health.relaunch -eq 0)
    Check 'logs the earlier give-up and clears the mark' ((Read-Stderr $app.Stderr) -match 'was not restarted' -and -not (Read-Crashes).gaveUp)
    $killedPid = $current.Id
    Stop-Process -Id $killedPid -Force
    [void]$current.WaitForExit(5000)
    $current = $null
    $app = Start-CrashApp
    $current = $app.Process
    $last = (Read-Crashes).crashes | Select-Object -Last 1
    Check 'the kill is recorded at the next start as an unclean exit' ($null -ne $last -and [int]$last.pid -eq $killedPid -and $last.action -eq 'unclean-exit' -and (Read-Stderr $app.Stderr) -match 'ended without shutting down') "$($last.pid) $($last.action)"
    Stop-ProbeApp ([pscustomobject]@{ Process = $current }) $Exe
    $current = $null
    Check 'a clean quit removes the running marker' (-not (Test-Path $runningFile))
    Remove-Item $crashFile -ErrorAction SilentlyContinue

    Note '6. watchdog: dead vsync thread'
    $app = Start-CrashApp
    $current = $app.Process
    Send-ProbeCommand $Exe '--selftest-vsync-dead'
    $current = Invoke-Crash $current '--selftest-show' '  show request with the vsync thread dead' 1 $false @(70)
    Check 'the panel was not shown' ($null -eq ($script:ProbeEvents | Where-Object { $_.event -eq 'shown' }))
    Check 'watchdog reason logged' ((Read-Stderr $app.Stderr) -match 'panel show: the vsync thread is not running')
    if ($null -ne $current) { Stop-ProbeApp ([pscustomobject]@{ Process = $current }) $Exe; $current = $null }
    Remove-Item $crashFile -ErrorAction SilentlyContinue

    Note '7. poison input at startup'
    $dir = (New-Item -ItemType Directory -Force -Path (Join-Path $results 'poison')).FullName
    $env:KWIKPASTE_SELFTEST = '1'
    $env:KWIKPASTE_SELFTEST_PANIC_AT_STARTUP = '1'
    $env:KWIKPASTE_PROBE_LOG = Join-Path $dir 'probe.jsonl'
    $first = Start-Process -FilePath $Exe -ArgumentList '--selftest-platform', '--selftest-crash-restart' -PassThru -NoNewWindow -RedirectStandardError (Join-Path $dir 'app-stderr.txt')
    $settled = Wait-NoInstances 60000
    Remove-Item Env:KWIKPASTE_SELFTEST_PANIC_AT_STARTUP
    $crashes = Read-Crashes
    $actions = ($crashes.crashes | ForEach-Object { $_.action }) -join ','
    $phases = ($crashes.crashes | ForEach-Object { $_.phase }) -join ','
    Check 'every start crashed until the restarts ran out' $settled "instances left $((Get-Instances).Count)"
    Check 'restart, restart-degraded, restart-degraded, give-up' ($actions -eq 'restart,restart-degraded,restart-degraded,give-up') $actions
    Check 'all recorded in phase startup' ($phases -eq 'startup,startup,startup,startup') $phases
    Note "  last-crash.json: $((Get-Content $crashFile -Raw) -replace '\s+', ' ')"
} catch {
    $failures.Add("ERROR: $($_.Exception.Message)")
    Note "ERROR: $($_.Exception.Message)"
} finally {
    Remove-Item Env:KWIKPASTE_SELFTEST_PANIC_AT_STARTUP -ErrorAction SilentlyContinue
    foreach ($process in Get-Instances) {
        try { Send-ProbeCommand $Exe '--selftest-quit' } catch {}
        if (-not $process.WaitForExit(10000)) { $process.Kill(); Write-Warning "killed $($process.Id)" }
    }
    if (Test-Path $crashFile) { Copy-Item $crashFile (Join-Path $results 'last-crash.json'); Remove-Item $crashFile }
    Remove-Item $runningFile -ErrorAction SilentlyContinue
    $report | Set-Content (Join-Path $results 'summary.txt') -Encoding UTF8
    Write-Host "results: $results"
}

if ($failures.Count -gt 0) { Write-Host "FAILED: $($failures -join '; ')"; exit 1 }
Write-Host 'PASSED'
