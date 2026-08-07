# Benchmark Report

## Scope

This report is a reproducible Phase 7 baseline. It compares the production
streamthumb pipeline with a full-frame image-rs native pipeline and a jSquash
WebAssembly PNG decode, resize, and encode pipeline. All outputs use a maximum
dimension of 512 pixels.

The measurements below are a single smoke run on a Windows development machine
using Rust 1.97.1, image 0.25.8, Node.js 24.14.1, and wasm-pack 0.15.0. They are
illustrative and should not be treated as stable release thresholds. The
jSquash adapter pins `@jsquash/png` 3.1.1 and `@jsquash/resize` 2.1.1.

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
