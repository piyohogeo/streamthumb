$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Get-MeanMicros([string]$Executable) {
    $line = & $Executable 100 | Select-String '^mean_micros='
    return [double]$line.ToString().Split('=')[1]
}

function Get-Median([double[]]$Values) {
    $sorted = $Values | Sort-Object
    return $sorted[[math]::Floor($sorted.Count / 2)]
}

function Measure-PeakWorkingSet([string]$Executable) {
    $process = Start-Process `
        -FilePath $Executable `
        -ArgumentList 200 `
        -WindowStyle Hidden `
        -PassThru
    $peak = 0

    while (-not $process.HasExited) {
        $process.Refresh()
        if ($process.WorkingSet64 -gt $peak) {
            $peak = $process.WorkingSet64
        }
        Start-Sleep -Milliseconds 20
    }

    return $peak
}

cargo test --all
cargo build --release
cargo build --release --target wasm32-unknown-unknown

$mozExecutable = ".\target\release\mozjpeg-rs-bench.exe"
$jpegExecutable = ".\target\release\jpeg-encoder-bench.exe"
$mozTimings = 1..5 | ForEach-Object { Get-MeanMicros $mozExecutable }
$jpegTimings = 1..5 | ForEach-Object { Get-MeanMicros $jpegExecutable }
$mozPeaks = 1..3 | ForEach-Object { Measure-PeakWorkingSet $mozExecutable }
$jpegPeaks = 1..3 | ForEach-Object { Measure-PeakWorkingSet $jpegExecutable }
$wasmDirectory = ".\target\wasm32-unknown-unknown\release"
$baselineBytes = (Get-Item "$wasmDirectory\jpeg_spike_baseline.wasm").Length
$mozWasmBytes = (Get-Item "$wasmDirectory\mozjpeg_rs_spike.wasm").Length
$jpegWasmBytes = (Get-Item "$wasmDirectory\jpeg_encoder_spike.wasm").Length

Write-Output "moz_timing_samples_micros=$($mozTimings -join ',')"
Write-Output "moz_median_micros=$(Get-Median $mozTimings)"
Write-Output "jpeg_timing_samples_micros=$($jpegTimings -join ',')"
Write-Output "jpeg_median_micros=$(Get-Median $jpegTimings)"
Write-Output "moz_peak_samples_bytes=$($mozPeaks -join ',')"
Write-Output "moz_median_peak_bytes=$(Get-Median $mozPeaks)"
Write-Output "jpeg_peak_samples_bytes=$($jpegPeaks -join ',')"
Write-Output "jpeg_median_peak_bytes=$(Get-Median $jpegPeaks)"
Write-Output "baseline_wasm_bytes=$baselineBytes"
Write-Output "moz_wasm_bytes=$mozWasmBytes"
Write-Output "moz_wasm_delta_bytes=$($mozWasmBytes - $baselineBytes)"
Write-Output "jpeg_wasm_bytes=$jpegWasmBytes"
Write-Output "jpeg_wasm_delta_bytes=$($jpegWasmBytes - $baselineBytes)"
