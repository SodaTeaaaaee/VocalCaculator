# Vocal Calculator Project Execution Checklist

更新日期：2026-06-08

本文档是弱模型执行整个项目时的总清单。
它覆盖从开工到交付的完整路径。

使用方式：

1. 先阅读本文件
2. 再执行 `.agents/phase1_bootstrap_checklist.md`
3. 后续每完成一个阶段，再进入下一阶段
4. 不得跳阶段宣称“项目已完成”

## 1. 总体执行顺序

必须按以下阶段推进：

1. Phase 0: 读文档并冻结边界
2. Phase 1: 环境 bootstrap 与仓库扫描
3. Phase 2: 项目骨架与最小可编译桌面壳
4. Phase 3: 纯计算核心与测试
5. Phase 4: 桌面 UI 接线与正常播报
6. Phase 5: 损坏播报模式
7. Phase 6: Android 构建与基本验证
8. Phase 7: 音乐模式支持层
9. Phase 8: 归因、文档、最终验证与交付

## 2. 全局硬规则

所有阶段都必须遵守：

1. 不得改技术栈
2. 不得新增 GUI 主栈
3. 不得新增音频主栈
4. 不得把核心数值改成浮点实现
5. 不得把短 WAV 做成运行时磁盘读取
6. 不得把损坏语音做成运行时目录扫描或文件名模糊匹配
7. 损坏语音优先使用固定硬编码表
8. 音乐模式不得依赖上游歌曲 MIDI 作为产品必需资源
9. 每个非平凡阶段都必须并发派出子 agent

## 3. Phase 0

目标：

1. 读完 goal 包
2. 理解平台边界
3. 理解法律边界
4. 理解写边界

完成判定：

1. 已读完 `.agents/project_execution_checklist.md`
2. 已读完 `.agents/phase1_bootstrap_checklist.md`
3. 已读完 `.agents/vocal_calculator_goal.md`
4. 已读完 `.agents/vocal_calculator_research_sync.md`
5. 已读完 `.agents/vocal_calculator_subagent_protocol.md`
6. 已读完 `.agents/vocal_calculator_asset_inventory.md`
7. 已读完 `.agents/vocal_calculator_local_environment.md`

## 4. Phase 1

本阶段必须完整执行：

1. `.agents/phase1_bootstrap_checklist.md`

额外完成判定：

1. 第一轮子 agent 已完成
2. 当前仓库状态、资源状态、环境状态都有主 agent 总结
3. 已明确第一笔实现的文件范围

## 5. Phase 2

目标：

1. 建立 Slint 最小桌面窗口
2. 建立共享 Rust 入口结构
3. 保持 Windows 桌面可编译

必须产出：

1. `src/main.rs`
2. `src/lib.rs`
3. 基础 `ui/` 或最小 Slint 入口
4. 最小构建成功路径

完成判定：

1. `cargo build` 通过
2. 桌面最小窗口可运行
3. 未引入第二套 GUI 栈

## 6. Phase 3

目标：

1. 完成纯计算核心
2. 完成百分比规则
3. 完成 MU 规则
4. 完成错误态规则
5. 补测试

必须产出：

1. 纯 Rust 核心模块
2. 单元测试
3. 边界行为测试

完成判定：

1. 核心逻辑不依赖 UI
2. 百分比语义符合 goal
3. MU 语义符合 goal
4. `cargo test` 通过

## 7. Phase 4

目标：

1. 把核心逻辑接到桌面 UI
2. 完成基础按键布局
3. 完成正常播报模式

必须产出：

1. 计算器式桌面界面
2. 显示区与按键交互
3. 正常语音 token 播报链路

完成判定：

1. 按键可以驱动核心逻辑
2. 结果可显示
3. 正常播报工作
4. 快速连续按键不会因音频逻辑卡死

## 8. Phase 5

目标：

1. 完成损坏播报模式
2. 固定损坏音频映射
3. 完成 normal/broken/noise 策略

必须产出：

1. 受版本控制的固定映射实现
2. 对不确定文件的明确降级策略
3. 不依赖运行时目录推断的损坏模式

完成判定：

1. 损坏模式可运行
2. 不会把 token 播成另一个 token 的正确语义
3. 硬编码表或等价受控实现可审计

## 9. Phase 6

目标：

1. 完成 Android 构建
2. 至少一次模拟器或真机基本交互验证

必须产出：

1. Android 构建配置
2. `cargo apk build` 成功路径
3. 基本验证记录

完成判定：

1. `cargo apk build --lib --target aarch64-linux-android` 可通过
2. Android 至少验证一次启动与基本按键交互
3. 未重写为单独 Android UI 逻辑

## 10. Phase 7

目标：

1. 完成音乐模式支持层
2. 缺失外部资产时正确降级

必须产出：

1. 音乐 token 映射逻辑
2. 外部资产存在与否的检测逻辑
3. 缺省禁用/不可用提示

完成判定：

1. 有合法本地资产包时可工作
2. 无资产包时应用仍正常运行
3. 不把未确认授权资产默认打进公开包

## 11. Phase 8

目标：

1. 完成 About/归因
2. 完成第三方说明
3. 完成验证脚本
4. 完成最终交付说明

必须产出：

1. About 页面或等价归因入口
2. 第三方说明文档
3. `.agents/verify_vocal_calculator.ps1`
4. 构建与运行说明

完成判定：

1. `pwsh .agents/verify_vocal_calculator.ps1` 通过或明确记录阻塞项
2. `.agents/sync/` 内已有关键阶段同步记录
3. 交付物与主 goal 一致

## 12. 最终完成定义

只有同时满足以下条件，才允许宣布整个项目完成：

1. 桌面应用可运行
2. 计算逻辑正确
3. 正常播报可工作
4. 损坏播报可工作
5. 音乐模式支持层完成
6. Windows 构建通过
7. Android 构建通过
8. 归因与文档齐全
9. 最终验证通过或明确记录剩余阻塞

## 13. 禁止的假完成

以下情况都不算完成：

1. 只有桌面窗口，没有核心逻辑
2. 只有核心逻辑，没有 UI 接线
3. 只有正常播报，没有损坏播报
4. Android 还没构建，却宣称跨平台完成
5. 音乐模式缺资产降级没做好，却宣称模式已完成
6. 归因与第三方说明缺失
