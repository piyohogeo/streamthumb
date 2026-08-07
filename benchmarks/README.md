# Benchmarks

This package measures streamthumb without adding benchmark-only dependencies to
the production workspace. It provides deterministic corpus generation, a
full-frame image-rs comparison, native peak-RSS runners, and a WebAssembly
linear-memory runner. A separately pinned jSquash adapter measures a composable
browser-oriented PNG decode, resize, and encode pipeline.

## Corpus profiles

`smoke` is intended for quick local and CI checks:

| Case | Dimensions | Pattern |
| --- | ---: | --- |
| square-blank | 2,048 x 2,048 | Highly compressible RGBA |
| wide-gradient | 8,192 x 64 | Deterministic gradient |
| tall-noise | 64 x 8,192 | Deterministic high-entropy RGB with opaque alpha |

`memory` contains the design-document stress cases: 8K and 16K blank squares,
100K x 32, and 32 x 100K gradients. The generator writes one row at a time and
does not allocate a full source frame.

`adam7` contains a 2,048-square blank image and an 8,192 x 64 gradient. The
generator writes samples in seven-pass order and retains compressed bytes, not a
full decoded source frame.

Generated corpora, result files, output thumbnails, temporary files, and WASM
packages are ignored by Git.

## Native benchmark

Windows PowerShell:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\benchmarks\run-native.ps1 -Profile smoke
```

Linux:

```bash
./benchmarks/run-native.sh smoke 512
```

Each method runs in a fresh process. The Windows runner samples peak working set
at one-millisecond intervals. The Linux runner uses GNU `time` maximum resident
set size. JSON Lines output is written to
`benchmarks/results/native-<profile>.jsonl`.

The `streamthumb` method uses the production streaming PNG pipeline. The
`image-rs` method intentionally performs a full RGBA decode, a Triangle resize,
and PNG encoding. Its filter is not pixel-equivalent to streamthumb's area
filter; this comparison measures full-frame pipeline cost rather than image
quality equivalence.

## WebAssembly benchmark

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\benchmarks\run-wasm.ps1 -Profile smoke
```

Each corpus case runs in a fresh Node.js process and a fresh WebAssembly
instance. WebAssembly memory only grows, so the post-operation byte length is
the linear-memory high-water mark for that case. It includes the allocator's
retained pages and the copied encoded input, but excludes Node.js heap memory.
Node RSS is reported separately.

The runner requires `wasm-pack` and Node.js. Its JSON Lines output is written to
`benchmarks/results/wasm-<profile>.jsonl`.

On Linux, use `./benchmarks/run-wasm.sh smoke 512`. The GitHub `Benchmarks`
workflow exposes all three profiles as a manual action and uploads native and WASM
JSON Lines files as workflow artifacts.

## jSquash benchmark

Windows PowerShell:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\benchmarks\run-jsquash.ps1 -Profile smoke
```

Linux:

```bash
./benchmarks/run-jsquash.sh smoke 512
```

The adapter pins `@jsquash/png` 3.1.1 and `@jsquash/resize` 2.1.1 in the
benchmark-only npm lockfile. Each case runs in a fresh Node.js process and
performs PNG decode, Triangle resize, and PNG encode. Output dimensions use the
same no-upscale contain calculation as streamthumb. Resize uses premultiplied
alpha and encoded-sample-space values (`linearRGB: false`). jSquash does not
provide streamthumb's exact area filter, so this is an end-to-end resource and
runtime comparison rather than a pixel-equivalence test.

The jSquash record reports the combined linear memory and binary size of the PNG
and resize WebAssembly modules. JavaScript glue size is excluded from both
jSquash and streamthumb binary-size fields. The manual GitHub workflow skips the
`memory` profile for jSquash because its full-frame 16K decode belongs on a
dedicated benchmark host with an explicit memory limit.

## Result schema

Every record includes method, input name, encoded input size, source and output
dimensions, elapsed milliseconds, and output size. Native records add
`peak_rss_bytes`. WASM records add linear-memory before, high-water, and growth
values, WebAssembly binary bytes, Node RSS, and process maximum RSS. The
jSquash record uses the same WASM and Node fields for direct schema-level
comparison.

Run multiple samples and summarize distributions before using these results for
release claims. The checked-in report is an illustrative single-run baseline,
not a cross-machine performance guarantee.
