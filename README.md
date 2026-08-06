# streamthumb

`streamthumb` is a memory-bounded streaming PNG thumbnail generator for Rust and WebAssembly.

The project currently implements checked thumbnail geometry, resource-limit validation, conservative working-memory planning, and bounded PNG thumbnail paths for non-interlaced and Adam7 8/16-bit grayscale, grayscale-alpha, RGB, and RGBA files plus 1/2/4/8-bit palette files. Raw RGBA, encoded PNG, native CLI, and WebAssembly APIs are available.

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

## Security and fuzzing

Resource limits are part of the public API. See [SECURITY.md](SECURITY.md) for the exact memory boundary and remaining deadline limitations.

Fuzz targets for row decoding, the fused thumbnail path, and the area downsampler live in `fuzz/`. Build them with:

```text
cargo +nightly fuzz build
```

## Benchmarks

The reproducible benchmark package generates deterministic PNG corpora, compares
the streaming pipeline with a full-frame image-rs path, measures native Peak
RSS, and records WebAssembly linear-memory high-water marks. See
[benchmarks/README.md](benchmarks/README.md) for commands and
[benchmarks/REPORT.md](benchmarks/REPORT.md) for the initial baseline.

## License

Licensed under either of Apache License, Version 2.0 or the MIT license, at your option.
