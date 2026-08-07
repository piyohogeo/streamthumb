# Incremental input feasibility

## Scope

This spike evaluated whether streamthumb could stop requiring a complete
encoded PNG byte slice. Its result has now been integrated into the production
native API. The original executable evidence remains in
[`spikes/incremental-input`](../spikes/incremental-input).

## Dependency contract

The selected `png 0.18.1` high-level decoder requires `BufRead + Seek`. Its row
methods decode incrementally and do not require a complete encoded input buffer,
but the seek bound excludes one-way readers.

The crate also exposes `png::StreamingDecoder::update`, which accepts pushed
byte slices. That layer reports PNG chunk and image-data events. The unfiltering
buffer used by the high-level row driver is crate-private, so streamthumb cannot
connect pushed bytes to the existing normalized-row pipeline through supported
public types.

## Native result

A native `Read + Seek` API is feasible. The spike:

1. records the initial stream position;
2. seeks to the end to determine the remaining encoded length;
3. rejects `maxInputBytes` before decoding;
4. restores the initial position;
5. wraps the reader in a small `BufReader`; and
6. decodes rows through `png::Reader::next_row`.

The deterministic test encodes a 256 x 256 high-entropy PNG and then decodes it
through reads capped at 23 bytes. It requires more than 1,000 non-empty reads,
all 256 rows, exact encoded-length accounting, exact-limit acceptance, and a
limit-minus-one rejection.

Production integration accepts `Read + Seek`, enforces the encoded input
limit before reading PNG data, and explicitly rewinds between bounded metadata
and decode passes. Palette and transparency decisions use validated
`png::Info`; a small allocation-free chunk walk preserves strict raw `tRNS`
length validation before decoding. Ordered and Adam7 paths share the same
seekable input abstraction.

The native CLI now opens its source as `File` and sends output to the
existing failure-safe staged writer. It therefore retains neither a complete
encoded input vector nor a complete encoded output vector.

## Browser result

A JavaScript `ReadableStream` is asynchronous and one-way. It cannot satisfy the
current decoder's synchronous seekable-reader contract. Declaring an async
wrapper around the current API would still require collecting the complete
input before decoding and would not be incremental.

A genuine browser API needs a push-oriented decoder state machine that can
pause between chunks while retaining bounded PNG, inflate, unfilter, and
resampling state. The current supported choices are:

- obtain or contribute an upstream push-to-row API in `png`;
- implement and maintain the missing PNG row driver in streamthumb; or
- change dependencies after a separate security, correctness, size, and MSRV
  evaluation.

`SharedArrayBuffer`-based synchronous blocking is not considered because the
project does not require cross-origin isolation, shared memory, or threads.

## Decision

The native seekable-reader API is complete and the existing slice API remains a
compatibility wrapper. Do not expose `thumbnailPngFromStream` in WebAssembly
yet. Keep the complete `Uint8Array` input copy documented until a supported
push-to-row path exists. Any future JavaScript proposal must demonstrate
bounded retained input, real asynchronous backpressure, callback cancellation,
and parity for ordered and Adam7 images before becoming public.
