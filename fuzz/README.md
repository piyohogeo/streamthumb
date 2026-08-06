# Fuzzing

The fuzz package is intentionally excluded from the main Cargo workspace. It requires nightly Rust and `cargo-fuzz`.

Build all targets:

```text
cargo +nightly fuzz build
```

Run individual targets:

```text
cargo +nightly fuzz run decode_rows -- -dict=png.dict
cargo +nightly fuzz run thumbnail_png -- -dict=png.dict
cargo +nightly fuzz run area_downsampler
```

The checked-in corpus contains minimal malformed seeds and a focused subset of Willem van Schaik's PNG Suite. The PNG Suite fixtures were copied from the Go project's `src/image/png/testdata/pngsuite` directory. Their permission notice is preserved in `corpus/PNG_SUITE_LICENSE.txt`.

The selected PNG Suite files cover supported grayscale8, palette8, RGB8, RGBA8, RGB16, and RGBA16 images. Reduced crash samples should preserve their provenance alongside the file.

The targets use deliberately small resource limits so decompression bombs and oversized headers are rejected early during continuous fuzzing. The fused thumbnail target exercises both non-interlaced and Adam7 paths when mutations produce valid supported headers and pass data.

Linux CI is the canonical sanitizer environment. On Windows, running an AddressSanitizer fuzz binary requires a `clang_rt.asan_dynamic-x86_64.dll` matching the LLVM version used by the active Rust nightly toolchain. `cargo fuzz build` can still validate all targets when that runtime is unavailable.
