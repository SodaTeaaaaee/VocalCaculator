<#
.SYNOPSIS
    Creates or refreshes the two Windows Defender Firewall inbound rules required by the
    portable Vocal Calculator LAN mode.

.DESCRIPTION
    Implements the contract from docs/windows-portable-firewall-policy.md:
      - Two rules, stable non-localized Names:
          VocalCalculator.Portable.TCP-In  (DisplayName "Vocal Calculator Portable (TCP-In)")
          VocalCalculator.Portable.UDP-In  (DisplayName "Vocal Calculator Portable (UDP-In)")
      - Direction=Inbound, Action=Allow, Enabled=True, Profile=Private only,
        RemoteAddress=LocalSubnet, EdgeTraversalPolicy=Block, Program=<absolute exe path>,
        LocalPort=42420 (TCP resp. UDP).
      - Public/Domain profiles, global firewall settings, notifications and default actions
        are never touched.

    This script does not self-elevate and deliberately omits a RunAsAdministrator requires
    directive, so that -DryRun/-WhatIf can be exercised by non-admin agents/CI without any
    admin rights and without touching firewall state at all (no reads, no writes).

.PARAMETER ExecutablePath
    Absolute or relative path to vocal-calculator-app.exe. Defaults to
    "vocal-calculator-app.exe" resolved next to this script ($PSScriptRoot).

.PARAMETER DryRun
    When set, performs zero firewall reads/writes and instead prints a deterministic,
    machine-checkable plan of what would be done, then exits 0. Equivalent to -WhatIf for
    the mutation steps, but also skips the admin-rights check entirely.

.NOTES
    Port 42420 mirrors the Rust constant `DISCOVERY_PORT` in src/net/protocol.rs. Keep both
    values in sync if the protocol ever changes port.
#>

[CmdletBinding(SupportsShouldProcess)]
param(
    [string] $ExecutablePath,
    [switch] $DryRun
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# Mirrors DISCOVERY_PORT in src/net/protocol.rs. Single source of truth for this script.
$Script:FirewallPort = 42420

$Script:RuleDefinitions = @(
    [PSCustomObject]@{
        Name        = 'VocalCalculator.Portable.TCP-In'
        DisplayName = 'Vocal Calculator Portable (TCP-In)'
        Protocol    = 'TCP'
    },
    [PSCustomObject]@{
        Name        = 'VocalCalculator.Portable.UDP-In'
        DisplayName = 'Vocal Calculator Portable (UDP-In)'
        Protocol    = 'UDP'
    }
)

function Resolve-DefaultExecutablePath {
    Join-Path -Path $PSScriptRoot -ChildPath 'vocal-calculator-app.exe'
}

function Write-DryRunPlan {
    param(
        [Parameter(Mandatory)] [string] $ExePath,
        [Parameter(Mandatory)] [bool] $ExeExists
    )

    Write-Output '=== Vocal Calculator portable firewall configuration (DRY RUN) ==='
    Write-Output "ExecutablePath: $ExePath"
    if (-not $ExeExists) {
        Write-Warning "Executable not found at '$ExePath' (expected for build-dir dry runs; not treated as an error in -DryRun mode)."
    }
    Write-Output ''
    Write-Output 'Planned removal steps (idempotent, by exact -Name, SilentlyContinue):'
    foreach ($rule in $Script:RuleDefinitions) {
        Write-Output "  Remove-NetFirewallRule -Name '$($rule.Name)' -ErrorAction SilentlyContinue"
    }

    Write-Output ''
    Write-Output 'Planned rule creation:'
    foreach ($rule in $Script:RuleDefinitions) {
        Write-Output '  New-NetFirewallRule'
        Write-Output "    Name               = $($rule.Name)"
        Write-Output "    DisplayName        = $($rule.DisplayName)"
        Write-Output '    Direction          = Inbound'
        Write-Output '    Action             = Allow'
        Write-Output '    Enabled            = True'
        Write-Output '    Profile            = Private'
        Write-Output '    RemoteAddress      = LocalSubnet'
        Write-Output '    EdgeTraversalPolicy = Block'
        Write-Output "    Program            = $ExePath"
        Write-Output "    Protocol           = $($rule.Protocol)"
        Write-Output "    LocalPort          = $Script:FirewallPort"
        Write-Output ''
    }

    Write-Output 'Planned post-creation verification: read back each rule plus its'
    Write-Output 'application/port/address filters and confirm every value above matches.'
    Write-Output ''
    Write-Output 'No firewall rules were read or modified (dry run).'
}

function Test-IsAdministrator {
    $identity = [System.Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object System.Security.Principal.WindowsPrincipal($identity)
    return $principal.IsInRole([System.Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Invoke-RealConfiguration {
    param(
        [Parameter(Mandatory)] [string] $ExePath
    )

    if (-not (Test-IsAdministrator)) {
        Write-Error '需要管理员权限才能配置防火墙规则。请以管理员身份重新运行本脚本（本脚本不会自动提权）。'
        exit 1
    }

    if (-not (Test-Path -LiteralPath $ExePath -PathType Leaf)) {
        Write-Error "找不到可执行文件：$ExePath"
        exit 1
    }

    $resolvedExe = (Resolve-Path -LiteralPath $ExePath).Path

    foreach ($rule in $Script:RuleDefinitions) {
        if ($PSCmdlet.ShouldProcess($rule.Name, 'Remove existing firewall rule (if present)')) {
            Get-NetFirewallRule -Name $rule.Name -ErrorAction SilentlyContinue |
                Remove-NetFirewallRule -ErrorAction SilentlyContinue
        }
    }

    foreach ($rule in $Script:RuleDefinitions) {
        if ($PSCmdlet.ShouldProcess($rule.Name, 'Create firewall rule')) {
            New-NetFirewallRule `
                -Name $rule.Name `
                -DisplayName $rule.DisplayName `
                -Direction Inbound `
                -Action Allow `
                -Enabled True `
                -Profile Private `
                -RemoteAddress LocalSubnet `
                -Program $resolvedExe `
                -Protocol $rule.Protocol `
                -LocalPort $Script:FirewallPort `
                -EdgeTraversalPolicy Block | Out-Null
        }
    }

    $failures = @()

    foreach ($rule in $Script:RuleDefinitions) {
        $created = Get-NetFirewallRule -Name $rule.Name -ErrorAction SilentlyContinue
        if (-not $created) {
            $failures += "规则 '$($rule.Name)' 创建后未能读取到。"
            continue
        }

        if ($created.DisplayName -ne $rule.DisplayName) { $failures += "规则 '$($rule.Name)' 的 DisplayName 不匹配。" }
        if ($created.Direction -ne 'Inbound') { $failures += "规则 '$($rule.Name)' 的 Direction 不是 Inbound。" }
        if ($created.Action -ne 'Allow') { $failures += "规则 '$($rule.Name)' 的 Action 不是 Allow。" }
        if ($created.Enabled -ne 'True' -and $created.Enabled -ne $true) { $failures += "规则 '$($rule.Name)' 未启用。" }
        if ($created.Profile -notmatch 'Private') { $failures += "规则 '$($rule.Name)' 的 Profile 不是 Private。" }
        if ($created.EdgeTraversalPolicy -ne 'Block') { $failures += "规则 '$($rule.Name)' 的 EdgeTraversalPolicy 不是 Block。" }

        $addressFilter = $created | Get-NetFirewallAddressFilter
        if ($addressFilter.RemoteAddress -ne 'LocalSubnet') { $failures += "规则 '$($rule.Name)' 的 RemoteAddress 不是 LocalSubnet。" }

        $appFilter = $created | Get-NetFirewallApplicationFilter
        if ($appFilter.Program -ne $resolvedExe) { $failures += "规则 '$($rule.Name)' 绑定的 Program 路径不匹配：期望 '$resolvedExe'，实际 '$($appFilter.Program)'。" }

        $portFilter = $created | Get-NetFirewallPortFilter
        if ($portFilter.Protocol -ne $rule.Protocol) { $failures += "规则 '$($rule.Name)' 的 Protocol 不是 $($rule.Protocol)。" }
        if ($portFilter.LocalPort -ne "$Script:FirewallPort") { $failures += "规则 '$($rule.Name)' 的 LocalPort 不是 $Script:FirewallPort。" }
    }

    if ($failures.Count -gt 0) {
        Write-Error ("防火墙规则校验失败：`n" + ($failures -join "`n"))
        exit 1
    }

    Write-Output '防火墙规则已成功配置并通过校验。'
}

# --- Entry point -------------------------------------------------------------------------
# -WhatIf is treated identically to -DryRun: no admin check, no firewall reads or writes
# (Get-NetFirewallRule is never called), only the plan is printed. This short-circuit runs
# before Invoke-RealConfiguration so that even the read-back verification step is skipped.

if (-not $ExecutablePath) {
    $ExecutablePath = Resolve-DefaultExecutablePath
}

if ($DryRun -or $WhatIfPreference) {
    $exeExists = Test-Path -LiteralPath $ExecutablePath -PathType Leaf
    Write-DryRunPlan -ExePath $ExecutablePath -ExeExists $exeExists
    exit 0
}

Invoke-RealConfiguration -ExePath $ExecutablePath
