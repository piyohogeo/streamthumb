param(
    [ValidateSet("smoke", "memory", "adam7")]
    [string]$Profile = "smoke",
    [int]$MaxDimension = 512
)

$ErrorActionPreference = "Stop"
$benchmarkRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$repositoryRoot = Split-Path -Parent $benchmarkRoot
$corpusDirectory = Join-Path $benchmarkRoot "corpus\$Profile"
$resultsDirectory = Join-Path $benchmarkRoot "results"
$resultFile = Join-Path $resultsDirectory "jsquash-$Profile.jsonl"

New-Item -ItemType Directory -Force -Path $resultsDirectory | Out-Null

Push-Location $repositoryRoot
try {
    cargo run --release --manifest-path benchmarks/Cargo.toml -- "generate-$Profile" $corpusDirectory
    if ($LASTEXITCODE -ne 0) { throw "Corpus generation failed." }
    npm.cmd ci --prefix benchmarks
    if ($LASTEXITCODE -ne 0) { throw "jSquash dependency installation failed." }
    node benchmarks/run-jsquash.mjs $corpusDirectory $resultFile $MaxDimension
    if ($LASTEXITCODE -ne 0) { throw "jSquash benchmark failed." }
}
finally {
    Pop-Location
}

Write-Output $resultFile
