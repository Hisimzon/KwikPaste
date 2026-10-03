# Frames right after a show (patch 0001 wake race): WM_SHOWWINDOW wakes the parked vsync thread
# before the window counts as visible; the thread must keep ticking, so a view made dirty without
# input (data refresh, thumbnail) renders within a frame. No input is injected.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File tools\platform-probes\async-frame.ps1 [-Exe <KwikPaste.exe>] [-Rounds 5]
#
# Each round: hide, wait 1.5 s (the vsync thread parks), show, then mark the panel dirty 100 ms later
# and measure until the next render. Pass: every latency <= 50 ms (before the fix: up to ~1 s).
param(
    [string]$Exe = '',
    [int]$Rounds = 5
)

. "$PSScriptRoot\common.ps1"
$Exe = Resolve-AppExe $Exe
$results = New-ResultDir 'async-frame'
$app = $null; $latencies = New-Object System.Collections.Generic.List[double]
try {
    $app = Start-ProbeApp $Exe $results
    for ($round = 1; $round -le $Rounds; $round++) {
        Send-ProbeCommand $Exe '--selftest-hide'
        [Probe]::Pump(1500)
        Clear-ProbeEvents
        Send-ProbeCommand $Exe '--selftest-show'
        Send-ProbeCommand $Exe '--selftest-async-frame=100'
        $event = Wait-ProbeEvent 'async_frame' 3000
        $latency = if ($null -ne $event) { [double]$event.latency_ms } else { 9999 }
        $latencies.Add($latency)
        Write-Host ("round {0}: dirty -> rendered {1:N1} ms" -f $round, $latency)
    }
    Send-ProbeCommand $Exe '--selftest-hide'
} finally {
    Stop-ProbeApp $app $Exe
}
$max = ($latencies | Measure-Object -Maximum).Maximum
"max {0:N1} ms over {1} rounds" -f $max, $latencies.Count | Tee-Object -FilePath (Join-Path $results 'summary.txt')
if ($max -gt 50) { Write-Host 'FAILED'; exit 1 }
Write-Host 'PASSED'
