# streamthumb — Streaming Output Extension Handoff

Implementation status: the row-sink refactor, streaming PNG encoder, PNG
configuration, JPEG encoder selection revision, and baseline JPEG output are
implemented on the current working branch. Historical “current” descriptions
below refer to the handoff baseline before these phases began.

## 0. Purpose

This document hands off the next implementation phase of `streamthumb` to Codex.

Repository:

- https://github.com/piyohogeo/streamthumb
- Current MVP status: complete on the current `main` branch
- Project goal: memory-bounded thumbnail generation for Rust and WebAssembly

The next phase extends the MVP in three directions:

1. **Do not materialize the complete resized RGBA thumbnail for encoded outputs**
   - The resampler should emit completed thumbnail rows incrementally.
   - PNG and JPEG encoders should consume those rows incrementally.
2. **Add JPEG output**
   - Baseline sequential JPEG only for the first implementation.
   - Keep the memory-bounded design.
   - Transparent source pixels must be composited against a configurable background.
3. **Add PNG output configuration**
   - Output color mode
   - Compression level
   - PNG filter strategy
   - Keep 8-bit output only for now.

This phase should preserve the current MVP guarantees and public behavior unless an intentionally versioned API extension is documented.

---

## 1. Current architecture

The workspace currently contains:

```text
streamthumb-core
streamthumb-png
streamthumb-wasm
streamthumb-cli
```

The current pipeline is conceptually:

```text
encoded PNG input
      ↓
row-oriented PNG decoder
      ↓
normalized source RGBA8 rows
      ↓
AreaDownsampler / SparseAreaDownsampler
      ↓
complete resized RgbaImage
      ↓
PNG encoder
      ↓
complete encoded Vec<u8>
```

The important MVP property is already achieved:

> A complete full-resolution source RGBA image is never allocated.

However, for encoded PNG output, the current implementation still retains:

- the complete resized RGBA output
- the complete encoded PNG output

The memory planner explicitly accounts for both.

For `OutputFormat::Rgba`, retaining the complete resized RGBA image is the intended public result and should remain supported.

---

## 2. New target architecture

For encoded outputs, change the pipeline to:

```text
encoded PNG input
      ↓
row-oriented PNG decoder
      ↓
normalized source RGBA8 rows
      ↓
streaming area downsampler
      ↓
completed thumbnail RGBA8 row
      ↓
codec-specific row sink
      ↓
PNG / JPEG encoder
      ↓
bounded encoded output
```

The central architectural change is:

> **The resampler should no longer require ownership of a complete output RGBA frame for encoded outputs.**

Instead, finalized output rows should be emitted to a downstream sink.

The implementation must still support raw RGBA output by attaching an RGBA collector sink.

---

## 3. Core abstraction: output row sink

Introduce a codec-independent output-row abstraction in `streamthumb-core`.

A possible shape:

```rust
pub trait RgbaRowSink {
    type Output;

    fn push_row(&mut self, y: u32, rgba: &[u8]) -> Result<()>;
    fn finish(self) -> Result<Self::Output>;
}
```

Exact naming may differ if a better API emerges, but the important properties are:

- accepts one completed thumbnail row
- row is tightly packed straight-alpha RGBA8
- rows are delivered in ascending `y`
- row slice is temporary and must not be retained unless copied
- codec implementation is not visible to the resampler
- output type is sink-specific

Potential sinks:

```text
RgbaCollector
PngEncoderSink
JpegEncoderSink
```

Avoid making the downsampler depend directly on PNG/JPEG crates.

---

## 4. Refactor `AreaDownsampler`

The current `AreaDownsampler` keeps:

```text
output_pixels: Vec<u8>
```

and appends completed output rows in `finalize_output_row()`.

Refactor so that the non-interlaced path can finalize one output row and immediately pass it to a sink.

Desired conceptual flow:

```text
source row
   ↓
reduce_horizontal()
   ↓
accumulate_vertical()
   ↓
when output row is complete:
    normalize accumulator
    write temporary RGBA row buffer
    sink.push_row(...)
    clear vertical accumulator
```

The resampler should only need memory proportional to:

- source row / normalized row
- horizontal accumulator width
- vertical accumulator width
- one output RGBA row
- sink state

It should not need `output_width * output_height * 4` for encoded outputs.

### Compatibility requirement

Raw RGBA output must still work.

Implement an `RgbaCollector` sink which appends every emitted output row into a bounded `Vec<u8>` and returns the current `RgbaImage`.

This preserves the current public behavior while making full-frame RGBA storage an explicit sink choice rather than a mandatory resampler property.

---

## 5. Adam7 handling

Adam7 is different from the normal ordered-row path.

The current `SparseAreaDownsampler` accepts source pixels in arbitrary order and keeps one accumulator per output pixel.

That output-sized accumulator storage remains necessary because Adam7 delivers source samples in interlaced pass order.

Do **not** attempt to force Adam7 into the same streaming finalization behavior as non-interlaced input.

Instead:

```text
Adam7 source
   ↓
SparseAreaDownsampler
   ↓
output-sized accumulators remain until all source samples arrive
   ↓
after final Adam7 pass:
    normalize output row 0 → sink
    normalize output row 1 → sink
    ...
```

The important improvement is:

> Adam7 requires output-sized accumulators, but should no longer allocate an additional complete RGBA output frame for encoded PNG/JPEG output.

A suitable API may be:

```rust
SparseAreaDownsampler::finish_into(sink)
```

or equivalent.

---

## 6. PNG streaming encoder

The current dependency is:

```toml
png = "0.18.1"
```

Keep this dependency unless there is a compelling reason to change.

Use the `png` crate's streaming writer functionality rather than waiting for a complete RGBA image and calling a full-frame encode method.

Desired flow:

```text
thumbnail RGBA row
      ↓
optional color conversion
      ↓
png streaming writer
      ↓
bounded encoded writer
```

No full resized RGBA image should be required.

### Encoded output buffering

Do not conflate two different meanings of "streaming".

#### Phase A — required in this extension

Incrementally encode completed thumbnail rows:

```text
resampler row
→ encoder
```

but it is acceptable for the compressed PNG/JPEG bytes to still be accumulated in a bounded `Vec<u8>` for the existing WASM API.

#### Phase B — optional later

Expose encoded bytes incrementally to:

- `std::io::Write`
- file output
- object storage
- JavaScript chunk callbacks / streams

Do not block Phase A on Phase B.

---

## 7. PNG output options

Introduce codec-specific PNG configuration instead of flattening every codec option directly into generic thumbnail options.

Suggested Rust model:

```rust
pub struct PngOptions {
    pub color: PngColorMode,
    pub compression: PngCompression,
    pub filter: PngFilter,
}
```

Suggested enums:

```rust
pub enum PngColorMode {
    Auto,
    Rgba8,
    Rgb8,
    GrayscaleAlpha8,
    Grayscale8,
}

pub enum PngCompression {
    NoCompression,
    Fastest,
    Fast,
    Balanced,
    High,
}

pub enum PngFilter {
    Default,
    None,
    Sub,
    Up,
    Average,
    Paeth,
    Adaptive,
    MinEntropy,
}
```

Naming should align reasonably with the underlying `png` crate while keeping the public API independent enough to change encoder implementations later.

### 7.1 Bit depth

For this phase:

> **PNG output remains 8-bit only.**

The current pipeline normalizes source samples to RGBA8 before resampling. Supporting a 16-bit output option without preserving higher precision internally would be misleading.

Do not expose 16-bit PNG output yet.

---

## 8. PNG color-mode behavior

The current implementation always encodes RGBA8. The extension should allow smaller output representations.

### Explicit modes

- `Rgba8`
- `Rgb8`
- `GrayscaleAlpha8`
- `Grayscale8`

If the requested mode cannot represent the generated thumbnail without information loss beyond the documented conversion, either perform a documented deterministic conversion or reject the request. Prefer explicit and unsurprising behavior.

### Auto mode

`Auto` must **not** mean:

> Encode the whole output, inspect it, and retroactively choose the smallest color type.

PNG color type is declared before image data, so that conflicts with true streaming encoding.

Instead define `Auto` as:

> Choose the smallest output color type that can be proven safe from input metadata and source format before output rows are encoded.

Safe examples:

```text
source grayscale with no transparency
    → Grayscale8

source grayscale with alpha / applicable tRNS
    → GrayscaleAlpha8

source RGB with no transparency
    → Rgb8

source RGB with transparency
    → Rgba8

source RGBA
    → Rgba8
```

For palette input, use a conservative policy initially:

- if palette analysis proves grayscale/transparency characteristics safely, choose an appropriate gray format
- otherwise choose RGB/RGBA
- do not attempt indexed-color PNG output in this phase

Document this clearly.

---

## 9. PNG output interlacing

Do not add Adam7 output in this phase.

Output PNG should remain:

> **non-interlaced PNG**

The purpose of this project is bounded streaming thumbnail generation, not full PNG feature parity.

Input Adam7 remains supported.

---

## 10. JPEG output

Add JPEG as a new encoded output format.

Extend:

```rust
pub enum OutputFormat {
    Png,
    Jpeg,
    Rgba,
}
```

or use a more structured encoding-selection API if that better accommodates codec-specific options.

### First JPEG scope

Support:

- baseline sequential JPEG
- RGBA source rows converted/composited to RGB
- configurable quality
- configurable background color for transparency
- optionally configurable chroma subsampling if supported cleanly

Do **not** initially support:

- progressive JPEG
- trellis optimization
- multi-pass Huffman optimization requiring whole-image retention
- ICC/profile preservation
- CMYK
- lossless JPEG variants

These features fight the primary streaming/memory objective.

---

## 11. JPEG transparency handling

JPEG does not support alpha.

For each RGBA thumbnail row:

```text
straight-alpha RGBA
      ↓
composite against configured background
      ↓
RGB
      ↓
JPEG encoder
```

Suggested configuration:

```rust
pub struct JpegOptions {
    pub quality: u8,
    pub background: [u8; 3],
    pub subsampling: JpegSubsampling,
}
```

Sensible defaults:

```text
quality = 85
background = white
subsampling = 4:2:0
```

If the chosen encoder does not expose useful subsampling control without complexity, defer subsampling configuration and document the encoder default.

Background compositing must be tested carefully around transparent and partially transparent edges.

---

## 12. JPEG encoder dependency selection

Do not choose a JPEG library solely from familiarity.

Run a short implementation spike comparing suitable current pure-Rust candidates.

Selection criteria:

1. builds on `wasm32-unknown-unknown`
2. compatible with the workspace `unsafe_code = "forbid"` policy
3. supports row-oriented / streaming encoding
4. bounded memory behavior
5. reasonable WASM binary-size increase
6. acceptable thumbnail encode performance
7. maintained project with a usable license
8. supports baseline sequential JPEG cleanly

Before final adoption, document:

```text
dependency
version
WASM binary size delta
512px encode time
peak memory
supported quality/subsampling controls
license
```

Avoid bringing a large C/C++ stack into the WASM package unless there is a compelling measured reason.

---

## 13. Crate organization

The current `streamthumb-png` crate contains PNG decoding and PNG encoding.

Once JPEG output is added, putting JPEG encoding into `streamthumb-png` becomes conceptually awkward.

Preferred direction:

```text
streamthumb-core
    dimensions
    limits
    options
    planning
    resampler
    row sink abstraction

streamthumb-png
    PNG input decoding
    PNG source-format normalization
    Adam7 source handling

streamthumb-encode
    PNG encoder sink
    JPEG encoder sink
    codec-specific output options

streamthumb-wasm
streamthumb-cli
```

Exact naming may change (`streamthumb-codecs`, `streamthumb-output`, etc.).

Do not over-engineer the split if it causes excessive churn, but avoid making JPEG output a permanent responsibility of a crate named `streamthumb-png`.

---

## 14. Memory planner changes

The current `MemoryEstimate` includes:

```text
decoder_rows_bytes
decoder_staging_bytes
normalized_row_bytes
horizontal_accumulator_bytes
vertical_accumulator_bytes
sparse_accumulator_bytes
output_rgba_bytes
encoder_state_bytes
encoded_output_bytes
total_bytes
```

Update the planner to reflect the new execution path.

### Raw RGBA output

Still include:

```text
output_rgba_bytes = output_width * output_height * 4
```

because this is the requested public result.

### Encoded PNG/JPEG returned as Vec

For non-interlaced input:

```text
output_rgba_bytes = 0
output_row_bytes = O(output_width)
encoder_state_bytes = codec-specific conservative allowance
encoded_output_bytes = bounded encoded Vec
```

For Adam7 input:

```text
sparse_accumulator_bytes = O(output_area)
output_rgba_bytes = 0
output_row_bytes = O(output_width)
encoder_state_bytes = codec-specific allowance
encoded_output_bytes = bounded encoded Vec
```

### Future writer/chunk API

When a direct writer API is eventually implemented:

```text
encoded_output_bytes = 0
```

and only bounded codec state + output chunk storage should remain.

Consider adding/renaming fields so the planner clearly exposes:

```text
output_row_bytes
output_rgba_bytes
encoder_state_bytes
encoded_output_bytes
```

Do not make memory estimates silently optimistic.

---

## 15. Encoded output bounds

The project currently uses a conservative `BoundedWriter` for encoded PNG output.

Generalize this so PNG and JPEG can share a bounded encoded-output writer.

It should:

- never allocate beyond the configured encoded-output allowance
- report a typed error on limit exceed
- preserve the current security/resource-limit philosophy
- avoid reserving absurd amounts if the conservative upper bound is very large

Review whether `try_reserve_exact(limit)` remains appropriate for all codec bounds. If a worst-case encoded bound can become very large, a growing writer with an explicit cap may be safer than pre-reserving the full cap.

---

## 16. WASM API extension

Current TypeScript output:

```ts
type ThumbnailOutputFormat = "png" | "rgba";
```

Extend to:

```ts
type ThumbnailOutputFormat = "png" | "jpeg" | "rgba";
```

Possible API:

```ts
export interface PngOptions {
  color?: "auto" | "rgba8" | "rgb8" | "grayscale-alpha8" | "grayscale8";
  compression?: "none" | "fastest" | "fast" | "balanced" | "high";
  filter?: "default" | "none" | "sub" | "up" | "average" | "paeth" | "adaptive" | "min-entropy";
}

export interface JpegOptions {
  quality?: number;
  background?: [number, number, number];
  subsampling?: "420" | "422" | "444";
}

export interface ThumbnailOptions {
  // existing generic options...
  output?: "png" | "jpeg" | "rgba";
  png?: PngOptions;
  jpeg?: JpegOptions;
}
```

If nested option parsing becomes awkward, a codec-discriminated configuration object is also acceptable.

Requirements:

- invalid option combinations fail deterministically
- PNG-only settings with JPEG output should preferably be rejected
- JPEG-only settings with PNG output should preferably be rejected
- defaults documented and tested
- TypeScript declarations and Rust defaults kept in sync

Result behavior:

```text
PNG  → mimeType = image/png,  format = png
JPEG → mimeType = image/jpeg, format = jpeg
RGBA → application/octet-stream, format = rgba
```

---

## 17. CLI extension

Current CLI assumes PNG output.

Extend based on filename extension and/or explicit flag:

```text
streamthumb input.png output.png
streamthumb input.png output.jpg
```

Optional explicit override:

```text
--format png
--format jpeg
```

Possible codec options:

```text
--png-color auto
--png-compression balanced
--png-filter adaptive

--jpeg-quality 85
--jpeg-background "#ffffff"
```

Avoid making the CLI the primary design driver. Rust and WASM APIs are more important.

---

## 18. Color handling

Current documented behavior:

- resampling is premultiplied-alpha aware
- channels are averaged in encoded sample space
- no conversion to linear light
- no ICC color management
- PNG color metadata is not copied

Preserve this behavior unless this phase intentionally expands color management.

Document:

> The encoder writes deterministic output based on current RGBA8 sample values; it does not perform ICC transforms or linear-light resampling.

Verify whether the PNG encoder writes any color metadata automatically and document the actual behavior.

---

## 19. Tests

Do not accept this extension without tests proving the memory architecture actually changed.

### 19.1 Core row-sink tests

Test:

- output rows arrive in ascending order
- each row arrives exactly once
- row lengths are exact
- final output equals the previous full-frame algorithm
- transparent-edge behavior remains unchanged
- arbitrary resize ratios remain correct

Use the current independent full-frame/reference comparisons.

### 19.2 Non-interlaced PNG tests

For PNG encoded output:

- decoded output pixels match old output where expected
- no complete output `RgbaImage` allocation is required internally
- all supported source color types still work
- `tRNS` behavior preserved
- malformed/oversized inputs still fail correctly

### 19.3 Adam7 tests

Test:

- all supported Adam7 formats still work
- result matches non-interlaced equivalent
- sparse accumulator remains output-area bounded
- no additional full output RGBA buffer is counted/allocated for encoded output

### 19.4 PNG encoder option tests

For each color mode:

- IHDR color type is correct
- dimensions correct
- decoded pixels correct

For compression/filter:

- option maps to intended encoder behavior
- output remains valid PNG
- deterministic defaults

Avoid tests that require a particular compressed byte size unless guaranteed by the encoder.

### 19.5 JPEG tests

Test:

- JPEG decodes to correct dimensions
- opaque RGB thumbnails are numerically/visually close to reference
- alpha compositing against background is correct before lossy encode
- quality range validation
- output signature / MIME
- baseline sequential format
- malformed JPEG options rejected

Lossy JPEG pixel tests should use sensible tolerance metrics rather than exact equality.

### 19.6 Memory tests

Critical acceptance criteria:

For encoded non-interlaced PNG/JPEG:

> peak planned storage must no longer include `output_width * output_height * 4`.

For Adam7 encoded output:

> output-area accumulators remain, but a second full RGBA output frame must not be allocated.

Add at least one benchmark with a larger thumbnail output so the removed full-frame RGBA allocation is measurable.

---

## 20. Fuzzing

Extend fuzzing after architecture stabilization.

Add/adjust targets for:

```text
PNG → streaming PNG encode
PNG → streaming JPEG encode
row sink transitions
PNG option parsing / encoder configuration
```

Assertions:

- no panic
- no unbounded allocation
- resource limits respected
- invalid codec options rejected
- bounded encoded output respected

Keep fuzz targets small and deterministic.

---

## 21. Benchmarks

Update the benchmark report with separate memory stages.

Compare:

```text
current v0.1 full resized RGBA → PNG
new row-streamed RGBA → PNG
new row-streamed RGBA → JPEG
```

Measure:

- native peak RSS
- WASM linear-memory high-water mark
- runtime
- encoded size
- WASM binary size

Test categories:

```text
large source → 512 thumbnail
large source → 2048 thumbnail
very tall source
very wide source
Adam7 source
opaque RGB
RGBA with transparency
```

The key expected property is:

> encoded-output memory should depend on output row width + codec state + encoded bytes, not on a complete uncompressed output frame.

---

## 22. Backward compatibility

Keep the current high-level API working where practical:

```rust
thumbnail_png(input, options)
```

Existing PNG and RGBA behavior should remain valid.

Adding JPEG should not force callers to use the low-level row-sink API.

Recommended layered design:

```text
high-level convenience API
    ↓
codec-independent streaming pipeline
    ↓
row sink
```

Low-level streaming-to-writer APIs can be added later without breaking convenience callers.

---

## 23. Suggested PR / implementation sequence

Do not implement everything in one change.

### PR 1 — Row-sink architecture

Goal:

- refactor `AreaDownsampler`
- refactor `SparseAreaDownsampler`
- introduce `RgbaCollector`
- preserve all existing public behavior
- no JPEG yet
- no PNG option expansion yet

Acceptance:

- all existing tests pass
- output pixels remain equivalent
- memory model can represent row-output architecture

### PR 2 — Streaming PNG encoder

Goal:

- use PNG streaming writer
- connect resampler row output directly to PNG encoder
- remove full resized RGBA allocation for PNG encoded output
- update memory planner/benchmarks

Acceptance:

- encoded PNG path has `output_rgba_bytes == 0`
- PNG output remains valid and behavior-compatible

### PR 3 — PNG encoder options

Goal:

- color mode
- compression
- filter strategy
- WASM/CLI exposure
- docs/tests

### PR 4 — JPEG encoder spike + selection

Goal:

- compare candidate libraries
- document choice
- no production dependency until decision is recorded

### PR 5 — JPEG output

Goal:

- baseline sequential JPEG
- configurable quality/background
- optional subsampling
- Rust/WASM/CLI support
- memory planning/tests/benchmarks

### PR 6 — Optional direct writer / chunked output API

Only if justified after the previous work.

Potential APIs:

```rust
thumbnail_png_to_writer(...)
thumbnail_jpeg_to_writer(...)
```

For WASM, prefer buffered chunks rather than one JS callback per row.

---

## 24. Non-goals

Do not expand scope into:

- JPEG input
- WebP input/output
- AVIF
- APNG
- progressive JPEG
- Adam7 output
- indexed PNG output
- 16-bit output
- ICC transforms
- linear-light resampling
- arbitrary rotations/crops
- full generic image-processing framework
- SharedArrayBuffer/thread requirements

The project should remain a focused, resource-bounded thumbnailer.

---

## 25. Key design principle

The key architectural principle for this phase is:

> **A finalized output row should be disposable.**

Once a thumbnail row has been normalized and passed to the selected sink:

- the resampler should not need it again
- PNG/JPEG encoder may consume it immediately
- only raw-RGBA callers should intentionally retain it

For non-interlaced input, this gives a genuine decode → resize → encode streaming pipeline.

For Adam7 input, source ordering prevents early output-row finalization, but the final RGBA frame should still not be duplicated after the sparse accumulators have completed.

---

## 26. Definition of done

This extension is complete when:

1. Existing MVP behavior remains supported.
2. Encoded PNG output no longer requires a complete resized RGBA frame.
3. Adam7 encoded output does not allocate a second complete RGBA frame after sparse accumulation.
4. PNG output supports documented color/compression/filter options.
5. JPEG baseline sequential output works in native Rust and WASM.
6. JPEG alpha compositing is configurable and tested.
7. Memory estimates reflect the actual new architecture conservatively.
8. Fuzzing covers the new encoder paths.
9. Benchmarks demonstrate the changed memory scaling.
10. Documentation clearly separates:
    - raw RGBA output
    - row-streamed encoding
    - complete encoded-byte buffering
    - any future direct-writer/chunk API.
11. `cargo fmt`, `clippy -D warnings`, native tests, WASM checks, browser tests, Node/Deno package tests, fuzz builds, and relevant benchmark smoke tests pass.
12. No new unsafe Rust is introduced.

---

## 27. Initial Codex instruction

Suggested first prompt for Codex:

```text
Read this handoff document and the current streamthumb repository before making changes.

Implement only PR 1: introduce a codec-independent row-sink architecture without changing public behavior.

Requirements:
- Refactor AreaDownsampler so completed thumbnail rows can be emitted without permanently storing the full output frame.
- Refactor SparseAreaDownsampler so its completed output can be emitted row-by-row after all Adam7 samples arrive.
- Implement an RgbaCollector sink so the existing raw RGBA and encoded PNG high-level APIs continue to behave exactly as before.
- Do not add JPEG yet.
- Do not add PNG encoder options yet.
- Update memory-planning primitives only as necessary to represent the new architecture, but preserve current public estimates until the PNG encoder is converted in the next PR.
- Add differential tests proving output equivalence with the current implementation.
- Run formatting, clippy, workspace tests, wasm32 check, and existing browser/package tests.
- Update the design/status documentation with the architectural decision and any deviations.

Do not proceed to JPEG until the row-sink refactor is clean and independently reviewable.
```
