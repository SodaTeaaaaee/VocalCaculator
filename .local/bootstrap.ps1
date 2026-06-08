param(
    [switch]$SkipAvd
)

$ErrorActionPreference = "Stop"

$ProjectRoot = Split-Path -Parent $PSScriptRoot
. (Join-Path $PSScriptRoot "activate.ps1") | Out-Null

$DownloadsRoot = Join-Path $ProjectRoot ".local\downloads"
$RustupInit = Join-Path $DownloadsRoot "rustup-init.exe"
$JdkArchive = Join-Path $DownloadsRoot "OpenJDK21U-jdk_x64_windows_hotspot.zip"
$AndroidToolsArchive = Join-Path $DownloadsRoot "commandlinetools-win-14742923_latest.zip"
$RustupUrl = "https://win.rustup.rs/x86_64"
$Temurin21Url = "https://api.adoptium.net/v3/binary/latest/21/ga/windows/x64/jdk/hotspot/normal/eclipse"
$AndroidToolsUrl = "https://dl.google.com/android/repository/commandlinetools-win-14742923_latest.zip"
$AvdName = "vocalcalc_api35_x86_64"
$DebugKeystore = Join-Path $env:ANDROID_USER_HOME "debug.keystore"

New-Item -ItemType Directory -Force -Path $DownloadsRoot | Out-Null

function Write-Step {
    param([string]$Message)
    Write-Host "==> $Message"
}

function Download-File {
    param(
        [string]$Url,
        [string]$Destination
    )

    if (Test-Path $Destination) {
        Write-Host "Using cached download: $Destination"
        return
    }

    Write-Step "Downloading $Url"
    Invoke-WebRequest -Uri $Url -OutFile $Destination
}

function Copy-DirectoryContent {
    param(
        [string]$Source,
        [string]$Destination
    )

    New-Item -ItemType Directory -Force -Path $Destination | Out-Null
    Get-ChildItem -LiteralPath $Destination -Force -ErrorAction SilentlyContinue | Remove-Item -Recurse -Force
    Copy-Item -Path (Join-Path $Source "*") -Destination $Destination -Recurse -Force
}

function Expand-ArchiveToTempRoot {
    param([string]$ArchivePath)

    $TempRoot = Join-Path $env:TEMP ("vocalcalc-bootstrap-" + [guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Force -Path $TempRoot | Out-Null
    Expand-Archive -LiteralPath $ArchivePath -DestinationPath $TempRoot -Force
    return $TempRoot
}

function Install-RustToolchain {
    $RustcPath = Join-Path $env:CARGO_HOME "bin\rustc.exe"
    if (-not (Test-Path $RustcPath)) {
        Download-File -Url $RustupUrl -Destination $RustupInit
        Write-Step "Installing Rust toolchain into .local"
        & $RustupInit -y --no-modify-path --default-toolchain stable --profile default
    } else {
        Write-Host "Rust toolchain already present."
    }

    $env:PATH = ((Join-Path $env:CARGO_HOME "bin") + [IO.Path]::PathSeparator + $env:PATH)

    Write-Step "Ensuring Rust components and Android targets"
    rustup component add clippy rustfmt
    rustup target add aarch64-linux-android x86_64-linux-android
}

function Install-CargoApk {
    if (Get-Command cargo-apk -ErrorAction SilentlyContinue) {
        Write-Host "cargo-apk already present."
        return
    }

    Write-Step "Installing cargo-apk"
    cargo install cargo-apk --version 0.10.0 --locked
}

function Install-Jdk {
    $JavaExe = Join-Path $env:JAVA_HOME "bin\java.exe"
    if (Test-Path $JavaExe) {
        Write-Host "JDK already present."
        return
    }

    Download-File -Url $Temurin21Url -Destination $JdkArchive
    Write-Step "Installing Temurin 21 into .local\\jdk"

    $TempRoot = Expand-ArchiveToTempRoot -ArchivePath $JdkArchive
    try {
        $JavaBinary = Get-ChildItem -Path $TempRoot -Recurse -Filter java.exe | Select-Object -First 1
        if (-not $JavaBinary) {
            throw "Unable to locate java.exe inside extracted JDK archive."
        }

        $JdkRoot = Split-Path $JavaBinary.Directory.FullName -Parent
        Copy-DirectoryContent -Source $JdkRoot -Destination $env:JAVA_HOME
    }
    finally {
        Remove-Item -LiteralPath $TempRoot -Recurse -Force
    }
}

function Install-AndroidCommandLineTools {
    $SdkManager = Join-Path $env:ANDROID_SDK_ROOT "cmdline-tools\latest\bin\sdkmanager.bat"
    if (Test-Path $SdkManager) {
        Write-Host "Android command-line tools already present."
        return
    }

    Download-File -Url $AndroidToolsUrl -Destination $AndroidToolsArchive
    Write-Step "Installing Android command-line tools"

    $TempRoot = Expand-ArchiveToTempRoot -ArchivePath $AndroidToolsArchive
    try {
        $SdkManagerBinary = Get-ChildItem -Path $TempRoot -Recurse -Filter sdkmanager.bat | Select-Object -First 1
        if (-not $SdkManagerBinary) {
            throw "Unable to locate sdkmanager.bat inside extracted Android tools archive."
        }

        $ToolsRoot = Split-Path $SdkManagerBinary.Directory.FullName -Parent
        $LatestToolsRoot = Join-Path $env:ANDROID_SDK_ROOT "cmdline-tools\latest"
        Copy-DirectoryContent -Source $ToolsRoot -Destination $LatestToolsRoot
    }
    finally {
        Remove-Item -LiteralPath $TempRoot -Recurse -Force
    }
}

function Install-AndroidPackages {
    $SdkManager = Join-Path $env:ANDROID_SDK_ROOT "cmdline-tools\latest\bin\sdkmanager.bat"
    $PackageChecks = @(
        @{ Package = "platform-tools"; Path = (Join-Path $env:ANDROID_SDK_ROOT "platform-tools\adb.exe") }
        @{ Package = "build-tools;35.0.1"; Path = (Join-Path $env:ANDROID_SDK_ROOT "build-tools\35.0.1\aapt.exe") }
        @{ Package = "platforms;android-35"; Path = (Join-Path $env:ANDROID_SDK_ROOT "platforms\android-35\android.jar") }
        @{ Package = "ndk;27.3.13750724"; Path = (Join-Path $env:ANDROID_SDK_ROOT "ndk\27.3.13750724\source.properties") }
        @{ Package = "emulator"; Path = (Join-Path $env:ANDROID_SDK_ROOT "emulator\emulator.exe") }
        @{ Package = "system-images;android-35;google_apis;x86_64"; Path = (Join-Path $env:ANDROID_SDK_ROOT "system-images\android-35\google_apis\x86_64\package.xml") }
    )

    $MissingPackages = @(
        $PackageChecks |
            Where-Object { -not (Test-Path $_.Path) } |
            ForEach-Object { $_.Package }
    )

    if ($MissingPackages.Count -eq 0) {
        Write-Host "Android SDK packages already present."
        return
    }

    Write-Step "Accepting Android SDK licenses"
    1..200 | ForEach-Object { "y" } | & $SdkManager "--sdk_root=$env:ANDROID_SDK_ROOT" --licenses | Out-Null

    Write-Step "Installing Android SDK packages"
    & $SdkManager "--sdk_root=$env:ANDROID_SDK_ROOT" @MissingPackages
}

function Ensure-AndroidDebugKeystore {
    if (Test-Path $DebugKeystore) {
        Write-Host "Android debug keystore already present."
        return
    }

    $Keytool = Join-Path $env:JAVA_HOME "bin\keytool.exe"
    if (-not (Test-Path $Keytool)) {
        throw "keytool.exe is missing. JDK installation did not complete successfully."
    }

    Write-Step "Generating Android debug keystore"
    & $Keytool `
        -genkeypair `
        -alias androiddebugkey `
        -keyalg RSA `
        -keysize 2048 `
        -validity 10000 `
        -keystore $DebugKeystore `
        -storepass android `
        -keypass android `
        -dname "CN=Android Debug,O=Android,C=US"
}

function Ensure-AndroidAvd {
    if ($SkipAvd) {
        Write-Host "Skipping AVD creation."
        return
    }

    $AvdConfig = Join-Path $env:ANDROID_AVD_HOME ($AvdName + ".ini")
    if (Test-Path $AvdConfig) {
        Write-Host "Android AVD already present."
        return
    }

    $AvdManager = Join-Path $env:ANDROID_SDK_ROOT "cmdline-tools\latest\bin\avdmanager.bat"
    if (-not (Test-Path $AvdManager)) {
        throw "avdmanager.bat is missing. Android command-line tools were not installed correctly."
    }

    Write-Step "Creating Android AVD $AvdName"
    "no" | & $AvdManager create avd `
        -n $AvdName `
        -k "system-images;android-35;google_apis;x86_64" `
        --force
}

Write-Step "Bootstrapping project-local toolchain"
Install-RustToolchain
Install-CargoApk
Install-Jdk
Install-AndroidCommandLineTools
Install-AndroidPackages
Ensure-AndroidDebugKeystore
Ensure-AndroidAvd

if (-not (Get-Command cl.exe -ErrorAction SilentlyContinue)) {
    Write-Warning "MSVC build tools were not found on PATH. Windows desktop builds may still fail until Visual Studio Build Tools are installed."
}

Write-Step "Bootstrap complete"
Write-Host "Next step: pwsh .\\.local\\status.ps1"
