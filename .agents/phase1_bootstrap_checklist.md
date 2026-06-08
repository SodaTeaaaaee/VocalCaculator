# Vocal Calculator Phase 1 Bootstrap Checklist

更新日期：2026-06-08

本文档是弱模型开工时必须先执行的第一阶段清单。
它是 `.agents/project_execution_checklist.md` 的 Phase 1 细化版。
目标不是“立刻做功能”，而是先把环境、边界、仓库现状和第一步实施范围固定住。

如果本清单与其他文档冲突，以 `.agents/vocal_calculator_goal.md` 为准；
但如果还没跑完本清单，不得开始非平凡实现。

## 1. 本阶段目标

完成以下事情：

1. 确认本地环境可用
2. 确认仓库状态干净且可读
3. 确认核心依赖可解析
4. 确认资源目录与 goal 包一致
5. 并发派出第一轮子 agent
6. 明确第一笔实现只做什么，不做什么

## 2. 开工前硬规则

在本阶段内：

1. 不得新增 Rust 依赖
2. 不得改技术栈
3. 不得重写为 Web UI
4. 不得开始音乐模式实现
5. 不得开始 Android UI 特化
6. 不得把损坏语音做成运行时目录扫描或文件名模糊匹配
7. 损坏语音策略默认使用固定硬编码表

## 3. 必跑步骤

按顺序执行：

1. 阅读以下文件：
   - `.agents/phase1_bootstrap_checklist.md`
   - `.agents/vocal_calculator_goal.md`
   - `.agents/vocal_calculator_research_sync.md`
   - `.agents/vocal_calculator_subagent_protocol.md`
   - `.agents/vocal_calculator_asset_inventory.md`
   - `.agents/vocal_calculator_local_environment.md`
2. 若 `.local/` 中缺少实际工具链或 SDK 内容，先运行：
   - `pwsh .\.local\bootstrap.ps1`
3. 激活项目环境：
   - PowerShell: `. .\.local\activate.ps1`
   - CMD: `call .local\activate.cmd`
4. 运行环境状态检查：
   - `pwsh .\.local\status.ps1`
5. 至少确认下列命令可用：
   - `rustc`
   - `cargo`
   - `cargo-apk`
   - `java`
   - `cl`
   - `sdkmanager`
6. 检查依赖解析是否正常：
   - `cargo metadata --locked --format-version 1`
7. 检查工作区状态：
   - `git status --short --ignored`
8. 检查最关键文件与目录是否存在：
   - `Cargo.toml`
   - `Cargo.lock`
   - `.cargo/config.toml`
   - `src/`
   - `resource/Vocal/Normal`
   - `resource/Vocal/Broken`
   - `.agents/`
9. 按 `.agents/vocal_calculator_subagent_protocol.md` 派出第一轮子 agent，至少覆盖：
   - 代码与目录现状扫描
   - 资源与音频资产扫描
   - 构建环境与平台依赖扫描
10. 要求每个子 agent 把结论同步到 `.agents/sync/`
11. 主 agent 吸收子 agent 结论后，先写一段简短现状总结，再开始实现

## 4. 第一笔实现的推荐范围

本项目的第一笔实现应只做下面这一小段：

1. 确认 Slint 最小桌面窗口可编译
2. 确认共享 Rust 入口结构成立
3. 建立纯计算核心模块骨架
4. 建立音频模块骨架
5. 不深入音乐模式
6. 不做损坏语音的复杂外置配置系统

## 5. 第一笔实现明确不要做的事

不要在第一笔实现里做这些：

1. 不要同时做完整 UI、完整音频、完整 Android
2. 不要引入新的 GUI 栈
3. 不要引入新的音频主栈
4. 不要写运行时文件名推断逻辑
5. 不要为了“灵活”先建大而全配置系统
6. 不要把损坏语音映射外置成必须依赖的 TOML
7. 不要在未验证桌面链路前深挖 Android 细节

## 6. 本阶段完成判定

只有同时满足以下条件，才算完成第一阶段：

1. `bootstrap` 已完成或确认无需执行
2. `activate` 成功
3. `status` 成功
4. `cargo metadata --locked --format-version 1` 成功
5. 第一轮子 agent 已完成并留下同步文件
6. 主 agent 已明确下一步只改哪些文件
7. 尚未发生技术栈漂移、依赖漂移或目标漂移

## 7. 本阶段停止条件

遇到以下情况必须先停并报告：

1. `cargo` 不可用
2. `cl` 不可用
3. `java` 或 Android SDK 关键命令缺失
4. `cargo metadata --locked` 失败
5. 资源目录与资产清单不一致
6. 子 agent 同步文件缺失
