# Windows 网络与防火墙方案调研

日期：2026-07-15  
结论状态：已由项目所有者选择“便携版 + 纯 LAN + 固定规则”  
执行策略：见 `docs/windows-portable-firewall-policy.md`

## 1. 调研问题

目标体验接近 LocalSend：无账号、附近设备自动出现、点击即可连接、局域网内直接通信。同时 Windows 版本是便携软件，没有安装阶段，并且无人值守 agent 测试不能卡在 Windows Defender Firewall 对话框上。

这包含三个不同问题：

1. 正式用户是否允许应用监听 LAN；
2. 防火墙规则如何在没有 installer 的情况下管理；
3. 自动测试如何保证永远不触发交互式系统提示。

## 2. 平台事实

### 2.1 Windows 对 listener 的处理

Microsoft 文档说明：网络应用调用 listen 后，Windows Firewall 默认 inbound block；如果没有已有 allow/block rule，系统可能提示管理员用户。非管理员用户即使看到提示也不能可靠创建 allow rule。

因此以下做法都不能保证消除提示：

- 使用随机 TCP 端口；
- 把 TCP 换成 UDP；
- 只使用 mDNS/multicast；
- 更换 Rust socket 库；
- 让 agent 等待或尝试点击系统对话框。

可靠做法只有：listen 前已有匹配规则，或者自动化根本不创建 LAN listener。

### 2.2 便携路径限制

Windows application firewall rule 只能绑定 executable 的完整路径，不支持 `C:\Portable\*\app.exe` 一类通配。软件目录移动后，旧规则仍指向旧路径。

所以便携版不能把规则视为一次性安装状态，必须提供“按固定 Name 删除旧规则，再按当前 `$PSScriptRoot` 重建”的刷新流程。

### 2.3 LocalSend 的实际网络模型

LocalSend 官方协议默认使用同一个 53317 数值端口承载 TCP 和 UDP discovery，并使用 `224.0.0.167` multicast。LocalSend 官方 README 也明确提醒防火墙需要允许入站 TCP/UDP。

值得借鉴的是：

- 固定 TCP/UDP 端口，规则容易解释；
- 随机设备名和设备卡片降低连接认知成本；
- 协议只要求一方运行 server；
- multicast 失败时可以主动扫描/注册。

不值得照搬的是把防火墙配置问题隐藏成“应该开箱即用”。便携版必须显式说明规则状态和移动目录后的刷新方式。

## 3. 本项目当前实现（2026-07-15 更新）

第 5-9 节的方案已落地为代码，状态如下（自动化验证包含 `cargo test --locked --lib`；未做真实 Windows LAN / 双机 / clean-VM 验证的部分单独标注）：

- `NetworkMode::{Lan, Offline, LoopbackTest}` 已实现（`src/app/network_mode.rs`），CLI `--network-mode` > 环境变量 `VOCAL_CALCULATOR_NETWORK_MODE` > `config.mode` > 旧版 `enabled` 回退优先级已实现并有单元测试；显式非法字符串为硬 `Err`，`main()` 以 exit code 2 终止。缺失配置文件仍使用产品默认 Lan，现存但不可读、损坏或字段类型错误的配置 fail-closed 为 Offline；未初始化全局模式也为 Offline——**已实现并验证（自动化）**。
- TCP session listener：`Lan` 模式下固定绑定 `0.0.0.0:42420`（`session_bind_addr`，`src/net/runtime.rs`），不再是 OS 随机端口；`LoopbackTest` 绑定 `127.0.0.1:0`——**已实现并验证（自动化）**。
- 自定义 UDP multicast：仍是 `224.0.0.167:42420`，`42420` 现在只有 `LAN_FIXED_PORT`（`src/net/protocol.rs`）一个权威字面量来源，`DISCOVERY_PORT`/`SESSION_TCP_PORT` 均为其别名——**已实现并验证（自动化）**。
- mDNS：Windows 上已禁用（`should_start_mdns(target_os) = target_os != "windows"`，`src/net/discovery/mod.rs`），非 Windows 平台（含 Android）保持启用——**已实现并验证（自动化）**；跨平台真实互通（Windows↔Android）**仍未做真实验证**。
- discovery 不再发布随机 session port，announce 消息携带固定 `SESSION_TCP_PORT`——**已实现并验证（自动化）**。
- `NetworkConfig::default()` 的旧 `enabled = true` 字段仍存在（向后兼容），新增 `mode: Option<String>` 字段作为优先数据源——**已实现并验证（自动化）**。
- `Offline` 模式下组合根（`ui::bridge::init_networking`）在构造 `NetworkManager` 之前直接返回；`run_network_runtime` 内部也有一层防御性重复检查——**代码门禁已实现但未做进程级验证**：模式和底层地址选择有单元测试，没有直接组合根 constructor-spy 测试，也没有启动真实 GUI 进程后审计其操作系统 socket 列表的 smoke test。
- 防火墙 dry-run/configure/remove 脚本已实现于 `packaging/windows/`，非 admin 环境下 `-DryRun`/`-WhatIf` 已实测通过（43 项断言，全部 NetSecurity cmdlet 由 fail-fast mock 遮蔽）；真实（非 dry-run）创建/删除规则、clean VM 上的提示行为、便携目录移动后的规则刷新——**仍未做真实 Windows LAN / 双机 / clean-VM 验证**。
- discovery task 的 `JoinHandle` 不再纳入顶层 `select!`，静态控制流显示其退出不会直接拖垮 listener/router/command 任务——**代码已实现，直接故障注入测试未实现**。
- `tests/discovery_multicast.rs`、`tests/test_udp_transport.rs` 已归入非默认 Cargo feature `real-network-tests`，且测试本身额外 `#[ignore]` 并要求 `VOCAL_CALCULATOR_ALLOW_LAN_TESTS=1`——**已实现并验证（自动化）**；真实 LAN 环境下这些测试的实际运行结果**仍未验证**（本次任务未设置该环境变量、未启用该 feature 运行）。

真正跨越"这是文档设想"到"这是已提交代码"边界后仍然为空的部分：两台 Windows 实机互相发现/控制、clean（无既有规则）VM 上的真实防火墙提示行为、便携目录实际移动后的规则刷新操作、Android 与 Windows 的跨平台 multicast/session 互通。这些都需要真实网络环境或人工介入，不属于本次自动化验证范围。

## 4. 候选方案

| 方案 | 纯 LAN | 便携 | 无规则 | 无人值守安全 | Windows↔Windows | 结论 |
|---|---:|---:|---:|---:|---:|---|
| 当前随机 listener + 系统提示 | 是 | 是 | 否 | 否 | 是 | 拒绝 |
| MSIX manifest firewall rule | 是 | 否 | 否 | 是 | 是 | 不符合便携要求 |
| Portable PowerShell 固定规则 | 是 | 是 | 否 | 配合 offline 后是 | 是 | **已选择** |
| Windows outbound-only + WSS relay | 否/混合 | 是 | 是 | 是 | 是 | 作为未来可选 |
| Windows outbound-only + Android hub | 是 | 是 | 是 | 是 | 否 | 可作 fallback，不是主方案 |
| BLE GATT | 是 | 是 | 是 | 是 | 视硬件 | 实现成本高，暂缓 |
| WebRTC/TURN-only | 需 relay | 是 | 是 | 是 | 是 | 对计算器过度设计 |

## 5. 已选方案

### 5.1 固定端口

目标协议：

- TCP 42420：session listener；
- UDP 42420：custom multicast discovery；
- multicast address 保持 `224.0.0.167`。

TCP 和 UDP 可以使用相同数值端口。第一版不暴露端口配置 UI，以免规则和运行配置漂移。

### 5.2 Windows mDNS 处理

Windows 便携版目标状态禁用 mDNS，只保留自定义 multicast。原因是 mDNS 通常额外引入 UDP 5353 rule，而且现有 custom discovery 已能承担 LocalSend 式 announce/response。

Android 或其他平台可以继续保留 mDNS 作为平台内 fallback，但跨 Windows 的共同最小协议必须是 UDP 42420 custom discovery。

若后续真实环境测试证明 multicast 覆盖不足，再单独决策是否恢复 Windows mDNS，并同步新增固定规则，而不是让库隐式开放端口。

### 5.3 固定规则

正式规则限定：

- 当前 exe 的绝对路径；
- TCP/UDP 42420；
- Private profile；
- RemoteAddress LocalSubnet；
- EdgeTraversal Block；
- stable rule Name。

详细 Name、脚本契约、路径刷新和验收见 `docs/windows-portable-firewall-policy.md`。

### 5.4 不自动提权

应用不得：

- 启动时静默调用 PowerShell/netsh；
- 自动发起 UAC elevation；
- 修改全局 firewall notifications/default action；
- 为 Public profile 添加宽范围 allow rule；
- 让 agent 在测试前“确保规则存在”。

用户需要 LAN 时，可以显式运行便携目录中的配置脚本。脚本本身必须清晰显示管理员权限要求。

## 6. Agent 和 CI 的正确解决方案

固定规则解决正式 LAN 使用；offline/loopback 模式解决自动测试。这两者不能混为一谈。

### T0：静态和纯逻辑验证

fmt、Clippy、`cargo test --lib`、build，不启动 executable，不接触防火墙。现在也可以默认使用 `cargo test --all-targets`：两个真实网络 integration target 受非默认 `real-network-tests` feature、`#[ignore]` 和 `VOCAL_CALCULATOR_ALLOW_LAN_TESTS=1` 三层门禁保护，默认构建为 0 tests。

### T1：loopback 网络测试

listener 只绑定 `127.0.0.1`/`::1` 和临时端口；不启动 mDNS、multicast 或 LAN listener。测试中的任何显式 peer 地址也必须保持为 loopback。

### T2：GUI smoke

必须同时使用已经实现的：

```text
--network-mode=offline
VOCAL_CALCULATOR_NETWORK_MODE=offline
```

offline 必须从组合根处完全不构造 NetworkManager，而不是启动后再关闭。

虽然模式门禁已实现，但 AUTO-002 进程级 socket 审计仍缺失；agent 不得主动启动 Windows GUI 做 smoke test，除非用户明确指示。仓库根 `AGENTS.md` 已固化该限制。

### T3：真实 LAN 测试

必须是 ignored/manual，并要求：

```text
VOCAL_CALCULATOR_ALLOW_LAN_TESTS=1
```

测试机防火墙规则由操作者预先配置，agent 不负责创建。

## 7. 用户体验

正式 UI 保留 LocalSend 式体验：

- 随机“形容词 + 动物”设备名；
- 附近设备卡片；
- 点击后直接连接/远程执行；
- 接收端只有“允许其他设备控制”总开关；
- 不显示复杂 NxN 授权矩阵；
- 网络设置中显示固定端口、当前 exe 路径和规则状态；
- 提供“打开防火墙脚本所在目录”，不提供自动提权按钮；
- 软件移动后提示重新运行配置脚本；
- discovery 或 firewall 失败不影响本地 Calculator/audio。

## 8. 为什么不让 network 默认关闭

完全默认关闭网络可以避免首次提示，但会削弱 LocalSend 式开箱发现。当前决策是不强行修改正式用户默认值，而是给自动化一个强制 offline 入口。

如果后续用户测试表明首次防火墙提示仍过于打扰，可再改成：第一次点击“启用局域网”时才构造 NetworkManager。这个 UX 决策不影响固定端口和 agent offline 契约。

## 9. 实施顺序（2026-07-15 状态更新）

1. 实现 `NetworkMode::{Lan,Offline,LoopbackTest}`——**已实现并验证（自动化）**。
2. 为 agent 添加 offline 启动门禁——**代码已实现、直接组合根测试与进程级验证未实现**：模式解析和底层选择器有单元测试，但没有 constructor-spy 直接执行 `init_networking`，也没有进程级 socket 审计 smoke test（对应 `repair-backlog.md` AUTO-002，仍未实现）。
3. TCP listener 改为固定 42420——**已实现并验证（自动化）**。
4. Windows 禁用 mDNS，确认 custom multicast 互通——mDNS 禁用本身**已实现并验证（自动化）**；"确认 custom multicast 互通"这半句**仍未做真实验证**，因为需要真实双机环境。
5. 实现 dry-run + configure + remove firewall scripts——**已实现并验证（自动化）**，仅限 dry-run 路径；真实（非 dry-run）执行**仍未做真实 Windows LAN / 双机 / clean-VM 验证**。
6. 在 clean Windows 10/11 VM 验证提示、规则、Private/Public profile 和软件移动——**仍未实现**，仍是待办项，本次任务范围内明确排除（禁止真实防火墙 mutation、禁止 GUI 启动）。
7. 最后才允许自动 GUI smoke 使用 offline 模式——**仍未实现**：即便 CLI/环境变量组合已就绪，第 6 步的真实环境验证和进程级 socket 审计 smoke test 都还没有完成，尚不构成"允许自动 GUI smoke"的前置条件全部满足。

## 10. 资料来源

- LocalSend protocol：<https://github.com/localsend/protocol>
- LocalSend firewall requirements：<https://github.com/localsend/localsend>
- Microsoft Windows Firewall rules：<https://learn.microsoft.com/windows/security/operating-system-security/network-security/windows-firewall/rules>
- Microsoft firewall rule configuration：<https://learn.microsoft.com/windows/security/operating-system-security/network-security/windows-firewall/configure>
- Microsoft `Remove-NetFirewallRule`：<https://learn.microsoft.com/powershell/module/netsecurity/remove-netfirewallrule>
- Microsoft firewall profiles/LocalSubnet guidance：<https://learn.microsoft.com/previous-versions/windows/desktop/ics/windows-firewall-profiles>
- Microsoft BLE advertisements：<https://learn.microsoft.com/windows/apps/develop/devices-sensors/ble-beacon>
