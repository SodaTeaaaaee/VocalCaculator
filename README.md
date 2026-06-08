# Vocal Calculator

主目标文档在 `.agents/`：

1. `.agents/project_execution_checklist.md`
2. `.agents/phase1_bootstrap_checklist.md`
3. `.agents/vocal_calculator_goal.md`
4. `.agents/vocal_calculator_research_sync.md`
5. `.agents/vocal_calculator_subagent_protocol.md`
6. `.agents/vocal_calculator_local_environment.md`

## 仓库约定

1. `.local/` 只提交顶层辅助脚本；其中的 Rust/JDK/Android SDK、模拟器镜像、下载缓存和用户态数据都是机器本地文件，不纳入版本控制。
2. `vendor/` 默认不入库；Rust 依赖由 `Cargo.lock` 固定版本，在本地初始化或 CI 构建时正常联网拉取。
3. 不要把新的全局工具安装结果、`.local/` 大体积内容、`vendor/` 或构建产物提交到仓库。

## 本地环境脚本

仓库提供了项目级环境脚本，默认约定本机工具安装在 `.local/` 下。
第一次使用前先执行初始化脚本，把 Rust / JDK / Android 组件安装到项目本地目录。

PowerShell:

```powershell
pwsh .\.local\bootstrap.ps1
```

CMD:

```bat
call .local\bootstrap.cmd
```

如果 `.local/` 中还没有实际的 toolchain / SDK 内容，请先完成初始化再激活环境。
激活脚本会自动探测本机已安装的 Visual Studio / MSVC Build Tools 并导入桌面构建环境；bootstrap 不负责安装它们。

PowerShell:

```powershell
. .\.local\activate.ps1
```

CMD:

```bat
call .local\activate.cmd
```

环境激活后，默认优先使用本机 `.local/` 下的：

1. 项目内 Rust toolchain
2. 项目内 cargo 子命令
3. 项目内 JDK
4. 项目内 Android SDK / NDK / emulator

## 常用命令

完整验证：

```powershell
pwsh .agents\verify_vocal_calculator.ps1
```

本地环境状态：

```powershell
pwsh .local\status.ps1
```

Android 模拟器：

```powershell
pwsh .local\start-emulator.ps1
```
