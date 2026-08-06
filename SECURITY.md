# Security

`streamthumb` is designed to process untrusted PNG files within caller-selected resource limits. Security issues include panics, integer overflows, out-of-bounds access, unexpectedly unbounded allocation, limit bypasses, and disproportionate CPU use caused by crafted inputs.

## Resource boundary

The current implementation checks encoded input bytes before parsing and checks dimensions, source pixels, output dimensions, output pixels, and estimated working memory after IHDR and before IDAT decoding. The PNG decoder also receives its own allocation allowance. Encoded PNG output is written through a bounded writer.

The working-memory budget covers decoder rows and staging, normalized source-row storage, ordered-row or Adam7 sparse area accumulators, output RGBA storage, encoder state, and bounded encoded output. It does not include:

- the caller-owned encoded input buffer;
- JavaScript memory outside WebAssembly;
- allocator bookkeeping and runtime code pages;
- process-wide memory used by unrelated work.

The current API does not impose a wall-clock deadline. Dimension and pixel limits bound the primary decode and resize work, but callers that require strict request deadlines must enforce them at the process, worker, or runtime level.

## Supported input contract

The thumbnail APIs accept static 8-bit grayscale, grayscale-alpha, RGB, and RGBA PNG files plus 1-, 2-, 4-, and 8-bit palette PNG files, with either no interlacing or Adam7 interlacing. Palette `tRNS` transparency is supported. Grayscale or RGB files that use a separate `tRNS` transparency chunk are rejected; callers should use an alpha color type. The lower-level row callback API accepts only non-interlaced input because its contract requires complete rows in ascending order. APNG and 16-bit inputs are rejected deterministically rather than silently converted.

## Fuzzing

Three libFuzzer targets cover row decoding, the fused PNG thumbnail API, and the codec-independent area downsampler. Scheduled Linux CI runs each target with AddressSanitizer. See `fuzz/README.md` for local commands and corpus provenance.

## Reporting

Do not publish exploit details or sensitive samples in a public issue. Use the repository host's private security-advisory mechanism when it is available. Include the smallest reproducer possible, the configured limits, target platform, and observed result.
