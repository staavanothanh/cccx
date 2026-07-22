[CmdletBinding()]
param(
    [string]$RepositoryRoot
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

if ([string]::IsNullOrWhiteSpace($RepositoryRoot)) {
    $RepositoryRoot = Split-Path -Parent $PSScriptRoot
}

$allowedGeneratedRoots = @(
    "codex-rs/app-server-protocol/schema/json",
    "codex-rs/app-server-protocol/schema/typescript",
    "codex-rs/hooks/schema/generated",
    "sdk/python/src/openai_codex/generated"
)

$allowedSecretFixturePatterns = @(
    '^codex-rs/http-client/tests/fixtures/test-(ca|ca-trusted|intermediate)\.pem$'
)

function Get-GeneratedRoot {
    param([string]$Path)

    foreach ($schemaRoot in $allowedGeneratedRoots[0..1]) {
        if ($Path.StartsWith("$schemaRoot/", [StringComparison]::Ordinal)) {
            return $schemaRoot
        }
    }

    $parts = $Path -split "/"
    for ($index = 0; $index -lt $parts.Length; $index++) {
        if ($parts[$index] -in @("gen", "generated")) {
            return ($parts[0..$index] -join "/")
        }
    }

    return $null
}

Push-Location $RepositoryRoot
try {
    $trackedFiles = @(& git ls-files)
    if ($LASTEXITCODE -ne 0) {
        throw "git ls-files failed with exit code $LASTEXITCODE"
    }

    $forbiddenTracked = @(
        $trackedFiles | Where-Object {
            $path = $_
            $isAllowedSecretFixture = $false
            foreach ($pattern in $allowedSecretFixturePatterns) {
                if ($path -match $pattern) {
                    $isAllowedSecretFixture = $true
                    break
                }
            }
            $_ -match '(^|/)(target|coverage|tmp|temp|logs?|\.cccx|\.cccx-test-home|\.codebase-memory|\.agents)(/|$)' -or
            $_ -match '\.(log|out|err|tmp|profraw|profdata|lcov|bak|swp|swo)$' -or
            ($_ -match '(^|/)\.env($|\.)' -and $_ -notmatch '(^|/)\.env\.example$') -or
            ($_ -match '\.(pem|p12|pfx|key)$' -and -not $isAllowedSecretFixture) -or
            $_ -match '(^|/)id_(rsa|dsa|ecdsa|ed25519)$' -or
            $_ -match '(^|/)(credentials|secrets?)\.(json|toml|ya?ml)$'
        }
    )

    $generatedRoots = @(
        $trackedFiles |
            ForEach-Object { Get-GeneratedRoot $_ } |
            Where-Object { $null -ne $_ } |
            Sort-Object -Unique
    )
    $unexpectedGenerated = @(
        $generatedRoots | Where-Object { $_ -notin $allowedGeneratedRoots }
    )
    $emptyAllowlistEntries = @(
        $allowedGeneratedRoots | Where-Object {
            $root = $_
            -not ($trackedFiles | Where-Object { $_.StartsWith("$root/", [StringComparison]::Ordinal) } | Select-Object -First 1)
        }
    )

    if ($forbiddenTracked.Count -gt 0) {
        throw "Forbidden tracked artifacts:`n$($forbiddenTracked -join "`n")"
    }
    if ($unexpectedGenerated.Count -gt 0) {
        throw "Generated source is outside the allowlist:`n$($unexpectedGenerated -join "`n")"
    }
    if ($emptyAllowlistEntries.Count -gt 0) {
        throw "Generated-source allowlist entries no longer contain tracked files:`n$($emptyAllowlistEntries -join "`n")"
    }

    Write-Output "CCCX repository hygiene PASS"
    Write-Output "tracked_files=$($trackedFiles.Count)"
    Write-Output "forbidden_tracked=0"
    Write-Output "generated_roots=$($generatedRoots.Count)"
    $generatedRoots | ForEach-Object { Write-Output "generated_allowlisted=$_" }
}
finally {
    Pop-Location
}
