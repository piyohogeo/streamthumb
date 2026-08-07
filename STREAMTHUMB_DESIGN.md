# streamthumb — Design and Implementation Handoff

## 1. Overview

`streamthumb` is a memory-bounded PNG thumbnail generator written in Rust, with WebAssembly as a first-class target.

The core goal is to generate thumbnails from large or untrusted PNG files **without decoding the full source image into an RGBA frame buffer**.

The intended architecture fuses PNG decoding and downsampling:

```text
PNG byte stream
  -> incremental DEFLATE decode
  -> PNG scanline reconstruction
  -> streaming area downsampler
  -> small output image buffer
  -> thumbnail encoder
```

The project is not intended to replace `libpng`, `libvips`, or general-purpose image processing libraries. It should instead provide a small, predictable, resource-bounded component for environments where a full-frame decode is undesirable or impossible.

Representative environments include:

- Browser Web Workers
- Cloudflare Workers
- Other memory-constrained serverless runtimes
- Node.js and Deno
- Electron and Tauri
- Embedded or sandboxed runtimes
- Services processing very large or untrusted PNG files

Cloudflare Workers is an important example and benchmark target, but it must not define the public architecture or branding.

---

## 2. Problem Statement

Most high-level image libraries implement thumbnail generation roughly as follows:

```text
PNG file
  -> decode full image into RGBA
  -> resize RGBA image
  -> encode result
```

This causes peak memory usage to scale with source image area.

For example:

```text
16,384 x 16,384 x 4 bytes ~= 1 GiB
```

Even when the desired result is only a 512 x 512 thumbnail, a conventional pipeline may allocate the full decoded source image, intermediate buffers, decoder state, and output buffers.

PNG does not fundamentally require this. For non-interlaced PNG, each scanline can be reconstructed using the current row, the previous row, and the DEFLATE state. A downsampler can consume each reconstructed row and discard it after contributing to the thumbnail.

The desired peak-memory behavior is approximately:

```text
O(source_width * bytes_per_pixel)
+ O(output_width * accumulator_size)
+ O(output_width * output_height)
+ decoder and encoder state
```

rather than:

```text
O(source_width * source_height * bytes_per_pixel)
```

---

## 3. Product Positioning

### 3.1 Core value proposition

`streamthumb` should be presented as:

> A memory-bounded streaming PNG thumbnailer for Rust and WebAssembly.

Key claims should be measurable and concrete:

- Does not allocate a full-size source RGBA frame buffer.
- Supports configurable input limits.
- Provides predictable peak-memory behavior.
- Runs without threads, SharedArrayBuffer, a filesystem, or native dependencies.
- Suitable for untrusted and extremely large PNG inputs.
- Offers Rust-native and WASM APIs using the same core implementation.

### 3.2 What is not novel

The following are not sufficient differentiators by themselves:

- Decoding PNG in Rust
- Decoding PNG in WASM
- Reading PNG scanlines incrementally
- Creating thumbnails in WASM
- Low-memory image processing in general

Existing projects already cover these areas:

- `libvips` / `wasm-vips`
- Rust `png` crate
- `libspng`
- `jSquash`
- `wasm-image-optimization`

### 3.3 Intended differentiation

The project should differentiate itself through the combination of:

1. Decode and resize fusion
2. No full source-frame allocation
3. Explicit resource bounds
4. Small WASM bundle and narrow scope
5. No threads or SharedArrayBuffer requirements
6. First-class support for untrusted inputs
7. Adam7 support without materializing a full-resolution image
8. Stable and simple API for serverless and browser environments

---

## 4. Existing Projects and Competitive Context

### 4.1 wasm-vips

The closest general-purpose competitor.

Strengths:

- Mature low-memory image processing architecture
- Supports many image formats and operations
- Can create thumbnails directly
- Backed by libvips

Limitations relative to `streamthumb`:

- Large general-purpose dependency stack
- Not PNG-specific
- No simple peak-memory contract as the primary API promise
- May use whole-image strategies for smaller files
- Heavier WASM bundle
- Can require environment features that are awkward in restricted runtimes

`streamthumb` should not claim to invent low-memory thumbnailing. It should claim to provide a smaller, resource-bounded, PNG-focused implementation.

### 4.2 jSquash

Strengths:

- Browser and Workers-friendly WASM codec packages
- Modular codec packages
- Mature lineage from Squoosh

Limitation:

- Typical pipeline decodes into a full RGBA image and then resizes.
- Decode and downsampling are not fused.

### 4.3 wasm-image-optimization

Strengths:

- Broad conversion and resizing features
- Cloudflare Workers support
- Multiple input and output formats

Limitations:

- Large general-purpose package
- No strong bounded-memory contract
- Not designed around a fused streaming PNG-to-thumbnail path

### 4.4 Rust `png` crate

This should be the preferred decoding foundation unless implementation constraints prove otherwise.

Relevant capabilities:

- Mature Rust PNG decoder
- Scanline and streaming APIs
- Adam7 support
- Resource limits
- Strong ecosystem adoption and fuzzing history

The project should avoid writing a new PNG parser, DEFLATE implementation, or filter decoder unless a required streaming capability is missing.

---

## 5. Scope

## 5.1 MVP scope

The first usable version should support:

- Static PNG only
- Non-interlaced PNG
- 8-bit RGB
- 8-bit RGBA
- Palette PNG with optional `tRNS`, if supported cleanly by the chosen decoder API
- Box / area downsampling
- Preserve aspect ratio
- `contain` fit mode
- Configurable maximum output width and height
- Configurable maximum input width and height
- Configurable maximum input pixels
- Configurable maximum encoded input bytes
- Configurable maximum working memory
- Rust-native API
- WASM API through `wasm-bindgen`
- Input from `Uint8Array`
- Output as either RGBA or encoded PNG
- Deterministic errors for unsupported or oversized inputs
- Peak-memory and throughput benchmarks

### 5.2 Second-stage scope

After the MVP is stable:

- Adam7 interlaced PNG
- Grayscale PNG
- 1-, 2-, 4-, and 16-bit sample depths
- Correct alpha-aware area filtering
- sRGB and gamma-aware downsampling
- Additional fit modes: `cover`, exact dimensions, no-upscale
- Incremental input / streaming JS API
- Optional WebP or JPEG output feature
- Native async reader support
- Additional resize filters where compatible with bounded memory

### 5.3 Explicitly out of scope for MVP

- APNG animation
- Full ICC color management
- Arbitrary image editing
- General-purpose image transformations
- JPEG, WebP, TIFF, AVIF decoding
- Lanczos or large-support filters
- GPU acceleration
- Filesystem-based APIs as the primary interface
- Browser canvas dependency
- SharedArrayBuffer or thread requirements
- Full libpng compatibility for malformed legacy files

---

## 6. Architecture

## 6.1 Crate layout

Recommended workspace:

```text
streamthumb/
  Cargo.toml
  README.md
  LICENSE
  DESIGN.md
  crates/
    streamthumb-core/
      Cargo.toml
      src/
    streamthumb-png/
      Cargo.toml
      src/
    streamthumb-wasm/
      Cargo.toml
      src/
    streamthumb-cli/
      Cargo.toml
      src/
  benches/
  fuzz/
  testdata/
  examples/
    browser/
    cloudflare-worker/
    node/
```

Possible simplification for the first commit:

```text
crates/
  streamthumb/
  streamthumb-wasm/
```

Split the decoder adapter into its own crate only after the interfaces stabilize.

## 6.2 Layering

```text
streamthumb-core
  - geometry
  - limits
  - resize plan
  - row accumulators
  - alpha handling
  - output surface
  - errors

streamthumb-png
  - PNG metadata parsing through existing crate
  - color normalization
  - row and Adam7 event adapter
  - streaming decode orchestration

streamthumb-wasm
  - wasm-bindgen bindings
  - Uint8Array conversion
  - JS-friendly options and errors
  - optional streaming API

streamthumb-cli
  - test and benchmark utility
  - native reference frontend
```

## 6.3 Processing pipeline

### Non-interlaced PNG

```text
read header
  -> validate dimensions and limits
  -> determine output dimensions
  -> initialize row decoder
  -> initialize horizontal reducer
  -> initialize vertical accumulator

for each decoded source row:
  -> normalize source row to internal pixel representation
  -> horizontally reduce into output-width row
  -> accumulate vertically with exact area weights
  -> when an output row is complete:
       write it into the output buffer or encoder
  -> discard source row

finalize output
```

The implementation should not retain source rows beyond what the PNG decoder requires.

### Adam7 PNG

Adam7 must not allocate a full-resolution image.

Possible design:

- Maintain accumulators indexed by output pixel, not input pixel.
- For each pass sample, compute the source-space area or point contribution represented by that sample.
- Add weighted premultiplied color and alpha contributions into the corresponding output pixel or output span.
- Track coverage / weight per output pixel.
- Normalize after all passes.

This needs careful validation. If exact area reconstruction from Adam7 pass samples is awkward or incorrect, a bounded intermediate representation at thumbnail resolution is acceptable.

The invariant remains:

> memory scales with output size and source row width, not source image area.

---

## 7. Resampling

## 7.1 MVP filter

Use an area / box filter.

Reasons:

- Naturally streamable
- Small filter support
- Correct for large downscales
- Simple to test
- Deterministic memory behavior
- Good quality for thumbnails

The implementation should support arbitrary scale ratios, not only integer ratios such as 1/8.

For each source pixel interval, distribute its contribution across overlapping destination pixel intervals using rational or fixed-point weights.

Avoid floating-point accumulation if a robust fixed-point implementation is straightforward. Otherwise, use `f32` or `f64` initially and add deterministic tests.

## 7.2 Alpha handling

Downsampling RGBA must use premultiplied alpha internally.

Correct approach:

```text
premultiplied_rgb = rgb * alpha
accumulate premultiplied_rgb and alpha
normalize by total weight
unpremultiply where alpha > 0
```

This prevents colored halos around transparent edges.

The public API may expose an option for:

- preserving straight alpha output
- compositing against a background color

The MVP should preserve alpha.

## 7.3 Color space

MVP may initially average encoded sRGB values for simplicity, but this limitation must be documented.

Preferred later behavior:

- Detect `sRGB` / `gAMA` where practical
- Convert to linear-light values before averaging
- Convert back to sRGB for output

Do not implement full ICC color management in the MVP.

---

## 8. Resource Limits and Security

This project is intended for untrusted inputs. Limits are part of the public API, not optional implementation details.

## 8.1 Required limits

```rust
pub struct Limits {
    pub max_input_bytes: u64,
    pub max_width: u32,
    pub max_height: u32,
    pub max_pixels: u64,
    pub max_output_width: u32,
    pub max_output_height: u32,
    pub max_output_pixels: u64,
    pub max_working_memory_bytes: usize,
}
```

Optional future limits:

```rust
pub struct ProcessingBudget {
    pub max_decoded_samples: u64,
    pub max_chunks: u32,
    pub max_idat_bytes: u64,
    pub max_cpu_work_units: u64,
}
```

## 8.2 Failure behavior

The library must reject inputs before large allocations whenever possible.

Errors should distinguish:

- malformed PNG
- unsupported PNG feature
- input dimension limit exceeded
- input byte limit exceeded
- output dimension limit exceeded
- memory budget exceeded
- decode failure
- encode failure
- integer overflow
- truncated input

No panics on malformed input.

## 8.3 Memory budgeting

Before processing, estimate the required upper bound from:

- PNG decoder row buffers
- internal normalized row
- horizontal output row
- vertical accumulators
- output RGBA buffer or streaming encoder state
- WASM copy overhead where applicable

If the estimate exceeds `max_working_memory_bytes`, fail before decoding.

For WASM, avoid unnecessary copies between JS and Rust. The first API may still accept a complete `Uint8Array`, but the internal image decode must remain streaming.

## 8.4 Fuzzing

Use `cargo-fuzz` and target at least:

- PNG header and chunk parsing
- all PNG filters
- truncated IDAT data
- invalid dimensions
- decompression bombs
- malformed palettes and transparency
- integer overflow paths
- non-interlaced thumbnail pipeline
- Adam7 pipeline once added

Add a corpus from:

- PNG Suite
- generated edge cases
- reduced crash samples

---

## 9. Public Rust API

Proposed initial API:

```rust
use streamthumb::{ThumbnailOptions, ThumbnailOutput, Limits};

let output = streamthumb::thumbnail_png(
    input_bytes,
    &ThumbnailOptions {
        max_width: 512,
        max_height: 512,
        fit: Fit::Contain,
        allow_upscale: false,
        filter: Filter::Area,
        limits: Limits::default(),
        output: OutputFormat::Png,
    },
)?;

match output {
    ThumbnailOutput::Encoded {
        bytes,
        width,
        height,
        mime_type,
    } => { /* ... */ }
    ThumbnailOutput::Rgba {
        pixels,
        width,
        height,
    } => { /* ... */ }
}
```

If supporting streaming output is simple, prefer a writer-based API internally:

```rust
pub fn thumbnail_png_to_writer<R: Read, W: Write>(
    reader: R,
    writer: W,
    options: &ThumbnailOptions,
) -> Result<ThumbnailInfo>;
```

The byte-slice API can wrap this.

---

## 10. Public WASM API

Initial JS API:

```ts
import init, { thumbnailPng } from "@streamthumb/wasm";

await init();

const result = thumbnailPng(inputBytes, {
  maxWidth: 512,
  maxHeight: 512,
  fit: "contain",
  allowUpscale: false,
  output: "png",
  maxInputBytes: 64 * 1024 * 1024,
  maxInputPixels: 500_000_000,
  maxMemoryBytes: 32 * 1024 * 1024,
});

console.log(result.width, result.height, result.mimeType);
const thumbnailBytes = result.bytes;
```

Avoid requiring:

- DOM
- Canvas
- filesystem
- threads
- SharedArrayBuffer
- Node-specific APIs

The same package should work in:

- browser main thread
- Web Worker
- Cloudflare Worker
- Node.js
- Deno, where packaging permits

A future streaming API may accept a `ReadableStream<Uint8Array>`, but do not block the MVP on it.

---

## 11. Output Strategy

## 11.1 MVP recommendation

Support two outputs:

1. Raw RGBA
2. Encoded PNG

Raw RGBA is useful for browser Canvas or external encoders.

Encoded PNG is important for environments without Canvas, including Cloudflare Workers.

## 11.2 Future optional outputs

Feature-gated:

- WebP
- JPEG
- AVIF only if bundle size and complexity remain acceptable

Do not pull a large general-purpose codec stack into the default build.

---

## 12. Testing

## 12.1 Correctness tests

For each supported color type:

- Decode source with a trusted full-frame reference library.
- Resize using a reference area filter.
- Compare pixel output with streamthumb.
- Permit only a documented small tolerance if floating point is used.

Test dimensions:

- 1 x 1
- 1 x N
- N x 1
- exact integer downscales
- non-integer downscales
- output dimension 1
- source smaller than requested output
- very wide and very tall images
- transparent borders
- fully transparent colored pixels
- palette transparency

## 12.2 Memory tests

Create synthetic PNGs such as:

- 8K x 8K
- 16K x 16K
- very wide 100K x 32
- very tall 32 x 100K
- highly compressible blank images
- high-entropy images

Measure:

- native peak RSS
- WASM linear memory high-water mark
- temporary JS allocations where possible

The benchmark report must separate:

- encoded input size
- source dimensions
- output dimensions
- peak memory
- runtime
- output size

## 12.3 Differential tests

Compare against:

- image-rs full decode + resize
- jSquash decode + resize
- wasm-image-optimization
- wasm-vips
- libvips / `vipsthumbnail` native reference

The purpose is not necessarily to beat libvips on all metrics. The project should demonstrate:

- lower or more predictable memory than full-frame pipelines
- much smaller and simpler deployment than wasm-vips
- acceptable throughput
- strict resource-limit behavior

## 12.4 Browser/runtime matrix

Test at minimum:

- Chrome
- Firefox
- Safari if the WASM build supports it cleanly
- Node.js current LTS
- Cloudflare Workers compatibility date used in example

---

## 13. Benchmark Targets

Initial aspirational targets for a 512px maximum-dimension thumbnail:

- No allocation proportional to source image area
- Peak WASM memory under 32 MiB for common 8-bit RGB/RGBA inputs up to documented width limits
- Bundle size below 1 MiB compressed if practical
- No threads or SharedArrayBuffer
- Process 16K x 16K highly compressible PNG within a restricted memory runtime
- Reject decompression-bomb inputs deterministically based on configured limits

Do not publish guarantees until measured across representative files.

A future README should include a table like:

```text
Input             Output   Tool                   Peak memory   Time   WASM size
16K RGBA PNG      512px    streamthumb            ...           ...    ...
16K RGBA PNG      512px    jSquash                ...           ...    ...
16K RGBA PNG      512px    wasm-vips              ...           ...    ...
16K RGBA PNG      512px    wasm-image-opt         ...           ...    ...
```

---

## 14. Implementation Plan for Codex

### Phase 0 — Repository bootstrap

1. Create Rust workspace.
2. Add `streamthumb-core` and `streamthumb-wasm` crates.
3. Add CI for stable Rust, formatting, Clippy, tests, and WASM build.
4. Add MIT or Apache-2.0 license. Dual-license MIT OR Apache-2.0 is recommended.
5. Add basic README and DESIGN.md.

Deliverable:

- Empty but compiling workspace
- CI green
- WASM package builds

### Phase 1 — Geometry and resource planning

Implement:

- output dimension calculation
- `contain` fit
- no-upscale behavior
- checked arithmetic
- memory estimator
- limit validation
- error types

Tests should be exhaustive for geometry edge cases.

Deliverable:

- No PNG dependency required yet
- Pure unit-tested core

### Phase 2 — Streaming non-interlaced RGB/RGBA PNG

1. Integrate Rust `png` crate.
2. Read metadata and validate limits before allocation.
3. Decode scanlines incrementally.
4. Normalize 8-bit RGB/RGBA rows.
5. Do not allocate a full source image.
6. Initially emit decoded rows into a test sink.

Deliverable:

- Verified row-streaming decoder adapter
- Memory does not scale with source height

### Phase 3 — Streaming area downsampler

1. Implement arbitrary-ratio horizontal area reduction.
2. Implement vertical area accumulation.
3. Use premultiplied alpha for RGBA.
4. Produce final small RGBA buffer.
5. Differential-test against a full-frame reference.

Deliverable:

- Correct PNG-to-RGBA thumbnail path

### Phase 4 — PNG output

1. Encode the small RGBA result to PNG.
2. Add raw RGBA and PNG output options.
3. Add CLI for manual testing.

Deliverable:

```bash
streamthumb input.png output.png --max-width 512 --max-height 512
```

### Phase 5 — WASM bindings

1. Add `wasm-bindgen` bindings.
2. Accept `Uint8Array`.
3. Return JS-friendly object with bytes and metadata.
4. Add browser and Cloudflare Worker examples.
5. Avoid DOM and Node dependencies.

Deliverable:

- npm package build
- browser demo
- Cloudflare Worker example

### Phase 6 — Security and fuzzing

1. Add limits to all allocations and decode paths.
2. Add `cargo-fuzz` targets.
3. Add PNG Suite corpus.
4. Add malformed and bomb-like test cases.
5. Document unsupported features and rejection behavior.

Deliverable:

- Fuzz target running in CI on a bounded schedule or separately
- No panic on malformed inputs

### Phase 7 — Benchmarks

1. Add benchmark corpus generator.
2. Compare with full-frame image-rs path.
3. Compare with jSquash, wasm-vips, and wasm-image-optimization where practical.
4. Measure native RSS and WASM linear-memory high-water mark.
5. Publish reproducible scripts.

Deliverable:

- Benchmark report showing project value

### Phase 8 — Adam7

Only begin after non-interlaced support is stable.

1. Design thumbnail-resolution accumulation for Adam7 passes.
2. Avoid full source image allocation.
3. Differential-test against reference decode.
4. Add memory benchmarks.

Deliverable:

- Adam7 support with the same resource-bounded architecture

---

## 15. Technical Risks

### 15.1 Decoder API may still allocate more than expected

The selected PNG crate may allocate frame-sized buffers depending on API use or color conversion settings.

Mitigation:

- Inspect implementation and benchmark actual peak memory early.
- Use lower-level row APIs.
- Disable automatic whole-frame transformations.
- Keep normalization in streamthumb-owned row buffers.

### 15.2 WASM input copy may dominate memory

Accepting a complete `Uint8Array` can duplicate the compressed PNG in WASM memory.

Mitigation:

- Document that compressed input bytes count toward memory.
- Add a streaming reader API later.
- Avoid extra copies in bindings.
- Permit caller-owned or borrowed memory where possible.

### 15.3 Adam7 exactness

Mapping sparse pass samples directly into a correct area-filtered thumbnail may be subtle.

Mitigation:

- Treat thumbnail-resolution accumulation as the bounded intermediate representation.
- Compare exactly against a full decode reference.
- Defer Adam7 until MVP is stable.

### 15.4 Color correctness

Naive sRGB averaging and straight-alpha averaging can create visible artifacts.

Mitigation:

- Implement premultiplied-alpha averaging in MVP.
- Clearly document color-space limitations.
- Add linear-light mode later.

### 15.5 Project may appear redundant next to wasm-vips

Mitigation:

- Keep the package small.
- Publish memory guarantees and benchmark methodology.
- Emphasize restricted runtimes, untrusted inputs, and no full-frame allocation.
- Include a direct comparison section in README.

---

## 16. Non-Goals and Design Principles

### Non-goals

- Becoming a general image manipulation framework
- Reimplementing every PNG specification detail immediately
- Competing with libvips on breadth
- Chasing maximum resize quality at the expense of bounded memory
- Depending on a browser canvas or platform image decoder

### Design principles

1. Resource bounds are part of correctness.
2. Never allocate proportional to source image area.
3. Reject before allocating whenever possible.
4. Prefer mature decoder components over rewriting parsers.
5. Keep the core independent of WASM and JS.
6. Keep default dependencies narrow.
7. Make malformed-input behavior deterministic.
8. Benchmark actual peak memory, not only theoretical complexity.
9. Optimize for deployment simplicity in constrained runtimes.
10. Add formats and filters only when they preserve the project’s core contract.

---

## 17. Open Questions

Codex may choose conservative defaults, but should record decisions in DESIGN.md.

1. Use one crate or a workspace from the first commit?
2. Output PNG encoder: reuse Rust `png` crate or another encoder?
3. Internal accumulator type: integer fixed-point, `f32`, or `f64`?
4. Exact memory-budget accounting strategy for WASM?
5. How to measure WASM peak linear memory reproducibly?
6. Whether palette and grayscale support belongs in MVP or immediately after RGB/RGBA?
7. Whether encoded PNG output should stream directly or use a small full output buffer?
8. What default hard limits are safe and practical?
9. Which existing tool should be the principal comparison target: jSquash or wasm-vips?
10. Whether to expose a low-level row-consumer API for reuse by other formats later?

---

## 18. Definition of MVP Completion

The MVP is complete when all of the following are true:

- A valid non-interlaced 8-bit RGB/RGBA PNG can be converted to a bounded-size thumbnail.
- The implementation never allocates a full-resolution source RGBA image.
- Output matches a trusted full-frame area-resize reference within the documented tolerance.
- Malformed or oversized inputs fail without panic.
- Resource limits are configurable.
- Rust-native CLI works.
- WASM package works in a browser and Cloudflare Worker example.
- Benchmarks demonstrate peak memory scaling with source width and output size, not source area.
- README clearly explains the difference from wasm-vips, jSquash, and general-purpose image libraries.

---

## 19. Suggested Repository Metadata

Repository name:

```text
streamthumb
```

Suggested description:

```text
Memory-bounded streaming PNG thumbnail generation for Rust and WebAssembly.
```

Suggested topics:

```text
rust
webassembly
wasm
png
thumbnail
image-processing
streaming
serverless
cloudflare-workers
memory-efficient
```

Suggested package names:

```text
streamthumb
@streamthumb/wasm
```

---

## 20. First Codex Task

Use the following as the initial implementation prompt:

```text
Read DESIGN.md completely.

Bootstrap the streamthumb Rust workspace and implement Phase 0 and Phase 1 only:

- Create the core and WASM crates.
- Add CI for fmt, clippy, tests, and WASM build.
- Implement checked output-dimension calculation for contain fit.
- Implement configurable resource limits and a memory-estimation skeleton.
- Define public option, output metadata, and error types.
- Add comprehensive unit tests for geometry, overflow, no-upscale behavior, and limit rejection.
- Do not implement PNG decoding yet.
- Update DESIGN.md with any concrete decisions or deviations.
- Keep dependencies minimal.
```

---

## 21. Initial Implementation Decisions

The Phase 0 and Phase 1 bootstrap made the following concrete decisions:

1. The repository starts as a two-crate workspace: `streamthumb-core` and `streamthumb-wasm`. Decoder and CLI crates remain deferred until their interfaces are needed.
2. The workspace uses Rust 2024 with a minimum supported Rust version of 1.85.
3. The project is dual-licensed under MIT OR Apache-2.0.
4. Contain-fit geometry uses checked integer arithmetic and rounds the unconstrained axis down. This guarantees that the result never exceeds the requested bounds.
5. The initial accumulator model reserves five `u64` values per output pixel: premultiplied red, green, blue, alpha, and weight. This is a planning assumption rather than a final resampler representation.
6. The initial working-memory estimate covers two packed decoder rows, one normalized RGBA row, horizontal and vertical accumulators, and the final RGBA output buffer. Encoded input storage, decoder implementation overhead, encoder state, and WASM boundary copies are explicitly excluded until concrete implementations exist.
7. Initial conservative defaults are 64 MiB encoded input, 100,000 pixels per source axis, 500 million source pixels, 8,192 pixels per output axis, 16,777,216 output pixels, and 32 MiB estimated working memory.
8. Palette, grayscale, Adam7, streaming encoded output, and a reusable row-consumer API remain deferred. The MVP decoder work will start with non-interlaced 8-bit RGB and RGBA.
9. PNG encoding and the principal external comparison target remain open until decoder and benchmark work begins.
10. The CI WebAssembly check uses the standard `wasm32-unknown-unknown` target and does not require Node.js, a DOM, threads, or `SharedArrayBuffer`.

### Phase 2 decisions

1. PNG integration lives in a separate `streamthumb-png` crate so `streamthumb-core` remains independent of codecs and I/O.
2. The decoder dependency is `png` 0.18.1 with identity transformations. Unsupported color types and bit depths are rejected rather than transformed implicitly.
3. The current decoder accepts only non-interlaced 8-bit RGB and RGBA. Each decoded row is normalized into a reusable RGBA8 row buffer before being passed to a callback.
4. The callback receives a borrowed row that is valid only for the duration of the call. Retaining source rows is therefore an explicit caller choice rather than decoder behavior.
5. Encoded byte limits are checked before PNG parsing. Dimensions, pixel limits, output planning, and working-memory limits are checked after IHDR and before IDAT decoding.
6. The `png` crate's allocation limit is also configured as a defense-in-depth measure. Text and ICC profile chunks are ignored because they are unnecessary for the MVP pixel pipeline.
7. The memory model now reserves three packed source rows plus 160 KiB of conservative decoder staging space. This accounts for caller-visible decoded-row storage, filter reconstruction, buffered DEFLATE output, and the DEFLATE history window without allocating a source-height-sized buffer.
8. APNG and Adam7 remain rejected. Palette and grayscale transformations remain deferred so the supported input contract stays explicit.

### Phase 3 decisions

1. Area overlap is calculated with exact integer interval coordinates. Arbitrary ratios do not depend on floating-point accumulation or integer-only scale factors.
2. Horizontal and vertical accumulators use `u128`. This safely accommodates channel, alpha, area-weight, and maximum `u32` dimension products while keeping deterministic results across native and WebAssembly targets.
3. RGBA is accumulated in premultiplied-alpha form. Final pixels are converted back to straight alpha, and fully transparent output pixels use canonical zero RGB values.
4. The downsampler accepts rows only once and in source order. It retains one horizontal accumulator row, one vertical accumulator row, and the final bounded output buffer.
5. The memory estimator now uses the concrete `u128` accumulator size rather than the earlier provisional `u64` assumption.
6. `thumbnail_png_rgba` fuses row decoding and area downsampling without materializing a full-resolution source image.
7. Correctness tests compare the streaming integer implementation with an independent full-frame floating-point area reference over downscale and upscale ratios. A one-value channel tolerance is allowed for rounding differences in that differential test.

### Phase 4 decisions

1. `thumbnail_png` returns either `ThumbnailOutput::Rgba` or `ThumbnailOutput::Encoded` according to `ThumbnailOptions::output`.
2. PNG encoding operates only on the bounded thumbnail RGBA buffer. It never observes or allocates the full-resolution source image.
3. PNG output planning includes a conservative encoded-output allowance and 128 KiB of encoder state. The encoded destination uses a bounded writer and fails if the planned allowance is exceeded.
4. `thumbnail_png_rgba` always plans for raw RGBA output regardless of the output field passed by the caller. The unified `thumbnail_png` API should be used when the output field must control representation.
5. The initial CLI deliberately uses manual argument parsing to avoid a command-line framework dependency. It checks file metadata against the encoded-input limit before reading the complete file.

### Phase 5 decisions

1. The initial WebAssembly API accepts a complete `Uint8Array` and a plain JavaScript options object. Input and output currently incur boundary copies, which remain documented until an incremental API is implemented.
2. JavaScript option names use camelCase and map directly to the Rust geometry and resource-limit fields. Numeric limits must be non-negative JavaScript safe integers.
3. The result is a WebAssembly-backed class with `bytes`, `width`, `height`, `mimeType`, and `format` getters. Accessing `bytes` returns a JavaScript-owned copy.
4. The bindings depend only on `wasm-bindgen` and `js-sys`; they do not require DOM, Canvas, filesystem, threads, `SharedArrayBuffer`, or Node-specific APIs.
5. Browser usage is demonstrated in a module Web Worker. A separate Cloudflare Worker example accepts the source PNG as a POST body and returns the encoded thumbnail.

### Phase 6 decisions

1. The production PNG adapter contains no `unwrap`, `expect`, `panic`, or `unreachable` paths. Unsupported color branches return typed errors even after earlier validation, preserving the no-panic contract if internal call ordering changes.
2. Three `cargo-fuzz` targets cover row decoding, the fused PNG-to-thumbnail path, and the codec-independent area downsampler.
3. Fuzz limits are intentionally smaller than public defaults: 1 MiB encoded input, 4,096 pixels per source axis, roughly one million source pixels, 64 pixels per output axis, and 8 MiB working memory.
4. The seed corpus contains malformed signatures and a focused subset of Willem van Schaik's PNG Suite. The upstream permission notice is checked in with the binary fixtures.
5. Scheduled Linux CI runs every target for 60 seconds with the default AddressSanitizer instrumentation. Linux is the canonical fuzz runtime because Windows execution requires an ASan DLL matching Rust nightly's LLVM version.
6. Regression tests cover pixel bombs declared in IHDR, oversized truncated ancillary chunks, highly compressible inputs, early callback suppression, input byte limits, memory limits, APNG, Adam7, truncation, and all five PNG row filters.
7. The memory budget excludes caller-owned encoded input, JavaScript memory outside WebAssembly, allocator bookkeeping, runtime code pages, and unrelated process memory. These exclusions are documented as part of the security contract.
8. CPU use is bounded indirectly by byte, dimension, and pixel limits. A wall-clock deadline and explicit decoded-sample work budget remain future work and must currently be enforced by the host runtime.

### Phase 7 decisions

1. Benchmark-only code lives in an excluded `benchmarks` package, so image-rs and measurement tooling do not enter the production dependency graph.
2. The corpus generator writes PNG rows incrementally. The smoke profile covers square, wide, tall, compressible, gradient, and high-entropy inputs; the memory profile covers the design's 8K, 16K, 100K x 32, and 32 x 100K cases.
3. Native comparisons execute each method in a fresh process. Windows samples Peak Working Set every millisecond, while Linux uses GNU `time` maximum resident set size.
4. The image-rs baseline deliberately performs a full RGBA decode, Triangle resize, and PNG encode. It measures the cost of a conventional full-frame pipeline but is not a pixel-equivalent filter comparison.
5. The WASM binding exposes the current linear-memory byte length for measurement. Each benchmark case uses a fresh Node process and WASM instance, making the post-operation value a per-case linear-memory high-water mark.
6. WASM linear-memory results include allocator-retained pages and the copied encoded input. Node heap and other JavaScript allocations are reported separately through process RSS and are outside the linear-memory value.
7. The first local smoke baseline shows a 7.9 MiB streamthumb Peak RSS versus 36.9 MiB for image-rs on a 2,048-square blank source. All measured WASM smoke cases remain at or below 4 MiB linear memory.
8. jSquash, wasm-image-optimization, and wasm-vips remain future benchmark adapters. Comparisons will pin distributable artifacts and equivalent settings rather than adding them to production dependencies.

### Phase 8 decisions

1. Adam7 samples are accumulated directly at their original source coordinates into exact output-resolution `u128` accumulators. No deinterlaced source frame or source-area bitmap is allocated.
2. The sparse accumulator accepts pixels in arbitrary order and uses the same premultiplied-alpha and integer-overlap math as the ordered-row implementation.
3. Adam7 has a distinct conservative memory plan: ordered horizontal and vertical row accumulators are replaced by five `u128` values per bounded output pixel. Decoder rows, staging, output, and encoder allowances remain included.
4. The public thumbnail, CLI, and WASM paths accept static Adam7 RGB8 and RGBA8 PNGs. The lower-level row callback API continues to reject Adam7 because it promises complete normalized rows in ascending order.
5. The implementation derives pass coordinates from the seven PNG pass constants and validates decoded pass row lengths before accessing samples. Empty passes in narrow or short images are skipped.
6. Differential tests generate valid Adam7 data independently and compare RGB and RGBA thumbnails with the existing non-interlaced path. Exhaustive dimensions from 1 through 9 cover empty and partial passes.
7. Adam7-specific tests cover truncation and rejection before decoding when the sparse memory plan exceeds the caller's budget.
8. The benchmark package adds an `adam7` profile. A 2,048-square source measured 27.5 MiB native Peak RSS and 22.75 MiB WASM linear memory for a 512-square output, without a source-frame allocation.

### Phase 9 decisions

1. The first input-format expansion accepts 8-bit grayscale and grayscale-alpha PNGs in both non-interlaced and Adam7 forms.
2. Grayscale samples are normalized by copying the gray value into all three RGB channels. Grayscale-alpha samples preserve their alpha value, so the existing premultiplied-alpha area filter applies without a separate path.
3. Normalization remains stream-local: one decoded row for non-interlaced input and one sparse pass sample at a time for Adam7. No source-area RGBA buffer is introduced.
4. Decoder transformations remain set to identity. The supported contract is explicit and does not depend on implicit codec color conversion.
5. The memory planner uses one source byte per grayscale pixel and two per grayscale-alpha pixel while retaining the same bounded RGBA normalization and output allowances.
6. Separate `tRNS` transparency on grayscale or RGB input is rejected before rows are exposed because identity decoding does not expand it. Callers must use grayscale-alpha or RGBA until explicit `tRNS` support is implemented.
7. Tests cover row normalization, grayscale-alpha transparency, Adam7 equivalence with non-interlaced input, truncated grayscale-alpha Adam7 data, and early `tRNS` rejection. Palette and 16-bit inputs remain deferred.

### Phase 10 decisions

1. Palette PNG support accepts 1-, 2-, 4-, and 8-bit indices for both non-interlaced and Adam7 input. `PLTE` colors and optional `tRNS` alpha values are normalized to straight-alpha RGBA8.
2. Palette expansion is implemented inside streamthumb while the decoder remains in identity mode. This permits explicit validation instead of inheriting the codec's fallback for indices outside the declared palette.
3. The lookup table contains at most 256 RGBA entries. Missing `PLTE`, invalid palette lengths, too many entries for the declared bit depth, excess `tRNS` entries, and out-of-range pixel indices fail deterministically.
4. Packed indices are extracted most-significant-bit first within each byte. Row padding is discarded independently for every non-interlaced row and Adam7 pass row.
5. Memory planning conservatively reserves one packed source byte per pixel, the existing normalized RGBA row, and decoder staging. The lookup table is bounded to 1 KiB and fits within the documented decoder staging allowance.
6. Tests cover exact palette and alpha expansion, all four legal bit depths, non-interlaced versus Adam7 equivalence, omitted trailing `tRNS` alpha values, and malformed out-of-range indices in both decode orders.
7. Palette support flows through the existing Rust thumbnail, row callback, CLI, and WASM APIs without format-specific public options. Sixteen-bit samples remain deferred.

### Phase 11 decisions

1. Direct-color 16-bit support covers grayscale, grayscale-alpha, RGB, and RGBA in both non-interlaced and Adam7 forms. Palette indices remain limited to the PNG-defined 1-, 2-, 4-, and 8-bit depths.
2. Sixteen-bit samples are read as big-endian integers and mapped to 8-bit with nearest-integer scaling: `(value * 255 + 32767) / 65535`. This preserves both endpoints and uses low-byte information instead of truncating it.
3. Conversion occurs per source pixel in the existing row or Adam7 pass pipeline. The resampler continues to receive straight-alpha RGBA8 and therefore requires no 16-bit-specific accumulator path.
4. Memory planning reserves 2, 4, 6, or 8 source bytes per pixel for 16-bit grayscale, grayscale-alpha, RGB, or RGBA respectively. Normalized rows, sparse accumulators, and bounded outputs remain unchanged.
5. The decoder remains in identity transformation mode. Streamthumb validates source sample lengths and performs the documented conversion itself for deterministic native and WebAssembly output.
6. Tests cover exact representative rounding values, all four direct color types, 8-byte RGBA16 decoder-row accounting, non-interlaced versus Adam7 equivalence, and truncated 16-bit Adam7 input.
7. Sixteen-bit support flows through the existing Rust row callback, thumbnail, CLI, and WASM APIs. Separate grayscale/RGB `tRNS` transparency and APNG remain unsupported.

### Phase 12 decisions

1. Grayscale input now accepts every PNG-defined depth: 1, 2, 4, 8, and 16 bits, in both non-interlaced and Adam7 forms.
2. Packed grayscale samples are extracted most-significant-bit first with row padding discarded independently for every source or pass row. Values are scaled to 8-bit by nearest-integer mapping over the source depth's full range.
3. Grayscale `tRNS` is supported at every grayscale depth. Transparency comparison uses the original unscaled sample, avoiding collisions introduced by 8-bit normalization.
4. Raw direct-color `tRNS` chunk lengths are checked before metadata normalization because the codec shortens sub-16-bit transparency values internally. Grayscale requires exactly two encoded bytes; malformed lengths and values outside the declared sample depth fail before rows are exposed.
5. Packed grayscale memory planning conservatively reserves one source byte per pixel plus the existing RGBA normalization row. Eight- and sixteen-bit grayscale reserve one and two source bytes per pixel respectively.
6. Tests cover exact 1-, 2-, and 4-bit scaling, row padding, 8- and 16-bit grayscale transparency, all grayscale depths in Adam7 order, malformed `tRNS` lengths, out-of-range transparent samples, and packed-row memory accounting.
7. Low-bit grayscale and grayscale transparency flow through the existing Rust row callback, thumbnail, CLI, and WASM APIs. RGB `tRNS` and APNG remain unsupported.

### Phase 13 decisions

1. RGB `tRNS` transparency is supported for both 8- and 16-bit samples in non-interlaced and Adam7 input.
2. The transparent RGB triplet is compared with the original source samples before 16-bit-to-8-bit normalization. Distinct 16-bit colors that round to the same RGBA8 color therefore retain distinct transparency behavior.
3. The codec normalizes 8-bit RGB `tRNS` metadata from six encoded bytes to three bytes while retaining all six bytes for 16-bit input. Streamthumb validates the raw chunk length first and then parses either normalized representation explicitly.
4. RGB transparency adds only three `u16` values to source-format state. Row buffers, sparse accumulators, output memory, and public options are unchanged.
5. Tests cover exact 8-bit transparency, a 16-bit normalization-collision case, both depths in Adam7 order, and malformed five- and seven-byte `tRNS` chunks rejected before callbacks.
6. With RGB transparency implemented, the supported static PNG contract covers every standard PNG color type and legal bit depth, including Adam7 and the applicable `tRNS` forms. APNG animation remains out of scope.

### Phase 14 decisions

1. Browser runtime validation uses `wasm-bindgen-test` in a Dedicated Worker against the packaged API in headless Chrome and Firefox. These tests verify successful PNG generation and JavaScript option errors rather than stopping at a `wasm32-unknown-unknown` compile check.
2. The module Web Worker example includes a deterministic smoke page that loads a checked-in PNG fixture, generates a thumbnail in the worker, and decodes the returned PNG in the browser.
3. GitHub Actions runs the Chrome and Firefox WebAssembly tests on every normal CI invocation. The existing compile-only WebAssembly job remains as a faster diagnostic boundary.
4. Cloudflare Worker runtime validation is deferred because no deployment account is available. The adapter remains an architecture example but is explicitly outside the browser CI matrix.

### Phase 15 decisions

1. The first external WebAssembly comparison pins `@jsquash/png` 3.1.1 and `@jsquash/resize` 2.1.1 in a benchmark-only npm package and lockfile. These dependencies do not enter the production workspace.
2. The jSquash adapter measures PNG decode, Triangle resize, and PNG encode in a fresh Node process per corpus case. Output dimensions use streamthumb's no-upscale contain calculation.
3. jSquash resize uses premultiplied alpha and `linearRGB: false`. Because jSquash does not expose streamthumb's exact area filter, results compare end-to-end resources and runtime rather than pixel equivalence or image quality.
4. jSquash linear memory is the sum of its PNG and resize WebAssembly memories. Binary size likewise sums the two `.wasm` artifacts while excluding JavaScript glue for both projects.
5. The local smoke baseline records a 4.00 MiB streamthumb high-water versus 120.12 MiB for jSquash on a 2,048-square input. jSquash is faster in the same single run, so the report presents the result as a memory-versus-runtime tradeoff.
6. Normal CI executes a small jSquash adapter smoke case. The manual benchmark workflow runs jSquash for smoke and Adam7 profiles but skips the 16K memory profile, which requires a dedicated host with an explicit memory limit.

### Phase 16 decisions

1. The browser-targeted npm package is named `@streamthumb/wasm`, while the Rust crate remains `streamthumb-wasm`.
2. A repository-owned Node script wraps `wasm-pack` so package naming, repository metadata, exports, public-scope configuration, and included license files are deterministic.
3. Package validation runs `npm pack --dry-run --json`, enforces an exact file list, and rejects an unpacked package larger than 500,000 bytes.
4. Normal CI rebuilds and validates the package, creates a tarball without publishing it, and uploads that tarball as the `npm-package` artifact.
5. Publishing remains an explicit maintainer action after version, changelog, CI, artifact, and tag checks. CI does not receive an npm publishing token.

### Phase 17 decisions

1. Package CI installs the generated tarball into a fresh private consumer project instead of importing the build directory directly.
2. The consumer starts from the public `@streamthumb/wasm` specifier and relies on the package's default WebAssembly URL resolution.
3. Headless Chrome verifies package import, WebAssembly initialization, the exported version, thumbnail generation, PNG signature, and browser decoding of the output dimensions.
4. The smoke-test server exposes an exact route allowlist and binds only to loopback. Generated consumer files and the isolated Chrome profile remain under the ignored `target` directory.
5. Artifact upload occurs only after the installed-package browser smoke test passes. npm publication and Cloudflare runtime validation remain outside this phase.

### Phase 18 decisions

1. The WebAssembly package exports strict `ThumbnailOptions`, `ThumbnailFit`, `ThumbnailFilter`, and `ThumbnailOutputFormat` declarations through wasm-bindgen's custom TypeScript section.
2. The JavaScript runtime continues to accept omitted, undefined, or null options. The public TypeScript signature models this behavior with an optional `ThumbnailOptions | null` parameter.
3. The package checker requires the public option declarations and exactly one typed `thumbnailPng` signature in every generated package.
4. The tarball consumer pins TypeScript 7.0.2 and esbuild 0.28.1 in a dedicated lockfile. These tools remain test-only dependencies outside the published package.
5. CI runs strict type checking, including negative literal and property tests, bundles the TypeScript entry point as an ES module, and executes the resulting bundle in Chrome.

### Phase 19 decisions

1. A manually dispatched release-candidate workflow builds the npm tarball without publishing, tagging, or creating a GitHub release.
2. Release-candidate builds pin Rust 1.85.0, Node.js 24.14.1 with npm 11.11.0, wasm-pack 0.15.0, and install-action 2.85.9. Normal package, browser, and benchmark workflows use the same pinned wasm-pack installer.
3. The workspace version, generated npm version, tarball filename, and versioned changelog heading must agree before a manifest can be created.
4. The release manifest records package identity, source revision, exact tool versions, tarball byte size, and SHA-256. A conventional `.sha256` file is generated alongside it.
5. Manifest verification recomputes the hash and size, checks the source revision and pinned tools, and rejects unexpected files in the release-candidate artifact directory.
6. The release-candidate artifact contains exactly the tarball, JSON manifest, and checksum file. It is retained for 30 days for independent inspection.

### Phase 20 decisions

1. Node.js and Deno compatibility is tested from an installed npm tarball, not from the generated package directory or Rust crate.
2. Both runtimes resolve the public `@streamthumb/wasm` entry point, derive the adjacent WebAssembly URL, read the bytes with consumer-owned filesystem APIs, and pass those bytes to `init`.
3. Runtime-specific filesystem APIs remain outside the package. Browser consumers retain automatic URL fetching, while filesystem consumers control byte loading explicitly.
4. Node.js runs the JavaScript consumer under the Node 24 CI environment. Deno 2.9.5 runs the TypeScript consumer in manual node_modules mode with read permission restricted to the generated consumer directory.
5. Deno is installed with the pinned official `denoland/setup-deno` 2.0.5 action. The Deno consumer also validates the package's public TypeScript declarations during execution.
6. Both consumers verify version export, output dimensions, PNG MIME type, and PNG signature. Cloudflare runtime validation and npm publication remain excluded.

### Phase 21 decisions

1. Public browser, Node.js, and Deno examples use the canonical generated package or an installed release-candidate tarball rather than the retired repository-level `pkg` directory.
2. Normal CI executes the public browser example in headless Chrome and copies the Node.js and Deno example sources into the isolated tarball consumer before executing them.
3. The WebAssembly API contract records every option default, input and output representation, resource boundary, initialization mode, utility export, disposal behavior, and error category.
4. A repository check ties the documented defaults to the Rust implementation and rejects stale example package paths.
5. The Cloudflare Worker source remains aligned with the canonical package directory, but live-account validation remains explicitly deferred.

### Phase 22 decisions

1. The published npm README begins with consumer installation and API usage, while repository-only build instructions are kept in a separate development section.
2. Package preflight checks require the workspace and npm versions to match, enforce public metadata and keywords, compare both packaged license files byte-for-byte, and validate every public TypeScript export.
3. The release procedure binds a signed tag and GitHub Release to the exact source revision and already verified tarball. Release assets and the npm package must never be rebuilt after verification.
4. The `@streamthumb/wasm` name is not currently present in the public npm registry. Access to the `@streamthumb` scope remains an external publication prerequisite.

### Phase 23 decisions

1. The design completion audit is recorded in `docs/MVP_STATUS.md`, including implementation evidence and explicitly deferred external runtime checks.
2. The public contract now states that resampling averages encoded sample values, does not convert sRGB to linear light, ignores color-management metadata, and does not copy that metadata to encoded PNG output.
3. Dedicated-worker WebAssembly tests cover raw RGBA output, input and working-memory limit rejection, numeric validation, boolean validation, and every supported string-literal option boundary.
4. Cloudflare live validation and Safari automation remain deferred. These external gaps do not change the runtime-neutral package architecture or the tested Chrome, Firefox, Node.js, and Deno contract.
