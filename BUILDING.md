# Building Vocal Calculator

## Prerequisites

- Windows with Visual Studio / MSVC Build Tools (for `cl` linker)
- Rust 1.96+ (managed by project-local `.local/rustup/`)
- Java 21 (for Android, managed by `.local/jdk/`)
- Android SDK/NDK (for Android builds, managed by `.local/android-sdk/`)

## Quick Start (Desktop)

```powershell
# Activate project environment
. .\.local\activate.ps1

# Build
cargo build --release

# Run
.\target\release\vocal_calculator.exe

# Run tests
cargo test
```

## Android Build

**Status**: The UI is Dioxus-based and the mobile feature is configured. Android
builds still require the project-local Android SDK/NDK environment to be active.

```powershell
. .\.local\activate.ps1
cargo build --features mobile --target aarch64-linux-android
```

## Verification

```powershell
pwsh .agents/verify_vocal_calculator.ps1
```

## Project Structure

```
src/core/       Pure calculator engine (no UI/audio dependencies)
src/audio/      kira-based audio system (normal/broken/music modes)
src/components/ Dioxus UI components
src/styles/     Dioxus WebView CSS
src\ui\         Dioxus signal state and backend bridge
src/app/        App config, identity, platform, and storage support
resource/       Voice WAV assets (embedded at compile time)
.local/         Project-local toolchain (not committed)
```
