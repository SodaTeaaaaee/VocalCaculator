<#
.SYNOPSIS
    Plain-PowerShell (no Pester) assertions for configure-firewall.ps1 and remove-firewall.ps1.

.DESCRIPTION
    Runs both scripts in -DryRun mode only. Must run non-admin with zero side effects: it
    never invokes either script without -DryRun, and never mutates real firewall state.

    Prints PASS/FAIL per assertion and exits non-zero if any assertion fails.
#>

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$Script:TestsRun = 0
$Script:TestsFailed = 0

# Safety net: these functions shadow the NetSecurity cmdlets for the lifetime
# of this test process. Any regression that reaches a firewall read or write
# aborts before the real cmdlet can run.
function global:Get-NetFirewallRule { throw 'FORBIDDEN_NETSECURITY_CALL: Get-NetFirewallRule' }
function global:New-NetFirewallRule { throw 'FORBIDDEN_NETSECURITY_CALL: New-NetFirewallRule' }
function global:Remove-NetFirewallRule { throw 'FORBIDDEN_NETSECURITY_CALL: Remove-NetFirewallRule' }
function global:Get-NetFirewallAddressFilter { throw 'FORBIDDEN_NETSECURITY_CALL: Get-NetFirewallAddressFilter' }
function global:Get-NetFirewallApplicationFilter { throw 'FORBIDDEN_NETSECURITY_CALL: Get-NetFirewallApplicationFilter' }
function global:Get-NetFirewallPortFilter { throw 'FORBIDDEN_NETSECURITY_CALL: Get-NetFirewallPortFilter' }

function Assert-True {
    param(
        [Parameter(Mandatory)] [bool] $Condition,
        [Parameter(Mandatory)] [string] $Description
    )

    $Script:TestsRun++
    if ($Condition) {
        Write-Output "PASS: $Description"
    }
    else {
        Write-Output "FAIL: $Description"
        $Script:TestsFailed++
    }
}

function Assert-Contains {
    param(
        [Parameter(Mandatory)] [string] $Haystack,
        [Parameter(Mandatory)] [string] $Needle,
        [Parameter(Mandatory)] [string] $Description
    )
    Assert-True -Condition ($Haystack -match [regex]::Escape($Needle)) -Description $Description
}

function Assert-NotContains {
    param(
        [Parameter(Mandatory)] [string] $Haystack,
        [Parameter(Mandatory)] [string] $Needle,
        [Parameter(Mandatory)] [string] $Description
    )
    Assert-True -Condition ($Haystack -notmatch [regex]::Escape($Needle)) -Description $Description
}

$Script:RepoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$Script:PackagingDir = Join-Path $PSScriptRoot '..'
$Script:ConfigureScript = Resolve-Path (Join-Path $Script:PackagingDir 'configure-firewall.ps1')
$Script:RemoveScript = Resolve-Path (Join-Path $Script:PackagingDir 'remove-firewall.ps1')

$Script:FakeExePath = Join-Path $env:TEMP 'vocal-calculator-app-test-fixture.exe'

Write-Output '=== firewall-scripts.tests.ps1 ==='
Write-Output "ConfigureScript: $Script:ConfigureScript"
Write-Output "RemoveScript:    $Script:RemoveScript"
Write-Output "FakeExePath:     $Script:FakeExePath"
Write-Output ''

# --- configure-firewall.ps1 -DryRun -------------------------------------------------------

$configureOutput = & $Script:ConfigureScript -DryRun -ExecutablePath $Script:FakeExePath 2>&1 |
    Out-String
$configureExitCode = $LASTEXITCODE

Assert-True -Condition ($configureExitCode -eq 0) `
    -Description "configure-firewall.ps1 -DryRun exits 0 (actual: $configureExitCode)"

Assert-Contains -Haystack $configureOutput -Needle 'VocalCalculator.Portable.TCP-In' `
    -Description 'configure plan contains TCP rule Name'
Assert-Contains -Haystack $configureOutput -Needle 'VocalCalculator.Portable.UDP-In' `
    -Description 'configure plan contains UDP rule Name'
Assert-Contains -Haystack $configureOutput -Needle 'vocal-calculator-app-test-fixture.exe' `
    -Description 'configure plan contains the fake executable file name'
Assert-Contains -Haystack $configureOutput -Needle 'Inbound' `
    -Description 'configure plan contains Inbound'
Assert-Contains -Haystack $configureOutput -Needle 'Allow' `
    -Description 'configure plan contains Allow'
Assert-Contains -Haystack $configureOutput -Needle 'Private' `
    -Description 'configure plan contains Private profile'
Assert-Contains -Haystack $configureOutput -Needle 'LocalSubnet' `
    -Description 'configure plan contains LocalSubnet'
Assert-Contains -Haystack $configureOutput -Needle 'Block' `
    -Description 'configure plan contains Block (edge traversal)'
Assert-Contains -Haystack $configureOutput -Needle 'TCP' `
    -Description 'configure plan mentions TCP'
Assert-Contains -Haystack $configureOutput -Needle 'UDP' `
    -Description 'configure plan mentions UDP'
Assert-Contains -Haystack $configureOutput -Needle '42420' `
    -Description 'configure plan contains port 42420'

Assert-NotContains -Haystack $configureOutput -Needle 'Public' `
    -Description 'configure plan does NOT mention Public profile'
Assert-NotContains -Haystack $configureOutput -Needle 'Domain' `
    -Description 'configure plan does NOT mention Domain profile'
Assert-True -Condition ($configureOutput -notmatch 'RemoteAddress\s*[:=]\s*Any\b') `
    -Description 'configure plan does NOT set RemoteAddress to Any'
Assert-Contains -Haystack $configureOutput -Needle 'exactly one rule for each stable Name' `
    -Description 'configure plan requires exactly one rule per stable Name'
Assert-Contains -Haystack $configureOutput -Needle 'Profile = Private only' `
    -Description 'configure plan requires Private as the only profile'

# With no override, the executable must resolve next to the configure script.
$expectedDefaultExePath = Join-Path `
    (Split-Path -Parent $Script:ConfigureScript.Path) `
    'vocal-calculator-app.exe'
$defaultOutput = & $Script:ConfigureScript -DryRun 2>&1 | Out-String
$defaultExitCode = $LASTEXITCODE
Assert-True -Condition ($defaultExitCode -eq 0) `
    -Description "configure default-path dry run exits 0 (actual: $defaultExitCode)"
Assert-True -Condition ([System.IO.Path]::IsPathRooted($expectedDefaultExePath)) `
    -Description 'default executable expectation is an absolute sibling path'
Assert-Contains -Haystack $defaultOutput -Needle "ExecutablePath: $expectedDefaultExePath" `
    -Description 'default ExecutablePath resolves next to configure-firewall.ps1'

# Relative paths must be normalized before both dry-run and real execution.
$relativeExePath = ".\relative fixture 'quoted'\vocal-calculator-app.exe"
$expectedAbsoluteExePath = [System.IO.Path]::GetFullPath(
    $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($relativeExePath)
)
$relativeOutput = & $Script:ConfigureScript -DryRun -ExecutablePath $relativeExePath 2>&1 |
    Out-String
$relativeExitCode = $LASTEXITCODE

Assert-True -Condition ($relativeExitCode -eq 0) `
    -Description "configure relative-path dry run exits 0 (actual: $relativeExitCode)"
Assert-True -Condition ([System.IO.Path]::IsPathRooted($expectedAbsoluteExePath)) `
    -Description 'relative-path test expectation is an absolute path'
Assert-Contains -Haystack $relativeOutput -Needle "ExecutablePath: $expectedAbsoluteExePath" `
    -Description 'relative ExecutablePath is normalized to the expected absolute path'
Assert-NotContains -Haystack $relativeOutput -Needle "ExecutablePath: $relativeExePath" `
    -Description 'relative ExecutablePath is never emitted verbatim in the plan'

# -WhatIf must take the same early, non-admin, zero-NetSecurity path as -DryRun.
$configureWhatIfOutput = & $Script:ConfigureScript -WhatIf -ExecutablePath $relativeExePath 2>&1 |
    Out-String
$configureWhatIfExitCode = $LASTEXITCODE
Assert-True -Condition ($configureWhatIfExitCode -eq 0) `
    -Description "configure-firewall.ps1 -WhatIf exits 0 (actual: $configureWhatIfExitCode)"
Assert-Contains -Haystack $configureWhatIfOutput -Needle "ExecutablePath: $expectedAbsoluteExePath" `
    -Description 'configure -WhatIf uses the same normalized absolute path as -DryRun'
Assert-Contains -Haystack $configureWhatIfOutput -Needle 'No firewall rules were read or modified' `
    -Description 'configure -WhatIf reports the zero-NetSecurity path'

# --- remove-firewall.ps1 -DryRun ----------------------------------------------------------

$removeOutput = & $Script:RemoveScript -DryRun 2>&1 | Out-String
$removeExitCode = $LASTEXITCODE

Assert-True -Condition ($removeExitCode -eq 0) `
    -Description "remove-firewall.ps1 -DryRun exits 0 (actual: $removeExitCode)"

Assert-Contains -Haystack $removeOutput -Needle 'VocalCalculator.Portable.TCP-In' `
    -Description 'remove plan contains TCP rule Name'
Assert-Contains -Haystack $removeOutput -Needle 'VocalCalculator.Portable.UDP-In' `
    -Description 'remove plan contains UDP rule Name'

Assert-NotContains -Haystack $removeOutput -Needle 'Public' `
    -Description 'remove plan does NOT mention Public profile'
Assert-NotContains -Haystack $removeOutput -Needle 'Domain' `
    -Description 'remove plan does NOT mention Domain profile'
Assert-NotContains -Haystack $removeOutput -Needle 'New-NetFirewallRule' `
    -Description 'remove plan does NOT plan any rule creation'

$removeWhatIfOutput = & $Script:RemoveScript -WhatIf 2>&1 | Out-String
$removeWhatIfExitCode = $LASTEXITCODE
Assert-True -Condition ($removeWhatIfExitCode -eq 0) `
    -Description "remove-firewall.ps1 -WhatIf exits 0 (actual: $removeWhatIfExitCode)"
Assert-Contains -Haystack $removeWhatIfOutput -Needle 'No firewall rules were read or modified' `
    -Description 'remove -WhatIf reports the zero-NetSecurity path'

# --- Static source checks ------------------------------------------------------------------

$configureSource = Get-Content -Raw -LiteralPath $Script:ConfigureScript
$removeSource = Get-Content -Raw -LiteralPath $Script:RemoveScript
$requiresDirectivePattern = '(?m)^\s*#Requires\s+-RunAsAdministrator'

Assert-True -Condition ($configureSource -notmatch $requiresDirectivePattern) `
    -Description 'configure-firewall.ps1 does not contain a #Requires -RunAsAdministrator directive'
Assert-True -Condition ($removeSource -notmatch $requiresDirectivePattern) `
    -Description 'remove-firewall.ps1 does not contain a #Requires -RunAsAdministrator directive'
Assert-True -Condition ($configureSource -match '\$createdRules\.Count\s+-ne\s+1') `
    -Description 'configure source requires exactly one rule for each stable Name'
Assert-True -Condition ($configureSource -match '\[string\]\s+\$created\.Profile\s+-ne\s+''Private''') `
    -Description 'configure source compares the read-back Profile exactly to Private'
Assert-Contains -Haystack $configureSource -Needle 'Test-IsAdministrator' `
    -Description 'configure mutation path contains an explicit administrator guard'
Assert-Contains -Haystack $removeSource -Needle 'Test-IsAdministrator' `
    -Description 'remove mutation path contains an explicit administrator guard'
Assert-NotContains -Haystack $configureSource -Needle 'Start-Process' `
    -Description 'configure source has no self-elevation process launch'
Assert-NotContains -Haystack $removeSource -Needle 'Start-Process' `
    -Description 'remove source has no self-elevation process launch'

# --- Summary --------------------------------------------------------------------------------

Write-Output ''
Write-Output "=== Summary: $($Script:TestsRun - $Script:TestsFailed)/$Script:TestsRun assertions passed ==="

if ($Script:TestsFailed -gt 0) {
    exit 1
}

exit 0
