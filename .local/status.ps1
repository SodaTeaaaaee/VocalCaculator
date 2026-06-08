$ErrorActionPreference = "Stop"
. "$PSScriptRoot\activate.ps1" | Out-Null

$checks = @(
    @{ Name = "rustc"; Command = "rustc --version" }
    @{ Name = "cargo"; Command = "cargo --version" }
    @{ Name = "cargo-apk"; Command = "cargo-apk apk version" }
    @{ Name = "java"; Command = "java --version" }
    @{ Name = "sdkmanager"; Command = "sdkmanager.bat --version" }
    @{ Name = "adb"; Command = "adb version" }
    @{ Name = "emulator"; Command = "emulator -version" }
)

foreach ($check in $checks) {
    Write-Host "==> $($check.Name)"
    Invoke-Expression $check.Command
}
