param(
    [ValidateSet("smoke", "memory", "adam7")]
    [string]$Profile = "smoke",
    [int]$MaxDimension = 512
)

$ErrorActionPreference = "Stop"
$benchmarkRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$repositoryRoot = Split-Path -Parent $benchmarkRoot
$corpusDirectory = Join-Path $benchmarkRoot "corpus\$Profile"
$packageDirectory = Join-Path $benchmarkRoot "wasm-pkg"
$resultsDirectory = Join-Path $benchmarkRoot "results"
$resultFile = Join-Path $resultsDirectory "wasm-$Profile.jsonl"
$temporaryDirectory = Join-Path $benchmarkRoot "tmp"

New-Item -ItemType Directory -Force -Path $resultsDirectory, $temporaryDirectory | Out-Null
$env:TEMP = $temporaryDirectory
$env:TMP = $temporaryDirectory

Push-Location $repositoryRoot
try {
    cargo run --release --manifest-path benchmarks/Cargo.toml -- "generate-$Profile" $corpusDirectory
    if ($LASTEXITCODE -ne 0) { throw "Corpus generation failed." }
    wasm-pack build crates/streamthumb-wasm --release --target nodejs --out-dir $packageDirectory
    if ($LASTEXITCODE -ne 0) { throw "wasm-pack build failed." }
    node benchmarks/run-wasm.cjs $packageDirectory $corpusDirectory $resultFile $MaxDimension
    if ($LASTEXITCODE -ne 0) { throw "WASM benchmark failed." }
}
finally {
    Pop-Location
}

Write-Output $resultFile
