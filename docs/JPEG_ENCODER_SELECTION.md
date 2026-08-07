# JPEG encoder selection

Status: selected for PR 5 with an MSRV prerequisite

Decision date: 2026-08-07

## Decision

Use [`mozjpeg-rs` 0.9.2](https://crates.io/crates/mozjpeg-rs/0.9.2) with
`default-features = false` for the first JPEG output implementation.

This is the only evaluated current pure-Rust candidate that accepts RGB rows
incrementally, buffers only one MCU row, writes to a caller-provided
`std::io::Write`, exposes quality and chroma-subsampling controls, and emits
baseline sequential JPEG without a whole resized image. Its streaming API can
write directly into Streamthumb's bounded encoded-output writer.
The [upstream repository](https://github.com/imazen/mozjpeg-rs) was not archived
and had activity on 2026-08-01 when reviewed.

Adoption requires raising Streamthumb's MSRV from Rust 1.85 to Rust 1.89.
`mozjpeg-rs` declares Rust 1.89, so PR 5 must update the workspace MSRV, pinned
CI toolchains, release documentation, and package checks together. If retaining
Rust 1.85 becomes a hard requirement, JPEG output must remain deferred; the
other evaluated candidates do not satisfy the streaming architecture.

The crate is young and its
[README discloses substantial AI assistance and incomplete human audit](https://github.com/imazen/mozjpeg-rs#ai-generated-code-notice).
PR 5 must therefore pin the exact version, retain default features as disabled,
add independent multi-decoder fixtures, exercise the path in fuzzing, and treat
dependency upgrades as reviewed changes rather than automatic version bumps.

## Candidate review

| Candidate | Version reviewed | Result | Reason |
| --- | ---: | --- | --- |
| `mozjpeg-rs` | 0.9.2 | Selected with prerequisite | Push-oriented scanline API, one-MCU-row input buffering, caller-owned writer, baseline mode, safe Rust in the selected feature set, and successful WASM build. Requires Rust 1.89 and extra validation because the project is young and not fully human-audited. |
| `jpeg-encoder` | 0.6.1 | Rejected | Rust 1.85 compatible, established, safe without its SIMD feature, and fast. Its `encode` API requires a complete image. Its [`ImageBuffer`](https://docs.rs/jpeg-encoder/0.6.1/jpeg_encoder/trait.ImageBuffer.html) callback is encoder-driven random-access input, not a sink that can consume transient rows from `RgbaRowSink`. Version 0.7.1 also raises MSRV to Rust 1.87. |
| `jpeg-rusturbo` | 0.9.2 | Rejected | Rust 1.85 and WASM scalar fallback are available, but [`JpegEncoder`](https://docs.rs/jpeg-rusturbo/0.9.2/jpeg_rusturbo/struct.JpegEncoder.html) accepts a complete pixel slice. It is also a young, pre-1.0, single-author project. |
| `pixo` | 0.4.1 | Rejected | MIT and a WASM feature are available, but the JPEG API accepts a complete image and writes into a `Vec<u8>`. It cannot preserve either the row-streaming input or the existing output cap without upstream changes. |
| `zenjpeg` | 0.8.4 | Rejected | It has a strong streaming API and forbids unsafe code, but requires Rust 1.93 and is licensed `AGPL-3.0-only OR LicenseRef-Imazen-Commercial`, which is unsuitable for this MIT/Apache workspace without a commercial agreement. |
| C `mozjpeg` wrappers | current | Rejected | FFI, CMake/NASM, unsafe boundaries, and WASM build complexity provide no measured advantage for this thumbnail-oriented scope. |

## Reproducible spike

The isolated workspace at `spikes/jpeg-encoder-selection` is excluded from the
production Cargo workspace. It pins both measured dependencies and contains no
Streamthumb production dependency changes.

The native programs encode the same deterministic 512 x 512 RGB pattern at
quality 85 with 4:2:0 subsampling. `mozjpeg-rs` receives one row at a time;
`jpeg-encoder` necessarily constructs a 786,432-byte complete RGB image.

Measurements were collected on Windows 11 build 22631, an Intel Family 6 Model
183 host with 28 logical processors, Rust 1.97.1, release `opt-level = "s"`,
LTO, one codegen unit, and aborting panics. Each timing sample is the mean of
100 encodes; the table reports the median of five samples. Peak working set is
the median of three separate 200-encode processes sampled every 20 ms. These
are selection evidence, not performance thresholds.

| Measurement | `mozjpeg-rs` 0.9.2 | `jpeg-encoder` 0.6.1 |
| --- | ---: | ---: |
| Median 512 x 512 encode time | 9.15 ms | 3.78 ms |
| Encoded bytes for the synthetic pattern | 323,029 | 154,552 |
| Median process peak working set | 4,214,784 bytes (4.02 MiB) | 4,706,304 bytes (4.49 MiB) |
| Raw release WASM module | 55,160 bytes | 92,968 bytes |
| WASM delta over the 75-byte spike baseline | +55,085 bytes | +92,893 bytes |

The byte-size difference is not a quality comparison: the encoders use
different quality mappings and quantization tables. The WASM values are minimal
modules that keep the encoder reachable and successfully execute `encode_512`
under Node.js. They estimate encoder code contribution but are not exact final
package deltas.

For 4:2:0, the selected encoder retains a 16-row RGB MCU buffer: 24,576 bytes at
512 pixels wide. Streamthumb adds one reusable 1,536-byte RGB conversion row.
Compressed output remains caller-owned and is limited independently. The spike
verifies that a writer limit error stops encoding.

## Selected controls and integration constraints

- Public quality range: 1 through 100; Streamthumb validates before calling the
  encoder. The planned default remains 85.
- Public subsampling modes for the first implementation: 4:4:4, 4:2:2, and
  4:2:0. The planned default remains 4:2:0. The encoder also exposes 4:4:0 and
  grayscale, but they are outside the initial public surface.
- Use `Encoder::streaming()`, `force_baseline(true)`, and fixed standard Huffman
  tables. Streaming mode does not enable progressive output, trellis, or
  optimized two-pass Huffman tables.
- Composite each incoming RGBA row over the configured background into one
  reusable RGB row before `write_scanlines`.
- Pass the existing bounded encoded-output writer to `start_rgb`; do not use the
  convenience API that returns an unconstrained `Vec<u8>`.
- Map writer-limit and allocation errors to the existing typed resource-limit
  errors while retaining codec-specific failures.
- Use `mozjpeg-rs = { version = "=0.9.2", default-features = false }`. The
  selected crate source applies `forbid(unsafe_code)` when its optional C
  compatibility feature is disabled.
- License: BSD-3-Clause. Preserve its license notice in dependency attribution.

## PR 5 entry criteria

PR 5 may begin after accepting the Rust 1.89 MSRV change. It must include the
toolchain updates and the independent JPEG validation described above in the
same reviewable change; no encoder dependency is added to production before
then.
