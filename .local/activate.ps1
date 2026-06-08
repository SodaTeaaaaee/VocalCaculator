$ErrorActionPreference = "Stop"

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
$env:PATH = (($PathEntries + @($env:PATH)) -join [IO.Path]::PathSeparator)

New-Item -ItemType Directory -Force -Path $AndroidHome, $AndroidAvdHome | Out-Null

Write-Host "Project environment activated."
Write-Host "CARGO_HOME=$env:CARGO_HOME"
Write-Host "RUSTUP_HOME=$env:RUSTUP_HOME"
Write-Host "JAVA_HOME=$env:JAVA_HOME"
Write-Host "ANDROID_SDK_ROOT=$env:ANDROID_SDK_ROOT"
Write-Host "ANDROID_NDK_ROOT=$env:ANDROID_NDK_ROOT"
