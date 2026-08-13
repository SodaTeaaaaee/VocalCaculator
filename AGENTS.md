# Agent Instructions

These rules apply to every automated agent working in this repository.

## Windows firewall safety

The desktop application starts LAN discovery and a fixed-port TCP listener when networking is enabled (`NetworkMode::Lan`, the default). Launching it in that mode on Windows can trigger an interactive Windows Defender Firewall prompt. An unattended agent must never cause that prompt.

An offline mode now exists (`NetworkMode::Offline`, see `src/app/network_mode.rs`): with it selected, the composition root (`ui::bridge::init_networking`) returns before constructing `NetworkManager`. An uninitialised process-wide mode also fails closed to `Offline`. A missing config file still uses the product default (`Lan`), while an existing unreadable or invalid config file fails closed to explicit `Offline`.

There is also `NetworkMode::LoopbackTest`, whose listener binds `127.0.0.1:0` and whose runtime skips discovery/mDNS. Use it only for tests whose explicit peer addresses are also loopback; unattended code must never send `ConnectToPeer` to a non-loopback address. Use `Offline` when no networking at all is required.

### Commands that are safe by default

- `cargo test --locked --lib`
- `cargo test --locked --all-targets` — safe now: the two real-network integration test files (`tests/discovery_multicast.rs`, `tests/test_udp_transport.rs`) are gated behind the non-default `real-network-tests` Cargo feature and every test inside them is additionally `#[ignore]`d and calls a `require_lan_opt_in()` helper that panics unless `VOCAL_CALCULATOR_ALLOW_LAN_TESTS=1`. Without the feature enabled, both binaries compile to zero tests; plain `--all-targets` does not touch the network.
- `cargo check` / `cargo build`
- `cargo check --locked --tests --features real-network-tests` — compile-only; do not add `--ignored` or set the opt-in env var alongside it, or it will actually run.
- `cargo fmt --all -- --check`
- `cargo clippy`
- Network tests that bind only to `127.0.0.1` or `::1`
- `packaging/windows/configure-firewall.ps1 -DryRun` and `packaging/windows/remove-firewall.ps1 -DryRun` (or `-WhatIf`) — verified to perform zero admin check and zero `Get/New/Remove-NetFirewallRule` calls; only print a plan and exit.
- `powershell -File packaging/windows/tests/firewall-scripts.tests.ps1` — the scripts' own dry-run/WhatIf test harness (43 assertions, non-elevated, with every NetSecurity cmdlet shadowed by a fail-fast mock).

### Commands and actions that require explicit user direction

- Launching `vocal-calculator-app.exe` without passing `--network-mode=offline` **and** setting `VOCAL_CALCULATOR_NETWORK_MODE=offline` (both are required together; either alone is not sufficient for an unattended launch)
- Running `cargo test --locked --all-targets --features real-network-tests -- --ignored` (or otherwise combining the feature, `--ignored`, and `VOCAL_CALCULATOR_ALLOW_LAN_TESTS=1`) — this is the only way to actually execute real multicast/broadcast traffic, and it requires a pre-provisioned machine
- Starting real mDNS, multicast discovery, or a LAN TCP listener
- Running any real-LAN integration test with all three opt-ins satisfied
- Creating, changing, disabling, or deleting Windows Firewall rules — i.e. running `configure-firewall.ps1`/`remove-firewall.ps1` in their real (non-`-DryRun`) path
- Running `New-NetFirewallRule`, `Set-NetFirewallRule`, `Remove-NetFirewallRule`, `netsh advfirewall`, or an equivalent tool
- Elevating through UAC or starting an elevated helper

### Automated GUI/smoke tests

`--network-mode=offline` and `VOCAL_CALCULATOR_NETWORK_MODE=offline` are implemented (`src/app/network_mode.rs`, `src/main.rs`). Passing both reaches an explicit `init_networking` branch that returns before `NetworkManager::new`; mode resolution, fail-closed config loading, listener-address selection and the Windows mDNS selector have unit tests. There is currently no constructor-spy test that directly exercises the Dioxus composition root, and no end-to-end process-level socket-audit smoke test (see `docs/repair-backlog.md` AUTO-002, still open). An agent must therefore not claim "no LAN socket is opened" was verified by launching the GUI process and inspecting its sockets.

Given that gap, unattended launches of the actual GUI executable — even in offline mode with both flags set — are still not something an agent should do proactively for smoke testing; treat it as requiring explicit user direction until AUTO-002 (or equivalent process-level verification) lands. Real LAN tests remain opt-in as described above and are forbidden by default.

### Firewall scripts

Portable firewall helper scripts exist under `packaging/windows/` (`configure-firewall.ps1`, `remove-firewall.ps1`). They are operator tools, not test setup, and do not self-elevate (no `#Requires -RunAsAdministrator`; they check `WindowsPrincipal.IsInRole(Administrator)` at runtime and exit 1 with a Chinese error if not elevated). Agents may run them with `-DryRun`/`-WhatIf` to inspect their argument-generation logic, and may run the pure-PowerShell test harness in `packaging/windows/tests/`, but must not execute their real (mutating) path unless the user explicitly requests that exact action.

See `docs/windows-portable-firewall-policy.md` for the accepted design.
