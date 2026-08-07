# streamthumb

`streamthumb` is a memory-bounded streaming PNG thumbnail generator for Rust and WebAssembly, with PNG, JPEG, and raw RGBA output.

The project currently implements checked thumbnail geometry, resource-limit validation, conservative working-memory planning, and bounded PNG thumbnail paths for non-interlaced and Adam7 1/2/4/8/16-bit grayscale, 8/16-bit grayscale-alpha, RGB, and RGBA files, plus 1/2/4/8-bit palette files. Palette, grayscale, and RGB `tRNS` transparency are supported. Encoded PNG and JPEG rows flow directly from the resampler into bounded encoders without a complete resized RGBA frame; raw RGBA, native CLI, and WebAssembly APIs remain available.

## Workspace

- `streamthumb-core`: platform-independent options, limits, geometry, errors, and processing plans
- `streamthumb-encode`: shared bounded output storage and the MCU-row JPEG sink
- `streamthumb-png`: bounded PNG header validation, row decoding, and RGBA8 normalization
- `streamthumb-wasm`: runtime-neutral `Uint8Array` WebAssembly bindings
- `streamthumb-cli`: native command-line frontend producing encoded PNG or JPEG thumbnails

## CLI

```text
cargo run -p streamthumb-cli -- input.png output.png --max-width 512 --max-height 512
```

Use `--fit cover` to fill the requested aspect ratio with a centered crop. The
crop is applied directly during area resampling and does not allocate an
intermediate resized image. `contain` remains the default.

Rust callers select the same behavior with `ThumbnailOptions::fit` and
`Fit::Cover`. WebAssembly callers use `fit: "cover"`.

```text
cargo run -p streamthumb-cli -- input.png square.png --max-width 256 --max-height 256 --fit cover
```

PNG encoding can be configured with `--png-color`, `--png-compression`, and
`--png-filter`. For example:

```text
cargo run -p streamthumb-cli -- input.png output.png --png-color auto --png-compression high --png-filter adaptive
```

The Rust API exposes `PngOptions` through
`thumbnail_png_with_encoder_options`; the existing `thumbnail_png` function
retains its RGBA8, balanced-compression defaults. Native callers that do not
need an owned encoded buffer can use `thumbnail_png_to_writer` or
`thumbnail_png_to_writer_with_encoder_options`. These APIs preserve the encoded
byte cap while excluding the completed encoded image from working memory.

Native callers can also avoid retaining the complete encoded input. The
`*_from_reader` APIs accept `Read + Seek`, measure the remaining input from
the reader's current position, enforce `max_input_bytes` before reading PNG
data, and rewind between bounded metadata and decode passes. For example,
`thumbnail_png_from_reader_to_writer_with_encoder_options` streams from a
`File` directly to a writer. Slice APIs remain compatibility
wrappers over `Cursor<&[u8]>`.

The runnable native example reads directly from a file and writes PNG or JPEG
without retaining either complete encoded byte stream:

```text
cargo run -p streamthumb-png --example native_reader -- input.png output.jpg cover
```

It demonstrates format selection, `contain` or centered `cover`, explicit
resource limits, and normal Rust error propagation. See
[`native_reader.rs`](crates/streamthumb-png/examples/native_reader.rs).
The direct writer APIs write immediately to their destination; callers that
need failure-safe replacement should use a staging file as the CLI does.

CLI values match the WebAssembly literals: color accepts `auto`, `rgba8`,
`rgb8`, `grayscale-alpha8`, or `grayscale8`; compression accepts `none`,
`fastest`, `fast`, `balanced`, or `high`; and filter accepts `default`, `none`,
`sub`, `up`, `average`, `paeth`, `adaptive`, or `min-entropy`.

JPEG output is selected by a `.jpg` or `.jpeg` extension, or explicitly with
`--format jpeg`. Quality, alpha-compositing background, and chroma subsampling
are configurable:

```text
cargo run -p streamthumb-cli -- input.png output.jpg --jpeg-quality 85 --jpeg-background ffffff --jpeg-subsampling 420
```

The Rust API exposes `JpegOptions` through
`thumbnail_png_with_jpeg_options`. JPEG output is baseline sequential and
supports 4:2:0, 4:2:2, and 4:4:4 subsampling. Direct native output uses
`thumbnail_jpeg_to_writer` or `thumbnail_jpeg_to_writer_with_options`.

The CLI uses seekable-reader and direct-writer APIs for both codecs, so it does
not retain either the complete encoded input or a second complete encoded
output copy. It writes to a same-directory staging file and replaces the
destination only after encoding and flushing succeed.
Writer APIs take ownership of the writer and return `ThumbnailInfo`; existing
buffer-returning APIs remain available.

## WebAssembly

Build a browser-targeted package with:

```text
node scripts/build-npm-package.mjs
node scripts/check-npm-package.mjs
```

The exported `thumbnailPng(inputBytes, options)` function returns owned bytes.
`thumbnailPngToChunks(inputBytes, onChunk, options)` instead delivers encoded
PNG or JPEG output synchronously in chunks of at most 64 KiB, avoiding a
complete encoded result in WebAssembly memory. Both work without DOM, Canvas,
filesystem, threads, or `SharedArrayBuffer` APIs. See [the WebAssembly API
contract](docs/WASM_API.md) and the examples for [browsers](examples/browser),
[Node.js](examples/node), [Deno](examples/deno), and [Cloudflare
Workers](examples/cloudflare-worker).

The generated package is prepared as `@streamthumb/wasm` in
`target/npm-package`. Normal CI validates its metadata and exact tarball
contents, installs it into an empty consumer project, checks its TypeScript
declarations, bundles it with pinned esbuild, exercises it in Chrome, and then
stores the unpublished tarball as the `npm-package` artifact.
The same installed tarball is also exercised in Node.js and pinned Deno with
explicit WebAssembly bytes, so the package remains free of runtime-specific
filesystem dependencies.
Input remains a complete `Uint8Array`. The [incremental-input feasibility
report](docs/INCREMENTAL_INPUT_SPIKE.md) records the bounded native-reader spike
and the decoder constraint that currently blocks a genuine asynchronous
`ReadableStream` API.
See [docs/RELEASING.md](docs/RELEASING.md) for the manual release process.
The [release readiness audit](docs/RELEASE_READINESS.md) records the current
package contents, dependency licenses, and remaining maintainer-only actions.
The manually dispatched `Release Candidate` workflow additionally records the
source revision, pinned build tools, byte size, and SHA-256 checksum without
publishing, tagging, or creating a GitHub release.

## Positioning

| | streamthumb | jSquash | wasm-vips |
| --- | --- | --- | --- |
| Primary scope | Bounded PNG-input thumbnail generation | Composable browser image codecs and resize operations | Broad libvips image processing |
| Source-memory model | Streaming rows or bounded Adam7 accumulation | Full decoded image passed between operations | Depends on the libvips operation and pipeline |
| Deployment shape | One narrow Rust/WASM package | Separate codec and resize WASM modules | General-purpose image runtime |
| Best fit | Predictable-memory PNG thumbnails | Flexible browser codec composition | Many formats and transformations |

The checked-in smoke benchmark uses a pinned jSquash PNG decode, Triangle
resize, and PNG encode pipeline. On a 2,048-square sample, streamthumb used 4.00
MiB of WebAssembly linear memory versus 120.12 MiB for jSquash, while jSquash
was faster in that single run. See [benchmarks/REPORT.md](benchmarks/REPORT.md)
for reproducible commands, exact versions, and limitations.

## Development

Run the native checks with:

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Check the WebAssembly build with:

```text
cargo check -p streamthumb-wasm --target wasm32-unknown-unknown
```

Run the WebAssembly API in headless browsers with:

```text
wasm-pack test --headless --chrome crates/streamthumb-wasm
wasm-pack test --headless --firefox crates/streamthumb-wasm
```

CI covers Chrome and Firefox. The Cloudflare Worker adapter remains an example and is not part of browser CI.

## Security and fuzzing

Resource limits are part of the public API. See [SECURITY.md](SECURITY.md) for the exact memory boundary and remaining deadline limitations.

See [docs/MVP_STATUS.md](docs/MVP_STATUS.md) for the design-to-implementation
traceability matrix, verified runtime coverage, and explicitly deferred items.

Fuzz targets for row decoding, the fused thumbnail path, and the area downsampler live in `fuzz/`. Build them with:

```text
cargo +nightly fuzz build
```

## Benchmarks

The reproducible benchmark package generates deterministic PNG corpora, compares
the streaming pipeline with full-frame image-rs and pinned jSquash paths,
compares slice and seekable-reader native input, measures native Peak RSS, and
records WebAssembly linear-memory high-water marks. See
[benchmarks/README.md](benchmarks/README.md) for commands and
[benchmarks/REPORT.md](benchmarks/REPORT.md) for the current baseline.

## License

Licensed under either of Apache License, Version 2.0 or the MIT license, at your option.
