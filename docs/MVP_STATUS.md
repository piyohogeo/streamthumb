# MVP implementation status

This document maps the completion criteria in `STREAMTHUMB_DESIGN.md` to
implementation and verification evidence. Version 0.1.0 was published through
the project's GitHub-only release process on 2026-08-07. This document also
tracks the additive browser-input and planning work in the v0.2.0 candidate.

## Completion matrix

| Design requirement | Status | Evidence |
| --- | --- | --- |
| Bounded non-interlaced RGB/RGBA thumbnailing | Complete | `streamthumb-png` row decoding and fused thumbnail tests cover RGB8 and RGBA8. |
| No full-resolution source RGBA allocation | Complete | Non-interlaced processing retains decoder rows and output-sized accumulators; Adam7 uses output-sized sparse accumulators. Native RSS and WebAssembly linear-memory benchmarks exercise height-independent planning. |
| Arbitrary-ratio area filtering | Complete | `streamthumb-core` differential tests compare integer streaming results with an independent full-frame floating-point reference using a one-channel-value tolerance. |
| Centered cover cropping | Complete | Rust, CLI, and WebAssembly expose `contain` and `cover`. Exact fractional crop boundaries feed the ordered and Adam7 area paths without an intermediate frame; PNG, JPEG, and RGBA integration tests share the result. |
| Premultiplied-alpha filtering | Complete | Transparent-edge and fully transparent pixel tests verify straight-alpha output without color halos. |
| Malformed and oversized input rejection | Complete | Typed error tests cover truncation, malformed chunks, APNG, byte, dimension, pixel, output, decoder-memory, and working-memory limits. Fuzz targets cover rows, thumbnails, and the area downsampler. |
| Configurable resource limits | Complete | Rust and WebAssembly APIs expose every required input, output, and working-memory limit. Defaults and documentation are checked together in CI. |
| Rust-native CLI | Complete | `streamthumb-cli` reads through the seekable-reader API without retaining complete encoded input, produces bounded encoded PNG or JPEG output, validates codec-specific arguments and input size, and replaces destinations only after staged output succeeds. A process-level test verifies that malformed input preserves an existing destination and removes staging files. |
| PNG encoder configuration | Complete | Rust, WebAssembly, and CLI APIs expose 8-bit color mode, compression, and filter settings. Tests verify IHDR color types, decoded pixels, every compression/filter combination, Adam7 input, defaults, and invalid boundary values. |
| JPEG output | Complete | Rust, WebAssembly, and CLI APIs expose baseline sequential JPEG with quality, compositing background, and 4:2:0/4:2:2/4:4:4 controls. Independent decoding covers ordered and Adam7 inputs, multiple MCU rows, quality 100, and limits. |
| Browser WebAssembly package | Complete | Chrome and Firefox run worker-based wasm-bindgen tests, including multi-chunk PNG/JPEG output and callback exception identity. A separately installed tarball consumer verifies multi-chunk output in headless Chrome. |
| Browser `File` and `Blob` input | Complete | Dedicated workers use `FileReaderSync` and `Blob.slice()` through the synchronous seekable WebAssembly APIs. Chrome and Firefox verify slice parity, bounded reads, limits, callback failures, and PNG/JPEG/RGBA output without a complete encoded-input copy. |
| Browser memory preflight | Complete | Slice and seekable planners return identical plain-object input metadata, output geometry, complete Rust-owned working-memory estimates, and typed configured-limit status without decoding pixels. |
| GitHub Pages demo | Complete | The deployed worker uses seekable planning and execution for local `File` and generated sample `Blob` inputs. Chrome verifies 128 KiB preflight rejection, 4 MiB recovery, every output format, previews, downloads, revisioned runtime assets, and bounded range reads. |
| Node.js and Deno package use | Complete | CI installs the tarball into an isolated consumer and runs the public examples with explicit WebAssembly bytes. |
| Cloudflare Worker adapter | Source complete; runtime deferred | The runtime-neutral adapter and local package reference are checked. Live-account validation is intentionally excluded because no Cloudflare account is available. |
| Memory and comparison benchmarks | Complete for current baselines | Reproducible native and WebAssembly measurements compare streamthumb with a full-frame image-rs baseline and pinned jSquash packages. Native runners also compare slice and seekable-reader input and require byte-identical PNG/JPEG output. |
| Product positioning | Complete | The README reports measured memory/runtime tradeoffs and compares the project scope with jSquash and wasm-vips without claiming novelty for streaming decode alone. |

## Support beyond the original MVP

Version 0.1.0 also supports Adam7, every standard PNG color type and legal bit
depth, applicable palette/grayscale/RGB `tRNS` transparency, centered cover
cropping, native `Read + Seek` input, raw RGBA output, strict TypeScript
declarations, and reproducible release-candidate artifacts.

The v0.2.0 candidate adds `planThumbnailPng`,
`planThumbnailPngFromSeekable`, `thumbnailPngFromSeekable`, and
`thumbnailPngFromSeekableToChunks`. Its Pages demo retains inputs as `File` or
`Blob`, uses revisioned assets to prevent mixed-cache deployments, and exposes
working-memory limits down to 128 KiB for preflight-failure demonstrations.

## Streaming output extension status

The first two architecture stages from
`STREAMTHUMB_STREAMING_OUTPUT_HANDOFF.md` are complete. `RgbaRowSink` separates
completed thumbnail rows from resampling, and `RgbaCollector` preserves the
existing full-image API. Ordered input emits rows directly into a streaming PNG
writer as soon as their coverage is complete. Adam7 retains its required
output-sized sparse accumulators, then emits normalized rows through the same
PNG sink without creating a second full RGBA frame.

For encoded PNG, `output_rgba_bytes` is now zero and `output_row_bytes` records
the one reusable completed row. The complete encoded PNG remains buffered and
bounded because the public API returns a `Vec<u8>` or JavaScript byte array.
Raw RGBA output intentionally continues to retain the complete output frame.

The third extension stage adds typed PNG encoder configuration. Rust callers
use `PngOptions` with `thumbnail_png_with_encoder_options`; WebAssembly uses a
nested `png` object; and the CLI exposes matching flags. RGBA8 remains the
backward-compatible default. Explicit RGB and grayscale conversion, safe
metadata-driven automatic color selection, five compression presets, and eight
filter strategies are covered by format and pixel tests.

The fourth extension stage adds JPEG without reintroducing a complete resized
RGBA frame. `streamthumb-encode` composites rows into an 8- or 16-row RGB MCU
buffer, encodes one MCU row at a time, and joins entropy segments with standard
restart markers. The final encoded bytes and each temporary segment are
bounded. The encoder selection was revised after independent decoding exposed
a correctness defect in the initially selected dependency; see
`docs/JPEG_ENCODER_SELECTION.md`.

The optional native I/O stage adds seekable PNG input and PNG/JPEG APIs that
take ownership of `Read + Seek` and `Write` implementations. Input length is
checked before decoding, bounded metadata and decode passes explicitly rewind,
and `ThumbnailInfo` describes direct encoded output. Memory planning excludes
caller-owned encoded input and records zero retained encoded-result bytes. The
CLI uses this path for direct file I/O, writes to a same-directory staging file,
and commits it only after the encoder and writer succeed.

The WebAssembly output stage adds `thumbnailPngToChunks`. It synchronously
delivers encoded PNG or JPEG output in chunks of at most 64 KiB and returns
metadata without retaining the complete encoded result in WebAssembly. The
working-memory plan includes the adapter buffer; JavaScript-owned chunk copies
remain outside the WebAssembly budget. Raw RGBA remains available only through
the owned-result API.

## Explicit limitations and deferred work

- APNG, JPEG input, WebP, AVIF, general transformations, additional filters,
  progressive JPEG, and an incremental JavaScript input API are not currently
  implemented.
- The incremental-input spike proves that native seekable readers can decode
  in small bounded reads. The selected PNG decoder's synchronous `BufRead +
  Seek` contract cannot consume an asynchronous one-way JavaScript
  `ReadableStream`; the browser API remains deferred pending a supported
  push-to-row decoder path.
- Resampling averages encoded color samples. It does not perform linear-light
  conversion or ICC color management, and encoded output does not inherit PNG
  color metadata.
- Resource limits do not impose a wall-clock deadline. Hosts that need request
  deadlines must enforce them outside the library.
- Safari and a live Cloudflare Worker have not been included in automated
  runtime validation. Chrome, Firefox, Node.js, and Deno are verified in CI.
- The checked-in external WebAssembly comparison covers jSquash. wasm-vips and
  wasm-image-optimization adapters remain deferred because the current report
  already demonstrates the targeted full-frame memory contrast without adding
  those toolchains to the repository.
- npm and crates.io publication remain deferred. Tags and GitHub Releases are
  explicit maintainer actions rather than CI operations; version 0.1.0 was
  published from a verified signed tag, and v0.2.0 remains an unreleased
  candidate until the same manual approval and verification process completes.
