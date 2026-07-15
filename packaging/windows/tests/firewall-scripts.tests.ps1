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
Assert-Contains -Haystack $configureOutput -Needle $Script:FakeExePath `
    -Description 'configure plan contains the exact fake exe path'
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

# --- Static source checks ------------------------------------------------------------------

$configureSource = Get-Content -Raw -LiteralPath $Script:ConfigureScript
$removeSource = Get-Content -Raw -LiteralPath $Script:RemoveScript
$requiresDirectivePattern = '(?m)^\s*#Requires\s+-RunAsAdministrator'

Assert-True -Condition ($configureSource -notmatch $requiresDirectivePattern) `
    -Description 'configure-firewall.ps1 does not contain a #Requires -RunAsAdministrator directive'
Assert-True -Condition ($removeSource -notmatch $requiresDirectivePattern) `
    -Description 'remove-firewall.ps1 does not contain a #Requires -RunAsAdministrator directive'

# --- Summary --------------------------------------------------------------------------------

Write-Output ''
Write-Output "=== Summary: $($Script:TestsRun - $Script:TestsFailed)/$Script:TestsRun assertions passed ==="

if ($Script:TestsFailed -gt 0) {
    exit 1
}

exit 0
