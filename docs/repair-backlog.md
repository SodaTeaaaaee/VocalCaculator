# Vocal Calculator 修复任务清单

日期：2026-07-15  
状态：待排期  
原则：先保证计算器和协议输入永不崩溃，再落实 Windows 固定端口、便携防火墙规则和 automation-safe offline；不把严格授权体系作为优先目标。

## 0. 产品决策

### DEC-001 Windows 便携版防火墙策略

状态：Accepted

决策：保留纯 LAN listener；正式协议使用固定端口和显式便携脚本；agent/CI 强制 offline 或 loopback，永远不自动修改 firewall policy。

权威记录：`docs/windows-portable-firewall-policy.md`。

### DEC-002 简化远控产品模型

优先级：Blocker

决定并记录：

- `allow_remote_control` 是接收远控的唯一开关；
- 不做逐次授权和复杂 trust state；
- controller 同时只允许一个 executor；
- node key/fingerprint 仅用于稳定身份、去重和可选屏蔽；
- 是否删除用户可见 NxN 矩阵。

验收：形成一页状态图和 UI 流程图。

## 1. Phase 0：输入边界和不崩溃保证

### CORE-001 引入 `Digit` 和 `ValidatedAction`

优先级：P0

- `Digit` 只能由 `0..=9` 构造；
- 网络、键盘和按钮都经同一转换；
- Calculator 内部仍做防御检查；
- 未知/非法 action 返回协议错误，不 panic。

验收：覆盖 Digit 10、255、未知 enum、畸形 bincode；Debug/Release 都不 panic，Calculator 状态不变。

### CORE-002 全部 Decimal 算术改为 checked

优先级：P0

覆盖 add/sub/mul/div、percent、MU、memory 和符号转换。

验收：Decimal 最大/最小值、超长输入、除零及中间结果 overflow 全部进入明确错误态；只有 AC 恢复；无 panic。

### CORE-003 限制输入长度和解析语义

优先级：P0

- 定义有效数字、整数位、小数位和总字符数；
- 删除 `unwrap_or(acc)` 静默回退；
- 输入失败保持旧状态并返回可展示错误。

验收：显示值、内部值和下一次运算结果始终一致。

### NET-001 建立协议 limits 模块

优先级：P0

初始值：frame 4 KiB、name 64 bytes、peer 64、session 16、action queue 256、30 actions/s。

验收：每个 Vec/String/Map 在分配前检查；边界值测试和超限拒绝测试齐全。

### NET-002 将 unbounded channel 改成 bounded

优先级：P0

状态：**已实现并通过自动化测试（2026-07-15）**。

按消息语义处理背压：

- action 保序，不可静默丢；队列满则断开/报告 overload；
- state snapshot、latency、peer presence 可覆盖旧值；
- UI refresh 合并。

验收：压力测试中内存有上界，UI 不因 10 倍正常流量无限积压。

实现说明：Router→runtime、runtime→UI、runtime command 和 per-session 队列均有明确容量。Router action 队列满时停止使用饱和的远端并将当前动作回落到本机执行；可替代状态丢弃 newest。UI 普通状态事件满载时丢弃 newest；未限速的远端消息若无法进入 UI 队列则取消该 peer session。容量和满载策略均有确定性测试。

## 2. Phase 1：Windows 固定端口与 automation-safe 网络模式

### ARCH-001 抽象 `RemoteTransport`

优先级：P0

把 discovery/session IO 与 Router/UI 分离，统一输出 typed peer/message events。

验收：Calculator/application 测试不依赖 TCP、mDNS 或 Tokio runtime。

### WIN-001 实现明确的 `NetworkMode`

优先级：P0，依赖 DEC-001

状态：已实现（`src/app/network_mode.rs`、`src/main.rs`、`src/ui/bridge.rs`）。

- `Lan`：正式 TCP/UDP discovery/session——已实现；
- `Offline`：从组合根处不构造 NetworkManager，不创建任何 network socket——已实现；
- `LoopbackTest`：listener 绑定 loopback 临时端口并跳过 mDNS/multicast——已实现；测试调用方仍必须保证所有显式 peer 地址为 loopback，完整 outbound guard 另行收尾；
- CLI `--network-mode` 优先于环境变量，环境变量优先于 config——已实现；
- 显式非法模式字符串必须失败，不能回退到 LAN——已实现，`main()` exit code 2；配置文件缺失仍使用产品默认，现存但不可读、损坏或字段类型错误的配置 fail-closed 为 Offline；未初始化全局模式也默认 Offline。

验收结果：
- 模式解析和配置 fail-closed 行为——**已实现并通过定向自动化测试**；覆盖优先级、显式非法值、legacy 迁移、缺失配置默认值及现存配置读取/解析失败。全量 `cargo test --locked --lib` 仍是所有并行修复线合并后的质量门。
- NetworkManager 未构造及 offline GUI 进程没有非 loopback socket——**代码门禁已实现，直接组合根测试和进程级验收未实现**：`init_networking` 的 return 位于 `NetworkManager::new` 前，但没有 constructor-spy 测试，也没有启动真实进程后审计 OS socket 列表的 smoke test（见 AUTO-002，仍未实现）。

### WIN-002 固定 TCP/UDP 42420

优先级：P0

状态：已实现（`src/net/protocol.rs`、`src/net/runtime.rs`）。

- TCP session listener 从 ephemeral port 改为 42420——已实现，`session_bind_addr(Lan)` 返回 `0.0.0.0:42420`；
- custom UDP discovery 保持 42420——已实现；
- protocol announce 不再发布随机 session port——已实现，announce 携带常量 `SESSION_TCP_PORT`；
- 端口常量只有一个权威来源——已实现，`LAN_FIXED_PORT` 是 src/ 下唯一 `42420` 字面量，`DISCOVERY_PORT`/`SESSION_TCP_PORT` 为别名；
- 绑定失败给出清楚错误，但不影响本地 Calculator——已实现，`runtime.rs` 绑定失败产出中文状态信息并返回，不影响其他任务。

验收结果：
- 端口被占用时无 panic；所有文档和规则参数一致——**已实现并验证（自动化）**，含一个真实用 `TcpListener` 占位后断言 bind 失败的单元测试。
- 两实例可用 TCP/UDP 同号端口互通——**仍未实现（验证层面）**，需要真实双机环境，本次任务未做。

### WIN-003 Windows 禁用 mDNS，收敛为两条规则

优先级：P1

状态：已实现（`src/net/discovery/mod.rs`，`should_start_mdns`）。

- Windows portable build 不创建 mDNS daemon/UDP 5353——已实现，`should_start_mdns(target_os) = target_os != "windows"`；
- custom multicast 承担 announce/response——已实现（沿用既有 multicast 路径）；
- Android 可以保留 mDNS fallback，但跨平台共同路径是 UDP 42420——已实现，非 Windows 平台的 `should_start_mdns` 仍为 true。

验收结果：
- 防火墙只需 TCP/UDP 42420——**已实现并验证（自动化）**（代码/常量层面）。
- Windows↔Windows、Windows↔Android 在无 mDNS 时仍能发现——**仍未实现（验证层面）**，需要真实双机/跨平台环境，本次任务未做。

### FW-001 实现便携防火墙 dry-run/configure/remove 脚本

优先级：P0，依赖 WIN-002

状态：已实现（`packaging/windows/configure-firewall.ps1`、`remove-firewall.ps1`、`packaging/windows/tests/firewall-scripts.tests.ps1`）。

- 固定 rule Name：`VocalCalculator.Portable.TCP-In` 和 `.UDP-In`——已实现；
- exe path 从 `$PSScriptRoot`（或 `-ExecutablePath`）解析——已实现；
- Private profile、LocalSubnet、EdgeTraversal Block——已实现；
- 仅当前 exe、TCP/UDP 42420——已实现；
- configure 先删除同名旧规则再重建——已实现；
- remove 只删除两个固定 Name——已实现；
- 支持 dry-run/`-WhatIf`——已实现，两者统一走同一短路路径；
- `#Requires -RunAsAdministrator`，不静默自提升——**未按原文字面实现**：脚本改为运行时自检管理员身份（`WindowsPrincipal.IsInRole`），非管理员时报中文错误并 exit 1，而不是用 `#Requires` 在加载阶段拒绝——这一设计变更已记录为 ADR 修订，见 `docs/windows-portable-firewall-policy.md` §6；不静默自提升这一条本身仍然成立。

验收结果：
- dry-run/WhatIf 计划、默认 sibling/相对路径绝对化、Public/Domain 不开放的参数契约、精确 Name、严格 Private 读回逻辑和管理员 guard——**已实现并验证（自动化）**，43 项无 NetSecurity 副作用断言全部通过。真实幂等 mutation、真实规则读回、删除不影响其他规则和真实错误退出码仍需 clean VM/管理员人工验收。
- 移动目录后刷新路径——**已实现但未做真实 Windows LAN / 双机 / clean-VM 验证**：脚本逻辑上每次都重新解析路径并先删后建，但没有在真实 Windows 环境里实际移动目录并重跑脚本核对过。

### AUTO-001 固化 agent/CI 网络测试分层

优先级：P0

状态：已实现（`AGENTS.md`）。

- 根 `AGENTS.md` 已更新为反映当前实现——已实现；
- 普通测试允许 `cargo test --lib` 和现在也允许 `cargo test --all-targets`（真实网络测试已被 feature gate + ignore + env var 三层拦住）——已实现；
- GUI smoke 必须同时传 CLI offline 和 environment offline——已在文档中固化，但 GUI smoke 本身仍需人工介入，见 AUTO-002；
- real LAN tests 标为 ignored/manual 且要求 `VOCAL_CALCULATOR_ALLOW_LAN_TESTS=1`——已实现；
- agent 不执行任何 NetSecurity mutation command——已在文档中固化。

验收结果：在无已有规则的 clean Windows VM 上运行默认 agent 验证，全程不出现 Windows Security Alert，firewall rules diff 为空——**仍未实现（验证层面）**：本次任务只在开发机上跑过默认验证命令，未出现 Security Alert，但没有专门在一台 clean VM 上跑过这套验收。

### AUTO-001A 隔离真实 multicast/broadcast integration tests

优先级：P0

状态：已实现（`Cargo.toml` `real-network-tests` feature，`tests/discovery_multicast.rs`、`tests/test_udp_transport.rs`）。

- 将两个测试文件标为非默认 `real-network-tests` feature，且每个测试额外 `#[ignore]`——已实现；
- 普通 `cargo test --all-targets` 在未显式开启 feature 时不绑定 `0.0.0.0`——已实现，验证方式：`cargo test --locked --all-targets -- --list` 确认两个二进制均为 0 tests；
- 把纯 endpoint/protocol 测试拆回 loopback/unit target——已实现，`protocol_magic_byte_layout` 已移至 `src/net/tests.rs` 作为普通 unit test；
- 真实网络测试要求 `VOCAL_CALCULATOR_ALLOW_LAN_TESTS=1`——已实现，`require_lan_opt_in()` helper。

验收结果：
- 显式 opt-in 后真实 multicast 测试仍可运行——**已实现但未做真实 Windows LAN / 双机 / clean-VM 验证**：`cargo check --locked --all-targets --features real-network-tests` 编译通过，证明代码仍是合法可编译状态，但本次任务未设置 `VOCAL_CALCULATOR_ALLOW_LAN_TESTS=1` 且未加 `--ignored` 实际跑过这些测试（按硬性规则禁止）。
- 无规则 clean VM 运行默认 `cargo test --all-targets` 不产生 LAN socket 和防火墙提示——**仍未实现（验证层面）**：本次只在开发机验证，未在 clean VM 上跑过。

### AUTO-002 增加 socket 审计 smoke test

优先级：P1，依赖 WIN-001

状态：**仍未实现**。启动 offline app 后检查进程关联 socket、只允许 loopback WebView/IPC、不允许 `0.0.0.0`/`::`/LAN interface listener 的端到端 smoke test仍不存在。当前只有模式/配置和底层地址选择器测试，以及 `init_networking` 在 `NetworkManager::new` 前返回的静态代码证据；没有直接组合根 constructor-spy 测试，更没有进程级 OS socket 枚举。因此不能满足本条目“若 future refactor 意外启动 discovery/listener，CI smoke 明确失败”的原始验收标准。

验收：若 future refactor 意外启动 discovery/listener，CI smoke 明确失败而不是弹窗等待——**仍未实现**。

### RELAY-001 可选 outbound relay 研究

优先级：P3，Deferred

仅在用户未来需要跨网段或无防火墙规则模式时恢复；当前里程碑不依赖 relay、TURN 或 WebRTC。

## 3. Phase 2：会话和状态正确性

### NET-003 SessionId/generation 生命周期

优先级：P1

register、action、state、unregister 携带唯一 session ID；替换时显式 cancel；注销 compare-and-remove。

验收：双向同时连接、三条重复连接、旧 session 延迟退出均不会删除 winner 或重复 action。

### NET-004 修复 heartbeat 和握手 timeout

优先级：P1

heartbeat failure 必须取消整个 session；入站/出站每阶段有 deadline；任务可 join。

验收：拒发 Pong、只发半个 handshake、Subscribe 静默三种情况都在 deadline 内清理。

### NET-005 动作 replay/sequence 防护

优先级：P1

使用 `(session_epoch, seq)`；拒绝重复和倒退 action；重连生成新 epoch。

验收：加法、M+、数字输入等非幂等 action 重放不会二次执行。

### STATE-001 确立单 executor 权威状态

优先级：P1，依赖 DEC-002

移除 fan-out 语义；executor 生成 revision；controller 只接受当前 active executor 的状态。

验收：切换设备、重连、乱序返回时结果确定。

### STATE-002 完整 CalculatorState 或纯投影

优先级：P1

在以下二者中选一：

- 版本化完整 CalculatorState；
- controller 不维护 Calculator 副本，只显示远端权威投影。

验收：repeat equals、MU、memory、pending operator、断线后的行为均有端到端测试。

### DISC-001 discovery 独立监督

优先级：P1

discovery 失败只改变 presence 状态，不关闭 transport/runtime；按平台单独重试。

验收：禁用 mDNS/multicast 后既有 session 和网络命令处理继续工作。

## 4. Phase 3：删除过度设计和拆分架构

### PROD-001 删除逐次授权和复杂 trust UX

优先级：P1

状态：**已实现并通过自动化测试（2026-07-15）**。

删除或迁移 RouteRequest/Grant/Deny、Trusted/AskEachTime/Blocked、pending timeout 和相关 UI。

验收：设备列表点击后直接连接；接收端总开关关闭时拒绝动作，开启时无需弹窗。

实现说明：`allow_remote_control` 是唯一持久化的入站权限边界；逐设备 trust、pending approval/timeout、批准/拒绝按钮均已删除。旧 v5 消息保留 wire discriminant 以兼容既有协议，但 Router 明确忽略且不再发送。测试覆盖开关即时生效、开启后无需审批，以及旧授权消息不能改变产品状态。

迁移说明：旧 `conflict_policy` 配置键被 serde 安全忽略；旧 `paired_devices` 表保留为不解释的 opaque 数据，损坏旧行不会阻止 schema 升级或授予权限。若持久化存储/identity 整体无法打开，组合根禁用网络但继续启动本机计算器。

### PROD-002 用 active session 取代授权路由矩阵

优先级：P1

状态：**已实现并通过自动化测试（2026-07-15）**。

保留必要的 peer/session 显示，不再复制 NxN 授权 cell、signed row 和 row version。

验收：Router 生产代码明显缩减；不再存在远端写本机 row version 的路径。

实现说明：控制端只保存一个 selected remote executor；认证 session 存在时 action 直接发往该设备，否则安全回落到本机计算。UI 仅展示设备列表、连接/执行状态和一个入站总开关，不再维护 NxN cell、signed row 或 row version。

### ARCH-002 拆分 Router 与 UI bridge

优先级：P2

优先抽出：TransportActor、RemoteCalculatorSession、ConfigRepository、PeerViewModel。

验收：网络协议变化不要求修改 Calculator/UI props；配置变化不要求修改 session actor。

### UI-001 收敛 Signals 和 props

优先级：P2

组件直接订阅所需 context slice；事件统一为 `AppCommand`；删除重复 PeerDisplayInfo 类型。

验收：单个 latency/presence 更新不要求根组件读取和 clone 全部状态。

### UI-002 LocalSend 式设备面板

优先级：P2

状态：**核心产品切片已实现；真实双机 UX 尚未验证（2026-07-15）**。

设备卡片状态限定为发现中/连接中/已连接/不可达；删除路由矩阵和逐次批准；增加非阻塞“正在被控制”提示与可选屏蔽。

验收：用户从启动到远程按键不超过两次点击。

## 5. Phase 4：持久化、资产和交付

### STORE-001 单一 ConfigRepository 与原子写

优先级：P2

显式路径注入、schema 校验、临时文件、fsync、rename、备份和损坏恢复。

验收：半写、损坏、只读目录和旧 schema 均有测试且不静默重置全部设置。

### STORE-002 简化 identity

优先级：P2

若 identity 只用于设备指纹，改为单文件原子容器；明确丢失后的 UX。

验收：部分文件丢失不会无提示旋转 identity。

### AUDIO-001 解决 Music 许可和配置矛盾

优先级：P1（公开发行 blocker）

确认许可前禁用/移除 AR7778 port，或取得可分发依据并同步 THIRD_PARTY；删除或实现 `music_assets_path`。

验收：代码行为、设置 UI 和 THIRD_PARTY 完全一致。

### AUDIO-002 稳定 registry 索引并做启动 profile

优先级：P2

使用固定 key/slot，不因单资源失败移位；在 Windows 和 Android 真机测冷启动和内存。

### FONT-001 接通字体子集管线

优先级：P2

只有一个 source font；从 UI 字符串自动提取 glyph；CSS/asset pipeline 使用生成物。

验收：关键中文 glyph 测试通过，包中不再携带两套全量字体。

### DOC-001 恢复权威文档入口

优先级：P2

README 链接到本 review、Windows 网络 ADR、repair backlog 和真实 verifier；删除不存在的文档引用。

### CI-001 建立基础质量门

优先级：P2

Windows：fmt、Clippy、unit/integration、Release build；Android：双 ABI check；另加协议 adversarial tests。

验收：当前 fmt、Clippy 和 feature gate 失败全部修复，PR 必须通过 CI。

### RELEASE-001 可复现和许可闭环

优先级：P2

固定 Rust/JDK/SDK 和下载 hash；增加 LICENSE、完整 THIRD_PARTY、字体/语音来源、SBOM 和 dependency audit。

## 6. 建议里程碑

### M1：本地计算和网络输入不会崩溃

完成 CORE-001/002/003、NET-001/002。

### M2：Windows 固定 LAN 契约可无人值守验证

完成 DEC-001/002、ARCH-001、WIN-001/002/003、FW-001、AUTO-001/001A/002。

状态（2026-07-15）：WIN-001/002/003、FW-001、AUTO-001/001A 的代码和自动化测试已完成，详见各条目；ARCH-001/DEC-002 状态不在本次变更范围内，未复核。AUTO-002（进程级 socket 审计 smoke test）仍未实现，M2 因此尚未全部完成；真实双机 LAN、clean VM 防火墙提示行为、便携目录移动、Android↔Windows 互通均未做真实环境验证。

### M3：远控状态机可预测

完成 NET-003/004/005、STATE-001/002、DISC-001。

### M4：产品复杂度收敛

完成 PROD-001/002、UI-002；用户体验达到“看到设备，点一下即可用”。

### M5：可分发

完成 Music/字体/文档/CI/许可和 Windows/Android 实机验收。
