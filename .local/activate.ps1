$ErrorActionPreference = "Stop"

function Find-VsDevCmdPath {
    $vswhereCandidates = @(
        "C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe"
        "C:\Program Files\Microsoft Visual Studio\Installer\vswhere.exe"
    )

    foreach ($vswhere in $vswhereCandidates) {
        if (-not (Test-Path $vswhere)) {
            continue
        }

        $installationPath = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
        if (-not $installationPath) {
            continue
        }

        $vsDevCmd = Join-Path $installationPath "Common7\Tools\VsDevCmd.bat"
        if (Test-Path $vsDevCmd) {
            return $vsDevCmd
        }

        $vcVars64 = Join-Path $installationPath "VC\Auxiliary\Build\vcvars64.bat"
        if (Test-Path $vcVars64) {
            return $vcVars64
        }
    }

    return $null
}

function Import-BatchEnvironment {
    param(
        [string]$BatchPath,
        [string[]]$Arguments = @()
    )

    if (-not (Test-Path $BatchPath)) {
        return $false
    }

    $argumentString = $Arguments -join " "
    $command = ('call "{0}" {1} >nul && set' -f $BatchPath, $argumentString).Trim()
    $environmentLines = & cmd.exe /d /s /c $command

    if ($LASTEXITCODE -ne 0) {
        return $false
    }

    foreach ($line in $environmentLines) {
        if ($line -match '^(.*?)=(.*)$') {
            Set-Item -Path ("Env:" + $matches[1]) -Value $matches[2]
        }
    }

    return $true
}

$ProjectRoot = Split-Path -Parent $PSScriptRoot
$LocalRoot = Join-Path $ProjectRoot ".local"
$CargoHome = Join-Path $LocalRoot "cargo"
$RustupHome = Join-Path $LocalRoot "rustup"
$JavaHome = Join-Path $LocalRoot "jdk"
$AndroidSdkRoot = Join-Path $LocalRoot "android-sdk"
$AndroidHome = Join-Path $LocalRoot "android-home"
$AndroidAvdHome = Join-Path $AndroidHome "avd"
$AndroidNdkRoot = Join-Path $AndroidSdkRoot "ndk\27.3.13750724"
$PathEntries = @(
    (Join-Path $CargoHome "bin")
    (Join-Path $JavaHome "bin")
    (Join-Path $AndroidSdkRoot "platform-tools")
    (Join-Path $AndroidSdkRoot "emulator")
    (Join-Path $AndroidSdkRoot "cmdline-tools\latest\bin")
)
$ImportedMsvcEnvironment = $false

$env:CARGO_HOME = $CargoHome
$env:RUSTUP_HOME = $RustupHome
$env:JAVA_HOME = $JavaHome
$env:ANDROID_SDK_ROOT = $AndroidSdkRoot
$env:ANDROID_HOME = $AndroidSdkRoot
$env:ANDROID_USER_HOME = $AndroidHome
$env:ANDROID_AVD_HOME = $AndroidAvdHome
$env:ANDROID_NDK_ROOT = $AndroidNdkRoot
$env:CARGO_TARGET_DIR = Join-Path $ProjectRoot "target"
$env:RUST_BACKTRACE = "1"

if (-not (Get-Command cl.exe -ErrorAction SilentlyContinue)) {
    $env:VSCMD_SKIP_SENDTELEMETRY = "1"
    $vsDevCmdPath = Find-VsDevCmdPath
    if ($vsDevCmdPath) {
        $ImportedMsvcEnvironment = Import-BatchEnvironment -BatchPath $vsDevCmdPath -Arguments @("-host_arch=x64", "-arch=x64")
    }
}

$env:PATH = (($PathEntries + @($env:PATH)) -join [IO.Path]::PathSeparator)

New-Item -ItemType Directory -Force -Path $AndroidHome, $AndroidAvdHome | Out-Null

Write-Host "Project environment activated."
Write-Host "CARGO_HOME=$env:CARGO_HOME"
Write-Host "RUSTUP_HOME=$env:RUSTUP_HOME"
Write-Host "JAVA_HOME=$env:JAVA_HOME"
Write-Host "ANDROID_SDK_ROOT=$env:ANDROID_SDK_ROOT"
Write-Host "ANDROID_NDK_ROOT=$env:ANDROID_NDK_ROOT"
if ($ImportedMsvcEnvironment -and $env:VCToolsInstallDir) {
    Write-Host "MSVC tools=$env:VCToolsInstallDir"
}
