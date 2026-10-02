<#
.SYNOPSIS
    Installs an NSIS setup silently for the current user, checks what it wrote, uninstalls it and checks
    that everything is back as it was.

.DESCRIPTION
    Checks, in order:
      - the setup's own VERSIONINFO names the expected product, so a real KwikPaste setup can never be run
        by mistake: the real identity is only accepted on a GitHub runner with -AllowProductionIdentity;
      - nothing of this identity exists yet (uninstall key, publisher key, install folder, shortcuts,
        file association);
      - a running process with the app's exe name is closed by the installer;
      - install: exe and uninstaller in %LOCALAPPDATA%\Programs\<product>, the uninstall key values, the
        publisher key, start menu and desktop shortcuts, the file association;
      - optional: the installed exe passes `--selftest-smoke`;
      - installing again over itself keeps the install location and removes the 1.2.0/1.3.0
        assets\tray.ico;
      - uninstall removes the exe, the uninstall key, the shortcuts and the file class; the template keeps
        the publisher key with the install location and an empty .<ext> key (1.x behaves the same), which
        this script then removes;
      - HKCU and HKLM ...\Run are unchanged;
      - optional: a setup whose OS gate requires build 99999 exits with 1150 and writes nothing.

    Everything this identity left behind is removed at the end, pass or fail. Kept ASCII-only so
    Windows PowerShell 5.1 reads it without a BOM.

.EXAMPLE
    ./scripts/release/test-nsis-install.ps1 -Setup dist\KwikPasteBundleTest_2.0.0_x64-setup.exe `
        -Product KwikPasteBundleTest -Binary KwikPasteBundleTest -Publisher fastthree-bundletest `
        -Extension kwikpastebundletest -Version 2.0.0 -GateSetup dist\tests\KwikPasteBundleTest_2.0.0_x64-os-gate-test-setup.exe
#>
param(
    [Parameter(Mandatory = $true)] [string] $Setup,
    [Parameter(Mandatory = $true)] [string] $Product,
    [Parameter(Mandatory = $true)] [string] $Binary,
    [Parameter(Mandatory = $true)] [string] $Publisher,
    [Parameter(Mandatory = $true)] [string] $Extension,
    [Parameter(Mandatory = $true)] [string] $Version,
    [string] $GateSetup,
    [switch] $AllUsers,
    [switch] $Launch,
    [switch] $AllowProductionIdentity
)

$ErrorActionPreference = 'Stop'
$failures = New-Object System.Collections.Generic.List[string]

function Fail([string] $message) {
    $failures.Add($message)
    Write-Output "FAIL $message"
}

function Pass([string] $message) {
    Write-Output "ok   $message"
}

function Assert-Identity([string] $path) {
    $info = [System.Diagnostics.FileVersionInfo]::GetVersionInfo((Resolve-Path -LiteralPath $path).Path)
    if ($info.ProductName -ne $Product) {
        throw "$path is a setup for '$($info.ProductName)', not '$Product'. Refusing to run it."
    }
    $real = ($Product -eq 'KwikPaste') -or ($Binary -eq 'KwikPaste') -or ($Publisher -eq 'fastthree') -or ($Extension -eq 'kwikpastebak')
    if ($real -and -not ($AllowProductionIdentity -and $env:GITHUB_ACTIONS -eq 'true')) {
        throw 'The real KwikPaste identity is only installed on a GitHub runner (-AllowProductionIdentity). Use the test identity on a developer machine.'
    }
}

# Autostart entries of both hives, as "hive name" -> command.
function Get-RunValues {
    $values = @{}
    foreach ($hive in @('HKCU:', 'HKLM:')) {
        $key = "$hive\Software\Microsoft\Windows\CurrentVersion\Run"
        if (-not (Test-Path $key)) { continue }
        $item = Get-Item $key
        foreach ($name in $item.GetValueNames()) {
            $values["$hive $name"] = [string]$item.GetValue($name)
        }
    }
    return $values
}

function Get-DefaultValue([string] $key) {
    if (-not (Test-Path $key)) { return $null }
    return (Get-Item $key).GetValue('')
}

function Get-ShortcutTarget([string] $path) {
    $shell = New-Object -ComObject WScript.Shell
    return $shell.CreateShortcut($path).TargetPath
}

# reg.exe rather than Remove-Item: the registry provider fails to delete some HKLM\Software keys with
# "Requested registry access is not allowed" even when elevated. Always the 64-bit view, like the installer.
function Remove-Path([string] $path) {
    if (-not $path -or -not (Test-Path $path)) { return }
    if ($path -match '^(HKCU|HKLM):\\(.+)$') {
        & reg.exe delete "$($Matches[1])\$($Matches[2])" /f /reg:64 | Out-Null
    } else {
        Remove-Item -LiteralPath $path -Recurse -Force -ErrorAction SilentlyContinue
    }
}

function Remove-KeyIfEmpty([string] $path) {
    if ((Test-Path $path) -and @(Get-ChildItem $path).Count -eq 0 -and (Get-Item $path).ValueCount -eq 0) {
        Remove-Path $path
    }
}

function Get-InstallLocation {
    if (-not (Test-Path $uninstallKey)) { return $null }
    return ([string](Get-Item $uninstallKey).GetValue('InstallLocation')).Trim('"')
}

function Invoke-Setup([string] $path, [string[]] $arguments) {
    $process = Start-Process -FilePath $path -ArgumentList $arguments -Wait -PassThru
    return $process.ExitCode
}

# Both install modes write the same names, into HKCU and the user's folders or HKLM and the shared ones.
if ($AllUsers) {
    $principal = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { throw '-AllUsers needs an elevated shell.' }
    $mode = '/ALLUSERS'
    $modeValue = 'AllUsers'
    $hive = 'HKLM:'
    $otherHive = 'HKCU:'
    $installDir = Join-Path $env:ProgramFiles $Product
    $startMenu = Join-Path $env:ProgramData "Microsoft\Windows\Start Menu\Programs\$Product.lnk"
    $desktop = Join-Path ([Environment]::GetFolderPath('CommonDesktopDirectory')) "$Product.lnk"
} else {
    $mode = '/CURRENTUSER'
    $modeValue = 'CurrentUser'
    $hive = 'HKCU:'
    $otherHive = 'HKLM:'
    $installDir = Join-Path $env:LOCALAPPDATA "Programs\$Product"
    $startMenu = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\$Product.lnk"
    $desktop = Join-Path ([Environment]::GetFolderPath('Desktop')) "$Product.lnk"
}
$uninstallKey = "$hive\Software\Microsoft\Windows\CurrentVersion\Uninstall\$Product"
$uninstallKeyOther = "$otherHive\Software\Microsoft\Windows\CurrentVersion\Uninstall\$Product"
$publisherKey = "$hive\Software\$Publisher"
$productKey = "$publisherKey\$Product"
$extensionKey = "$hive\Software\Classes\.$Extension"
$identityPaths = @($uninstallKey, $uninstallKeyOther, $productKey, "$otherHive\Software\$Publisher\$Product", $extensionKey, $installDir, $startMenu, $desktop)

Assert-Identity $Setup
if ($GateSetup) { Assert-Identity $GateSetup }

foreach ($path in $identityPaths) {
    if (Test-Path $path) { throw "$path already exists; this identity is not clean, refusing to test over it." }
}

$runBefore = Get-RunValues
$work = Join-Path ([System.IO.Path]::GetTempPath()) ("kwikpaste-nsis-test-" + [guid]::NewGuid().ToString('N').Substring(0, 8))

# A stand-in for a running app: any process with the app's exe name must be closed by the installer.
New-Item -ItemType Directory -Force -Path $work | Out-Null
$fakeExe = Join-Path $work "$Binary.exe"
Copy-Item -LiteralPath (Join-Path $env:SystemRoot 'System32\PING.EXE') -Destination $fakeExe
$fake = Start-Process -FilePath $fakeExe -ArgumentList @('-n', '600', '127.0.0.1') -WindowStyle Hidden -PassThru

try {
    $code = Invoke-Setup $Setup @('/S', $mode)
    if ($code -ne 0) { Fail "install exited with $code" } else { Pass "silent $mode install exited with 0" }

    Start-Sleep -Milliseconds 500
    if ($fake.HasExited) { Pass "the running $Binary.exe was closed" } else { Fail "the running $Binary.exe was not closed" }

    $location = Get-InstallLocation
    if ($location -ne $installDir) { Fail "install location is '$location', expected '$installDir'" }
    if (Test-Path $uninstallKey) {
        $key = Get-Item $uninstallKey
        $expected = @{
            DisplayName     = $Product
            DisplayVersion  = $Version
            Publisher       = $Publisher
            MainBinaryName  = "$Binary.exe"
            UninstallString = "`"$installDir\uninstall.exe`""
            DisplayIcon     = "`"$installDir\$Binary.exe`""
        }
        foreach ($name in $expected.Keys) {
            $actual = [string]$key.GetValue($name)
            if ($actual -ne $expected[$name]) { Fail "uninstall key $name is '$actual', expected '$($expected[$name])'" }
        }
        foreach ($name in @('NoModify', 'NoRepair', $modeValue)) {
            if ($key.GetValue($name) -ne 1) { Fail "uninstall key $name is not 1" }
        }
        Pass "uninstall key $uninstallKey"
    } else {
        Fail "uninstall key $uninstallKey is missing"
    }
    if (Test-Path $uninstallKeyOther) { Fail "a $mode install wrote $uninstallKeyOther" }

    foreach ($file in @("$Binary.exe", 'uninstall.exe')) {
        if (Test-Path (Join-Path $installDir $file)) { Pass "installed $file" } else { Fail "$file is missing" }
    }
    if ((Get-DefaultValue $productKey) -eq $installDir) { Pass "$productKey holds the install location" } else { Fail "$productKey does not hold the install location" }
    foreach ($link in @($startMenu, $desktop)) {
        if (-not (Test-Path $link)) { Fail "$link is missing"; continue }
        $target = Get-ShortcutTarget $link
        if ($target -eq (Join-Path $installDir "$Binary.exe")) { Pass "shortcut $link" } else { Fail "$link points at $target" }
    }
    $class = Get-DefaultValue $extensionKey
    $classKey = if ($class) { "$hive\Software\Classes\$class" } else { $null }
    $command = if ($classKey) { Get-DefaultValue "$classKey\shell\open\command" } else { $null }
    if ($command -and $command.Contains("$installDir\$Binary.exe")) { Pass ".$Extension opens with $Binary.exe" } else { Fail ".$Extension association is missing or wrong ($class / $command)" }

    if ($Launch) {
        $env:KWIKPASTE_SELFTEST = '1'
        $app = Start-Process -FilePath (Join-Path $installDir "$Binary.exe") -ArgumentList '--selftest-smoke' -PassThru
        Remove-Item Env:KWIKPASTE_SELFTEST
        if (-not $app.WaitForExit(60000)) { $app.Kill(); Fail 'the installed app did not exit within 60 s' }
        elseif ($app.ExitCode -ne 0) { Fail "the installed app exited with $($app.ExitCode)" }
        else { Pass 'the installed app started and exited cleanly' }
    }

    # What an upgrade over 1.2.0/1.3.0 finds next to the exe.
    New-Item -ItemType Directory -Force -Path (Join-Path $installDir 'assets') | Out-Null
    Set-Content -LiteralPath (Join-Path $installDir 'assets\tray.ico') -Value 'legacy'
    $code = Invoke-Setup $Setup @('/S', $mode)
    if ($code -eq 0 -and (Get-InstallLocation) -eq $installDir) { Pass 'installing again keeps the install location' } else { Fail "reinstall exited with $code into '$(Get-InstallLocation)'" }
    if (Test-Path (Join-Path $installDir 'assets')) { Fail 'assets\tray.ico from 1.2.0/1.3.0 was not removed' } else { Pass 'legacy assets\tray.ico removed' }

    $code = Invoke-Setup (Join-Path $installDir 'uninstall.exe') @('/S', "_?=$installDir")
    if ($code -ne 0) { Fail "uninstall exited with $code" } else { Pass 'silent uninstall exited with 0' }

    foreach ($path in @($uninstallKey, $startMenu, $desktop, (Join-Path $installDir "$Binary.exe"))) {
        if (Test-Path $path) { Fail "$path is still there after uninstall" }
    }
    if ($classKey -and (Test-Path $classKey)) { Fail "$classKey is still there after uninstall" }
    # The template backs up the previous handler on every install and restores it on uninstall; after a
    # reinstall that "previous handler" is our own, now deleted, class. Anything else is a leftover.
    $handler = Get-DefaultValue $extensionKey
    if ($handler -and $handler -ne $class) { Fail ".$Extension opens with '$handler' after uninstall" }
    if (-not ($failures | Where-Object { $_ -like '*after uninstall*' })) { Pass 'uninstall removed the exe, the uninstall key, the shortcuts and the file class' }

    # The template keeps these on uninstall unless "delete app data" is ticked; clean up our own.
    Remove-Path $productKey
    Remove-KeyIfEmpty $publisherKey
    Remove-Path $extensionKey
    Remove-Path $installDir

    if ($GateSetup) {
        $code = Invoke-Setup $GateSetup @('/S', $mode)
        if ($code -eq 1150) { Pass 'the OS gate exits with 1150' } else { Fail "the OS gate test setup exited with $code, expected 1150" }
        foreach ($path in $identityPaths + @($publisherKey)) {
            if (Test-Path $path) { Fail "the OS gate test setup created $path" }
        }
    }

    $runAfter = Get-RunValues
    $changed = @($runBefore.Keys + $runAfter.Keys | Sort-Object -Unique | Where-Object { $runBefore[$_] -ne $runAfter[$_] })
    if ($changed.Count -eq 0) { Pass 'HKCU and HKLM Run are unchanged' } else { Fail "Run changed: $($changed -join ', ')" }
}
finally {
    if (-not $fake.HasExited) { $fake.Kill() }
    Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
    # Pass or fail, leave nothing of this identity behind (the identity check above keeps this away from a
    # real install outside a GitHub runner).
    $uninstaller = Join-Path $installDir 'uninstall.exe'
    if (Test-Path $uninstaller) { Start-Process -FilePath $uninstaller -ArgumentList @('/S', "_?=$installDir") -Wait | Out-Null }
    foreach ($path in @($uninstallKey, $productKey, $extensionKey, $classKey, $installDir, $startMenu, $desktop)) {
        Remove-Path $path
    }
    Remove-KeyIfEmpty $publisherKey
}

if ($failures.Count -gt 0) {
    Write-Output "$($failures.Count) check(s) failed."
    exit 1
}
Write-Output 'All install checks passed.'
