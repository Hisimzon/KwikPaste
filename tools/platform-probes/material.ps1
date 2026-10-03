# Window material (appendix C section 7): screenshot the panel over a striped window of this script for
# default, Mica and Acrylic. Only screenshots count (DwmGetWindowAttribute reads back values the
# screen does not show).
#
#   powershell -STA -NoProfile -ExecutionPolicy Bypass -File tools\platform-probes\material.ps1 [-Exe <KwikPaste.exe>]
#
# The probe view paints its root translucent under a material (bg_container at 58% / 34%, as 1.x).
# For each material the script samples an empty area of the panel: stripe residual (std of column
# luminance; the bare stripes are ~100, an unblurred transparent window would keep ~40% of that) and
# the mean colour (the red/green stripes tint a translucent panel, not an opaque one).
# Pass: default is opaque (the bg_container colour, no stripes); Mica and Acrylic differ from it (the
# DWM backdrop shows through the translucent root) and show no sharp stripes (residual < 3: a window that
# is transparent without a backdrop keeps ~40% of the stripes). Mica samples the wallpaper, not the
# windows behind, so only Acrylic can pick up the stripes' colour; when DWM has transparency effects off
# it draws its solid fallback fill instead of the blur (reported, not failed).
# Screenshots: material-<name>.png in the results folder.
#
# The system "Transparency effects" switch (HKCU ...\Themes\Personalize\EnableTransparency) must be on
# for any backdrop to show; with it off every material falls back to default (checked first). When it
# is off the script turns it on for the test (WM_SETTINGCHANGE "ImmersiveColorSet", as the Settings app
# does) and puts the original value back at the end.
param(
    [string]$Exe = '',
    [int]$IdleSeconds = 10
)

. "$PSScriptRoot\common.ps1"
Add-Type -ReferencedAssemblies System.Windows.Forms, System.Drawing @'
using System;
using System.Drawing;
using System.Drawing.Imaging;
using System.Windows.Forms;

/// Vertical 12 px red / green stripes behind the panel.
public class Stripes : Form {
    public Stripes() {
        Text = "kwikpaste-probe-stripes";
        StartPosition = FormStartPosition.Manual;
        FormBorderStyle = FormBorderStyle.None;
        ShowInTaskbar = false;
        DoubleBuffered = true;
        Paint += (s, e) => {
            for (int x = 0; x < ClientSize.Width; x += 24) {
                e.Graphics.FillRectangle(Brushes.Red, x, 0, 12, ClientSize.Height);
                e.Graphics.FillRectangle(Brushes.Lime, x + 12, 0, 12, ClientSize.Height);
            }
        };
    }
}

public static class Broadcast {
    [System.Runtime.InteropServices.DllImport("user32.dll", CharSet = System.Runtime.InteropServices.CharSet.Unicode)]
    static extern IntPtr SendMessageTimeoutW(IntPtr h, uint msg, IntPtr w, string l, uint flags, uint timeout, out IntPtr result);
    /// Tells every top-level window that the colour settings changed.
    public static void ColorSetChanged() {
        IntPtr result;
        SendMessageTimeoutW((IntPtr)0xFFFF, 0x001A, IntPtr.Zero, "ImmersiveColorSet", 2, 3000, out result);
    }
}

public static class Shot {
    /// Captures the screen rect, saves it, and measures the inner sample rect (screen coordinates):
    /// returns { mean R, mean G, mean B, std of column luminance }.
    public static double[] Measure(int x, int y, int w, int h, int sx, int sy, int sw, int sh, string path) {
        using (var bmp = new Bitmap(w, h, PixelFormat.Format32bppArgb)) {
            using (var g = Graphics.FromImage(bmp)) g.CopyFromScreen(x, y, 0, 0, new Size(w, h));
            bmp.Save(path, ImageFormat.Png);
            double r = 0, gr = 0, b = 0;
            var columns = new double[sw];
            for (int i = 0; i < sw; i++) {
                double sum = 0;
                for (int j = 0; j < sh; j++) {
                    Color c = bmp.GetPixel(sx - x + i, sy - y + j);
                    r += c.R; gr += c.G; b += c.B;
                    sum += 0.299 * c.R + 0.587 * c.G + 0.114 * c.B;
                }
                columns[i] = sum / sh;
            }
            int n = sw * sh;
            double mean = 0; foreach (var v in columns) mean += v; mean /= sw;
            double variance = 0; foreach (var v in columns) variance += (v - mean) * (v - mean); variance /= sw;
            return new double[] { r / n, gr / n, b / n, Math.Sqrt(variance) };
        }
    }
}
'@

$Exe = Resolve-AppExe $Exe
$results = New-ResultDir 'material'
$report = New-Object System.Collections.Generic.List[string]
function Note([string]$Line) { $report.Add($Line); Write-Host $Line }
$failures = New-Object System.Collections.Generic.List[string]
function Check([string]$Name, [bool]$Passed, [string]$Detail = '') {
    Note ("  {0}: {1}{2}" -f $Name, $(if ($Passed) { 'ok' } else { 'FAIL' }), $(if ($Detail) { " ($Detail)" } else { '' }))
    if (-not $Passed) { $failures.Add($Name) }
}

Assert-Desktop -IdleSeconds $IdleSeconds
$userCursor = New-Object Probe+POINT; [void][Probe]::GetCursorPos([ref]$userCursor)
$personalize = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize'
$transparencyBefore = (Get-ItemProperty $personalize -ErrorAction SilentlyContinue).EnableTransparency
$app = $null; $stripes = $null
try {
    $app = Start-ProbeApp $Exe $results
    $panel = $app.Hwnd
    $work = [Probe]::PrimaryWorkArea()
    [void][Probe]::MoveTo($work[0] + 200, $work[1] + 150)
    Send-ProbeCommand $Exe '--selftest-show'
    if ($null -eq (Wait-ProbeEvent 'shown' 3000)) { throw 'The panel did not show.' }
    [Probe]::Pump(300)
    $client = [Probe]::ClientRectOnScreen($panel)
    Send-ProbeCommand $Exe '--selftest-hide'
    [void](Wait-ProbeEvent 'hidden' 3000)

    $stripes = New-Object Stripes
    $stripes.Bounds = New-Object System.Drawing.Rectangle(($client[0] - 60), ($client[1] - 60), ($client[2] - $client[0] + 120), ($client[3] - $client[1] + 120))
    $stripes.Show()
    [Probe]::Pump(500)
    [void][Probe]::MoveTo($work[0] + 200, $work[1] + 150)
    Send-ProbeCommand $Exe '--selftest-show'
    [void](Wait-ProbeEvent 'shown' 3000)

    # An empty area of the probe view: the lower middle, above the status line.
    $width = $client[2] - $client[0]; $height = $client[3] - $client[1]
    $sx = $client[0] + [int]($width * 0.25); $sw = [int]($width * 0.5)
    $sy = $client[1] + [int]($height * 0.55); $sh = [int]($height * 0.2)
    $shotX = $client[0] - 30; $shotY = $client[1] - 30; $shotW = $width + 60; $shotH = $height + 60
    $bare = [Shot]::Measure($client[2] + 5, $client[1], 40, 200, $client[2] + 10, $client[1] + 10, 30, 150, (Join-Path $results 'stripes.png'))
    Note ("bare stripes: residual {0:N1}" -f $bare[3])

    if ($transparencyBefore -eq 0) {
        Note 'system transparency effects are off'
        Send-ProbeCommand $Exe '--selftest-settings={"appearance":{"material":"mica"}}'
        $event = Wait-ProbeEvent 'material' 3000
        Check 'transparency off: mica falls back to default' ($null -ne $event -and $event.effective -eq 'Default')
        Set-ItemProperty $personalize -Name EnableTransparency -Value 1 -Type DWord
        [Broadcast]::ColorSetChanged()
        Note '  turned transparency effects on for the test (restored at the end)'
        [Probe]::Pump(2000)
    }

    $measured = @{}
    foreach ($material in 'default', 'mica', 'acrylic') {
        Clear-ProbeEvents
        Send-ProbeCommand $Exe ('--selftest-settings={"appearance":{"material":"' + $material + '"}}')
        $event = Wait-ProbeEvent 'material' 3000
        [Probe]::Pump(1200)
        $stats = [Shot]::Measure($shotX, $shotY, $shotW, $shotH, $sx, $sy, $sw, $sh, (Join-Path $results "material-$material.png"))
        $tint = [math]::Abs($stats[0] - $stats[1])
        $measured[$material] = $stats
        Note ("{0,-8} effective {1,-8} mean RGB ({2:N0},{3:N0},{4:N0}), red-green tint {5:N1}, stripe residual {6:N2}" -f $material, $event.effective, $stats[0], $stats[1], $stats[2], $tint, $stats[3])
    }
    $default = $measured['default']
    Check 'default: opaque (no stripes, no tint)' ($default[3] -lt 1 -and [math]::Abs($default[0] - $default[1]) -lt 3)
    foreach ($material in 'mica', 'acrylic') {
        $stats = $measured[$material]
        $differs = [math]::Abs($stats[0] - $default[0]) + [math]::Abs($stats[1] - $default[1]) + [math]::Abs($stats[2] - $default[2])
        Check "${material}: backdrop under a translucent root, no sharp stripes" ($stats[3] -lt 3 -and $differs -gt 6) ("residual {0:N2} (bare {1:N1}), colour shift from default {2:N0}" -f $stats[3], $bare[3], $differs)
    }
    $acrylic = $measured['acrylic']
    $blurred = [math]::Abs($acrylic[2] - ($acrylic[0] + $acrylic[1]) / 2) -gt 8
    Note ("  acrylic picks up the stripes' colour (blur rendered by DWM): {0}" -f $(if ($blurred) { 'yes' } else { 'no: DWM draws its solid fallback (system transparency effects not active in DWM)' }))
} catch {
    $failures.Add("ERROR: $($_.Exception.Message)")
    Note "ERROR: $($_.Exception.Message)"
} finally {
    try { if ($null -ne $app -and -not $app.Process.HasExited) { Send-ProbeCommand $Exe '--selftest-settings={"appearance":{"material":"default"}}' } } catch {}
    Stop-ProbeApp $app $Exe
    if ($null -ne $stripes) { $stripes.Close() }
    if ($null -ne $transparencyBefore -and (Get-ItemProperty $personalize).EnableTransparency -ne $transparencyBefore) {
        Set-ItemProperty $personalize -Name EnableTransparency -Value $transparencyBefore -Type DWord
        [Broadcast]::ColorSetChanged()
    }
    Note "transparency effects: before $transparencyBefore, after $((Get-ItemProperty $personalize).EnableTransparency)"
    [void][Probe]::SetCursorPos($userCursor.X, $userCursor.Y)
    $report | Set-Content (Join-Path $results 'summary.txt') -Encoding UTF8
    Write-Host "results: $results"
}

if ($failures.Count -gt 0) { Write-Host "FAILED: $($failures -join '; ')"; exit 1 }
Write-Host 'PASSED'
