$ErrorActionPreference = "Stop"

$ProjectRoot = Split-Path -Parent $PSScriptRoot
. (Join-Path $ProjectRoot ".local\activate.ps1") | Out-Null

$requiredCommands = @("rustc", "cargo")
$missingCommands = @()

foreach ($requiredCommand in $requiredCommands) {
    if (-not (Get-Command $requiredCommand -ErrorAction SilentlyContinue)) {
        $missingCommands += $requiredCommand
    }
}

if ($missingCommands.Count -gt 0) {
    throw "Missing local toolchain command(s): $($missingCommands -join ', '). Run 'pwsh .\.local\bootstrap.ps1' first."
}

$steps = @(
    @{
        Name = "cargo fmt"
        Command = "cargo fmt --all -- --check"
    },
    @{
        Name = "cargo clippy"
        Command = "cargo clippy --all-targets --all-features -- -D warnings"
    },
    @{
        Name = "cargo test"
        Command = "cargo test --all-targets --all-features"
    },
    @{
        Name = "cargo doc"
        Command = "cargo doc --all-features --no-deps"
    },
    @{
        Name = "cargo build --release"
        Command = "cargo build --release"
    }
)

foreach ($step in $steps) {
    Write-Host "==> $($step.Name)"
    Invoke-Expression $step.Command
}

$hasAndroidEnv =
    $env:ANDROID_HOME -and
    $env:ANDROID_NDK_ROOT -and
    (Get-Command cargo-apk -ErrorAction SilentlyContinue)

if ($hasAndroidEnv) {
    Write-Host "==> cargo apk build"
    cargo apk build --lib --target aarch64-linux-android
} else {
    Write-Warning "Skipping Android build: ANDROID_HOME / ANDROID_NDK_ROOT / cargo-apk is missing."
}
