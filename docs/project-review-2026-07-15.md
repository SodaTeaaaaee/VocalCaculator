# Vocal Calculator 全项目 Review

日期：2026-07-15  
审查对象：当前工作区快照，而非仅 `HEAD`  
产品定位校准：这是一个语音计算器；远控追求低摩擦体验，安全重点是输入注入、崩溃、状态污染和资源耗尽，而不是严格的用户授权体系。

## 1. 执行摘要

项目已经具备可用的计算器核心、中文语音、Dioxus 桌面 UI、Android 编译配置和一套较完整的 LAN 网络原型。310 项 Rust 测试通过，Windows Release、Android ARM64/x86_64 check 和 ARM64 Debug APK 构建均已成功。

当前最主要的问题不是“缺少更多安全授权”，而是产品范围和实现复杂度失衡：一个语音计算器承担了 NxN 路由矩阵、配对信任表、授权请求、签名路由行、版本复制、多执行器状态同步等分布式系统复杂度，但真正重要的输入边界、算术不崩溃、会话生命周期和 Windows 分发体验反而没有闭环。

建议把网络产品模型收敛为：

1. 显示附近设备，点击即可连接或远程执行；
2. 接收端只有一个总开关：“允许其他设备控制”；
3. 不弹逐次授权对话框，不维护复杂 Trusted/AskEachTime/Blocked 状态；
4. 每个 controller 同时只选择一个 executor；
5. 严格验证所有网络输入，限制消息大小、频率、队列和连接数量；
6. Windows 保留 LAN listener，但统一固定 TCP/UDP 端口和规则标识；自动测试强制 offline/loopback，绝不触发交互式防火墙提示。具体策略见 `docs/windows-portable-firewall-policy.md`。

## 2. 产品意图

### 2.1 核心产品

- 传统即时执行计算器：四则、百分比、MU、平方根、内存、重复等号；
- Normal、Broken、Music 和静音音频模式；
- Casio 风格 LCD、实体按键质感、桌面键盘操作；
- Windows 为主要桌面平台，Android 为移动入口；
- 类似 LocalSend 的随机设备名、附近设备列表和低摩擦连接体验。

### 2.2 远控的合理边界

远控不是高价值资产管理系统。开启“允许远控”后，同一使用场景中的其他 Vocal Calculator 节点可以直接发送计算器动作；潜在的误操作属于用户可接受的产品权衡。

仍必须保证：

- 任意网络字节不能触发 panic、任意代码/命令执行或文件访问；
- 非法 Digit、超长名称、异常集合、未知版本和畸形消息必须被拒绝；
- 重放和重复 session 不能让同一个按键被执行多次；
- 消息洪泛不能无限增长内存、线程或 UI 工作量；
- 远端状态同步不能破坏 Calculator 内部状态；
- 关闭远控后立即停止接收远端动作。

## 3. 做得好的部分

- `src/core` 基本保持 UI/audio/network 无关，是可靠的领域层基础。
- Calculator、format、speech 有较丰富的纯逻辑测试。
- `CalcAction` 为结构化枚举，天然优于传递任意命令字符串。
- Dioxus、手写 CSS、组件、Signals 和 channel 驱动网络均已有实际实现。
- Ed25519 握手可以继续作为稳定节点指纹，不必再承载复杂授权语义。
- Windows Release 和两个 Android ABI 已能编译，项目不是原型空壳。

## 4. 重点发现

### R0-1：Calculator 和协议入口没有建立“永不 panic”边界

优先级：P0

证据：

- `CalcAction::Digit(u8)` 接受所有 `u8`：`src/core/action.rs:5-26`；
- UI 会检查 `0..=9`，网络解码后却直接 dispatch；
- `Calculator::digit` 使用 `b'0' + d`：`src/core/calculator.rs:183-195`；
- 四则、percent、MU、memory 使用 `rust_decimal` 直接算术；极值 overflow 会 panic；
- 不可解析输入使用旧 accumulator 作为 fallback：`src/core/calculator.rs:117-135`。

影响：

- `Digit(255)` 在 Debug 中可 panic，Release 中会产生非法字符并污染状态；
- 正常用户输入极值也可能触发 Decimal overflow panic；
- 超长输入可能显示一套内容、实际按旧值计算。

方向：

- 引入 `Digit` newtype 或 `TryFrom<u8>`；
- wire message 解码后统一转换成 `ValidatedAction`；
- 所有 Decimal 运算改用 checked API；
- 明确输入有效数字和总长度上限；
- 删除解析失败静默回退。

### R0-2：网络资源边界不适合处理不可信输入

优先级：P0

当前 UI、NetworkManager、runtime 和 session 广泛使用 unbounded channel；listener 可无限 spawn；入站握手没有完整 timeout；PeerTable、endpoint attempts、路由行和身份数量没有容量上限；发现到 endpoint 后会自动连接；NxN 路由 UI 随节点数近似 O(n²) 增长。

`LengthDelimitedCodec` 默认约有 8 MiB 帧上限，并非真正无限，但对计算器动作协议仍明显过大，且解码后字段和集合仍缺少更小的应用级限制。

建议初始预算：

- 单帧最大 4 KiB；
- 设备名最大 64 UTF-8 bytes；
- 已知 peer 最大 64，活动 session 最大 16；
- action 队列容量 256；可覆盖的状态更新只保留最新一项；
- 单 peer 平均 30 actions/s、短时 burst 60；
- handshake 总超时 5 秒，各阶段不超过 3 秒；
- 所有集合在分配前检查长度。

这些数值应集中在协议 limits 模块中，并通过压力测试调整。

### R0-3：Windows listener、便携路径和自动测试缺少固定契约

优先级：P0（产品交付）

`src/net/runtime.rs` 当前在 Windows 上绑定 `0.0.0.0:0` 随机 TCP listener；discovery 同时启用 mDNS 和 `224.0.0.167:42420` UDP multicast receiver。Windows 官方文档明确指出，应用发起 listen 且没有预置 inbound allow rule 时，防火墙默认阻止并可能提示用户。

项目已决定保留纯 LAN 直连和防火墙规则，但软件是便携版，没有安装阶段。因此需要同时解决：

- TCP 随机端口无法形成最小、稳定的端口规则；
- mDNS 可能额外需要 UDP 5353，扩大规则和排障面；
- Windows application rule 绑定 exe 完整路径，便携目录移动后规则失效；
- agent 若直接启动 GUI，可能在无人值守时卡住防火墙提示；
- agent 不应通过自动提权或修改 firewall policy 来解决测试问题。

目标方案：

- TCP session 和自定义 UDP discovery 都固定使用 42420；
- Windows 默认只保留自定义 multicast，禁用 mDNS，避免第三条规则；
- 提供显式、幂等的 configure/remove PowerShell 脚本，规则限定当前 exe、Private profile、LocalSubnet 和固定端口；
- 应用提供 `offline`、`loopback-test`、`lan` 三种明确模式；
- 所有 agent/CI 默认 offline 或 loopback，真实 LAN 测试必须 opt-in 且运行在预配置机器上；
- 应用和 agent 永远不静默提权、不自动创建规则。

详见 `docs/windows-portable-firewall-policy.md`。**该目标方案自本 review 日期后已实现，见文末 2026-07-15 补充说明与 `docs/repair-backlog.md` WIN-001/002/003、FW-001、AUTO-001/001A 各条目。**

### R1-1：配对、授权和 NxN 路由矩阵对产品过度设计

优先级：P1

当前 Router 同时维护配对、信任、授权请求、路由版本、签名行、矩阵同步、多执行器和冲突策略。此前审查确认，self-owned `RoutingDelta`、签名行和 `RoutingSync` 可以绕过逐次授权流程；在严格安全模型下这是访问控制问题。

按本次产品定位，它更重要的含义是：系统同时存在两套互相矛盾的产品语义——一套是“打开总开关即可远控”，另一套是“配对并逐次授权后才能远控”。继续补强 grant ledger 会增加用户摩擦，也会让 Router 更复杂。

建议直接做产品收敛：

- 删除逐次 RouteRequest/Approve/Deny UX；
- `allow_remote_control` 成为唯一接收开关；
- 保留稳定 node fingerprint，仅用于识别设备和去重；
- 每个 controller 只选择一个 executor；
- 用简单的 active route/session 代替复制式 NxN 授权矩阵；
- 可保留“屏蔽此设备”作为本地 UX 功能，但不建立复杂信任等级。

### R1-2：重复 session、heartbeat 和注销存在生命周期缺陷

优先级：P1

- heartbeat timeout 只退出心跳子任务，不会终止 session 收发循环；
- 被替换 session 自身仍持有 sender clone，丢弃 registry sender 不能取消它；
- unregister 只携带 NodeId，没有 session generation；
- 旧 session 延迟退出时可能删除当前 winner；
- 重复连接可能造成动作重复投递。

建议每条连接使用唯一 `SessionId/generation`，register、action、state、unregister 全部绑定它；替换时显式 cancel；删除时 compare-and-remove；heartbeat 失败直接关闭 socket。

### R1-3：动作重放和状态同步模型超出实际产品需要

优先级：P1

远端 action 不拒绝重复/旧 sequence；`StateSnapshot` 只有显示投影，不能恢复 pending op、repeat、MU 和真实 memory；全局 state seq 又无法支持多个 executor。

建议放弃多 executor fan-out：

- controller 同时只能选一个 executor；
- 每条 session 有 epoch，action 按 `(session_id, seq)` 去重；
- executor 产生权威 revision；
- 传输完整、版本化 `CalculatorState`，或 controller 仅展示 executor 状态，不维护可执行副本。

### R1-4：discovery 与主 runtime 的故障域没有隔离

优先级：P1

mDNS 和 multicast 都失败时 discovery task 返回，而顶层 `select!` 会结束整个 runtime。发现不可用不应关闭既有 session、listener 或命令处理。

建议 discovery 为可降级、可重启的独立 actor；网络地址选择不要依赖连接硬编码 `8.8.8.8:80`，应按接口枚举。

### R1-5：持久化没有单一事实源和事务边界

优先级：P1

配置读取失败静默回默认，保存直接覆盖；`Storage::open(config_dir)` 与 AppConfig 的真实加载目录不一致；identity 的公私钥分成两个文件写入；UI、Router、Signals、NetworkState 和配置快照都可能成为事实源。

建议建立单一 ConfigRepository，使用 schema validation、临时文件、fsync、原子替换和明确的损坏恢复 UX。Node identity 若只作设备指纹，可以简化成单文件原子持久化。

### R1-6：Music 行为、配置和许可证声明不一致

优先级：P1（公开发行阻塞）

Music 音色启动即生成、模式始终可进入；`music_assets_path` 没有生产消费路径；`THIRD_PARTY.md` 却声称无合法外部包时 Music 被禁用。`src/audio/music.rs` 又声明算法 port 自许可证未确认的 AR7778 来源。

这不能直接证明侵权，但公开发行前必须选择：移除/禁用该实现，或完成来源和许可证确认并同步文档。字体和语音资产也需要补齐来源材料。

### R2-1：Router、UI bridge 和 props 形成维护性瓶颈

优先级：P2

Router 超过四千行，UI bridge 超过一千五百行；授权、路由、协议、Calculator、audio、UI 和持久化副作用集中。Signals 已按域拆分，但 main 仍读取大量快照并透传巨型 props，细粒度收益没有兑现。

建议在同一 crate 内渐进拆分，不做多 crate 大重写：

```text
Dioxus adapter
    -> AppCommand
    -> small application use-cases
    -> Calculator/domain
    -> AppEvent/ViewModel

TransportActor
    -> ValidatedNetworkMessage
    -> RemoteCalculatorSession
    -> Calculator use-case
```

优先拆 `TransportActor`、`RemoteCalculatorSession`、`ConfigRepository`；Calculator 本身已经内聚，不需要形式化 Service 薄壳。

### R2-2：字体、音频启动和 UI 测试未闭环

优先级：P2

- build.rs 生成字体子集，但 CSS 仍引用完整字体；
- `assets/fonts` 和 `resource/fonts` 内容重复，每套约 20.2 MB；
- 手工字符表遗漏实际 UI 字符；
- audio registry 的 push-on-success 会让固定索引在单个资源失败时整体错位；
- 音频只在 App 初始化一次，模式切换不会重建；真实问题是启动成本尚未测量；
- 缺 Dioxus render/bridge、真实 WebView、视觉和可访问性测试。

### R2-3：工程门禁和文档不可作为稳定交付入口

优先级：P2

- README/BUILDING 引用七个不存在的文件；
- 没有 CI、release profile、SBOM、依赖审计或固定 Rust toolchain；
- bootstrap 使用 mutable/latest 下载地址且无 hash；
- `cargo fmt --check`、严格 Clippy 和 Windows `--no-default-features` 当前失败；
- 孤立的 broadcast 实现和部分 UDP 测试不覆盖生产路径。

## 5. 需求完成度

| 目标 | 状态 |
|---|---|
| 本地计算器核心 | 基本完成；overflow/非法输入仍需修复 |
| 中文语音 | 基本完成；资产许可和真机听感未确认 |
| Dioxus/手写 CSS/自建组件 | 基本完成 |
| 分散 Signals | 结构完成，渲染收益未充分兑现 |
| Windows 桌面 | Release 可构建；固定端口、便携规则脚本和 `NetworkMode`（含 offline/loopback-test）已实现并有自动化测试覆盖；真实双机 LAN、clean VM 防火墙提示、便携目录移动实测仍未完成——见文末补充说明 |
| Android | 双 ABI check 和 ARM64 Debug APK 完成；真机未验证 |
| LocalSend 式附近设备体验 | 随机名称和设备面板已有；底层传输需重构 |
| 低摩擦远控 | 被复杂配对/路由模型拖累，应简化 |
| 网络输入安全 | 未完成：validation、bounds、replay、lifecycle 有缺口 |
| 字体子集/包体 | 未完成 |
| 公开发行 | 未达到：网络、许可、CI、签名和实机验收均未闭环 |

## 6. 已验证和未验证

审查期间实测：

| 检查 | 结果 |
|---|---|
| `cargo test --locked --all-targets --quiet` | 通过，共 310 项 |
| Windows `cargo build --locked --release` | 通过，EXE 14,415,360 bytes |
| Android ARM64/x86_64 check | 通过 |
| ARM64 Debug APK | 构建、对齐和 debug 签名通过 |
| `cargo fmt --all -- --check` | 失败 |
| Clippy `-D warnings` | 失败，一个 `collapsible_if` |
| Windows `--no-default-features` | 失败，desktop feature gate 不完整 |

注意：审查期间虽然执行过 `cargo test --all-targets`，但后续核验确认其中两个 integration target 会真实绑定 `0.0.0.0` 并运行 multicast/broadcast。它们不能再作为无人值守 agent 的默认命令；在隔离完成前只允许默认运行 `cargo test --lib`。**这一隔离已在本 review 日期后完成，两个 integration target 已归入非默认 `real-network-tests` feature，`cargo test --all-targets` 现在默认安全——见文末补充说明。**

尚未验证：Windows GUI、固定防火墙规则、offline 模式无 socket 保证、双机 LAN、真实声卡、Android 真机生命周期、Release/AAB 签名、依赖 CVE、完整许可证、覆盖率和真实性能。**其中"固定防火墙规则"和"offline 模式无 socket 保证"的代码/自动化测试层面已完成，实机/真机部分仍未验证——见文末补充说明。**

## 7. 总优先级

```text
ValidatedAction + checked math
> Windows 固定端口、规则与 automation-safe offline
> network bounds / session lifecycle / replay
> 单 executor 权威状态
> 简化配对与 NxN 路由
> 持久化
> UI/Signals
> 字体、包体和音频润色
```

## 补充说明（2026-07-15，同日追加）

本 review 正文完成后，同一天内完成了 R0-3 / WIN-001/002/003 / FW-001 / AUTO-001/001A 范围的实现。本节不改写上文正文，只补充随后发生的状态变化；正文中受影响的段落已就地加了指回本节的标注。

### 已实现并验证（自动化）

- `NetworkMode::{Lan, Offline, LoopbackTest}`（`src/app/network_mode.rs`）：CLI `--network-mode` > 环境变量 `VOCAL_CALCULATOR_NETWORK_MODE` > `config.mode` > 旧版 `enabled` 回退优先级，非法值一律硬 `Err`（`main()` exit code 2）。
- TCP session listener 固定绑定 `0.0.0.0:42420`（`Lan`）/ `127.0.0.1:0`（`LoopbackTest`），`LAN_FIXED_PORT` 是 src/ 下唯一 `42420` 字面量来源。
- Windows 上通过 `should_start_mdns` 禁用 mDNS（非 Windows 平台不受影响）。
- `ui::bridge::init_networking` 在 `Offline` 模式下于任何 socket 调用之前返回；discovery 任务的 `JoinHandle` 不再纳入顶层 `select!`，其失败不再拖垮 listener/router/command 任务。
- `tests/discovery_multicast.rs`、`tests/test_udp_transport.rs` 已归入非默认 Cargo feature `real-network-tests`，且每个测试额外 `#[ignore]` 并要求 `VOCAL_CALCULATOR_ALLOW_LAN_TESTS=1`；默认 `cargo test --locked --all-targets` 现在不产生任何真实网络流量（`-- --list` 确认两个二进制均为 0 tests）。
- `packaging/windows/configure-firewall.ps1`、`remove-firewall.ps1`、`packaging/windows/tests/firewall-scripts.tests.ps1` 已实现；`-DryRun`/`-WhatIf` 路径在非管理员环境下已实测，23 项断言全部通过。
- 全量验证：`cargo test --locked --lib` 328 通过（0 失败）；`cargo check --locked`、`cargo check --locked --all-targets --features real-network-tests`、`cargo clippy --locked --all-targets -- -D warnings`、`cargo fmt --all -- --check`、`git diff --check` 全部干净。

### 已实现但未做真实 Windows LAN / 双机 / clean-VM 验证

- offline 模式"应用进程没有非 loopback socket"这一保证目前只有组合根调用路径和 `session_bind_addr`/discovery 门禁的单元测试支撑，没有启动真实 GUI 进程后审计其操作系统 socket 列表的 smoke test（对应 `repair-backlog.md` AUTO-002，见下）。
- 防火墙脚本的真实（非 dry-run）创建/删除规则路径、clean（无既有规则）Windows VM 上的提示行为、便携目录实际移动后重新运行脚本的规则刷新——均未做真实环境验证。
- Windows↔Windows 双机发现与远控、Windows↔Android 跨平台 multicast/session 互通——均未做真实环境验证。
- `src/net/android.rs` 中 multicast lock 从私有字段反射改为调用公开 `WifiManager.createMulticastLock` API（并配套 `setReferenceCounted(false)`）——该改动是任务开始前已存在于工作区的未提交修改，并非本次任务所做，本次任务也未有 agent 触碰过此文件；此处提及仅因它与本次改动同处一份工作区快照。功能上看是修正而非回归，但未做 Android 真机验证。

### 仍未实现

- `repair-backlog.md` AUTO-002（进程级 socket 审计 smoke test）：本次任务改用一组确定性单元测试替代，但这类测试不启动真实进程、不做 OS 层 socket 枚举，不满足 AUTO-002 原始验收标准（"若 future refactor 意外启动 discovery/listener，CI smoke 明确失败"）。AUTO-002 仍在待办清单中，需要后续单独实现。
- clean Windows 10/11 VM 上的完整验收（提示行为、规则、Private/Public profile、软件移动）——见 `docs/windows-networking-research.md` §9 第 6 步，仍是待办项。

结论：R0-3 描述的目标方案已从"决策待实现"变为"代码与自动化测试已完成，真实网络环境验证仍是独立待办"。§5 需求完成度表的 Windows 桌面行、§6 已验证/未验证列表的对应条目按上述口径更新；正文其余部分（架构、产品范围、Router/UI/持久化/音频/字体等问题）未受本次变更影响，原样保留。
