<#
.SYNOPSIS
    Runner-only verification that a production-identity 2.0 install preserves a 1.x install.

.DESCRIPTION
    This script is intentionally approval-gated by the GitHub runner check. It never runs against the
    developer machine's official installation. The old process is started by PID, seeded through the
    system clipboard, and stopped by that PID only. The native setup is then installed over it and the
    record, settings, Run value, shortcut, file association, and uninstall metadata are checked.
#>
param(
    [Parameter(Mandatory = $true)] [string] $Setup,
    [Parameter(Mandatory = $true)] [string] $NativeSetup,
    [Parameter(Mandatory = $true)] [string] $Version,
    [switch] $AllUsers,
    [string] $FixtureVersion,
    [switch] $CustomStorage
)

$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true') { throw 'This script may only run on a GitHub runner.' }
$product = 'KwikPaste'
$mode = if ($AllUsers) { '/ALLUSERS' } else { '/CURRENTUSER' }
$installDir = if ($AllUsers) { Join-Path $env:ProgramFiles $product } else { Join-Path $env:LOCALAPPDATA "Programs\$product" }
$exe = Join-Path $installDir 'KwikPaste.exe'
$data = Join-Path $env:LOCALAPPDATA 'com.fastthree.kwikpaste\prod'
$db = Join-Path $data 'db\kwikpaste.db'
$settings = Join-Path $data 'config\settings.json'
$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$startMenu = if ($AllUsers) { Join-Path $env:ProgramData 'Microsoft\Windows\Start Menu\Programs\KwikPaste.lnk' } else { Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\KwikPaste.lnk' }
$association = 'HKCU:\Software\Classes\.kwikpastebak'
$uninstall = if ($AllUsers) { 'HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\KwikPaste' } else { 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\KwikPaste' }
$record = 'native-upgrade-record-' + [guid]::NewGuid().ToString('N')
$old = $null
$customStorage = $null
$storageManifestPath = Join-Path $data 'storage.json'
$storageManifestBefore = if (Test-Path $storageManifestPath) { Get-Content -LiteralPath $storageManifestPath -Raw } else { $null }

function Write-Utf8([string] $path, [string] $text) {
    [IO.File]::WriteAllText($path, $text, [Text.UTF8Encoding]::new($false))
}

function Invoke-Setup([string] $path, [string[]] $args) {
    $process = Start-Process -FilePath $path -ArgumentList $args -Wait -PassThru
    if ($process.ExitCode -ne 0) { throw "$path $($args -join ' ') exited with $($process.ExitCode)" }
}

function Wait-Path([string] $path) {
    for ($i = 0; $i -lt 60; $i++) {
        if (Test-Path -LiteralPath $path) { return }
        Start-Sleep -Seconds 1
    }
    throw "timed out waiting for $path"
}

function Query-Record([string] $dbPath, [string] $needle) {
    if (-not (Test-Path $dbPath)) { throw "database $dbPath is missing" }
    $code = @'
import sqlite3, sys
db, needle = sys.argv[1:]
con = sqlite3.connect(db)
try:
    row = con.execute("select count(*) from clipboard_items where content like ? or summary like ?", ("%"+needle+"%", "%"+needle+"%")).fetchone()
    print(row[0])
finally:
    con.close()
'@
    $value = (& python -c $code $dbPath $needle).Trim()
    if ($LASTEXITCODE -ne 0 -or $value -lt 1) { throw "record '$needle' was not readable in $dbPath (python=$LASTEXITCODE value=$value)" }
}

function Shortcut-Target([string] $path) {
    $shell = New-Object -ComObject WScript.Shell
    return $shell.CreateShortcut($path).TargetPath
}

function Default-Value([string] $path) {
    $item = Get-Item -LiteralPath $path -ErrorAction Stop
    return [string]$item.GetValue('')
}

try {
    if (Test-Path $installDir) { throw "$installDir already exists before the 1.x test" }
    Invoke-Setup $Setup @('/S', $mode)
    Wait-Path $exe

    # Start exactly one old process and seed through its real clipboard listener.
    $old = Start-Process -FilePath $exe -ArgumentList '--auto-launch' -PassThru -WindowStyle Hidden
    Start-Sleep -Seconds 5
    Set-Clipboard -Value $record
    Start-Sleep -Seconds 4
    if (-not $old.HasExited) { $old.Kill(); $old.WaitForExit() }
    Query-Record $db $record
    if (-not (Test-Path $settings)) { throw "1.x settings file $settings is missing after seeding" }
    $settingsBefore = (Get-FileHash $settings -Algorithm SHA256).Hash
    $dbBefore = (Get-FileHash $db -Algorithm SHA256).Hash

    if ($CustomStorage) {
        $customStorage = Join-Path $env:RUNNER_TEMP ('kwikpaste-custom-storage-' + [guid]::NewGuid().ToString('N'))
        New-Item -ItemType Directory -Force -Path $customStorage | Out-Null
        Copy-Item -Path (Join-Path $data '*') -Destination $customStorage -Recurse -Force
        $stamp = [DateTime]::UtcNow.ToString('o')
        $identity = [ordered]@{ version = 1; environment = 'prod'; createdAt = $stamp }
        $manifest = [ordered]@{ version = 1; environment = 'prod'; dataDir = $customStorage; updatedAt = $stamp }
        Write-Utf8 (Join-Path $customStorage '.kwikpaste-storage.json') ($identity | ConvertTo-Json -Depth 4)
        Write-Utf8 $storageManifestPath ($manifest | ConvertTo-Json -Depth 4)
        $data = $customStorage
        $db = Join-Path $data 'db\kwikpaste.db'
        $settings = Join-Path $data 'config\settings.json'
        $settingsBefore = (Get-FileHash $settings -Algorithm SHA256).Hash
        $dbBefore = (Get-FileHash $db -Algorithm SHA256).Hash
        Write-Host "ok   configured custom storage location $customStorage"
    }

    # Enable the same startup value that the installer hook preserves over an upgrade.
    New-Item -Path $runKey -Force | Out-Null
    Set-ItemProperty -Path $runKey -Name KwikPaste -Value ('"' + $exe + '" --auto-launch')
    $runBefore = (Get-ItemProperty -Path $runKey).KwikPaste

    Invoke-Setup $NativeSetup @('/S', $mode)
    Wait-Path $exe
    $running = Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath -eq $exe }
    if ($running) { throw 'the 1.x process was not closed by the 2.0 installer' }
    Query-Record $db $record
    if (-not (Test-Path $settings)) { throw 'settings.json was removed by the 2.0 installer' }
    if ((Get-FileHash $settings -Algorithm SHA256).Hash -ne $settingsBefore) { throw 'settings.json changed during install-over-1.x' }
    if ((Get-ItemProperty -Path $runKey).KwikPaste -ne ('"' + $exe + '" --auto-launch')) { throw 'HKCU Run KwikPaste does not point to the installed 2.0 exe' }
    if (-not (Test-Path $startMenu) -or (Shortcut-Target $startMenu) -ne $exe) { throw 'Start menu shortcut does not point to the installed 2.0 exe' }
    $class = Default-Value $association
    $command = if ($class) { Default-Value "HKCU:\Software\Classes\$class\shell\open\command" } else { '' }
    if (-not $command.Contains($exe)) { throw '.kwikpastebak association does not point to the installed 2.0 exe' }
    $uninstallValues = Get-ItemProperty -Path $uninstall
    if ($uninstallValues.DisplayVersion -ne $Version) { throw "uninstall entry version is '$($uninstallValues.DisplayVersion)', expected '$Version'" }

    $env:KWIKPASTE_SELFTEST = '1'
    $smoke = Start-Process -FilePath $exe -ArgumentList '--selftest-smoke' -PassThru -WindowStyle Hidden
    if (-not $smoke.WaitForExit(60000) -or $smoke.ExitCode -ne 0) { throw "2.0 --selftest-smoke failed with $($smoke.ExitCode)" }
    Remove-Item Env:KWIKPASTE_SELFTEST -ErrorAction SilentlyContinue
    Write-Host "ok   $Version ${mode}: records/settings/autostart/shortcut/association/uninstall/smoke"
    Write-Host "     db-before=$dbBefore settings-before=$settingsBefore fixture=$FixtureVersion"
} finally {
    Remove-Item Env:KWIKPASTE_SELFTEST -ErrorAction SilentlyContinue
    if ($old -and -not $old.HasExited) { $old.Kill(); $old.WaitForExit() }
    $uninstaller = Join-Path $installDir 'uninstall.exe'
    if (Test-Path $uninstaller) { Start-Process $uninstaller -ArgumentList @('/S', "_?=$installDir") -Wait | Out-Null }
    if (Test-Path $runKey) { Remove-ItemProperty -Path $runKey -Name KwikPaste -ErrorAction SilentlyContinue }
    if ($storageManifestBefore -ne $null) {
        Write-Utf8 $storageManifestPath $storageManifestBefore
    } elseif (Test-Path $storageManifestPath) {
        Remove-Item -LiteralPath $storageManifestPath -Force -ErrorAction SilentlyContinue
    }
    if ($customStorage -and (Test-Path $customStorage)) { Remove-Item -LiteralPath $customStorage -Recurse -Force -ErrorAction SilentlyContinue }
}
