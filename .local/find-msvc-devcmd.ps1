$ErrorActionPreference = "Stop"

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
        Write-Output $vsDevCmd
        exit 0
    }

    $vcVars64 = Join-Path $installationPath "VC\Auxiliary\Build\vcvars64.bat"
    if (Test-Path $vcVars64) {
        Write-Output $vcVars64
        exit 0
    }
}
