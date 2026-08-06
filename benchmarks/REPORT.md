# Benchmark Report

## Scope

This report is the first reproducible Phase 7 baseline. It compares the
production streamthumb pipeline with a full-frame image-rs pipeline and records
WebAssembly linear-memory high-water marks. All outputs use a maximum dimension
of 512 pixels.

The measurements below are a single smoke run on a Windows development machine
using Rust 1.97.1, image 0.25.8, Node.js 24.14.1, and wasm-pack 0.15.0. They are
illustrative and should not be treated as stable release thresholds.

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

## WebAssembly results

| Input | Runtime | Initial linear memory | High-water | Growth | Output |
| --- | ---: | ---: | ---: | ---: | ---: |
| 2,048 x 2,048 blank | 238.5 ms | 1.13 MiB | 4.00 MiB | 2.88 MiB | 2.1 KiB |
| 64 x 8,192 noise | 42.7 ms | 1.13 MiB | 3.31 MiB | 2.19 MiB | 5.1 KiB |
| 8,192 x 64 gradient | 42.5 ms | 1.13 MiB | 1.69 MiB | 0.56 MiB | 287 B |

All smoke cases remained below the initial aspirational 32 MiB WASM target. A
fresh Node process and WebAssembly instance were used for every row so retained
pages from one case could not affect another case.

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
  than compared with native RSS. WASM linear memory is the relevant bounded
  allocation measure.
- jSquash, wasm-image-optimization, and wasm-vips are not included in this first
  baseline. Reproducible comparisons require pinned distributable artifacts and
  equivalent codec/filter settings; they should be added as separate adapters
  rather than production dependencies.
- The memory corpus is provided but was not run for this local baseline. The
  16K image-rs comparison can require over one GiB and should run on a dedicated
  benchmark host with an explicit memory limit.
