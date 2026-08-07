# Browser File input spike

This excluded package tests whether a browser `Blob` can act as the existing
streamthumb `Read + Seek` PNG input without creating a complete JavaScript
`ArrayBuffer` or WebAssembly input copy.

The adapter accepts a synchronous `readAt(offset, length)` callback. Browser
tests run in a dedicated worker, construct the callback with `FileReaderSync`
and `Blob.slice()`, and pass it to the production
`thumbnail_png_rgba_from_reader` path.

This is experimental evidence, not a supported WebAssembly API. It currently
targets the first adoption milestone: RGBA8 non-interlaced parity, checked seek
behavior, bounded reads, input-limit rejection before the first read, and
callback exception identity.

Run the checks from the repository root:

```text
cargo fmt --manifest-path spikes/browser-file-input/Cargo.toml --check
cargo clippy --manifest-path spikes/browser-file-input/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path spikes/browser-file-input/Cargo.toml
wasm-pack test --headless --chrome spikes/browser-file-input
wasm-pack test --headless --firefox spikes/browser-file-input
```

The initial adapter intentionally has no JavaScript or Rust read cache beyond
the `BufReader` already used by the production reader path. Read-call and
largest-read measurements determine whether a fixed-size prefetch cache is
worth testing later.
