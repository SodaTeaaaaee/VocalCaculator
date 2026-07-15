# 项目审查与修复文档索引

当前文档基线日期：2026-07-15。

## 已接受的产品/架构方向

- 这是语音计算器，不建设高摩擦的严格远控授权系统；
- 远控安全重点是输入验证、永不 panic、资源边界、重放和状态正确性；
- 远控 UX 向 LocalSend 靠拢：随机设备名、设备卡片、点击即用、单 executor；
- Windows 是无安装步骤的便携软件，保留纯 LAN listener；
- 正式 LAN 使用固定 TCP/UDP 42420 和固定 firewall rule；
- agent/CI 默认 offline/loopback，不得触发 Windows Firewall prompt。

## 文档

1. [`project-review-2026-07-15.md`](project-review-2026-07-15.md)  
   全项目功能、架构、风险和完成度审查。

2. [`windows-networking-research.md`](windows-networking-research.md)  
   LocalSend、Windows listener、防火墙、MSIX、relay 和 BLE 方案比较，以及最终选择。

3. [`windows-portable-firewall-policy.md`](windows-portable-firewall-policy.md)  
   已接受的 ADR：固定端口、固定规则、便携路径刷新、offline/loopback/lan 模式和 agent 测试约束。

4. [`repair-backlog.md`](repair-backlog.md)  
   带优先级、依赖和验收标准的修复任务清单。

5. [`../AGENTS.md`](../AGENTS.md)  
   自动化 agent 的立即生效约束。offline mode 完成前禁止启动 Windows GUI 或真实 LAN tests。

## 当前重要提醒（2026-07-15 更新）

- `--network-mode=offline`（及等价的环境变量 `VOCAL_CALCULATOR_NETWORK_MODE=offline`）已实现（`src/app/network_mode.rs`），并有单元测试覆盖优先级、非法值和 legacy 配置迁移；但尚未有启动真实 GUI 进程后审计其 OS socket 列表的 smoke test，agent 仍不应把"启动 offline GUI 做 smoke 测试"当作默认可执行动作。
- `cargo test --all-targets` 现在默认安全：`tests/discovery_multicast.rs`、`tests/test_udp_transport.rs` 已归入非默认 Cargo feature `real-network-tests`，且每个测试都额外 `#[ignore]` 并要求 `VOCAL_CALCULATOR_ALLOW_LAN_TESTS=1`，三者缺一都不会触发真实网络流量。
- 默认安全测试入口仍是 `cargo test --locked --lib`，现在 `cargo test --locked --all-targets` 也可以默认运行。
- 防火墙 PowerShell 脚本已实现，位于 `packaging/windows/`（`configure-firewall.ps1`、`remove-firewall.ps1`），支持 `-DryRun`/`-WhatIf`；`-DryRun` 路径 agent 可自行验证，真实创建/删除规则仍需用户显式要求。
- 已实现且有自动化测试的部分：`NetworkMode` 优先级与硬失败、TCP/UDP 固定端口 42420、Windows 禁用 mDNS、offline 组合根门禁、discovery 故障隔离、真实网络测试的三层 opt-in 门禁、防火墙脚本的 dry-run 路径。
- 仍未做真实验证的部分：两台 Windows 实机互相发现/控制、clean（无既有规则）VM 上的真实防火墙提示行为、便携目录实际移动后的规则刷新、Android 与 Windows 的跨平台 multicast/session 互通、AUTO-002（进程级 socket 审计 smoke test，仍未实现）。详见 `docs/repair-backlog.md` 和 `docs/project-review-2026-07-15.md` 文末补充说明。
- Review 和历史验证针对 dirty workspace snapshot，不能自动等同于 HEAD/origin。

