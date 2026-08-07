# streamthumb

`streamthumb` is a memory-bounded streaming PNG thumbnail generator for Rust and WebAssembly.

The project currently implements checked thumbnail geometry, resource-limit validation, conservative working-memory planning, and bounded PNG thumbnail paths for non-interlaced and Adam7 1/2/4/8/16-bit grayscale, 8/16-bit grayscale-alpha, RGB, and RGBA files, plus 1/2/4/8-bit palette files. Palette, grayscale, and RGB `tRNS` transparency are supported. Raw RGBA, encoded PNG, native CLI, and WebAssembly APIs are available.

## Workspace

- `streamthumb-core`: platform-independent options, limits, geometry, errors, and processing plans
- `streamthumb-png`: bounded PNG header validation, row decoding, and RGBA8 normalization
- `streamthumb-wasm`: runtime-neutral `Uint8Array` WebAssembly bindings
- `streamthumb-cli`: native command-line frontend producing encoded PNG thumbnails

## CLI

```text
cargo run -p streamthumb-cli -- input.png output.png --max-width 512 --max-height 512
```

## WebAssembly

Build a browser-targeted package with:

```text
wasm-pack build crates/streamthumb-wasm --target web --out-dir ../../pkg
```

The exported `thumbnailPng(inputBytes, options)` function works without DOM, Canvas, filesystem, threads, or `SharedArrayBuffer` APIs. See `examples/browser` and `examples/cloudflare-worker`.

## Positioning

| | streamthumb | jSquash | wasm-vips |
| --- | --- | --- | --- |
| Primary scope | Bounded PNG thumbnail generation | Composable browser image codecs and resize operations | Broad libvips image processing |
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

Fuzz targets for row decoding, the fused thumbnail path, and the area downsampler live in `fuzz/`. Build them with:

```text
cargo +nightly fuzz build
```

## Benchmarks

The reproducible benchmark package generates deterministic PNG corpora, compares
the streaming pipeline with full-frame image-rs and pinned jSquash paths,
measures native Peak RSS, and records WebAssembly linear-memory high-water marks. See
[benchmarks/README.md](benchmarks/README.md) for commands and
[benchmarks/REPORT.md](benchmarks/REPORT.md) for the current baseline.

## License

Licensed under either of Apache License, Version 2.0 or the MIT license, at your option.
