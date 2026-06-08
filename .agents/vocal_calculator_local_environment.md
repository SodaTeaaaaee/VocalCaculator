# Vocal Calculator Local Environment

更新日期：2026-06-08

本文档记录项目约定的本地环境布局与目标版本。
为避免把 10GB 级别的 toolchain / SDK / emulator / user-state 提交进 Git，`.local/` 中只有顶层辅助脚本纳入版本控制；实际工具内容、下载缓存和 Android 用户态目录均为本机本地文件。
如果当前机器尚未准备这些工具，先运行 `.local/bootstrap.ps1` 进行本地初始化；执行模型不得盲目安装新的全局依赖，也不得随意新增 Rust crate。
Windows 桌面编译所需的 Visual Studio / MSVC Build Tools 视为机器级前置条件，激活脚本应自动探测并导入其环境。

## 1. 本地工具根目录

项目级环境根目录约定为 `.local/`：

1. `.local/cargo/`
2. `.local/rustup/`
3. `.local/jdk/`
4. `.local/android-sdk/`
5. `.local/android-home/`

## 2. 目标本地工具版本

### 2.1 Rust

本地 Rust 工具链：

1. `rustc 1.96.0`
2. `cargo 1.96.0`
3. `clippy`
4. `rustfmt`
5. Android targets:
   - `aarch64-linux-android`
   - `x86_64-linux-android`

### 2.2 Cargo 子命令

已安装到项目本地 `CARGO_HOME`：

1. `cargo-apk 0.10.0`

### 2.3 Java

本地 JDK：

1. Eclipse Temurin 21

### 2.4 Android

本地 Android 组件：

1. command-line tools `14742923`
2. `platform-tools`
3. `build-tools;35.0.1`
4. `platforms;android-35`
5. `ndk;27.3.13750724`
6. `emulator`
7. `system-images;android-35;google_apis;x86_64`

## 3. 已提交的项目脚本

1. `.local/activate.ps1`
2. `.local/activate.cmd`
3. `.local/status.ps1`
4. `.local/start-emulator.ps1`
5. `.agents/verify_vocal_calculator.ps1`

## 4. Rust 依赖白名单

当前已钉入 `Cargo.toml` 的依赖：

1. `slint = 1.16.1`
2. `slint-build = 1.16.1`
3. `kira = 0.12.1`
4. `rust_decimal = 1.42.0`
5. `rust_decimal_macros = 1.40.0`
6. `serde = 1.0.228`
7. `toml = 1.1.2`
8. `sysdirs = 0.9.4`
9. `log = 0.4.32`
10. `env_logger = 0.11.10`
11. `android_logger = 0.15.1`
12. `thiserror = 1.0.69`
13. `anyhow = 1.0.102`
14. `proptest = 1.11.0`
15. `rstest = 0.26.1`

## 5. 强制规则

1. 每次工作前必须先激活项目本地环境
2. 如果 `.local/` 工具链缺失，先运行 `.local/bootstrap.ps1`
3. 默认不得新增 Rust 依赖
4. 若确实需要新增依赖，必须先证明现有白名单无法满足需求
5. 默认不得安装新的全局工具
6. Android 构建默认使用项目内 SDK/NDK/JDK
