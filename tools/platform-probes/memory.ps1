# H1 memory: the release app with the real list and a real core in the probe's own development data
# directory, seeded with synthetic history (real-size and 4K images included).
#
#   powershell -STA -NoProfile -ExecutionPolicy Bypass -File tools\platform-probes\memory.ps1 -Exe target\x86_64-pc-windows-msvc\release\KwikPaste.exe [-Records 1000] [-Keep]
#
# 1. Seed: the probe data directory (...selftest-platform\dev) is wiped, the app starts with
#    --selftest-ui-panel and stores -Records records through core (build_item -> store_item), then quits.
# 2. Measure, in a fresh process (sampled every second: Working Set - Private = PrivateWorkingSetSize
#    and Private Bytes = PrivateUsage, from GetProcessMemoryInfo):
#    start hidden (no images loaded) -> first screen (panel shown) -> scroll through the whole list
#    with Down (one row per key, hook keys, nothing reaches other apps) -> hide -> 30 s hidden.
# Targets: start hidden <= 30 MB; peak <= 45 MB; 30 s after hiding <= 40 MB (ideal <= 30 MB), all on
# Working Set - Private. The samples go to samples.csv. -Keep keeps the seeded data directory.
param(
    [string]$Exe = '',
    [int]$Records = 1000,
    [int]$StartSeconds = 15,
    [int]$HiddenSeconds = 30,
    [int]$IdleSeconds = 30,
    [switch]$Keep
)

. "$PSScriptRoot\common.ps1"
Add-Type @'
using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Threading;

/// Samples a process's private working set and private bytes once a second on a thread-pool timer.
public class MemSampler {
    [StructLayout(LayoutKind.Sequential)]
    struct PMC_EX2 {
        public uint cb, PageFaultCount;
        public UIntPtr PeakWorkingSetSize, WorkingSetSize, QuotaPeakPagedPoolUsage, QuotaPagedPoolUsage;
        public UIntPtr QuotaPeakNonPagedPoolUsage, QuotaNonPagedPoolUsage, PagefileUsage, PeakPagefileUsage;
        public UIntPtr PrivateUsage, PrivateWorkingSetSize;
        public ulong SharedCommitUsage;
    }
    [DllImport("kernel32.dll")] static extern IntPtr OpenProcess(uint access, bool inherit, int pid);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr h);
    [DllImport("kernel32.dll", EntryPoint = "K32GetProcessMemoryInfo")] static extern bool GetProcessMemoryInfo(IntPtr h, out PMC_EX2 counters, uint cb);

    IntPtr process;
    Timer timer;
    readonly Stopwatch clock = Stopwatch.StartNew();
    public volatile string Phase = "start";
    public readonly List<double[]> Samples = new List<double[]>();
    public readonly List<string> Phases = new List<string>();

    public MemSampler(int pid) {
        process = OpenProcess(0x1000, false, pid);
        timer = new Timer(_ => Sample(), null, 0, 1000);
    }

    /// One reading in MB: private working set, private bytes, total working set.
    public double[] Read() {
        PMC_EX2 counters;
        if (!GetProcessMemoryInfo(process, out counters, (uint)Marshal.SizeOf(typeof(PMC_EX2)))) return null;
        const double mb = 1024.0 * 1024.0;
        return new double[] { (double)counters.PrivateWorkingSetSize.ToUInt64() / mb, (double)counters.PrivateUsage.ToUInt64() / mb, (double)counters.WorkingSetSize.ToUInt64() / mb };
    }

    void Sample() {
        var reading = Read();
        if (reading == null) return;
        lock (Samples) {
            Samples.Add(new double[] { clock.Elapsed.TotalSeconds, reading[0], reading[1], reading[2] });
            Phases.Add(Phase);
        }
    }

    public void Stop() {
        timer.Dispose();
        CloseHandle(process);
    }
}
'@

$Exe = Resolve-AppExe $Exe
$results = New-ResultDir 'memory'
$report = New-Object System.Collections.Generic.List[string]
function Note([string]$Line) { $report.Add($Line); Write-Host $Line }
$failures = New-Object System.Collections.Generic.List[string]
function Check([string]$Name, [bool]$Passed, [string]$Detail = '') {
    Note ("  {0}: {1}{2}" -f $Name, $(if ($Passed) { 'ok' } else { 'OVER' }), $(if ($Detail) { " ($Detail)" } else { '' }))
    if (-not $Passed) { $failures.Add($Name) }
}

$script:Run = 0
function Start-MemoryApp {
    $script:Run++
    $dir = (New-Item -ItemType Directory -Force -Path (Join-Path $results "run$($script:Run)")).FullName
    return Start-ProbeApp $Exe $dir -Arguments @('--selftest-ui-panel')
}

Assert-Desktop -IdleSeconds $IdleSeconds -NeedsInput
$userForeground = [Probe]::GetForegroundWindow()
$userCursor = New-Object Probe+POINT; [void][Probe]::GetCursorPos([ref]$userCursor)

$app = $null; $form = $null; $sampler = $null
try {
    Note "1. seed $Records records into $script:ProbeDataDir"
    if (Test-Path $script:ProbeDataDir) { Remove-Item $script:ProbeDataDir -Recurse -Force }
    $app = Start-MemoryApp
    Send-ProbeCommand $Exe "--selftest-seed=$Records"
    $seeded = Wait-ProbeEvent 'seeded' 600000
    if ($null -eq $seeded) { throw 'Seeding did not finish.' }
    $dbBytes = (Get-ChildItem (Join-Path $script:ProbeDataDir 'db') -Recurse -File | Measure-Object Length -Sum).Sum
    $imageBytes = (Get-ChildItem (Join-Path $script:ProbeDataDir 'resources') -Recurse -File -ErrorAction SilentlyContinue | Measure-Object Length -Sum).Sum
    Note ("  {0} records ({1} images, {2} of them 4K, {3:N1} MB of PNG) in {4:N1} s; db {5:N1} MB, resources {6:N1} MB" -f $seeded.records, $seeded.images, $seeded.four_k, ($seeded.image_bytes / 1MB), ($seeded.elapsed_ms / 1000), ($dbBytes / 1MB), ($imageBytes / 1MB))
    Stop-ProbeApp $app $Exe
    $app = $null

    Note '2. measure'
    $app = Start-MemoryApp
    $panel = $app.Hwnd
    $sampler = New-Object MemSampler $app.Process.Id
    $sampler.Phase = 'start-hidden'
    [Probe]::Pump($StartSeconds * 1000)

    $form = New-TargetForm
    $sampler.Phase = 'first-screen'
    Send-ProbeCommand $Exe '--selftest-show'
    if ($null -eq (Wait-ProbeEvent 'shown' 3000)) { throw 'The panel did not show.' }
    [Probe]::Pump(3000)

    $sampler.Phase = 'scroll'
    for ($row = 1; $row -lt $seeded.records; $row++) {
        [Probe]::TapFor($form.Handle, $panel, 0x28)
        if ($row % 50 -eq 0) { [Probe]::Pump(200) }
    }
    [Probe]::Pump(2000)

    $sampler.Phase = 'hidden'
    Send-ProbeCommand $Exe '--selftest-hide'
    [void](Wait-ProbeEvent 'hidden' 3000)
    [Probe]::Pump($HiddenSeconds * 1000 + 500)
    $sampler.Stop()

    $rows = New-Object System.Collections.Generic.List[string]
    $rows.Add('seconds,phase,private_ws_mb,private_bytes_mb,working_set_mb')
    $byPhase = @{}
    for ($i = 0; $i -lt $sampler.Samples.Count; $i++) {
        $sample = $sampler.Samples[$i]; $phase = $sampler.Phases[$i]
        $rows.Add(('{0:F0},{1},{2:F1},{3:F1},{4:F1}' -f $sample[0], $phase, $sample[1], $sample[2], $sample[3]))
        if (-not $byPhase.ContainsKey($phase)) { $byPhase[$phase] = New-Object System.Collections.Generic.List[object] }
        $byPhase[$phase].Add($sample)
    }
    $rows | Set-Content (Join-Path $results 'samples.csv') -Encoding ASCII

    function Max([object[]]$Samples, [int]$Index) { return ($Samples | ForEach-Object { $_[$Index] } | Measure-Object -Maximum).Maximum }
    function Last([object[]]$Samples, [int]$Index) { return $Samples[$Samples.Count - 1][$Index] }
    foreach ($phase in 'start-hidden', 'first-screen', 'scroll', 'hidden') {
        $samples = $byPhase[$phase]
        Note ("  {0,-13} {1,3} s: private WS max {2,6:N1} MB, last {3,6:N1} MB; private bytes max {4,6:N1} MB, last {5,6:N1} MB" -f $phase, $samples.Count, (Max $samples 1), (Last $samples 1), (Max $samples 2), (Last $samples 2))
    }
    $all = $sampler.Samples
    $startHidden = Last $byPhase['start-hidden'] 1
    $peak = Max $all 1
    $hiddenEnd = Last $byPhase['hidden'] 1
    Check 'start hidden, no images loaded <= 30 MB (private WS)' ($startHidden -le 30) ('{0:N1} MB' -f $startHidden)
    Check 'peak <= 45 MB (private WS)' ($peak -le 45) ('{0:N1} MB' -f $peak)
    Check "after $HiddenSeconds s hidden <= 40 MB (private WS)" ($hiddenEnd -le 40) ('{0:N1} MB; ideal <= 30 MB: {1}' -f $hiddenEnd, $(if ($hiddenEnd -le 30) { 'yes' } else { 'no' }))
    Note ("  private bytes: start hidden {0:N1} MB, peak {1:N1} MB, after hiding {2:N1} MB" -f (Last $byPhase['start-hidden'] 2), (Max $all 2), (Last $byPhase['hidden'] 2))
} catch {
    $failures.Add("ERROR: $($_.Exception.Message)")
    Note "ERROR: $($_.Exception.Message)"
} finally {
    if ($null -ne $sampler) { try { $sampler.Stop() } catch {} }
    try { if ($null -ne $app -and [Probe]::IsWindowVisible($app.Hwnd)) { Send-ProbeCommand $Exe '--selftest-hide' } } catch {}
    Stop-ProbeApp $app $Exe
    if ($null -ne $form) { $form.Close(); [Probe]::Pump(200) }
    if (-not $Keep -and (Test-Path $script:ProbeDataDir)) { Remove-Item $script:ProbeDataDir -Recurse -Force -ErrorAction SilentlyContinue }
    [void][Probe]::SetCursorPos($userCursor.X, $userCursor.Y)
    if ($userForeground -ne [IntPtr]::Zero) { [void][Probe]::SetForegroundWindow($userForeground) }
    $report | Set-Content (Join-Path $results 'summary.txt') -Encoding UTF8
    Write-Host "results: $results"
}

if ($failures.Count -gt 0) { Write-Host "OVER TARGET / FAILED: $($failures -join '; ')"; exit 1 }
Write-Host 'WITHIN TARGETS'
