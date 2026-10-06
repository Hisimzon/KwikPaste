<#
.SYNOPSIS
    Builds two throwaway test-identity versions and drives the native updater locally.

.DESCRIPTION
    This is deliberately separate from the production release workflow.  It generates a fresh minisign
    key in a temporary directory, builds with `e2e-overrides`, serves a v2 manifest from loopback, and
    runs the real self-test update driver.  The production identifier, `~/.tauri`, and the installed
    official KwikPaste are never touched.  The script is Windows-only; the same manifest/package
    contract is exercised by the macOS job in native-update-e2e.yml.

    The app self-test performs check -> signed download -> platform handoff.  A second and a wrong-key
    manifest are tried first; their processes must report signature refusal and leave the old install in
    place.  The final valid run must replace the portable executable or install the NSIS setup and
    restart the new version.
#>
param(
    [ValidateSet('Portable', 'Nsis', 'Both')]
    [string] $Mode = 'Both',
    [string] $Target = 'x86_64-pc-windows-msvc',
    [switch] $SkipBuild
)

$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$run = Join-Path ([IO.Path]::GetTempPath()) ('kwikpaste-native-update-e2e-' + [guid]::NewGuid().ToString('N'))
$keyDir = Join-Path $run 'key'
$serverDir = Join-Path $run 'server'
$oldOut = Join-Path $run 'old'
$newOut = Join-Path $run 'new'
$portableRoot = Join-Path $run 'portable'
$oldVersion = '2.0.0-e2e.1'
$newVersion = '2.0.0-e2e.2'
$seed = 'native-update-e2e-record-' + [guid]::NewGuid().ToString('N')
$e2eDataRoot = Join-Path $env:LOCALAPPDATA 'com.fastthree.kwikpaste.native-dev.selftest-update-e2e\dev'
$manifestPath = Join-Path $serverDir 'latest.json'
$server = $null
$oldManifest = $null
$originalManifest = [IO.File]::ReadAllText((Join-Path $root 'Cargo.toml'))
$lockPath = Join-Path $root 'Cargo.lock'
$originalLock = [IO.File]::ReadAllText($lockPath)
$originalEnv = @{}
foreach ($name in @('TAURI_SIGNING_PRIVATE_KEY', 'TAURI_SIGNING_PRIVATE_KEY_PASSWORD', 'KWIKPASTE_UPDATE_ENDPOINT', 'KWIKPASTE_UPDATE_PUBLIC_KEY', 'KWIKPASTE_SELFTEST')) {
    $originalEnv[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}
$tauriCli = $env:KWIKPASTE_TAURI_CLI
if (-not $tauriCli) {
    $candidates = @(
        (Join-Path $root 'node_modules\@tauri-apps\cli\tauri.js'),
        (Join-Path $root '..\..\..\node_modules\@tauri-apps\cli\tauri.js')
    )
    $tauriCli = $candidates | Where-Object { Test-Path $_ } | Select-Object -First 1
}

function Write-Utf8([string] $path, [string] $text) {
    [IO.File]::WriteAllText($path, $text, [Text.UTF8Encoding]::new($false))
}

function Run([string] $file, [string[]] $arguments, [hashtable] $env = @{}) {
    Write-Host "> $file $($arguments -join ' ')"
    $saved = @{}
    foreach ($entry in $env.GetEnumerator()) {
        $saved[$entry.Key] = [Environment]::GetEnvironmentVariable($entry.Key, 'Process')
        [Environment]::SetEnvironmentVariable($entry.Key, [string]$entry.Value, 'Process')
    }
    try {
        $p = Start-Process -FilePath $file -ArgumentList $arguments -WorkingDirectory $root -Wait -PassThru -NoNewWindow
        if ($p.ExitCode -ne 0) { throw "$file exited with $($p.ExitCode)" }
    } finally {
        foreach ($entry in $saved.GetEnumerator()) { [Environment]::SetEnvironmentVariable($entry.Key, $entry.Value, 'Process') }
    }
}

function Run-Tauri {
    param([Parameter(Position = 0, ValueFromRemainingArguments = $true)] [string[]] $arguments)
    if (-not $tauriCli) { throw 'the pinned @tauri-apps/cli entry point was not found; run pnpm install first' }
    $nodeArguments = [System.Collections.Generic.List[string]]::new()
    [void]$nodeArguments.Add([string]$tauriCli)
    foreach ($argument in $arguments) { [void]$nodeArguments.Add([string]$argument) }
    Run 'node' $nodeArguments.ToArray()
}

function Set-Version([string] $version) {
    $text = [regex]::Replace(
        $originalManifest,
        '(?ms)(\[workspace\.package\].*?^version\s*=\s*")[^"]+(")',
        [Text.RegularExpressions.MatchEvaluator]{ param($match) "$($match.Groups[1].Value)$version$($match.Groups[2].Value)" },
        1
    )
    Write-Utf8 (Join-Path $root 'Cargo.toml') $text
}

function Build-Version([string] $version, [string] $out) {
    Set-Version $version
    # Cargo does not always fingerprint a workspace package version change. Clean only the
    # app target so the old archive cannot accidentally contain the newer binary.
    Run 'cargo' @('clean', '--release', '-p', 'kwikpaste-app', '--target', $Target)
    Run 'cargo' @('build', '--locked', '--release', '--target', $Target, '-p', 'kwikpaste-app', '--features', 'e2e-overrides') @{ CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS = '-C target-feature=+crt-static' }
    New-Item -ItemType Directory -Force -Path $out | Out-Null
    Run 'node' @('scripts/release/package-native.mjs', '--target', $Target, '--out', $out, '--identity', 'test', '--pubkey', (Join-Path $keyDir 'e2e.key.pub'), '--features', 'e2e-overrides') @{ TAURI_SIGNING_PRIVATE_KEY = (Join-Path $keyDir 'e2e.key'); TAURI_SIGNING_PRIVATE_KEY_PASSWORD = $env:E2E_SIGNING_PASSWORD; KWIKPASTE_TAURI_CLI = $tauriCli }
}

function Make-Manifest([string] $artifact, [string] $signature) {
    $url = "http://127.0.0.1:$($script:Port)/$([IO.Path]::GetFileName($artifact))"
    $platforms = [ordered]@{
        'windows-x86_64-nsis' = @{ url = $url; signature = $signature }
        'windows-x86_64-portable' = @{ url = $url; signature = $signature }
    }
    $manifest = [ordered]@{ kwikpaste = @{ schema = 1; requires = @{ windowsBuild = 17134 } }; notes = 'local e2e'; pub_date = (Get-Date).ToUniversalTime().ToString('o'); version = $newVersion; platforms = $platforms }
    Write-Utf8 $manifestPath (($manifest | ConvertTo-Json -Depth 8) + "`n")
}

function Start-LocalServer {
    $script:Port = Get-Random -Minimum 18000 -Maximum 48000
    $stdout = Join-Path $run 'http.stdout.log'
    $stderr = Join-Path $run 'http.stderr.log'
    $script:server = Start-Process python -ArgumentList @('-m', 'http.server', $script:Port, '--bind', '127.0.0.1', '--directory', $serverDir) -RedirectStandardOutput $stdout -RedirectStandardError $stderr -PassThru -WindowStyle Hidden
    for ($i = 0; $i -lt 50; $i++) {
        if ((Test-NetConnection 127.0.0.1 -Port $script:Port -InformationLevel Quiet) -and -not $script:server.HasExited) { return }
        Start-Sleep -Milliseconds 100
    }
    throw "local HTTP server did not listen on $script:Port"
}

function Invoke-App([string] $exe, [string] $label) {
    $stdout = Join-Path $run "$label.stdout.log"
    $stderr = Join-Path $run "$label.stderr.log"
    $env:KWIKPASTE_SELFTEST = '1'
    $env:KWIKPASTE_UPDATE_ENDPOINT = "http://127.0.0.1:$($script:Port)/latest.json"
    $env:KWIKPASTE_UPDATE_PUBLIC_KEY = [IO.File]::ReadAllText((Join-Path $keyDir 'e2e.key.pub')).Trim()
    $env:KWIKPASTE_E2E_SEED = $seed
    try {
        $p = Start-Process -FilePath $exe -ArgumentList '--selftest-update-e2e' -RedirectStandardOutput $stdout -RedirectStandardError $stderr -PassThru -WindowStyle Hidden
        if (-not $p.WaitForExit(120000)) { $p.Kill(); throw "$label did not exit within 120 s" }
        $text = (Get-Content $stderr -Raw -ErrorAction SilentlyContinue) + (Get-Content $stdout -Raw -ErrorAction SilentlyContinue)
        Write-Host "[$label] exit=$($p.ExitCode)"
        Write-Host $text
        return $text
    } finally {
        Remove-Item Env:KWIKPASTE_SELFTEST -ErrorAction SilentlyContinue
        Remove-Item Env:KWIKPASTE_UPDATE_ENDPOINT -ErrorAction SilentlyContinue
        Remove-Item Env:KWIKPASTE_UPDATE_PUBLIC_KEY -ErrorAction SilentlyContinue
        Remove-Item Env:KWIKPASTE_E2E_SEED -ErrorAction SilentlyContinue
    }
}

function Assert-SeededStorage([string] $dataRoot = $e2eDataRoot) {
    $db = Join-Path $dataRoot 'db\clipboard.db'
    $settings = Join-Path $dataRoot 'config\settings.json'
    if (-not (Test-Path $db) -or -not (Test-Path $settings)) { throw "self-update data dir is incomplete: $dataRoot" }
    $queryScript = Join-Path $run 'assert-seed.py'
    Write-Utf8 $queryScript 'import sqlite3, sys
db, seed = sys.argv[1:]
with sqlite3.connect(db) as connection:
    print(connection.execute("select count(*) from clipboard_items where content like ?", ("%" + seed + "%",)).fetchone()[0])
'
    $count = (& python $queryScript $db $seed).Trim()
    if ($LASTEXITCODE -ne 0 -or $count -lt 1) { throw "seed record was not preserved in $db (count=$count)" }
    $settingsText = [IO.File]::ReadAllText($settings)
    if ($settingsText -notmatch '"autoStart"\s*:\s*true') { throw "seed setting was not preserved in $settings" }
    Write-Host "ok   seeded record and autoStart setting survived in $dataRoot"
}

function Assert-Refused([string] $exe, [string] $label) {
    $before = (Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash
    $text = Invoke-App $exe $label
    $after = (Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash
    if ($before -ne $after) { throw "$label changed the old executable" }
    if ($text -notmatch 'signature|验签|download failed') { throw "$label did not report signature refusal" }
    Write-Host "ok   $label refused and kept the old executable"
}

function Wait-HashChange([string] $path, [string] $before, [int] $seconds = 120) {
    for ($i = 0; $i -lt $seconds; $i++) {
        Start-Sleep -Seconds 1
        $current = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash
        if ($current -ne $before) { return $current }
    }
    throw "installer did not replace $path within $seconds s"
}

function Run-Portable {
    $oldZip = Join-Path $oldOut "KwikPaste_${oldVersion}_x64_portable.zip"
    $newZip = Join-Path $newOut "KwikPaste_${newVersion}_x64_portable.zip"
    Expand-Archive -LiteralPath $oldZip -DestinationPath $portableRoot -Force
    $exe = Join-Path $portableRoot 'KwikPaste\KwikPaste.exe'
    Copy-Item $newZip (Join-Path $serverDir (Split-Path $newZip -Leaf))
    $sig = (Get-Content "$newZip.sig" -Raw).Trim()
    Make-Manifest (Join-Path $serverDir (Split-Path $newZip -Leaf)) $sig
    $script:oldManifest = Get-Content $manifestPath -Raw
    $json = Get-Content $manifestPath -Raw | ConvertFrom-Json
    $json.platforms.'windows-x86_64-portable'.signature = $sig.Substring(0, [Math]::Max(1, $sig.Length - 8)) + 'AAAA===='
    Write-Utf8 $manifestPath (($json | ConvertTo-Json -Depth 8) + "`n")
    Assert-Refused $exe 'portable-tampered-signature'
    Write-Utf8 $manifestPath $script:oldManifest
    $wrong = Join-Path $run 'wrong.key'
    $wrongPub = Join-Path $run 'wrong.key.pub'
    Run-Tauri @('signer', 'generate', '--ci', '-p', 'wrong-e2e', '-w', $wrong, '-f')
    $savedKey = $env:TAURI_SIGNING_PRIVATE_KEY
    $savedPassword = $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD
    $env:TAURI_SIGNING_PRIVATE_KEY = [IO.File]::ReadAllText($wrong).Trim()
    $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = 'wrong-e2e'
    try {
        Run-Tauri @('signer', 'sign', (Join-Path $serverDir (Split-Path $newZip -Leaf)))
    } finally {
        $env:TAURI_SIGNING_PRIVATE_KEY = $savedKey
        $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = $savedPassword
    }
    $wrongSig = (Get-Content "$(Join-Path $serverDir (Split-Path $newZip -Leaf)).sig" -Raw).Trim()
    $json = Get-Content $manifestPath -Raw | ConvertFrom-Json
    $json.platforms.'windows-x86_64-portable'.signature = $wrongSig
    Write-Utf8 $manifestPath (($json | ConvertTo-Json -Depth 8) + "`n")
    Assert-Refused $exe 'portable-wrong-key'
    Write-Utf8 $manifestPath $script:oldManifest
    $oldHash = (Get-FileHash $exe -Algorithm SHA256).Hash
    $text = Invoke-App $exe 'portable-valid-update'
    $newHash = (Get-FileHash $exe -Algorithm SHA256).Hash
    if ($oldHash -eq $newHash -or $text -notmatch 'installing|downloaded') { throw 'portable valid update did not replace the executable' }
    Assert-SeededStorage (Join-Path $portableRoot 'KwikPaste\data\dev')
    Write-Host "ok   portable valid update replaced the executable ($oldHash -> $newHash)"
}

function Run-Nsis {
    $setup = Join-Path $oldOut "KwikPasteBundleTest_${oldVersion}_x64-setup.exe"
    $newSetup = Join-Path $newOut "KwikPasteBundleTest_${newVersion}_x64-setup.exe"
    $install = Join-Path $env:LOCALAPPDATA 'Programs\KwikPasteBundleTest'
    if (Test-Path $install) { throw "$install already exists; refusing to touch a pre-existing test identity" }
    Copy-Item $newSetup (Join-Path $serverDir (Split-Path $newSetup -Leaf))
    $sig = (Get-Content "$newSetup.sig" -Raw).Trim()
    Make-Manifest (Join-Path $serverDir (Split-Path $newSetup -Leaf)) $sig
    & $setup /S /CURRENTUSER | Out-Null
    try {
        $exe = Join-Path $install 'KwikPasteBundleTest.exe'
        if (-not (Test-Path $exe)) { throw 'test-identity NSIS setup did not install its executable' }
        $oldHash = (Get-FileHash $exe -Algorithm SHA256).Hash
        $text = Invoke-App $exe 'nsis-valid-update'
        $newHash = Wait-HashChange $exe $oldHash
        if ($oldHash -eq $newHash -or $text -notmatch 'installing|downloaded') { throw 'NSIS valid update did not replace the executable' }
        Assert-SeededStorage
        Write-Host "ok   NSIS valid update replaced the executable ($oldHash -> $newHash)"
    } finally {
        $uninstaller = Join-Path $install 'uninstall.exe'
        if (Test-Path $uninstaller) { & $uninstaller /S "_?=$install" | Out-Null }
    }
}

function Remove-TestIdentityArtifacts {
    # The selftest deliberately enables autostart so the update preserves a real setting. Remove only
    # the two development identifiers and the test extension; never touch the installed production value.
    $runKey = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Software\Microsoft\Windows\CurrentVersion\Run', $true)
    if ($null -ne $runKey) {
        foreach ($name in @(
                'com.fastthree.kwikpaste.native-dev.selftest-update-e2e',
                'com.fastthree.kwikpaste.native-dev.selftest-update-e2e Portable')) {
            if ($runKey.GetValueNames() -contains $name) { $runKey.DeleteValue($name) }
        }
        $runKey.Close()
    }

    $classes = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Software\Classes', $true)
    if ($null -ne $classes) {
        if ($classes.GetSubKeyNames() -contains '.kwikpastebundletest') {
            $classes.DeleteSubKeyTree('.kwikpastebundletest')
        }
        $classes.Close()
    }

    $testInstall = Join-Path $env:LOCALAPPDATA 'Programs\KwikPasteBundleTest'
    if (Test-Path -LiteralPath $testInstall) {
        $resolved = [IO.Path]::GetFullPath($testInstall)
        $expected = [IO.Path]::GetFullPath((Join-Path $env:LOCALAPPDATA 'Programs\KwikPasteBundleTest'))
        if ($resolved -eq $expected) {
            [IO.Directory]::Delete($resolved, $true)
        }
    }
}

try {
    New-Item -ItemType Directory -Force -Path $run, $keyDir, $serverDir, $oldOut, $newOut | Out-Null
    if (-not (Get-Command node -ErrorAction SilentlyContinue)) { throw 'node is required' }
    if (-not $SkipBuild) {
        $password = 'e2e-' + [guid]::NewGuid().ToString('N')
        $env:E2E_SIGNING_PASSWORD = $password
        Run-Tauri @('signer', 'generate', '--ci', '-p', $password, '-w', (Join-Path $keyDir 'e2e.key'), '-f')
        Build-Version $oldVersion $oldOut
        Build-Version $newVersion $newOut
    } else {
        $env:E2E_SIGNING_PASSWORD = ''
    }
    Start-LocalServer
    if ($Mode -in @('Portable', 'Both')) { Run-Portable }
    if ($Mode -in @('Nsis', 'Both')) { Run-Nsis }
    Write-Host 'All native self-update e2e checks passed.'
} finally {
    if ($server -and -not $server.HasExited) { $server.Kill() }
    Write-Utf8 (Join-Path $root 'Cargo.toml') $originalManifest
    Write-Utf8 $lockPath $originalLock
    foreach ($name in $originalEnv.Keys) { [Environment]::SetEnvironmentVariable($name, $originalEnv[$name], 'Process') }
    Remove-TestIdentityArtifacts
    Remove-Item -LiteralPath (Join-Path $env:LOCALAPPDATA 'com.fastthree.kwikpaste.native-dev.selftest-update-e2e') -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $run -Recurse -Force -ErrorAction SilentlyContinue
}
