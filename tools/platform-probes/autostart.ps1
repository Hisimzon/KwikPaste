# Autostart and run-as-admin on the real machine, with the probe app's own names only.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File tools\platform-probes\autostart.ps1 [-Exe <KwikPaste.exe>]
#
# The probe app (identity ...selftest-platform) uses the Run value and admin task names derived from
# its identifier; the official HKCU Run value KwikPaste and the task KwikPasteAdmin belong to the
# installed app and must be untouched: the script reads them before and after and fails on any change.
# It never writes them. No input is injected; settings are changed through --selftest-settings.
#
# 1. general.autoStart on/off three times: the HKCU Run value "<exe>" --auto-launch (quoted when the path
#    has spaces) and the task manager's StartupApproved mark appear and disappear with the setting.
# 2. A restart with autoStart on rewrites the value at startup; turning it off removes it again.
# 3. general.runAsAdmin on/off (the probe process is elevated when UAC is off or the shell is elevated):
#    the task is created from the 1.x XML (HighestAvailable, --kwikpaste-admin-restarted, no trigger)
#    and removed again. Skipped when the process is not elevated.
# Any probe entry left behind is removed at the end.
param(
    [string]$Exe = ''
)

. "$PSScriptRoot\common.ps1"
$Exe = Resolve-AppExe $Exe
$results = New-ResultDir 'autostart'
$report = New-Object System.Collections.Generic.List[string]
function Note([string]$Line) { $report.Add($Line); Write-Host $Line }
$failures = New-Object System.Collections.Generic.List[string]
function Check([string]$Name, [bool]$Passed, [string]$Detail = '') {
    Note ("  {0}: {1}{2}" -f $Name, $(if ($Passed) { 'ok' } else { 'FAIL' }), $(if ($Detail) { " ($Detail)" } else { '' }))
    if (-not $Passed) { $failures.Add($Name) }
}

$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$approvedKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run'
$officialName = 'KwikPaste'
$officialTask = 'KwikPasteAdmin'
$probeName = 'com.fastthree.kwikpaste.native-dev.selftest-platform'
$probeTask = "$probeName.admin"
$quotedExe = if ($Exe -match '\s') { '"' + $Exe + '"' } else { $Exe }
$expected = "$quotedExe --auto-launch"

function Get-Value([string]$Key, [string]$Name) {
    $item = Get-ItemProperty $Key -ErrorAction SilentlyContinue
    if ($null -eq $item) { return $null }
    $property = $item.PSObject.Properties[$Name]
    if ($null -eq $property) { return $null }
    return $property.Value
}
function Get-Approved([string]$Name) {
    $value = Get-Value $approvedKey $Name
    if ($null -eq $value) { return '<none>' }
    return ($value | ForEach-Object { '{0:X2}' -f $_ }) -join ''
}
function Get-TaskXml([string]$Name) {
    # schtasks writes "not found" to stderr; with ErrorActionPreference Stop that would throw.
    $ErrorActionPreference = 'Continue'
    $xml = & schtasks /Query /TN $Name /XML 2>$null
    if ($LASTEXITCODE -ne 0) { return $null }
    return ($xml -join "`n")
}
function Get-OfficialState {
    $task = Get-TaskXml $officialTask
    return [pscustomobject]@{
        Run = Get-Value $runKey $officialName
        Approved = Get-Approved $officialName
        Task = if ($null -eq $task) { '<none>' } else { $task }
    }
}
function Set-Setting([string]$Json, [int]$WaitMs = 800) {
    Send-ProbeCommand $Exe "--selftest-settings=$Json"
    [Probe]::Pump($WaitMs)
}
function Remove-ProbeEntries {
    if ($null -ne (Get-Value $runKey $probeName)) { Remove-ItemProperty $runKey -Name $probeName }
    if ($null -ne (Get-Value $approvedKey $probeName)) { Remove-ItemProperty $approvedKey -Name $probeName }
    if ($null -ne (Get-TaskXml $probeTask)) { & schtasks /Delete /TN $probeTask /F | Out-Null }
}

$before = Get-OfficialState
Note "official before: Run '$($before.Run)', StartupApproved $($before.Approved), task $(if ($before.Task -eq '<none>') { 'absent' } else { 'present' })"
$elevated = (New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
Remove-ProbeEntries

$app = $null
$script:Run = 0
function Start-AutostartApp {
    $script:Run++
    $dir = (New-Item -ItemType Directory -Force -Path (Join-Path $results "run$($script:Run)")).FullName
    return Start-ProbeApp $Exe $dir -KeepState
}
try {
    $app = Start-AutostartApp
    Note "probe app pid $($app.Process.Id); entry '$probeName', task '$probeTask'; expected value '$expected'"
    Set-Setting '{"general":{"autoStart":false,"runAsAdmin":false}}'

    Note '1. autoStart on/off x3'
    $count = @{ On = 0; Off = 0 }
    for ($round = 1; $round -le 3; $round++) {
        Set-Setting '{"general":{"autoStart":true}}'
        $value = Get-Value $runKey $probeName
        $approved = Get-Approved $probeName
        if ($value -eq $expected -and $approved.StartsWith('02')) { $count.On++ } else { Note "  on #${round}: value '$value', StartupApproved $approved" }
        Set-Setting '{"general":{"autoStart":false}}'
        $value = Get-Value $runKey $probeName
        $approved = Get-Approved $probeName
        if ($null -eq $value -and $approved -eq '<none>') { $count.Off++ } else { Note "  off #${round}: value '$value', StartupApproved $approved" }
    }
    Check 'on: HKCU Run value and StartupApproved mark written' ($count.On -eq 3) "$($count.On)/3"
    Check 'off: both removed' ($count.Off -eq 3) "$($count.Off)/3"

    Note '2. startup sync'
    Set-Setting '{"general":{"autoStart":true}}'
    Stop-ProbeApp $app $Exe
    $app = $null
    Remove-ItemProperty $runKey -Name $probeName -ErrorAction SilentlyContinue
    $app = Start-AutostartApp
    [Probe]::Pump(800)
    Check 'a restart with autoStart on writes the value again' ((Get-Value $runKey $probeName) -eq $expected) "$(Get-Value $runKey $probeName)"
    Set-Setting '{"general":{"autoStart":false}}'
    Check 'turned off again' ($null -eq (Get-Value $runKey $probeName))

    Note '3. runAsAdmin'
    if (-not $elevated) {
        Note '  the probe process is not elevated: task creation needs administrator rights, not tested'
    } else {
        Set-Setting '{"general":{"runAsAdmin":true}}' 3000
        $xml = Get-TaskXml $probeTask
        $ok = $null -ne $xml -and $xml -match '<RunLevel>HighestAvailable</RunLevel>' -and $xml -match '<Arguments>--kwikpaste-admin-restarted</Arguments>' -and $xml -match [regex]::Escape("<Command>$Exe</Command>") -and $xml -notmatch '<Triggers>' -and $xml -match '<DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>'
        Check 'runAsAdmin on: task created from the 1.x XML' $ok
        if ($null -ne $xml) { $xml | Set-Content (Join-Path $results 'probe-task.xml') -Encoding UTF8 }
        Set-Setting '{"general":{"runAsAdmin":false}}' 3000
        Check 'runAsAdmin off: task removed' ($null -eq (Get-TaskXml $probeTask))
    }
} catch {
    $failures.Add("ERROR: $($_.Exception.Message)")
    Note "ERROR: $($_.Exception.Message)"
} finally {
    # The settings persist in the probe's data directory: leave both off for the other probes.
    try { if ($null -ne $app -and -not $app.Process.HasExited) { Set-Setting '{"general":{"autoStart":false,"runAsAdmin":false}}' 3000 } } catch {}
    Stop-ProbeApp $app $Exe
    Remove-ProbeEntries
    $after = Get-OfficialState
    Note "official after:  Run '$($after.Run)', StartupApproved $($after.Approved), task $(if ($after.Task -eq '<none>') { 'absent' } else { 'present' })"
    Check 'official Run value, StartupApproved mark and admin task unchanged' ($before.Run -eq $after.Run -and $before.Approved -eq $after.Approved -and $before.Task -eq $after.Task)
    Check 'no probe entries left' ($null -eq (Get-Value $runKey $probeName) -and (Get-Approved $probeName) -eq '<none>' -and $null -eq (Get-TaskXml $probeTask))
    $report | Set-Content (Join-Path $results 'summary.txt') -Encoding UTF8
    Write-Host "results: $results"
}

if ($failures.Count -gt 0) { Write-Host "FAILED: $($failures -join '; ')"; exit 1 }
Write-Host 'PASSED'
