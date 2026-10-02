# Clipboard watcher and write-back on the real system clipboard (the core report's checklist).
#
#   powershell -STA -NoProfile -ExecutionPolicy Bypass -File tools\platform-probes\installed-app.ps1 -Action Prepare
#   powershell -STA -NoProfile -ExecutionPolicy Bypass -File tools\platform-probes\real-clipboard.ps1 [-Exe <KwikPaste.exe>] [-SkipApps]
#   powershell -STA -NoProfile -ExecutionPolicy Bypass -File tools\platform-probes\installed-app.ps1 -Action Restore
#
# The probe app runs with --selftest-real-clipboard: the system clipboard and the watcher, in its own
# data directory. The installed app must not be running (installed-app.ps1 quits and restarts it), or the
# test content would land in the user's history. Every test value is generated here.
#
# Reading: plain text, HTML + text, RTF + text, files, a browser-style image (PNG + CF_DIB + HTML with
#   an <img>), a CF_DIB-only image; the source app (this script's form) and its icon; an excluded app is
#   not stored; the copy sound reaches the audio engine.
# Write-back: copying an item back adds no item; an image's hash survives the round trip; the formats
#   written (HTML + plain fallback, PNG + CF_DIB, CF_HDROP).
# Read retry: right after a copy another window takes the clipboard; held 60 ms it is read on a retry,
#   held 400 ms (longer than the 15 + 35 + 75 ms retries) the read gives up.
# Paste targets (unless -SkipApps): HTML into an HTML editor (MSHTML contenteditable, Word / WPS are not
# installed here), the plain fallback into Notepad (its session is saved before and put back after), the
# image into Paint, the files into an Explorer folder.
param(
    [string]$Exe = '',
    [int]$IdleSeconds = 30,
    [switch]$SkipApps
)

. "$PSScriptRoot\common.ps1"
Add-Type -ReferencedAssemblies System.Windows.Forms, System.Drawing @'
using System;
using System.Drawing;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;
using System.Text;
using System.Windows.Forms;
[StructLayout(LayoutKind.Sequential)] public struct WinRect { public int L, T, R, B; }
public static class Apps {
    public delegate bool EnumProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc cb, IntPtr l);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] static extern bool PrintWindow(IntPtr h, IntPtr dc, uint flags);
    [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h, out WinRect r);
    /// First visible top-level window of this class whose title contains `title` ("" for any).
    public static IntPtr Find(string cls, string title) {
        IntPtr found = IntPtr.Zero;
        EnumWindows((h, l) => {
            if (!IsWindowVisible(h)) return true;
            var c = new StringBuilder(256); GetClassNameW(h, c, 256);
            if (c.ToString() != cls) return true;
            var t = new StringBuilder(512); GetWindowTextW(h, t, 512);
            if (t.ToString().IndexOf(title, StringComparison.OrdinalIgnoreCase) < 0) return true;
            found = h; return false;
        }, IntPtr.Zero);
        return found;
    }
    /// Counts the pixels of a window (PrintWindow, full content) within `tolerance` of `color`.
    public static int CountColor(IntPtr h, Color color, int tolerance, string savePath) {
        WinRect r; GetWindowRect(h, out r);
        int w = r.R - r.L, hgt = r.B - r.T;
        using (var bitmap = new Bitmap(w, hgt, PixelFormat.Format32bppArgb)) {
            using (var g = Graphics.FromImage(bitmap)) {
                IntPtr dc = g.GetHdc();
                PrintWindow(h, dc, 2);
                g.ReleaseHdc(dc);
            }
            if (!String.IsNullOrEmpty(savePath)) bitmap.Save(savePath, ImageFormat.Png);
            int count = 0;
            for (int y = 0; y < hgt; y += 2) for (int x = 0; x < w; x += 2) {
                Color c = bitmap.GetPixel(x, y);
                if (Math.Abs(c.R - color.R) <= tolerance && Math.Abs(c.G - color.G) <= tolerance && Math.Abs(c.B - color.B) <= tolerance) count++;
            }
            return count;
        }
    }
}
public class HtmlEditor : Form {
    public WebBrowser Browser;
    public HtmlEditor() {
        Text = "kwikpaste-probe-html-editor";
        StartPosition = FormStartPosition.Manual;
        Browser = new WebBrowser();
        Browser.Dock = DockStyle.Fill;
        Browser.ScriptErrorsSuppressed = true;
        Browser.DocumentText = "<html><body contenteditable='true' style='font:16px sans-serif'></body></html>";
        Controls.Add(Browser);
    }
    public string Html() {
        if (Browser.Document == null || Browser.Document.Body == null) return "";
        return Browser.Document.Body.InnerHtml ?? "";
    }
    public void FocusBody() {
        if (Browser.Document != null && Browser.Document.Body != null) Browser.Document.Body.Focus();
    }
}
'@


$Exe = Resolve-AppExe $Exe
$results = New-ResultDir 'real-clipboard'
$report = New-Object System.Collections.Generic.List[string]
function Note([string]$Line) { $report.Add($Line); Write-Host $Line }
$failures = New-Object System.Collections.Generic.List[string]
function Check([string]$Name, [bool]$Passed, [string]$Detail = '') {
    Note ("  {0}: {1}{2}" -f $Name, $(if ($Passed) { 'ok' } else { 'FAIL' }), $(if ($Detail) { " ($Detail)" } else { '' }))
    if (-not $Passed) { $failures.Add($Name) }
}

$stamp = Get-Date -Format 'HHmmss'
$VK_CONTROL = [uint16]0x11
$VK_V = [uint16]0x56
$script:Run = 0

# Waits for the watcher to store (or match) an item whose content satisfies the filter.
function Wait-Captured([scriptblock]$Filter, [int]$TimeoutMs = 3000) {
    $watch = [Diagnostics.Stopwatch]::StartNew()
    while ($watch.ElapsedMilliseconds -lt $TimeoutMs) {
        foreach ($event in @(Get-ProbeEvents 'clipboard')) {
            if (& $Filter $event) { return $event }
            $script:OtherCaptures.Add($event)
        }
        [Probe]::Pump(30)
    }
    return $null
}
$script:OtherCaptures = New-Object System.Collections.Generic.List[object]

# Each launch logs into its own folder (an earlier launch's "ready" would answer the wait otherwise).
function Start-RealApp {
    $script:Run++
    $dir = (New-Item -ItemType Directory -Force -Path (Join-Path $results "run$($script:Run)")).FullName
    return Start-ProbeApp $Exe $dir -KeepState -Arguments @('--selftest-real-clipboard')
}

function Get-Count {
    Send-ProbeCommand $Exe '--selftest-count'
    $event = Wait-ProbeEvent 'count' 3000
    if ($null -eq $event) { return -1 }
    return [int]$event.total
}

function Copy-Item-Back([string]$Id) {
    Send-ProbeCommand $Exe "--selftest-copy-item=$Id"
    return Wait-ProbeEvent 'copied' 3000
}

function Set-AppSettings([string]$Json) {
    Send-ProbeCommand $Exe "--selftest-settings=$Json"
    [Probe]::Pump(300)
}

function Get-AppLog { return (Get-Content $app.Stderr -Encoding UTF8 -ErrorAction SilentlyContinue) -join "`n" }
function Count-Log([string]$Pattern) { return ([regex]::Matches((Get-AppLog), [regex]::Escape($Pattern))).Count }

# Real keystroke Ctrl+V into a window this script opened (it must be the foreground window).
function Paste-Into([IntPtr]$Window) {
    [Probe]::ChordFor($Window, [IntPtr]::Zero, [uint16[]]@($VK_CONTROL), $VK_V, 30)
}

$running = @(Get-Process KwikPaste -ErrorAction SilentlyContinue | Where-Object { $_.Path -and $_.Path -notlike '*\target\*' })
if ($running.Count -gt 0) { throw "The installed KwikPaste is running ($($running[0].Path)); quit it first with installed-app.ps1 -Action Prepare." }
Assert-Desktop -IdleSeconds $IdleSeconds -NeedsInput
$userForeground = [Probe]::GetForegroundWindow()
$userCursor = New-Object Probe+POINT; [void][Probe]::GetCursorPos([ref]$userCursor)

$app = $null; $form = $null; $editor = $null
$notepadSaved = $false
$opened = New-Object System.Collections.Generic.List[IntPtr]
$items = @{}
try {
    $app = Start-RealApp
    $form = New-TargetForm
    Note "probe app pid $($app.Process.Id); target form 0x$('{0:X}' -f $form.Handle.ToInt64()) is the foreground"
    Set-AppSettings '{"clipboard":{"feedback":{"copySound":false},"filters":{"excludedAppIds":[]}}}'

    Note '1. reading'
    $token = "kp-text-$stamp"
    [Clip]::SetText($token)
    $text = Wait-Captured { param($e) $e.item.content -eq $token }
    Check 'plain text stored' ($null -ne $text -and $text.item.kind -eq 'text' -and $null -eq $text.item.subKind)
    if ($null -ne $text) {
        $items.text = $text.item.id
        Note "  source app: $($text.item.sourceAppId); icon $($text.icon)"
        Check 'source app is this script''s process' ($text.item.sourceAppId -like '*\powershell.exe')
        Check 'source app icon extracted' ([bool]$text.icon_exists)
    }

    $htmlPlain = "kp-html-$stamp tail"
    $formats = New-Object 'System.Collections.Generic.Dictionary[string,byte[]]'
    $formats['HTML Format'] = [Clip]::Html("<b>kp-html-$stamp</b> tail")
    $formats['#13'] = [Clip]::Text($htmlPlain)
    [Clip]::Set($formats)
    $html = Wait-Captured { param($e) $e.item.content -like "*kp-html-$stamp*" }
    Check 'HTML + text stored as html with the plain text for search' ($null -ne $html -and $html.item.subKind -eq 'html' -and $html.item.content -like "*<b>kp-html-$stamp</b>*" -and $html.item.searchText -like "*$htmlPlain*") "$($html.item.subKind)"
    if ($null -ne $html) { $items.html = $html.item.id }

    $formats = New-Object 'System.Collections.Generic.Dictionary[string,byte[]]'
    $formats['Rich Text Format'] = [Clip]::Ascii("{\rtf1\ansi {\b kp-rtf-$stamp} tail}")
    $formats['#13'] = [Clip]::Text("kp-rtf-$stamp tail")
    [Clip]::Set($formats)
    $rtf = Wait-Captured { param($e) $e.item.content -like "*kp-rtf-$stamp*" }
    Check 'RTF + text stored as rtf' ($null -ne $rtf -and $rtf.item.subKind -eq 'rtf' -and $rtf.item.content -like '{\rtf1*') "$($rtf.item.subKind)"

    $fileDir = New-Item -ItemType Directory -Force -Path (Join-Path $results 'files')
    $fileA = Join-Path $fileDir "kp-file-$stamp-a.txt"; Set-Content $fileA 'a' -Encoding ASCII
    $fileB = Join-Path $fileDir "kp-file-$stamp-b.txt"; Set-Content $fileB 'b' -Encoding ASCII
    $formats = New-Object 'System.Collections.Generic.Dictionary[string,byte[]]'
    $formats['#15'] = [Clip]::Files(@($fileA, $fileB))
    [Clip]::Set($formats)
    $files = Wait-Captured { param($e) $e.item.kind -eq 'files' -and $e.item.content -like "*kp-file-$stamp-a*" }
    Check 'files stored' ($null -ne $files -and $files.item.content -like "*kp-file-$stamp-b*")
    if ($null -ne $files) { $items.files = $files.item.id }

    $magenta = [System.Drawing.Color]::FromArgb(255, 0, 200)
    $teal = [System.Drawing.Color]::FromArgb(0, 200, 120)
    $formats = New-Object 'System.Collections.Generic.Dictionary[string,byte[]]'
    $formats['PNG'] = [Clip]::Png(64, 40, $magenta, $teal)
    $formats['#8'] = [Clip]::Dib(64, 40, $magenta, $teal)
    $formats['HTML Format'] = [Clip]::Html('<img src="https://example.com/kp-probe.png">')
    [Clip]::Set($formats)
    $browser = Wait-Captured { param($e) $e.item.kind -eq 'image' -and $e.item.width -eq 64 -and $e.item.height -eq 40 }
    Check 'browser-style image (PNG + CF_DIB + HTML) stored as a 64x40 image' ($null -ne $browser)
    if ($null -ne $browser) { $items.browser = $browser.item.id }

    $red = [System.Drawing.Color]::FromArgb(230, 30, 30)
    $blue = [System.Drawing.Color]::FromArgb(30, 60, 230)
    $formats = New-Object 'System.Collections.Generic.Dictionary[string,byte[]]'
    $formats['#8'] = [Clip]::Dib(50, 30, $red, $blue)
    [Clip]::Set($formats)
    $dib = Wait-Captured { param($e) $e.item.kind -eq 'image' -and $e.item.width -eq 50 -and $e.item.height -eq 30 }
    Check 'CF_DIB-only image stored as a 50x30 image' ($null -ne $dib)
    if ($null -ne $dib) { $items.dib = $dib.item.id }

    Note '2. excluded app'
    if ($null -ne $text) {
        $excluded = ($text.item.sourceAppId | ConvertTo-Json)
        Set-AppSettings ('{"clipboard":{"filters":{"excludedAppIds":[' + $excluded + ']}}}')
        $token = "kp-excluded-$stamp"
        [Clip]::SetText($token)
        $leaked = Wait-Captured { param($e) $e.item.content -eq $token } 1500
        Check 'a copy from an excluded app is not stored' ($null -eq $leaked)
        Set-AppSettings '{"clipboard":{"filters":{"excludedAppIds":[]}}}'
    }

    Note '3. copy sound'
    $before = [Sessions]::StateFor([uint32]$app.Process.Id)
    Set-AppSettings '{"clipboard":{"feedback":{"copySound":true}}}'
    $token = "kp-sound-$stamp"
    [Clip]::SetText($token)
    $sound = Wait-Captured { param($e) $e.item.content -eq $token }
    [Probe]::Pump(400)
    $after = [Sessions]::StateFor([uint32]$app.Process.Id)
    Set-AppSettings '{"clipboard":{"feedback":{"copySound":false}}}'
    Check 'copy sound played (PlaySoundW started, an audio session appeared for the app)' ($null -ne $sound -and (Get-AppLog) -like '*copy sound started*' -and $after -ge 0) "session state before $before, after $after"

    Note '4. write-back'
    $count = Get-Count
    $script:OtherCaptures.Clear()
    $copied = Copy-Item-Back $items.text
    [Probe]::Pump(1000)
    [void](Wait-Captured { param($e) $false } 300)
    Check 'copying text back puts it on the clipboard' ($null -ne $copied -and [Clip]::GetText() -eq "kp-text-$stamp")
    Check 'copying back adds no item' ((Get-Count) -eq $count -and $script:OtherCaptures.Count -eq 0) "count $count -> $(Get-Count), other captures $($script:OtherCaptures.Count)"

    foreach ($kind in 'browser', 'dib') {
        if (-not $items.ContainsKey($kind)) { continue }
        [void](Copy-Item-Back $items[$kind])
        [Probe]::Pump(300)
        $formatsOnClipboard = [Clip]::Formats()
        Check "$kind image written as PNG + CF_DIB" (($formatsOnClipboard -contains 'PNG') -and ($formatsOnClipboard -contains '#8')) ($formatsOnClipboard -join ',')
        Send-ProbeCommand $Exe '--selftest-read-now'
        $read = Wait-ProbeEvent 'read_now' 3000
        Check "$kind image hash survives the round trip (read back as the same item)" ($null -ne $read -and $read.id -eq $items[$kind] -and $read.deduplicated) "read $($read.id) dedup $($read.deduplicated)"
    }

    if ($items.ContainsKey('html')) {
        [void](Copy-Item-Back $items.html)
        [Probe]::Pump(300)
        $formatsOnClipboard = [Clip]::Formats()
        Check 'HTML written with the plain text fallback' (($formatsOnClipboard -contains 'HTML Format') -and ($formatsOnClipboard -contains '#13') -and [Clip]::GetText() -eq $htmlPlain) ($formatsOnClipboard -join ',')
    }
    if ($items.ContainsKey('files')) {
        [void](Copy-Item-Back $items.files)
        [Probe]::Pump(300)
        $written = [Clip]::GetFiles()
        Check 'files written as CF_HDROP' ($null -ne $written -and $written -contains $fileA -and $written -contains $fileB) ($written -join ';')
    }

    Note '5. read retry'
    # Right after the new content is closed, another window of this script takes the clipboard and holds
    # it (a second clipboard manager reading a big image does the same). OpenClipboard(NULL) would not
    # keep the app out; a window does.
    [void]$form.Activate()
    $retries = Count-Log 'retrying in'
    $token = "kp-busy60-$stamp"
    $formats = New-Object 'System.Collections.Generic.Dictionary[string,byte[]]'
    $formats['#13'] = [Clip]::Text($token)
    [Clip]::SetContended($formats, 60)
    $captured = Wait-Captured { param($e) $e.item.content -eq $token } 1500
    Check 'clipboard busy for 60 ms: read on a retry' ($null -ne $captured -and (Count-Log 'retrying in') -gt $retries) "retry log lines +$((Count-Log 'retrying in') - $retries)"
    $gaveUp = Count-Log 'read failed:'
    $token = "kp-busy400-$stamp"
    $formats = New-Object 'System.Collections.Generic.Dictionary[string,byte[]]'
    $formats['#13'] = [Clip]::Text($token)
    [Clip]::SetContended($formats, 400)
    $captured = Wait-Captured { param($e) $e.item.content -eq $token } 1500
    Check 'clipboard busy for 400 ms: the read gives up after the bounded retries (15 + 35 + 75 ms)' ($null -eq $captured -and (Count-Log 'read failed:') -gt $gaveUp)
    Note "  log: $(((Get-AppLog) -split "`n" | Select-String 'retrying in|read failed:' | Select-Object -Last 4 | ForEach-Object { $_.Line.Trim() }) -join ' | ')"

    if (-not $SkipApps) {
        Note '6. paste into other apps'
        if ($items.ContainsKey('html')) {
            $editor = New-Object HtmlEditor
            $work = [Probe]::PrimaryWorkArea()
            $editor.Bounds = New-Object System.Drawing.Rectangle(($work[0] + 700), ($work[1] + 60), 600, 300)
            $editor.TopMost = $true
            $editor.Show()
            [Probe]::Pump(1500)
            $center = $editor.PointToScreen((New-Object System.Drawing.Point(300, 150)))
            $focused = [Probe]::ClickIntoForegroundAt($editor.Handle, $center.X, $center.Y)
            $editor.FocusBody(); [Probe]::Pump(200)
            [void](Copy-Item-Back $items.html)
            [Probe]::Pump(200)
            if ($focused) { Paste-Into $editor.Handle; [Probe]::Pump(800) }
            $inner = $editor.Html()
            Note "  HTML editor: $inner"
            Check 'HTML pasted into an HTML editor keeps the bold' ($focused -and $inner -match "<(b|strong)>kp-html-$stamp</(b|strong)>")
            $editor.Close(); $editor = $null
        }

        # Notepad: a file of our own. Its session (tabs it restores) is saved first and put back afterwards.
        if (Get-Process Notepad -ErrorAction SilentlyContinue) {
            Note '  Notepad is already running (the user''s windows): not tested'
        } elseif ($items.ContainsKey('html')) {
            Save-NotepadSession
            $notepadSaved = $true
            $noteFile = Join-Path $results "kp-notepad-$stamp.txt"
            [System.IO.File]::WriteAllText($noteFile, '')
            Start-Process notepad.exe -ArgumentList "`"$noteFile`""
            $notepad = [IntPtr]::Zero
            $watch = [Diagnostics.Stopwatch]::StartNew()
            while ($notepad -eq [IntPtr]::Zero -and $watch.ElapsedMilliseconds -lt 8000) { [Probe]::Pump(200); $notepad = [Apps]::Find('Notepad', "kp-notepad-$stamp") }
            if ($notepad -ne [IntPtr]::Zero) {
                $opened.Add($notepad)
                [Probe]::Pump(500)
                $rect = [Probe]::ClientRectOnScreen($notepad)
                $focused = [Probe]::ClickIntoForegroundAt($notepad, [int](($rect[0] + $rect[2]) / 2), [int](($rect[1] + $rect[3]) / 2))
                [void](Copy-Item-Back $items.html)
                [Probe]::Pump(200)
                # Only into our own tab: the title names the active tab.
                $mine = [Apps]::Find('Notepad', "kp-notepad-$stamp") -eq $notepad
                if ($focused -and $mine) { Paste-Into $notepad; [Probe]::Pump(800) }
                # Every open tab has a document; only the active one is on screen.
                $document = [System.Windows.Automation.AutomationElement]::FromHandle($notepad).FindAll([System.Windows.Automation.TreeScope]::Descendants,
                    (New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::ControlTypeProperty, [System.Windows.Automation.ControlType]::Document))) |
                    Where-Object { -not $_.Current.IsOffscreen } | Select-Object -First 1
                $noteText = if ($null -ne $document) { $document.GetCurrentPattern([System.Windows.Automation.TextPattern]::Pattern).DocumentRange.GetText(-1) } else { '<no document>' }
                Note "  Notepad: '$($noteText.Trim())' (foreground $focused, our tab active $mine)"
                Check 'HTML item pasted into Notepad as its plain text fallback' ($focused -and $mine -and $noteText.Trim() -eq $htmlPlain)
                Close-WithoutSaving $notepad
            } else {
                Check 'Notepad opened' $false
            }
        }

        # Paint: paste the CF_DIB image, find its colors in the window, close without saving.
        if ($items.ContainsKey('dib')) {
            Start-Process mspaint.exe
            $paint = [IntPtr]::Zero
            $watch = [Diagnostics.Stopwatch]::StartNew()
            while ($paint -eq [IntPtr]::Zero -and $watch.ElapsedMilliseconds -lt 10000) { [Probe]::Pump(200); $paint = [Apps]::Find('MSPaintApp', '') }
            if ($paint -ne [IntPtr]::Zero) {
                $opened.Add($paint)
                [Probe]::Pump(1500)
                $window = [Probe]::WindowRect($paint)
                # The title bar: a click there never draws on the canvas.
                $focused = [Probe]::ClickIntoForegroundAt($paint, [int](($window[0] + $window[2]) / 2), $window[1] + 12)
                [void](Copy-Item-Back $items.dib)
                [Probe]::Pump(200)
                $redBefore = [Apps]::CountColor($paint, $red, 12, '')
                $blueBefore = [Apps]::CountColor($paint, $blue, 12, '')
                if ($focused) { Paste-Into $paint; [Probe]::Pump(1500) }
                $redAfter = [Apps]::CountColor($paint, $red, 12, (Join-Path $results 'paint-after-paste.png'))
                $blueAfter = [Apps]::CountColor($paint, $blue, 12, '')
                Check 'CF_DIB image pasted into Paint (its two colors appear)' ($focused -and $redAfter -gt $redBefore + 20 -and $blueAfter -gt $blueBefore + 20) "red $redBefore -> $redAfter, blue $blueBefore -> $blueAfter (sampled every 2 px; screenshot paint-after-paste.png)"
                Close-WithoutSaving $paint
            } else {
                Check 'Paint opened' $false
            }
        }

        # Explorer: paste the files item into an empty folder window.
        if ($items.ContainsKey('files')) {
            $dropDir = New-Item -ItemType Directory -Force -Path (Join-Path $results "kp-drop-$stamp")
            Start-Process explorer.exe -ArgumentList "`"$dropDir`""
            $folder = [IntPtr]::Zero
            $watch = [Diagnostics.Stopwatch]::StartNew()
            while ($folder -eq [IntPtr]::Zero -and $watch.ElapsedMilliseconds -lt 8000) { [Probe]::Pump(200); $folder = [Apps]::Find('CabinetWClass', "kp-drop-$stamp") }
            if ($folder -ne [IntPtr]::Zero) {
                $opened.Add($folder)
                [Probe]::Pump(1000)
                $window = [Probe]::WindowRect($folder)
                $focused = [Probe]::ClickIntoForegroundAt($folder, [int](($window[0] + $window[2]) / 2), [int](($window[1] + $window[3]) / 2) + 60)
                [void](Copy-Item-Back $items.files)
                [Probe]::Pump(200)
                if ($focused) { Paste-Into $folder; [Probe]::Pump(2000) }
                $pasted = @(Get-ChildItem $dropDir | ForEach-Object { $_.Name })
                Check 'files pasted into an Explorer folder' ($focused -and $pasted -contains (Split-Path $fileA -Leaf) -and $pasted -contains (Split-Path $fileB -Leaf)) ($pasted -join ',')
                Close-WithoutSaving $folder
            } else {
                Check 'Explorer folder window opened' $false
            }
        }
        Note '  Word / WPS: not installed on this machine, not tested'
    }
} catch {
    $failures.Add("ERROR: $($_.Exception.Message)")
    Note "ERROR: $($_.Exception.Message)"
} finally {
    if ($null -ne $editor) { $editor.Close() }
    foreach ($window in $opened) {
        if ([Probe]::IsWindow($window)) { Close-WithoutSaving $window }
    }
    if ($notepadSaved) { Restore-NotepadSession; Note '  Notepad session restored from the backup taken before the test' }
    Stop-ProbeApp $app $Exe
    if ($null -ne $form) { $form.Close(); [Probe]::Pump(200) }
    [void][Probe]::SetCursorPos($userCursor.X, $userCursor.Y)
    if ($userForeground -ne [IntPtr]::Zero) { [void][Probe]::SetForegroundWindow($userForeground) }
    $report | Set-Content (Join-Path $results 'summary.txt') -Encoding UTF8
    Write-Host "results: $results"
}

if ($failures.Count -gt 0) { Write-Host "FAILED: $($failures -join '; ')"; exit 1 }
Write-Host 'PASSED'
