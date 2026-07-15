# ADR: Windows 便携版 LAN 与防火墙策略

- 状态：Accepted
- 日期：2026-07-15
- 决策人：项目所有者
- 关联：`docs/windows-networking-research.md`、`docs/repair-backlog.md`

## 1. 决策

Windows 版本继续保留纯局域网发现和入站 session，不引入必须联网的 relay，也不要求安装 MSIX/MSI。

为了兼顾便携软件和无人值守测试：

1. 正式 LAN 协议使用固定端口；
2. 提供可选、显式、幂等的 PowerShell 防火墙配置/清理脚本；
3. 脚本只允许操作者主动运行，应用和 agent 不得静默提权或自动修改防火墙；
4. 所有自动测试默认 offline/loopback，绝不启动真实 LAN listener；
5. 真实 LAN 测试只允许在已经预配置规则的专用机器或 VM 上运行。

## 2. 为什么这样选择

- 项目是便携软件，没有安装阶段，无法像 MSIX 一样在首次启动前可靠预置规则；
- 用户希望保留 LocalSend 类似的无服务器、同一局域网自动发现体验；
- Windows 官方说明，监听应用若没有 inbound allow rule，可能触发交互式防火墙提示；
- 自动化若启动正常 GUI，会在无人值守时卡在提示上；
- 让 agent 自动执行管理员 PowerShell 既不可靠，也不应成为测试前置条件。

因此把“真实 LAN”和“自动验证”分成两个显式运行模式，而不是尝试检测 agent 或自动点击系统对话框。

## 3. 固定网络契约

### 3.1 目标端口

目标状态使用同一个数值端口、不同传输协议：

| 用途 | 协议 | 本地端口 | 地址范围 |
|---|---|---:|---|
| Session listener | TCP | 42420 | LocalSubnet |
| 自定义发现 | UDP | 42420 | `224.0.0.167` / LocalSubnet |

这样只需要两条 inbound rule，并与 LocalSend“同一 TCP/UDP 端口”的部署形态相似。端口在首个稳定协议版本中不提供 UI 修改，避免配置和规则漂移。

### 3.2 mDNS

当前 Windows 构建还使用 mDNS，通常涉及 UDP 5353。目标方案建议在 Windows 便携版禁用 mDNS，仅保留现有自定义 multicast discovery：

- 减少第三条防火墙规则；
- 规则和故障诊断更确定；
- Android 已有 multicast lock 支持；
- LocalSend 风格的主动 announce/response 可以在自定义 UDP 协议中完成。

如果实际测试证明单一 multicast 在目标网络中不足，再明确增加名为 `VocalCalculator.Portable.mDNS.UDP-In` 的第三条规则；不能让 mDNS 隐式扩张端口面。

### 3.3 Profile 和来源范围

默认规则必须同时满足：

- `Direction=Inbound`
- `Action=Allow`
- `Profile=Private`
- `RemoteAddress=LocalSubnet`
- `EdgeTraversalPolicy=Block`
- 同时限定 executable 的绝对路径和固定本地端口

默认不开放 Public profile。Domain profile 由企业管理员决定，不由便携脚本擅自添加。

## 4. 固定规则标识

使用稳定、非本地化的 `Name`，显示名可本地化：

| Name | DisplayName |
|---|---|
| `VocalCalculator.Portable.TCP-In` | Vocal Calculator Portable (TCP-In) |
| `VocalCalculator.Portable.UDP-In` | Vocal Calculator Portable (UDP-In) |

脚本必须按 `Name` 查找和删除，不能依赖可能本地化的 `DisplayName`。

## 5. 便携路径生命周期

Windows application rule 不支持通配路径，只能绑定 executable 的完整路径。便携目录移动后，旧规则不会自动跟随。

因此：

- `configure-firewall.ps1` 每次从 `$PSScriptRoot` 解析当前 exe 绝对路径；
- 创建前按固定 `Name` 删除旧规则；
- 在新路径重建相同两条规则；
- `remove-firewall.ps1` 只按固定 `Name` 删除本项目规则；
- UI 的网络设置页应显示当前 exe 路径和规则状态，并提示“移动软件后重新运行配置脚本”。

应用不能自行提权，也不能在启动时自动运行脚本。

## 6. PowerShell 实现现状

脚本已实现，位于 `packaging/windows/`：`configure-firewall.ps1`、`remove-firewall.ps1`，以及非 Pester 的 `tests/firewall-scripts.tests.ps1` 测试脚本。行为摘要：

- `configure-firewall.ps1`：`[CmdletBinding(SupportsShouldProcess)]`，参数 `-ExecutablePath`（默认 `$PSScriptRoot\vocal-calculator-app.exe`）、`-DryRun`。先按固定 `Name` 删除同名旧规则（`SilentlyContinue`），再用 `New-NetFirewallRule` 建立 TCP/UDP 两条规则（Direction Inbound、Action Allow、Profile Private、RemoteAddress LocalSubnet、EdgeTraversalPolicy Block、LocalPort 42420、Program 为解析后的绝对路径），随后逐项读取 rule 及其 application/port/address filter 校验，任一项不符即非零退出并给出中文错误列表。
- `remove-firewall.ps1`：只按固定 `Name` 删除两条规则；规则本就不存在不算错误。
- 两个脚本的 `-DryRun`/`-WhatIf` 路径完全等价，统一在最前面短路：不做管理员检查、不调用任何 `Get/New/Remove-NetFirewallRule`，只打印计划并 `exit 0`。
- `tests/firewall-scripts.tests.ps1`：只以 `-DryRun` 调用两个脚本，23 项断言，全部通过（非管理员环境下已验证）。

### ADR 修订（2026-07-15）

原第 4 版决策文本要求脚本使用 `#Requires -RunAsAdministrator`。实际实现改为运行时自检，理由：`#Requires -RunAsAdministrator` 会让 PowerShell 在脚本最顶层直接拒绝以非管理员身份加载/解析该脚本，这也会连带拒绝 `-DryRun`/`-WhatIf` 这类只读路径——而 agent 和普通用户都需要能够在非管理员环境下安全跑一次 dry-run 来验证生成的参数。因此两个脚本都不带该 `#Requires` 指令，而是在真正执行修改前，用 `[Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)` 自行判断；非管理员时输出中文错误并 `exit 1`，不做静默自提升。dry-run 路径完全绕开这个检查，因为它不读取也不写入任何防火墙状态。

这一修订视为对本 ADR 第 3 节"实现要求"的技术修正，其余要求（幂等刷新、逐项校验、非零退出码、不动全局 profile、不用 `netsh advfirewall reset`）均已按原计划实现，未变更。

## 7. 运行模式

### 7.1 `lan`

正式用户模式：启动固定 TCP/UDP listener 和 discovery。若规则缺失，Windows 仍可能提示；应用应在启动 LAN 前显示自身说明，不尝试代替系统点击。

### 7.2 `offline`

本地计算器和音频正常，整个 network runtime 不创建。不是“创建 socket 但不发消息”，而是从组合根处不构造 NetworkManager。

已实现接口（`src/app/network_mode.rs`、`src/main.rs`、`src/ui/bridge.rs`）：

- CLI：`--network-mode=offline`（或 `--network-mode offline` 两个 token 形式）
- 环境变量：`VOCAL_CALCULATOR_NETWORK_MODE=offline`
- 配置：`NetworkConfig.mode: Option<String>`，值为 `"lan"` / `"offline"` / `"loopback-test"`

优先级：CLI > environment > config > 旧版 `network.enabled` 字段回退（`true`→Lan，`false`→Offline，仅当 `mode` 字段确实缺失时生效）。任一层出现非法值都是硬 `Err`，`main()` 以 exit code 2 终止并打印中文错误，绝不会静默回退到 Lan。`ui::bridge::init_networking` 用 `network_mode::current() == NetworkMode::Offline` 门禁，命中时在 `NetworkManager::new`/任何 socket 调用之前直接返回。

已验证：单元测试覆盖 CLI/env/config 三层优先级和非法值场景（`cargo test --locked --lib`，328 通过）。**未验证**：尚无端到端"启动 offline 进程后审计其真实 OS socket 列表"的 smoke test（`docs/repair-backlog.md` AUTO-002 仍是待办），当前的保证停留在组合根调用路径的静态/单元测试层面。

### 7.3 `loopback-test`

网络状态机测试模式，只能绑定 `127.0.0.1`/`::1` 和 OS 分配的临时端口；不启动 mDNS/multicast，不访问真实网卡。

已实现：`session_bind_addr(NetworkMode::LoopbackTest)`（`src/net/runtime.rs`）返回 `127.0.0.1:0`；discovery 任务在 `mode != NetworkMode::Lan` 时整体跳过构造。单元测试覆盖该分支。当前 `LoopbackTest` 在 `init_networking` 里走的是与 `Lan` 相同的"启动 networking"分支，只是运行时内部按 `session_bind_addr`/discovery 门禁收敛到 loopback；尚未有独立于 `Lan` 的组合根级别开关暴露给 UI/CLI 之外的调用方，这是已知的后续收尾项，不是缺陷。

### 7.4 `lan-test`

真实网络集成测试模式，必须同时满足：

- `VOCAL_CALCULATOR_ALLOW_LAN_TESTS=1`；
- 测试机已经由操作者预配置规则；
- 测试任务明确标记为 ignored/manual；
- 不能在普通 `cargo test`、CI 或 agent 默认验证中运行。

## 8. Agent/CI 防弹窗规则

无人值守验证分层：

| 层级 | 内容 | 可否默认运行 |
|---|---|---:|
| T0 | fmt、Clippy、`cargo test --lib`、build | 是 |
| T1 | Calculator/audio/storage 纯逻辑 | 是 |
| T2 | loopback session/runtime tests | 是 |
| T3 | GUI smoke，强制 offline | offline 实现后可以 |
| T4 | `discovery_multicast`、`test_udp_transport`、mDNS/真实 LAN | 否，人工预配置后 opt-in |
| T5 | 创建/删除 firewall rules | 否，必须用户明确要求 |

仓库根目录 `AGENTS.md` 固化了这些约束。offline mode 实现之前，agent 不得启动 Windows GUI executable。

## 9. 应用 UX

网络设置页建议提供：

- 网络模式：关闭 / 局域网；
- 固定端口只读展示：42420 TCP+UDP；
- 当前网络 profile；
- “防火墙规则未配置/路径不匹配/已配置”状态；
- “打开脚本所在目录”按钮，而不是“自动提权安装规则”；
- 便携目录移动后的明确修复提示；
- 扫描和连接错误不得阻止本地计算器启动。

## 10. 验收标准

状态标注口径：**已实现并验证（自动化）** = 有自动化测试实测通过；**已实现但未做真实 Windows LAN / 双机 / clean-VM 验证** = 代码/脚本已落地，但需要真实网卡、真实防火墙提示或多机环境才能验证的部分尚未做；**仍未实现** = 代码或测试都不存在。

### 自动化安全

1. `cargo test --locked --lib` 不启动 LAN listener——**已实现并验证（自动化）**；328 项测试通过，未产生 LAN socket。
2. 默认 CI 不运行当前两个真实 multicast/broadcast integration targets——**已实现并验证（自动化）**；两个 target 已归入非默认 `real-network-tests` feature，`cargo test --locked --all-targets -- --list` 确认默认 feature 下两个测试二进制均为 0 tests。
3. offline GUI smoke 不产生非 loopback socket——**已实现但未做真实 Windows LAN / 双机 / clean-VM 验证**；组合根门禁和 `session_bind_addr`/discovery 跳过路径有单元测试覆盖，但没有启动真实 GUI 进程审计其 OS socket 列表的 smoke test（`repair-backlog.md` AUTO-002 仍未实现）。
4. 普通 agent 工作流不会执行任何 NetSecurity mutation command——**已实现并验证（自动化）**；`AGENTS.md` 已固化该约束，本次验证过程只用过 `-DryRun`/`-WhatIf`。
5. 无人值守验证不会出现 Windows Security Alert——**已实现但未做真实 Windows LAN / 双机 / clean-VM 验证**；本机开发环境的自动化验证未触发过任何提示，但没有在 clean（无既有规则的）Windows VM 上实测过。

### 规则正确性

1. 两条规则 Name 稳定且可幂等刷新——**已实现并验证（自动化）**；dry-run 测试断言了两个固定 Name 出现在计划输出中。
2. 仅 Private + LocalSubnet——**已实现并验证（自动化）**；dry-run 断言计划中出现 Private/LocalSubnet，且不出现 Public/Domain/`RemoteAddress: Any`。
3. 仅当前 exe 绝对路径——**已实现并验证（自动化）**；dry-run 断言计划中包含解析后的绝对路径。
4. 仅 TCP/UDP 42420——**已实现并验证（自动化）**；dry-run 断言计划中包含 TCP、UDP、42420。
5. Public profile 保持关闭——**已实现并验证（自动化）**；同上，dry-run 断言其不出现。
6. remove 脚本不删除任何其他规则——**已实现但未做真实 Windows LAN / 双机 / clean-VM 验证**；脚本按精确 `Name` 删除，逻辑上不会波及其他规则，但未实测过一台装有其他自定义规则的真实 Windows 机器上跑 remove 后再核对规则表。
7. 软件移动后重新配置会删除旧路径规则并建立新路径规则——**仍未实现（验证层面）**；`configure-firewall.ps1` 每次都按 `$PSScriptRoot`/`-ExecutablePath` 重新解析路径并先删后建，逻辑已实现，但"移动目录后重新运行脚本"这一场景从未在真实 Windows 环境里实际操作验证过。

### 功能

1. 预配置规则后，两台 Windows 可互相发现和控制——**仍未实现（验证层面）**；代码路径（固定 42420、门禁逻辑）已就绪，但从未有过真实双机 LAN 测试。
2. 未配置规则时失败信息清楚，本地计算器仍可用——**已实现并验证（自动化）**；`runtime.rs` 绑定失败会产出中文状态信息并让本地计算器继续运行，单元测试覆盖 bind 失败路径。
3. Android 与 Windows multicast/session 互通——**仍未实现（验证层面）**；`src/net/android.rs` 中 multicast lock 改为调用公开 `WifiManager.createMulticastLock` API 是任务开始前已存在于工作区的未提交修改，并非本次任务所做，跨平台真机互通仍从未测试过。
4. discovery 失败不关闭 Calculator 或整个 network supervisor——**已实现并验证（自动化）**；discovery `JoinHandle` 不再纳入顶层 `select!`，任务退出只记录 warning，listener/router/command 任务继续运行，单元测试和代码走查确认。

## 11. 官方依据

- Windows Firewall 应用规则与完整路径限制：<https://learn.microsoft.com/windows/security/operating-system-security/network-security/windows-firewall/rules>
- 创建规则的通用建议：<https://learn.microsoft.com/windows/security/operating-system-security/network-security/windows-firewall/configure>
- `Remove-NetFirewallRule`：<https://learn.microsoft.com/powershell/module/netsecurity/remove-netfirewallrule>
- LocalSubnet 和 Private profile 建议：<https://learn.microsoft.com/previous-versions/windows/desktop/ics/windows-firewall-profiles>
- LocalSend protocol：<https://github.com/localsend/protocol>
