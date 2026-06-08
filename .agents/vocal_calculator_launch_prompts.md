# Vocal Calculator Launch Prompts

更新日期：2026-06-08

本文件提供三种可直接复制的启动提示词，用于让较弱模型按当前 `.agents/` goal 包执行本仓库。

权威入口仍然是：

1. `.agents/vocal_calculator_goal.md`
2. `.agents/project_execution_checklist.md`
3. `.agents/phase1_bootstrap_checklist.md`
4. `.agents/vocal_calculator_subagent_protocol.md`

## 超短版

```text
进入 goal 模式。把 `.agents/vocal_calculator_goal.md` 作为最高优先级目标文件，并继续阅读 `.agents/project_execution_checklist.md`、`.agents/phase1_bootstrap_checklist.md`、`.agents/vocal_calculator_subagent_protocol.md`、`.agents/vocal_calculator_research_sync.md`、`.agents/vocal_calculator_asset_inventory.md` 和 `.agents/vocal_calculator_local_environment.md`。若 `.local/` 缺少工具链或 SDK，先运行 `pwsh .\.local\bootstrap.ps1`；然后激活本地环境（PowerShell: `. .\.local\activate.ps1`，CMD: `call .local\activate.cmd`）。从 Phase 0 和 Phase 1 开始执行，并在无主路径级硬阻塞时自动继续推进到 Phase 8 与最终交付；每个非平凡阶段先并发使用多个 subagent 查官方文档，并把结论同步到 `.agents/sync/`，再实现。默认用户长时间离线；除非桌面主路径在自动 bootstrap、合理修复和可行降级后仍完全不可继续，否则不要停下来等确认，Android、签名、模拟器和外部资产问题只阻塞对应子路径。
```

## 标准版

```text
进入 goal 模式，执行这个仓库。

先检查 `.local/` 是否缺少实际工具链或 SDK 内容；若缺少，先运行：
`pwsh .\.local\bootstrap.ps1`

然后激活项目本地环境：
PowerShell: `. .\.local\activate.ps1`
CMD: `call .local\activate.cmd`

把以下文件视为权威入口，并按顺序阅读和执行：
1. `.agents/vocal_calculator_goal.md`
2. `.agents/project_execution_checklist.md`
3. `.agents/phase1_bootstrap_checklist.md`
4. `.agents/vocal_calculator_subagent_protocol.md`
5. `.agents/vocal_calculator_research_sync.md`
6. `.agents/vocal_calculator_asset_inventory.md`
7. `.agents/vocal_calculator_local_environment.md`

执行要求：
- `.agents/vocal_calculator_goal.md` 是最高优先级，不得擅自偏离技术栈、平台边界、法律边界、音频策略和写边界。
- 默认用户会长时间离线；除非桌面主路径在自动 bootstrap、合理修复、官方文档核实和可行降级后仍完全不可继续，否则不要等待确认，也不要中止整个项目。
- 当前从 Phase 0 和 Phase 1 开始；未完整跑完 `.agents/phase1_bootstrap_checklist.md` 前，不得开始非平凡实现；但完成后若无主路径级硬阻塞，必须自动继续推进 Phase 2 到 Phase 8，直到最终交付完成。
- 每进入一个非平凡阶段，先尽可能并发派出多个 subagent；能并发的任务不要串行化。
- 研究必须优先使用官方文档、官方仓库说明、docs.rs、Rust 官方文档、Slint 官方文档、Android 官方文档；非官方资料只能作补充，不能主导技术决策。
- 每个 subagent 都必须把结论同步到 `.agents/sync/`，主 agent 必须吸收结论后再继续。
- 仅允许修改 `Cargo.toml`、`build.rs`、`src/**`、`ui/**`、`resource/**`、`.github/workflows/**`、`.agents/**`、`.cargo/**` 和必要文档。
- 必须优先复用项目已经配置好的本地 Rust/JDK/Android/cargo-apk/vendor 环境；默认不得新增全局依赖，默认不得新增 Rust crate；若确实必须新增，先证明现有白名单不足，再核实官方文档与许可。
- 必须使用 `Slint + kira + rust_decimal + serde/toml + sysdirs + cargo-apk`，不得改用 Web 前端或第二套 GUI 栈。
- 音乐模式必须保留支持，但外部 `AR7778-digitized-MIDI` 资产不得默认视为可公开再分发；应实现“有本地资产包则启用、无资产包则正确降级”的策略。
- Android、签名、模拟器、外部音乐资产、第三方授权等问题默认只阻塞对应子路径；必须继续推进其他可做部分，并把阻塞写入 `.agents/sync/` 与最终说明。
- 除非遇到主路径级硬阻塞，否则不要先停下来问我；先按 goal 和 checklist 执行。
- 完成前必须运行 `pwsh .agents/verify_vocal_calculator.ps1`，并在最终汇报中明确说明：Windows 桌面是否可运行、Android 是否可构建、哪些验证通过、哪些因外部前置条件被阻塞。
```

## 强约束版

```text
进入 goal 模式，执行这个仓库。

你必须把以下文件视为权威入口，并按顺序阅读和执行：
1. `.agents/vocal_calculator_goal.md`
2. `.agents/project_execution_checklist.md`
3. `.agents/phase1_bootstrap_checklist.md`
4. `.agents/vocal_calculator_subagent_protocol.md`
5. `.agents/vocal_calculator_research_sync.md`
6. `.agents/vocal_calculator_asset_inventory.md`
7. `.agents/vocal_calculator_local_environment.md`

执行前先检查 `.local/` 是否缺少实际工具链或 SDK 内容；若缺少，先运行：
`pwsh .\.local\bootstrap.ps1`

然后激活项目本地环境：
PowerShell: `. .\.local\activate.ps1`
CMD: `call .local\activate.cmd`

强制要求：
- `.agents/vocal_calculator_goal.md` 是最高优先级；不得擅自偏离技术栈、平台边界、法律边界、音频策略、写边界和阶段顺序。
- 默认用户会长时间离线；除非桌面主路径在自动 bootstrap、合理修复、官方文档核实和可行降级后仍完全不可继续，否则不要等待确认，也不要中止整个项目。
- 当前从 Phase 0 和 Phase 1 开始；未完整跑完 `.agents/phase1_bootstrap_checklist.md` 前，不得开始非平凡实现；但完成后若无主路径级硬阻塞，必须自动继续推进 Phase 2 到 Phase 8，直到最终交付完成。
- 每个非平凡阶段开始前，必须先并发派出多个 subagent；能并发的任务不得串行化。
- 研究时必须优先查官方文档、官方仓库说明、docs.rs、Rust 官方文档、Slint 官方文档、Android 官方文档；非官方博客或帖子只能作补充，不能主导技术决策。
- 每个 subagent 都必须把结论同步到 `.agents/sync/`；主 agent 不得只转发结论，必须吸收、冲突消解并写回最终决策。
- 默认不得新增全局依赖或 Rust crate；若确实必须新增，先证明现有白名单不足，再联网核实官方文档与许可。
- 仅允许修改 `Cargo.toml`、`build.rs`、`src/**`、`ui/**`、`resource/**`、`.github/workflows/**`、`.agents/**`、`.cargo/**` 和必要文档。
- 必须使用 `Slint + kira + rust_decimal + serde/toml + sysdirs + cargo-apk`；不得改用 Web 前端、第二套 GUI 栈、第二套音频主栈或浮点核心数值实现。
- 音乐模式必须保留支持，但 `AR7778-digitized-MIDI` 资产不得默认视为可公开再分发；必须实现“有本地资产包则启用、无资产包则正确降级”的策略。
- Android、签名、模拟器、外部音乐资产、第三方授权等问题默认只阻塞对应子路径；必须继续推进其他可做部分，并把阻塞写入 `.agents/sync/` 与最终说明。
- 除非遇到主路径级硬阻塞，否则不要先停下来问我；先按 goal 和 checklist 执行。
- 完成前必须运行 `pwsh .agents/verify_vocal_calculator.ps1`，并在最终汇报中明确说明：Windows 桌面是否可运行、Android 是否可构建、哪些验证通过、哪些因外部前置条件被阻塞。

现在开始执行。
```
