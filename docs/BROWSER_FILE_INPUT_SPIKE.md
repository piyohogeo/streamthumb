# Browser File input spike

## Decision

Proceed with the seekable browser File input design and promote the proven
adapter into provisional WebAssembly APIs. Keep the current Rust `BufReader`
and do not add a second JavaScript prefetch cache yet.

This decision covers local browser `File` and `Blob` values in a dedicated
worker. It does not introduce asynchronous `ReadableStream` input, network
range input, or main-thread `FileReaderSync` use.

## Proven architecture

The isolated `spikes/browser-file-input` package passes a synchronous
`readAt(offset, length)` callback to a checked Rust `Read + Seek` adapter. The
browser callback uses `Blob.slice()` and one worker-owned `FileReaderSync`.
Production `streamthumb-png` reader APIs perform the existing metadata,
rewind, decode, resize, and encode paths.

The adapter retains only its callback, position, error slot, and counters. It
allocates no complete encoded-input buffer and adds no cache beyond the
production `BufReader`. Each JavaScript return value must be an exact-length
`Uint8Array`.

## Correctness evidence

Dedicated-worker tests pass in current local Chrome and Firefox installations.
They cover:

- grayscale, RGB, indexed, and RGBA PNG Suite corpus inputs at 8 or 16 bits;
- a generated Adam7 RGBA input;
- byte-identical raw RGBA, encoded PNG, and deterministic JPEG output against
  the existing slice APIs;
- buffered output, direct writers, and 64 KiB PNG/JPEG chunk writers;
- PNG and JPEG outputs large enough to cross multiple chunk boundaries;
- checked Start, Current, and End seeks, rewind, EOF, and invalid seeks;
- input-limit rejection before the first content read;
- invalid signatures, truncation, malformed ancillary chunks, and APNG;
- invalid JavaScript input lengths, Promise returns, wrong types, and short or
  long callback results; and
- identity-preserving JavaScript exceptions from input and output callbacks.

The `png` dependency and production decoder implementation are unchanged.

## Measurement

The repeatable test creates a 1024 by 1024 high-entropy RGB PNG, decodes it to
RGBA through the seekable path, then runs the slice path for output parity. Run
it with `--nocapture` to print the metric line:

```text
wasm-pack test --headless --chrome spikes/browser-file-input -- --nocapture
wasm-pack test --headless --firefox spikes/browser-file-input -- --nocapture
```

One local debug-build run on 2026-08-07 produced:

| Browser | Input bytes | Reads | Bytes read | Largest read | Seeks | Seekable | Slice | WASM before | WASM after |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Chrome | 3,147,060 | 391 | 3,196,212 | 8,192 | 18 | 437 ms | 343 ms | 18,612,224 | 18,612,224 |
| Firefox | 3,147,060 | 391 | 3,196,212 | 8,192 | 18 | 436 ms | 415 ms | 18,612,224 | 18,612,224 |

The decoder reread 49,152 bytes, about 1.6% of the encoded input. The maximum
callback request remained the existing 8 KiB `BufReader` capacity. A second
64 KiB JavaScript cache would increase retained caller/runtime memory without
evidence that it would remove meaningful decoder rereads, so it is deferred.

These timings are single-run development observations, not performance
thresholds. The memory values are WebAssembly linear-memory high-water marks,
not live allocation measurements. The test constructs its fixture in
WebAssembly before creating the Blob, so the equal before/after values prove no
additional linear-memory growth during seekable decoding but do not quantify
the package-level saving from avoiding the normal JavaScript-to-WebAssembly
complete-input copy. Installed-package fresh-worker comparison remains part of
the production integration verification.

## Production requirements

The provisional implementation should:

1. expose `thumbnailPngFromSeekable` and
   `thumbnailPngFromSeekableToChunks` without changing existing slice APIs;
2. reuse the existing option parser, result types, output chunk adapter, and
   input/output exception precedence;
3. document that the callback is synchronous and intended for dedicated
   workers;
4. include generated TypeScript declarations and installed-package Chrome
   coverage;
5. run the seekable matrix in Chrome and Firefox CI; and
6. compare slice and File paths in fresh workers before changing the Pages
   worker to pass `File` directly.

The API remains unsuitable for asynchronous streams. A caller that returns a
Promise fails closed as an invalid callback result.
