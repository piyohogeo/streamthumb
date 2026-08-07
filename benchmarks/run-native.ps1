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
$binary = Join-Path $benchmarkRoot "target\release\streamthumb-benchmarks.exe"
$resultFile = Join-Path $resultsDirectory "native-$Profile.jsonl"

New-Item -ItemType Directory -Force -Path $resultsDirectory | Out-Null

Push-Location $repositoryRoot
try {
    cargo build --release --manifest-path benchmarks/Cargo.toml
    & $binary "generate-$Profile" $corpusDirectory

    if (Test-Path -LiteralPath $resultFile) {
        Remove-Item -LiteralPath $resultFile
    }

    Get-ChildItem -LiteralPath $corpusDirectory -Filter "*.png" | Sort-Object Name | ForEach-Object {
        $inputFile = $_.FullName
        foreach ($method in @("streamthumb-png", "streamthumb-jpeg", "streamthumb-cover-png", "streamthumb-cover-jpeg", "image-rs")) {
            $extension = if ($method.EndsWith("jpeg")) { "jpg" } else { "png" }
            $outputFile = Join-Path $resultsDirectory "$($_.BaseName)-$method.$extension"
            $stdoutFile = Join-Path $env:TEMP "streamthumb-benchmark-stdout-$PID.txt"
            $stderrFile = Join-Path $env:TEMP "streamthumb-benchmark-stderr-$PID.txt"
            try {
                $process = Start-Process -FilePath $binary -ArgumentList @(
                    "run", $method, $inputFile, $outputFile, $MaxDimension
                ) -RedirectStandardOutput $stdoutFile -RedirectStandardError $stderrFile -PassThru
                $null = $process.Handle
                $peakWorkingSet = 0L
                while (-not $process.HasExited) {
                    $process.Refresh()
                    $peakWorkingSet = [Math]::Max($peakWorkingSet, $process.PeakWorkingSet64)
                    Start-Sleep -Milliseconds 1
                }
                $process.WaitForExit()
                if ($process.ExitCode -ne 0) {
                    $errorText = Get-Content -Raw $stderrFile
                    throw "Benchmark process failed for $method and $inputFile`: $errorText"
                }
                $record = Get-Content -Raw $stdoutFile | ConvertFrom-Json
                $record | Add-Member -NotePropertyName peak_rss_bytes -NotePropertyValue $peakWorkingSet
                $record | Add-Member -NotePropertyName platform -NotePropertyValue "windows"
                Add-Content -LiteralPath $resultFile -Value ($record | ConvertTo-Json -Compress)
            }
            finally {
                Remove-Item -LiteralPath $stdoutFile, $stderrFile -ErrorAction SilentlyContinue
            }
        }
    }
}
finally {
    Pop-Location
}

Write-Output $resultFile
