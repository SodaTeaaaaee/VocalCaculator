<#
.SYNOPSIS
    Removes the two Windows Defender Firewall inbound rules created by
    configure-firewall.ps1 for the portable Vocal Calculator LAN mode.

.DESCRIPTION
    Deletes only the two exact rule Names defined by
    docs/windows-portable-firewall-policy.md:
        VocalCalculator.Portable.TCP-In
        VocalCalculator.Portable.UDP-In
    Nothing else is touched: no other rules, no profile settings, no notifications,
    no default actions.

    Idempotent: missing rules are not treated as an error.

.PARAMETER DryRun
    When set, performs zero firewall reads/writes and instead prints the two removal
    steps that would be executed, then exits 0. No admin rights are required.

.NOTES
    Port 42420 mirrors the Rust constant `DISCOVERY_PORT` in src/net/protocol.rs. The port
    itself is not read or written by this script (removal is by rule Name only); it is
    documented here for context only.
#>

[CmdletBinding(SupportsShouldProcess)]
param(
    [switch] $DryRun
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# Mirrors DISCOVERY_PORT in src/net/protocol.rs (context only; not used for removal).
$Script:FirewallPort = 42420

$Script:RuleNames = @(
    'VocalCalculator.Portable.TCP-In',
    'VocalCalculator.Portable.UDP-In'
)

function Write-DryRunPlan {
    Write-Output '=== Vocal Calculator portable firewall removal (DRY RUN) ==='
    Write-Output 'Planned removal steps (idempotent, by exact -Name, SilentlyContinue):'
    foreach ($name in $Script:RuleNames) {
        Write-Output "  Remove-NetFirewallRule -Name '$name' -ErrorAction SilentlyContinue"
    }
    Write-Output ''
    Write-Output 'No other rules, profiles, or firewall settings are touched.'
    Write-Output 'No firewall rules were read or modified (dry run).'
}

function Test-IsAdministrator {
    $identity = [System.Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object System.Security.Principal.WindowsPrincipal($identity)
    return $principal.IsInRole([System.Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Invoke-RealRemoval {
    if (-not (Test-IsAdministrator)) {
        Write-Error '需要管理员权限才能删除防火墙规则。请以管理员身份重新运行本脚本（本脚本不会自动提权）。'
        exit 1
    }

    $failures = @()

    foreach ($name in $Script:RuleNames) {
        if ($PSCmdlet.ShouldProcess($name, 'Remove firewall rule')) {
            try {
                Get-NetFirewallRule -Name $name -ErrorAction SilentlyContinue |
                    Remove-NetFirewallRule -ErrorAction Stop
            }
            catch {
                # Missing rule is not an error (idempotent); anything else is a real failure.
                if ($_.Exception.Message -notmatch 'No MSFT_NetFirewallRule objects found') {
                    $failures += "删除规则 '$name' 失败：$($_.Exception.Message)"
                }
            }
        }
    }

    if ($failures.Count -gt 0) {
        Write-Error ("防火墙规则删除失败：`n" + ($failures -join "`n"))
        exit 1
    }

    Write-Output '防火墙规则已删除（若规则原本不存在则视为已满足条件）。'
}

# --- Entry point -------------------------------------------------------------------------
# -WhatIf is treated identically to -DryRun: no admin check, no firewall reads or writes.

if ($DryRun -or $WhatIfPreference) {
    Write-DryRunPlan
    exit 0
}

Invoke-RealRemoval
