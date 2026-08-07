# JPEG encoder selection

Status: revised during PR 5 after independent decoder validation

Decision date: 2026-08-07

## Decision

Use [`jpeg-encoder` 0.6.1](https://crates.io/crates/jpeg-encoder/0.6.1) with
default features disabled and only its `std` feature enabled. Streamthumb feeds
the encoder one JPEG MCU row at a time and joins the independently encoded
entropy segments with a standard DRI marker and rotating RST0 through RST7
restart markers. The final result is one baseline sequential JPEG.

This integration retains at most 16 RGB rows for 4:2:0 or 8 RGB rows for
4:2:2 and 4:4:4. Each temporary segment and the final encoded result use the
same bounded writer. The final SOF0 height is the complete image height, the
restart interval is one MCU row, and every segment begins with reset DC
predictors as required after a restart marker.

`jpeg-encoder` declares Rust 1.61, is licensed `(MIT OR Apache-2.0) AND IJG`,
and contains no unsafe code when its optional SIMD feature is disabled. The
workspace can therefore retain its Rust 1.85 MSRV.

## Why the PR 4 selection changed

The initial spike selected `mozjpeg-rs` 0.9.2 because its public streaming API
matched the required push-oriented shape. The spike verified dimensions,
markers, bounded-writer failure, native execution, and WASM execution, but it
did not independently compare decoded colors.

PR 5 added that missing check before exposing the codec. Solid-color output
decoded to substantially incorrect RGB values at ordinary quality settings,
and quality 99 or 100 could panic in entropy bit-buffer handling. Source review
found that the streaming implementation calls `quantize_block` in three places
while its DCT produces coefficients scaled by eight. The crate's batch encoder
correctly calls `quantize_block_raw` for the same representation. The upstream
`main` branch still contained the mismatch when reviewed.

Streamthumb does not vendor or silently patch the young dependency. The
production dependency was changed before merge, and independent decoder tests
now cover multi-MCU-row output, all public subsampling modes, quality 100,
alpha compositing, ordered PNG input, and Adam7 PNG input.

## Candidate review

| Candidate | Version reviewed | Result | Reason |
| --- | ---: | --- | --- |
| `jpeg-encoder` | 0.6.1 | Selected with MCU-row segmentation | Established safe Rust implementation, caller-owned writer, baseline output, fixed Huffman tables, Rust 1.61, and correct independent decoding. Its whole-image API is bounded by invoking it only for one MCU row and joining segments with standard restart markers. |
| `mozjpeg-rs` | 0.9.2 | Rejected after validation | Its direct streaming API has a quantization mismatch that corrupts decoded colors and can overflow the entropy bit buffer at high quality. The issue was present in the reviewed upstream `main` source. |
| `libjpeg-turbo-rs` | 0.8.0 | Rejected | Pure Rust, Rust 1.87, and extensively cross-validated, but its public `ScanlineEncoder` allocates `width * height * bytes_per_pixel` and compresses only in `finish`. |
| `jpeg-rusturbo` | 0.9.2 | Rejected | Rust 1.85 and WASM scalar fallback are available, but its encoder accepts a complete pixel slice. It is also a young pre-1.0 project. |
| `pixo` | 0.4.1 | Rejected | Its JPEG API accepts a complete image and writes into an unconstrained `Vec<u8>`. |
| `zenjpeg` | 0.8.4 | Rejected | Strong streaming API, but Rust 1.93 and `AGPL-3.0-only OR LicenseRef-Imazen-Commercial` do not fit this workspace. |
| C `mozjpeg` wrappers | current | Rejected | FFI, CMake/NASM, unsafe boundaries, and WASM build complexity are disproportionate for this scope. |

## Public controls and invariants

- Quality is an integer from 1 through 100; the default is 85.
- Subsampling is 4:2:0, 4:2:2, or 4:4:4; the default is 4:2:0.
- Output is baseline sequential JPEG with fixed Huffman tables.
- Straight-alpha RGBA is composited over a configurable RGB background using
  `(source * alpha + background * (255 - alpha) + 127) / 255` per channel.
- JPEG width and height must each be at most 65,535 pixels.
- Encoded output, temporary entropy segments, allocations, row ordering, and
  incomplete input all retain typed failure paths.
- Dependency upgrades require the same independent decoder and WASM checks.

## Validation boundary

The tests intentionally use `jpeg-decoder`, not the selected encoder, to read
the result. They verify SOI, EOI, SOF0, DRI, restart markers, sampling factors,
decoded dimensions, bounded output failure, alpha compositing, and lossy pixel
tolerances. Browser tests additionally execute JPEG generation in a dedicated
Web Worker so the scalar WebAssembly path is covered.
