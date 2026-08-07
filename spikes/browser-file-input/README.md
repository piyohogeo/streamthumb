# Browser File input spike

This excluded package tests whether a browser `Blob` can act as the existing
streamthumb `Read + Seek` PNG input without creating a complete JavaScript
`ArrayBuffer` or WebAssembly input copy.

The adapter accepts a synchronous `readAt(offset, length)` callback. Browser
tests run in a dedicated worker, construct the callback with `FileReaderSync`
and `Blob.slice()`, and pass it to the production
`thumbnail_png_rgba_from_reader` path.

This is experimental evidence, not a supported WebAssembly API. The current
matrix covers the PNG Suite grayscale, RGB, palette, and RGBA fixtures at 8 or
16 bits, a generated Adam7 RGBA fixture, buffered and chunked PNG/JPEG output,
raw RGBA output, checked seek behavior, bounded reads, input-limit rejection
before the first read, invalid/truncated input, and callback exception identity.
The evidence and production-adoption requirements are recorded in
`docs/BROWSER_FILE_INPUT_SPIKE.md`.

Run the checks from the repository root:

```text
cargo fmt --manifest-path spikes/browser-file-input/Cargo.toml --check
cargo clippy --manifest-path spikes/browser-file-input/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path spikes/browser-file-input/Cargo.toml
wasm-pack test --headless --chrome spikes/browser-file-input
wasm-pack test --headless --firefox spikes/browser-file-input
wasm-pack test --headless --chrome spikes/browser-file-input -- --nocapture
```

The adapter intentionally has no JavaScript or Rust read cache beyond
the `BufReader` already used by the production reader path. Read-call and
largest-read measurements determine whether a fixed-size prefetch cache is
worth testing later.
