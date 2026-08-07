# Benchmark Report

## Scope

This report is a reproducible Phase 7 baseline. It compares the production
streamthumb pipeline with a full-frame image-rs native pipeline and a jSquash
WebAssembly PNG decode, resize, and encode pipeline. The main baseline uses a
maximum dimension of 512 pixels. A separate large-output run uses 2,048 pixels
to make output-buffer scaling visible.

The measurements below are a single smoke run on a Windows development machine
using Rust 1.97.1, image 0.25.8, Node.js 24.14.1, and wasm-pack 0.15.0. They are
illustrative and should not be treated as stable release thresholds. The
jSquash adapter pins `@jsquash/png` 3.1.1 and `@jsquash/resize` 2.1.1.

All streamthumb baseline rows use the backward-compatible PNG encoder defaults:
RGBA8, balanced compression, and the compression preset's default scanline
filter. Alternate color, compression, and filter settings are functional
choices and are not mixed into these architecture comparisons.

## Streaming-output architecture comparison

A separate single-case run compares the row-streamed PNG encoder with commit
`c8d0a24`, whose encoded path retained a complete resized RGBA frame before
encoding. The source and output are both 2,048 x 2,048 blank RGBA, making the
removed frame 16 MiB. The old run required a temporary 64 MiB budget because
its 36,382,979-byte plan exceeded the public 32 MiB default; the new
19,605,763-byte plan succeeds under that default.

| Path | Runtime | Native Peak RSS | WASM high-water | Output |
| --- | ---: | ---: | ---: | ---: |
| Full resized RGBA then PNG (`c8d0a24`) | 263.5 ms native / 623.5 ms WASM | 23.80 MiB | 36.06 MiB | 20.6 KiB |
| Row-streamed RGBA to PNG | 288.3 ms native / 592.3 ms WASM | 8.31 MiB | 2.38 MiB | 20.7 KiB |

This single run measured about 65% lower native Peak RSS and 93% lower WASM
linear-memory high-water. Encoded bytes can differ because streaming changes
the encoder's data and chunk layout; regression tests decode both results and
require exact RGBA pixel equivalence. These numbers use the same Windows
development environment described below and are architectural evidence, not
stable release thresholds.

## Large-output PNG and JPEG comparison

A second smoke run raises the maximum output dimension from 512 to 2,048. It
uses the same Windows development environment and fresh processes as the main
baseline. Both streamthumb codecs consume resized rows directly. The public
convenience API still returns one complete encoded byte buffer, so the memory
figures include encoded bytes but not a complete uncompressed output frame.

### Native, 2,048 maximum dimension

| Input | Codec | Output dimensions | Runtime | Peak RSS | Encoded output |
| --- | --- | ---: | ---: | ---: | ---: |
| 2,048 x 2,048 blank | PNG | 2,048 x 2,048 | 276.5 ms | 4.57 MiB | 20.7 KiB |
| 2,048 x 2,048 blank | JPEG | 2,048 x 2,048 | 312.9 ms | 4.28 MiB | 65.4 KiB |
| 64 x 8,192 noise | PNG | 16 x 2,048 | 38.2 ms | 6.03 MiB | 99.0 KiB |
| 64 x 8,192 noise | JPEG | 16 x 2,048 | 25.9 ms | 5.49 MiB | 14.6 KiB |
| 8,192 x 64 gradient | PNG | 2,048 x 16 | 19.5 ms | 4.64 MiB | 1.1 KiB |
| 8,192 x 64 gradient | JPEG | 2,048 x 16 | 25.8 ms | 4.26 MiB | 6.7 KiB |

### WebAssembly, 2,048 maximum dimension

| Input | Codec | Output dimensions | Runtime | Linear-memory high-water | Linear-memory growth | Encoded output |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| 2,048 x 2,048 blank | PNG | 2,048 x 2,048 | 666.7 ms | 2.38 MiB | 1.25 MiB | 20.7 KiB |
| 2,048 x 2,048 blank | JPEG | 2,048 x 2,048 | 638.1 ms | 2.13 MiB | 1.00 MiB | 65.4 KiB |
| 64 x 8,192 noise | PNG | 16 x 2,048 | 86.7 ms | 3.81 MiB | 2.69 MiB | 99.0 KiB |
| 64 x 8,192 noise | JPEG | 16 x 2,048 | 70.0 ms | 3.44 MiB | 2.31 MiB | 14.6 KiB |
| 8,192 x 64 gradient | PNG | 2,048 x 16 | 58.2 ms | 2.44 MiB | 1.31 MiB | 1.1 KiB |
| 8,192 x 64 gradient | JPEG | 2,048 x 16 | 59.4 ms | 2.13 MiB | 1.00 MiB | 6.7 KiB |

An uncompressed 2,048 x 2,048 RGBA frame is 16 MiB. The square measurements
remain far below that amount in both codecs, which is direct evidence that the
encoded paths scale with row state and encoded bytes instead of retaining that
frame. The tall noise case has a 1.73 MiB encoded input copied into WASM and a
larger PNG result, explaining why its high-water mark exceeds the square case.

The optimized streamthumb WebAssembly binary is 306.6 KiB after adding JPEG.
The earlier PNG-only baseline was 228.7 KiB, so the measured codec addition is
77.9 KiB (about 34%). JavaScript glue and package metadata remain excluded from
both values.

## Centered cover comparison

A post-cover smoke run compares `contain` and centered `cover` with a 512-pixel
square bound. Upscaling remains disabled. The square source therefore produces
the same 512 x 512 output in both modes. The 64-pixel short axis limits cover
output for the panoramic cases to 64 x 64, while contain preserves the complete
source at 512 x 4 or 4 x 512.

| Input | Fit | Output | PNG native / WASM | JPEG native / WASM | PNG / JPEG WASM high-water |
| --- | --- | ---: | ---: | ---: | ---: |
| 2,048 x 2,048 blank | contain | 512 x 512 | 113.3 / 340.7 ms | 139.0 / 307.0 ms | 2.06 / 1.75 MiB |
| 2,048 x 2,048 blank | cover | 512 x 512 | 131.5 / 344.1 ms | 130.4 / 315.4 ms | 2.06 / 1.75 MiB |
| 64 x 8,192 noise | contain | 4 x 512 | 26.1 / 59.7 ms | 23.4 / 51.3 ms | 3.81 / 3.50 MiB |
| 64 x 8,192 noise | cover | 64 x 64 | 19.0 / 57.6 ms | 26.3 / 60.2 ms | 3.81 / 3.50 MiB |
| 8,192 x 64 gradient | contain | 512 x 4 | 17.9 / 56.8 ms | 19.5 / 45.6 ms | 2.12 / 1.81 MiB |
| 8,192 x 64 gradient | cover | 64 x 64 | 14.4 / 30.9 ms | 15.3 / 33.0 ms | 2.06 / 1.75 MiB |

Cover did not increase the WebAssembly high-water mark in any smoke case. A
same-toolchain control build of the pre-cover revision measured the square
contain PNG path at 112.6 to 116.1 ms across three direct runs, compared with
113.3 ms in the recorded post-cover run. This check found no contain-path
runtime regression beyond single-run noise. The cover implementation keeps the
original integer contain path and uses exact fractional coordinates only when
cropping is required.

The optimized WebAssembly binary is 323.6 KiB with centered cover support,
17.0 KiB (about 5.5%) above the 306.6 KiB JPEG baseline. The package and binary
sizes remain descriptive measurements rather than release thresholds.

## Native results

| Input | Method | Encoded input | Runtime | Peak RSS | Output |
| --- | --- | ---: | ---: | ---: | ---: |
| 2,048 x 2,048 blank | streamthumb | 20.7 KiB | 55.2 ms | 7.9 MiB | 2.1 KiB |
| 2,048 x 2,048 blank | image-rs | 20.7 KiB | 41.2 ms | 36.9 MiB | 6.1 KiB |
| 64 x 8,192 noise | streamthumb | 1.73 MiB | 14.0 ms | 2.2 MiB | 5.1 KiB |
| 64 x 8,192 noise | image-rs | 1.73 MiB | 10.8 ms | 5.8 MiB | 4.2 KiB |
| 8,192 x 64 gradient | streamthumb | 10.0 KiB | 11.9 ms | 7.7 MiB | 287 B |
| 8,192 x 64 gradient | image-rs | 10.0 KiB | 7.2 ms | 2.2 MiB | 1.6 KiB |

The square case demonstrates the intended value: streamthumb used about 79%
less peak RSS than the full-frame pipeline while remaining in the same runtime
order of magnitude. The wide case also shows that streamthumb memory scales
with source width and output accumulator shape rather than source area; this can
exceed a small full-frame allocation for a narrow source. The architecture is a
predictable-memory tradeoff, not an unconditional win for every geometry.

## WebAssembly comparison

| Input | Method | Runtime | WASM high-water | WASM growth | Process max RSS | WASM binaries | Output |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 2,048 x 2,048 blank | streamthumb | 332.9 ms | 4.00 MiB | 2.88 MiB | 62.9 MiB | 228.7 KiB | 2.1 KiB |
| 2,048 x 2,048 blank | jSquash | 231.6 ms | 120.12 MiB | 118.00 MiB | 196.9 MiB | 210.6 KiB | 9.1 KiB |
| 64 x 8,192 noise | streamthumb | 68.8 ms | 3.31 MiB | 2.19 MiB | 67.8 MiB | 228.7 KiB | 5.1 KiB |
| 64 x 8,192 noise | jSquash | 57.6 ms | 17.00 MiB | 14.88 MiB | 77.9 MiB | 210.6 KiB | 5.8 KiB |
| 8,192 x 64 gradient | streamthumb | 56.7 ms | 1.69 MiB | 0.56 MiB | 62.0 MiB | 228.7 KiB | 287 B |
| 8,192 x 64 gradient | jSquash | 41.9 ms | 15.50 MiB | 13.38 MiB | 74.6 MiB | 210.6 KiB | 2.3 KiB |

All streamthumb smoke cases remained below the initial aspirational 32 MiB WASM
target. In the square case, its linear-memory high-water was about 97% lower
than jSquash's, while jSquash completed about 30% faster. The narrow and wide
cases show the same architectural distinction at smaller scale: jSquash retains
a decoded source frame, while streamthumb memory follows source-row and bounded
output state. A fresh Node process and fresh WebAssembly instances were used for
every row so retained pages from one case could not affect another case.

## Adam7 results

| Input | Method | Runtime | Peak RSS / WASM high-water | Output |
| --- | --- | ---: | ---: | ---: |
| Adam7 2,048 x 2,048 blank | streamthumb native | 116.8 ms | 27.5 MiB RSS | 2.1 KiB |
| Adam7 2,048 x 2,048 blank | image-rs | 60.6 ms | 40.2 MiB RSS | 6.1 KiB |
| Adam7 2,048 x 2,048 blank | streamthumb WASM | 280.0 ms | 22.75 MiB linear | 2.1 KiB |
| Adam7 8,192 x 64 gradient | streamthumb native | 15.0 ms | 7.8 MiB RSS | 287 B |
| Adam7 8,192 x 64 gradient | image-rs | 7.0 ms | 4.7 MiB RSS | 1.6 KiB |
| Adam7 8,192 x 64 gradient | streamthumb WASM | 37.4 ms | 1.94 MiB linear | 287 B |

Adam7 uses one exact `u128` accumulator per bounded output pixel so it can
consume sparse pass samples without retaining the source frame. The 512-square
case therefore uses more memory than the ordered-row path, but remains below the
32 MiB WASM target. The wide, short case confirms that the accumulator scales
with output area rather than source area.

## Interpretation and limitations

- Native Peak RSS on Windows is sampled every millisecond and can under-report
  extremely short-lived peaks. Linux release measurements should use the GNU
  `time` runner.
- image-rs uses its Triangle filter; streamthumb uses exact integer area
  coverage with premultiplied alpha. Output bytes and runtime are informative,
  but the filters are not pixel-equivalent.
- Node RSS includes the JavaScript runtime and is kept in raw results rather
  than compared with native RSS. Process maximum RSS is included for the two
  Node-based pipelines, while WASM linear memory remains the more direct view of
  codec and resizer allocation.
- jSquash uses its Triangle filter with premultiplied alpha and
  `linearRGB: false`; streamthumb uses exact integer area coverage. The outputs
  are not pixel-equivalent, so runtime and output size must not be interpreted
  as image-quality rankings.
- The binary column includes only the WebAssembly files used by each adapter.
  It excludes JavaScript glue, npm metadata, and runtime code.
- wasm-image-optimization and wasm-vips remain future adapters. They should stay
  benchmark-only dependencies with pinned distributable artifacts.
- The memory corpus is provided but was not run for this local baseline. The
  16K image-rs comparison can require over one GiB and should run on a dedicated
  benchmark host with an explicit memory limit.
