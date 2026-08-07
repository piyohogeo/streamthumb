# Security

`streamthumb` is designed to process untrusted PNG files within caller-selected resource limits. Security issues include panics, integer overflows, out-of-bounds access, unexpectedly unbounded allocation, limit bypasses, and disproportionate CPU use caused by crafted inputs.

## Resource boundary

The current implementation checks encoded input bytes before parsing and checks dimensions, source pixels, output dimensions, output pixels, and estimated working memory after IHDR and before IDAT decoding. The PNG decoder also receives its own allocation allowance. Encoded PNG and JPEG output is written through a bounded writer.

The working-memory budget covers decoder rows and staging, normalized source-row storage, one reusable completed output row, ordered-row or Adam7 sparse area accumulators, output RGBA storage where retained by the raw-output collector, codec state (including PNG conversion rows or a JPEG MCU row and temporary segment), and bounded encoded output where retained by the return type. Buffer-returning PNG and JPEG APIs retain the complete bounded encoded result. Direct Rust writer APIs enforce the same encoded byte cap without counting caller-owned destination storage as streamthumb working memory. The WebAssembly chunk API counts its 64 KiB adapter buffer but not JavaScript-owned chunk copies. It does not include:

- the caller-owned encoded input buffer;
- storage owned by a direct output writer;
- JavaScript memory outside WebAssembly;
- allocator bookkeeping and runtime code pages;
- process-wide memory used by unrelated work.

The current API does not impose a wall-clock deadline. Dimension and pixel limits bound the primary decode and resize work, but callers that require strict request deadlines must enforce them at the process, worker, or runtime level.

## Supported input contract

The thumbnail APIs accept static 1-, 2-, 4-, 8-, and 16-bit grayscale PNG files; 8- and 16-bit grayscale-alpha, RGB, and RGBA PNG files; and 1-, 2-, 4-, and 8-bit palette PNG files, with either no interlacing or Adam7 interlacing. Palette, grayscale, and RGB `tRNS` transparency are supported. The lower-level row callback API accepts only non-interlaced input because its contract requires complete rows in ascending order. APNG is rejected deterministically rather than silently converted.

Color-profile and transfer-function metadata is not interpreted during
resampling or copied into encoded output. Samples are averaged in their encoded
value space. Callers requiring linear-light processing or ICC color management
must use a color-managed pipeline instead.

## Fuzzing

Three libFuzzer targets cover row decoding, the fused PNG thumbnail API, and the codec-independent area downsampler. Scheduled Linux CI runs each target with AddressSanitizer. See `fuzz/README.md` for local commands and corpus provenance.

## Reporting

Do not publish exploit details or sensitive samples in a public issue. Use the repository host's private security-advisory mechanism when it is available. Include the smallest reproducer possible, the configured limits, target platform, and observed result.
