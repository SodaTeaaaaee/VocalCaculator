$ErrorActionPreference = "Stop"
. "$PSScriptRoot\activate.ps1" | Out-Null

$avdName = "vocalcalc_api35_x86_64"
$emulator = Join-Path $env:ANDROID_SDK_ROOT "emulator\emulator.exe"
$avdConfig = Join-Path $env:ANDROID_AVD_HOME "$avdName.ini"

if (-not (Test-Path $avdConfig)) {
    throw "AVD '$avdName' is not configured. Run the local AVD creation step first."
}

Start-Process -FilePath $emulator -ArgumentList @("-avd", $avdName) -WorkingDirectory $env:ANDROID_SDK_ROOT
