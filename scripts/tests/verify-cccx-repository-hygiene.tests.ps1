[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$hygieneScript = Join-Path $repoRoot "scripts/verify-cccx-repository-hygiene.ps1"

$allowedGeneratedFiles = @(
    "codex-rs/app-server-protocol/schema/json/schema.json",
    "codex-rs/app-server-protocol/schema/typescript/schema.ts",
    "codex-rs/hooks/schema/generated/schema.json",
    "sdk/python/src/openai_codex/generated/types.py"
)

$allowedSecretFixtures = @(
    "codex-rs/http-client/tests/fixtures/test-ca.pem",
    "codex-rs/http-client/tests/fixtures/test-ca-trusted.pem",
    "codex-rs/http-client/tests/fixtures/test-intermediate.pem"
)

function New-TestRepository {
    $testRoot = Join-Path ([System.IO.Path]::GetTempPath()) "cccx-hygiene-tests-$([System.Guid]::NewGuid().ToString('N'))"
    New-Item -ItemType Directory -Path $testRoot | Out-Null

    Push-Location $testRoot
    try {
        & git init --quiet
        if ($LASTEXITCODE -ne 0) {
            throw "git init failed with exit code $LASTEXITCODE"
        }

        foreach ($path in @($allowedGeneratedFiles + $allowedSecretFixtures + @("src/main.rs"))) {
            $fullPath = Join-Path $testRoot $path
            New-Item -ItemType Directory -Path (Split-Path -Parent $fullPath) -Force | Out-Null
            Set-Content -LiteralPath $fullPath -Value "fixture" -NoNewline
        }

        & git add .
        if ($LASTEXITCODE -ne 0) {
            throw "git add baseline failed with exit code $LASTEXITCODE"
        }
    }
    finally {
        Pop-Location
    }

    return $testRoot
}

function Add-TrackedFile {
    param(
        [Parameter(Mandatory = $true)]
        [string]$RepositoryRoot,

        [Parameter(Mandatory = $true)]
        [string]$Path
    )

    $fullPath = Join-Path $RepositoryRoot $Path
    New-Item -ItemType Directory -Path (Split-Path -Parent $fullPath) -Force | Out-Null
    Set-Content -LiteralPath $fullPath -Value "fixture" -NoNewline

    Push-Location $RepositoryRoot
    try {
        & git add -- $Path
        if ($LASTEXITCODE -ne 0) {
            throw "git add $Path failed with exit code $LASTEXITCODE"
        }
    }
    finally {
        Pop-Location
    }
}

function Invoke-Hygiene {
    param(
        [Parameter(Mandatory = $true)]
        [string]$RepositoryRoot
    )

    $previousErrorActionPreference = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        $output = & powershell -NoProfile -ExecutionPolicy Bypass -File $hygieneScript -RepositoryRoot $RepositoryRoot 2>&1
        $exitCode = $LASTEXITCODE
    }
    finally {
        $ErrorActionPreference = $previousErrorActionPreference
    }

    return [PSCustomObject]@{
        ExitCode = $exitCode
        Output = ($output -join "`n")
    }
}

function Assert-Pass {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Name,

        [Parameter(Mandatory = $true)]
        [object]$Result
    )

    if ($Result.ExitCode -ne 0) {
        throw "$Name expected PASS but failed with exit code $($Result.ExitCode):`n$($Result.Output)"
    }
    if ($Result.Output -notmatch "CCCX repository hygiene PASS") {
        throw "$Name expected PASS output, got:`n$($Result.Output)"
    }
}

function Assert-Fail {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Name,

        [Parameter(Mandatory = $true)]
        [object]$Result,

        [Parameter(Mandatory = $true)]
        [string]$ExpectedPattern
    )

    if ($Result.ExitCode -eq 0) {
        throw "$Name expected failure but passed:`n$($Result.Output)"
    }
    if ($Result.Output -notmatch $ExpectedPattern) {
        throw "$Name expected output matching '$ExpectedPattern', got:`n$($Result.Output)"
    }
}

$cases = @(
    @{
        Name = "rejects tracked target artifacts"
        Path = "target/debug/cccx.exe"
        ExpectedPattern = "Forbidden tracked artifacts:[\s\S]*target/debug/cccx\.exe"
    },
    @{
        Name = "rejects .cccx-test-home local state"
        Path = ".cccx-test-home/local/state.json"
        ExpectedPattern = "Forbidden tracked artifacts:[\s\S]*\.cccx-test-home/local/state\.json"
    },
    @{
        Name = "rejects logs and tmp artifacts"
        Path = "logs/session.tmp"
        ExpectedPattern = "Forbidden tracked artifacts:[\s\S]*logs/session\.tmp"
    },
    @{
        Name = "rejects .env files"
        Path = ".env.local"
        ExpectedPattern = "Forbidden tracked artifacts:[\s\S]*\.env\.local"
    },
    @{
        Name = "rejects secret-like PEM files outside exact fixture exception"
        Path = "codex-rs/http-client/tests/fixtures/test-ca-copy.pem"
        ExpectedPattern = "Forbidden tracked artifacts:[\s\S]*test-ca-copy\.pem"
    },
    @{
        Name = "rejects secret-like key files"
        Path = "config/service.key"
        ExpectedPattern = "Forbidden tracked artifacts:[\s\S]*config/service\.key"
    },
    @{
        Name = "rejects unexpected generated roots"
        Path = "src/generated/bindings.rs"
        ExpectedPattern = "Generated source is outside the allowlist:[\s\S]*src/generated"
    }
)

$testRepos = New-Object System.Collections.Generic.List[string]

try {
    $positiveRepo = New-TestRepository
    $testRepos.Add($positiveRepo)
    Assert-Pass -Name "accepts allowed generated roots and exact public CA fixtures" -Result (Invoke-Hygiene -RepositoryRoot $positiveRepo)
    Write-Output "PASS accepts allowed generated roots and exact public CA fixtures"

    foreach ($case in $cases) {
        $testRepo = New-TestRepository
        $testRepos.Add($testRepo)
        Add-TrackedFile -RepositoryRoot $testRepo -Path $case.Path
        Assert-Fail -Name $case.Name -Result (Invoke-Hygiene -RepositoryRoot $testRepo) -ExpectedPattern $case.ExpectedPattern
        Write-Output "PASS $($case.Name)"
    }
}
finally {
    $testRepoPrefix = Join-Path ([System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath())) "cccx-hygiene-tests-"
    foreach ($testRepo in $testRepos) {
        if (Test-Path -LiteralPath $testRepo) {
            $resolvedTestRepo = [System.IO.Path]::GetFullPath($testRepo)
            if (-not $resolvedTestRepo.StartsWith($testRepoPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
                throw "Refusing to remove unexpected test repository path: $resolvedTestRepo"
            }
            Remove-Item -LiteralPath $resolvedTestRepo -Recurse -Force
        }
    }
}
