param(
    [Parameter(Mandatory)][string]$Exe,
    [Parameter(Mandatory)][string]$Out,
    [string]$Only = '*',
    [int]$SettleMs = 5000
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
Add-Type -ReferencedAssemblies System.Drawing @'
using System;
using System.Collections.Generic;
using System.Drawing;
using System.Runtime.InteropServices;
public static class ThemeCapture {
    public delegate bool EnumProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc f, IntPtr l);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h, ref POINT p);
    [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr v);
    [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr v);
    [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr dc, uint flags);
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int w, int height, uint flags);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L,T,R,B; }
    [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X,Y; }
    public static List<IntPtr> Windows(uint pid) {
        var result = new List<IntPtr>();
        EnumWindows((h,l) => { uint p; GetWindowThreadProcessId(h,out p); RECT r;
            if (p == pid && IsWindowVisible(h) && GetClientRect(h,out r) && r.R > 100 && r.B > 100) result.Add(h);
            return true; }, IntPtr.Zero);
        result.Sort((a,b) => { RECT ra,rb; GetClientRect(a,out ra); GetClientRect(b,out rb);
            return (rb.R*rb.B).CompareTo(ra.R*ra.B); });
        return result;
    }
    public static bool HasContent(Bitmap bitmap) {
        var colors = new HashSet<int>();
        for (int y=20;y<bitmap.Height-20;y+=11) for(int x=20;x<bitmap.Width-20;x+=11)
            colors.Add(bitmap.GetPixel(x,y).ToArgb());
        return colors.Count > 32;
    }
}
'@
[ThemeCapture]::SetProcessDpiAwarenessContext([IntPtr](-4)) | Out-Null
[ThemeCapture]::SetThreadDpiAwarenessContext([IntPtr](-4)) | Out-Null
$Exe = (Resolve-Path -LiteralPath $Exe).Path
$Out = [IO.Path]::GetFullPath($Out)
New-Item -ItemType Directory -Force $Out | Out-Null
$states = @()
foreach ($demo in 'idle','bottom','toasts','confirm') {
    $states += @{ Name="gallery-$demo"; Flag='gallery'; Vars=@{ KP_GALLERY_DEMO=$demo } }
}
foreach ($demo in 'idle','hover','hints','selection','search-empty','search-focus','search-typed','group','group-empty','note','delete','shortcuts','menu','menu-group','group-menu','preview-words','preview-text','preview-image','preview-files','preview-html','group-new','group-edit','group-manage') {
    $states += @{ Name="panel-$demo"; Flag='list-demo'; Vars=@{ KP_PANEL_DEMO=$demo } }
}
foreach ($tab in 'general','shortcuts','appearance','capture','window','paste','items','sync','overview','data','about') {
    $states += @{ Name="preferences-$tab"; Flag='preferences'; Vars=@{ KP_PREFERENCES_TAB=$tab } }
}
foreach ($demo in 'rule','export','apps') {
    $states += @{ Name="preferences-dialog-$demo"; Flag='preferences'; Vars=@{ KP_PREFERENCES_TAB='data'; KP_PREFERENCES_DEMO=$demo } }
}
foreach ($step in 0..4) {
    $states += @{ Name="onboarding-$step"; Flag='onboarding'; Vars=@{ KP_ONBOARDING_STEP="$step" } }
}
$records = [Collections.Generic.List[object]]::new()
$environment = @{}
$names = 'KWIKPASTE_SELFTEST','KP_THEME_GATE','KP_GALLERY_THEME','KP_GALLERY_LANG','KP_TEXT_SCALE','KP_GALLERY_DEMO','KP_PANEL_DEMO','KP_PREFERENCES_TAB','KP_PREFERENCES_DEMO','KP_ONBOARDING_STEP'
foreach ($name in $names) { $environment[$name] = [Environment]::GetEnvironmentVariable($name) }
try {
    foreach ($theme in 'light','dark') {
        foreach ($state in $states) {
            $key = "$theme-$($state.Name)"
            if ($key -notlike $Only) { continue }
            foreach ($name in $names) { [Environment]::SetEnvironmentVariable($name,$null) }
            $env:KWIKPASTE_SELFTEST='1'; $env:KP_THEME_GATE='1'; $env:KP_GALLERY_LANG='zh-CN'; $env:KP_TEXT_SCALE='1'
            if ($state.Flag -eq 'gallery') { $env:KP_GALLERY_THEME=$theme }
            foreach ($name in $state.Vars.Keys) { [Environment]::SetEnvironmentVariable($name,$state.Vars[$name]) }
            # 只操作开发自测身份，保存并恢复原来的设置字节。
            $settings = Join-Path $env:LOCALAPPDATA "com.fastthree.kwikpaste.native-dev.selftest-$($state.Flag)\dev\config\settings.json"
            $original = if (Test-Path -LiteralPath $settings) { [IO.File]::ReadAllBytes($settings) } else { $null }
            New-Item -ItemType Directory -Force ([IO.Path]::GetDirectoryName($settings)) | Out-Null
            if (Test-Path -LiteralPath $settings) {
                $json = [IO.File]::ReadAllText($settings)
                $json = [regex]::Replace($json,'("theme"\s*:\s*")[a-zA-Z]+("\s*[,}])',"`$1$theme`$2",1)
                $json = [regex]::Replace($json,'("material"\s*:\s*")[a-zA-Z]+("\s*[,}])',"`$1default`$2",1)
                [IO.File]::WriteAllText($settings,$json,[Text.UTF8Encoding]::new($false))
            }
            $process = $null
            try {
                $process = Start-Process -FilePath $Exe -ArgumentList "--selftest-$($state.Flag)" -PassThru -WindowStyle Hidden -RedirectStandardOutput "$Out\$key.stdout.txt" -RedirectStandardError "$Out\$key.stderr.txt"
                Start-Sleep -Milliseconds $SettleMs
                if ($process.HasExited) { throw "$key exited early: $($process.ExitCode)" }
                $windows = [ThemeCapture]::Windows([uint32]$process.Id)
                if ($windows.Count -eq 0) { throw "$key has no visible window" }
                for ($index=0; $index -lt $windows.Count; $index++) {
                    $handle = $windows[$index]
                    $rect = [ThemeCapture+RECT]::new(); [ThemeCapture]::GetClientRect($handle,[ref]$rect) | Out-Null
                    $origin = [ThemeCapture+POINT]::new(); [ThemeCapture]::ClientToScreen($handle,[ref]$origin) | Out-Null
                    $dpi = [ThemeCapture]::GetDpiForWindow($handle)
                    $bitmap = [Drawing.Bitmap]::new($rect.R,$rect.B)
                    $graphics = [Drawing.Graphics]::FromImage($bitmap)
                    $dc = $graphics.GetHdc()
                    $printed = [ThemeCapture]::PrintWindow($handle,$dc,3)
                    $graphics.ReleaseHdc($dc)
                    $method='PrintWindow(PW_CLIENTONLY|PW_RENDERFULLCONTENT)'
                    if (-not $printed -or -not [ThemeCapture]::HasContent($bitmap)) {
                        # 不激活窗口、不移动鼠标；只在截屏时把本次启动的窗口放到顶层。
                        [ThemeCapture]::SetWindowPos($handle,[IntPtr](-1),0,0,0,0,0x53) | Out-Null
                        Start-Sleep -Milliseconds 160
                        $graphics.CopyFromScreen($origin.X,$origin.Y,0,0,$bitmap.Size)
                        $method='CopyFromScreen'
                    }
                    $graphics.Dispose()
                    $inset = [int][Math]::Ceiling(10*$dpi/96)
                    $crop = [Drawing.Rectangle]::new($inset,$inset,$bitmap.Width-2*$inset,$bitmap.Height-2*$inset)
                    $client = $bitmap.Clone($crop,$bitmap.PixelFormat)
                    $file = "$key-window$index.png"
                    $client.Save((Join-Path $Out $file),[Drawing.Imaging.ImageFormat]::Png)
                    $client.Dispose(); $bitmap.Dispose()
                    $masks = @()
                    if ($state.Name -like 'gallery-*') {
                        $masks = @(@{name='gallery-search-caret';rect=@(880,975,920,1015);reason='GPUI text caret blink'})
                    }
                    if ($state.Name -like 'preferences-*') {
                        $masks += @{name='preferences-status-footer';rect=@(0,950,80,1000);reason='selftest status line timing'}
                    }
                    if ($state.Name -eq 'preferences-overview') {
                        $masks += @{name='preferences-overview-live-data';rect=@(0,170,1000,990);reason='fixture overview values are generated by the core selftest'}
                    }
                    $records.Add(@{state=$key;file=$file;window=$index;dpi=$dpi;width=$crop.Width;height=$crop.Height;inset=$inset;method=$method;masks=$masks})
                    Write-Output "$key window$index $($crop.Width)x$($crop.Height) dpi=$dpi $method"
                }
            } finally {
                if ($process -and -not $process.HasExited) { Stop-Process -Id $process.Id -Force }
                Start-Sleep -Milliseconds 250
                if ($null -ne $original) { [IO.File]::WriteAllBytes($settings,$original) } else { Remove-Item -LiteralPath $settings -ErrorAction SilentlyContinue }
            }
        }
    }
} finally {
    foreach ($name in $names) { [Environment]::SetEnvironmentVariable($name,$environment[$name]) }
    [IO.File]::WriteAllText((Join-Path $Out 'manifest.json'),(ConvertTo-Json -InputObject @($records.ToArray()) -Depth 10),[Text.UTF8Encoding]::new($false))
}

