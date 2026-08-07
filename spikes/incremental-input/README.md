# Incremental input spike

This excluded package tests whether the selected `png` dependency can decode a
PNG from a bounded native reader without retaining the complete encoded input.
It is evidence for a later API decision and is not a supported streamthumb API.

## Run

```text
cargo run --manifest-path spikes/incremental-input/Cargo.toml -- input.png
cargo test --manifest-path spikes/incremental-input/Cargo.toml
```

The spike determines the remaining byte length with `Seek`, rejects inputs over
the configured limit before decoding, and then constrains every underlying read
to a small chunk. Tests use high-entropy input and require thousands of reads
whose individual size never exceeds 23 bytes.

## Finding

`png 0.18.1` can decode rows incrementally through its high-level API, but
`png::Decoder` requires `BufRead + Seek`. This works for files and cursors and
removes the need for a complete streamthumb-owned input buffer. It does not
provide a direct bridge from a one-way pipe or JavaScript `ReadableStream`.

The public low-level `png::StreamingDecoder` accepts pushed byte slices, but its
high-level row unfiltering driver uses crate-private types. Reusing
streamthumb's current normalization and resampling path from pushed JavaScript
chunks would therefore require one of these changes:

- an upstream push-oriented row API in `png`;
- a streamthumb-owned PNG chunk, inflate, filter, and Adam7 driver; or
- buffering the complete input behind a seekable cursor, which would not reduce
  the current input memory boundary.

JavaScript callbacks also cannot synchronously wait for the next asynchronous
`ReadableStream` chunk. A real browser API needs an async push state machine;
wrapping the current synchronous decoder in an async function would only hide
complete buffering and is not acceptable as incremental input.

## Recommendation

Implement a native `Read + Seek` API only after refactoring metadata inspection
away from direct slice scans and proving byte-for-byte parity for ordered and
Adam7 input. Defer the JavaScript incremental-input API until the decoder has a
supported push-oriented row interface. Keep the current complete `Uint8Array`
contract explicit in the meantime.
