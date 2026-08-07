# WebAssembly API contract

The `@streamthumb/wasm` package exposes a runtime-neutral byte-array API for bounded PNG-input thumbnail generation with PNG, JPEG, or raw RGBA output. It does not depend on DOM, Canvas, filesystem, threads, `SharedArrayBuffer`, or runtime-specific APIs.

## Initialization

Import the default initializer and call it once before using the named exports:

```js
import init, {
  planThumbnailPng,
  thumbnailPng,
  thumbnailPngToChunks,
} from "@streamthumb/wasm";

await init();
```

Browsers can let the initializer fetch the adjacent WebAssembly file. Filesystem runtimes should resolve the package module, read `streamthumb_wasm_bg.wasm`, and pass its bytes as `module_or_path`. The [Node.js](../examples/node) and [Deno](../examples/deno) examples show that pattern.

Initialization is asynchronous. `planThumbnailPng`, `thumbnailPng`, and
`thumbnailPngToChunks` are synchronous after initialization.

Advanced consumers may instead call `initSync({ module })` with WebAssembly
bytes or a precompiled `WebAssembly.Module`. The asynchronous initializer is
preferred unless the caller already owns a synchronously available module.

## Function

```ts
type OutputDelivery = "buffered" | "chunks";

function planThumbnailPng(
  input: Uint8Array,
  options?: ThumbnailOptions | null,
  delivery?: OutputDelivery | null,
): ThumbnailPlan;

function thumbnailPng(
  input: Uint8Array,
  options?: ThumbnailOptions | null,
): ThumbnailResult;

function thumbnailPngToChunks(
  input: Uint8Array,
  onChunk: (chunk: Uint8Array) => void,
  options?: ThumbnailOptions | null,
): ChunkedThumbnailResult;
```

`input` contains one encoded PNG. Passing it into WebAssembly copies the input bytes. An omitted, `undefined`, or `null` options value selects every default below.

`planThumbnailPng` validates PNG header metadata, options, and every input and
output resource limit without decoding image pixels. It uses the same Rust
geometry and memory planner as execution. The default `"buffered"` delivery
matches `thumbnailPng`; `"chunks"` matches `thumbnailPngToChunks` and includes
its 64 KiB adapter buffer. Chunk delivery supports only PNG and JPEG output.

The planning call does not enforce only the final `maxMemoryBytes` comparison.
Instead, it returns the required conservative total, configured limit, and a
typed `withinMemoryLimit` result. All other validation still throws. The
thumbnail functions independently plan again and continue to enforce the
working-memory limit before processing.

`thumbnailPngToChunks` supports encoded PNG and JPEG output. It calls `onChunk`
synchronously while encoding and does not retain the complete encoded result in
WebAssembly. Chunks contain at most 64 KiB and each callback receives a new,
JavaScript-owned `Uint8Array` that remains valid after the callback returns.
The callback must not assume asynchronous backpressure: it should return
promptly or copy/queue the chunk under an application-defined limit. A thrown
callback value aborts processing and is rethrown unchanged. Raw RGBA output is
rejected by this function; use `thumbnailPng` for RGBA.

## Options

All numeric options must be non-negative JavaScript safe integers. Operational dimensions and limits must be greater than zero. Width and height values must also fit in an unsigned 32-bit integer.

| Property | Default | Contract |
| --- | ---: | --- |
| `maxWidth` | `512` | Requested bounding-box width. |
| `maxHeight` | `512` | Requested bounding-box height. |
| `fit` | `"contain"` | `"contain"` preserves the complete input inside the box. `"cover"` fills the box aspect ratio with a centered crop. |
| `filter` | `"area"` | Uses the area resampling filter. This is the only supported filter. |
| `allowUpscale` | `false` | When false, neither output dimension exceeds the corresponding input dimension. A cover box is reduced uniformly when filling it would require enlargement. |
| `output` | `"png"` | Selects encoded `"png"`, encoded `"jpeg"`, or raw `"rgba"` output. |
| `png` | RGBA8, balanced compression, default filter | Optional PNG-only encoder settings described below. Supplying this object with raw RGBA output is an error. |
| `jpeg` | quality 85, white background, 4:2:0 | Optional JPEG-only encoder settings described below. Supplying this object with PNG or raw RGBA output is an error. |
| `maxInputBytes` | `67,108,864` (64 MiB) | Maximum encoded input length. |
| `maxInputWidth` | `100,000` | Maximum width declared by the PNG header. |
| `maxInputHeight` | `100,000` | Maximum height declared by the PNG header. |
| `maxInputPixels` | `500,000,000` | Maximum input width multiplied by height. |
| `maxOutputWidth` | `8,192` | Maximum planned output width. |
| `maxOutputHeight` | `8,192` | Maximum planned output height. |
| `maxOutputPixels` | `16,777,216` | Maximum planned output width multiplied by height. |
| `maxMemoryBytes` | `33,554,432` (32 MiB) | Maximum conservative working-memory estimate. It covers decoder storage, resize storage, one completed output row for encoded output or the complete frame for raw RGBA, codec state, a bounded JPEG MCU segment where applicable, and retained encoded output. The chunk API counts its 64 KiB adapter buffer instead of the complete encoded result. It excludes caller-owned input and chunks, JavaScript memory, WebAssembly runtime overhead, and allocator slack. |

The requested bounding box and all applicable resource limits must pass. Setting a limit lower than the requested or calculated operation does not clamp the output; it rejects the operation.

Cover cropping removes equal margins from opposite sides. Fractional source
boundaries are preserved in the area weights, including half-pixel-centered
regions, rather than rounded to an integer crop rectangle. Cropping is fused
with row resampling and does not allocate an intermediate scaled or cropped
frame. The output is exactly the requested box when downscaling is possible or
`allowUpscale` is true. With upscaling disabled and a smaller input, the box is
reduced before cropping.

### PNG encoder options

The nested `png` object accepts these optional string properties:

| Property | Default | Values |
| --- | --- | --- |
| `color` | `"rgba8"` | `"auto"`, `"rgba8"`, `"rgb8"`, `"grayscale-alpha8"`, `"grayscale8"` |
| `compression` | `"balanced"` | `"none"`, `"fastest"`, `"fast"`, `"balanced"`, `"high"` |
| `filter` | `"default"` | `"default"`, `"none"`, `"sub"`, `"up"`, `"average"`, `"paeth"`, `"adaptive"`, `"min-entropy"` |

`"rgba8"` remains the default for backward compatibility. `"rgb8"` discards
alpha. The grayscale modes convert the current encoded-space RGB samples using
the integer formula `(77 * R + 150 * G + 29 * B + 128) >> 8`; `"grayscale8"`
also discards alpha.

`"auto"` does not buffer or inspect the resized output. Before decoding image
data, it selects the smallest lossless representation proven by input metadata:
grayscale remains grayscale, RGB remains RGB, and an alpha channel is retained
when the source type or `tRNS` requires it. Palette input is grayscale only when
every declared palette entry is grayscale; otherwise it becomes RGB. Palette
transparency selects an alpha-bearing form. Output PNG files remain 8-bit and
non-interlaced.

`"default"` lets the selected compression preset choose its scanline filter.
Explicit filters override that choice. Compression and filtering can change
runtime and encoded size but not decoded pixels.

### JPEG encoder options

The nested `jpeg` object accepts:

| Property | Default | Values |
| --- | --- | --- |
| `quality` | `85` | Integer from `1` through `100` |
| `background` | `[255, 255, 255]` | Three integer RGB channels from `0` through `255` |
| `subsampling` | `"420"` | `"420"`, `"422"`, `"444"` |

JPEG has no alpha channel. Straight-alpha thumbnail pixels are composited over
`background` before encoding. Output is an 8-bit baseline sequential JPEG with
fixed Huffman tables. Width and height must each be at most 65,535 pixels.

## Result

`planThumbnailPng` returns a plain JavaScript `ThumbnailPlan` object:

```ts
interface ThumbnailPlan {
  input: {
    width: number;
    height: number;
    encodedBytes: number;
    colorType: "grayscale" | "rgb" | "indexed" | "grayscale-alpha" | "rgba";
    bitDepth: number;
    interlaced: boolean;
  };
  output: {
    width: number;
    height: number;
    format: "png" | "jpeg" | "rgba";
  };
  memory: {
    decoderRowsBytes: number;
    decoderStagingBytes: number;
    normalizedRowBytes: number;
    horizontalAccumulatorBytes: number;
    verticalAccumulatorBytes: number;
    sparseAccumulatorBytes: number;
    outputRowBytes: number;
    outputRgbaBytes: number;
    encoderStateBytes: number;
    encodedOutputBytes: number;
    totalBytes: number;
  };
  configuredMaxMemoryBytes: number;
  withinMemoryLimit: boolean;
}
```

Non-interlaced input uses the ordered row accumulators. Adam7 input uses the
sparse accumulator, so `sparseAccumulatorBytes` is non-zero while the two
ordered accumulator fields are zero. Buffered RGBA includes the complete
output frame. Buffered PNG and JPEG include retained encoded output; chunk
delivery replaces that retained result with the 64 KiB adapter buffer.

This result owns no WebAssembly allocation and has no `free()` method. The
estimate excludes the caller-owned input, JavaScript-retained chunks, combined
JavaScript output, preview objects, and WebAssembly runtime overhead.

`thumbnailPng` returns a `ThumbnailResult` with these getters:

| Property | Type | Contract |
| --- | --- | --- |
| `bytes` | `Uint8Array` | A copy of the encoded PNG, encoded JPEG, or raw RGBA bytes. |
| `width` | `number` | Output width in pixels. |
| `height` | `number` | Output height in pixels. |
| `mimeType` | `string` | `image/png` for PNG, `image/jpeg` for JPEG, or `application/octet-stream` for RGBA. |
| `format` | `string` | `png`, `jpeg`, or `rgba`. |

PNG and JPEG outputs are complete encoded images. RGBA output is tightly packed, row-major, straight-alpha RGBA8 data whose length is `width * height * 4`.

Reading `bytes` copies the output from WebAssembly. Read all required properties, then call `result.free()` to release the result's WebAssembly allocation promptly. `ThumbnailResult` also implements `Symbol.dispose` for runtimes that support explicit resource management. The copied `Uint8Array` remains valid after disposal.

`thumbnailPngToChunks` returns a `ChunkedThumbnailResult` after the last callback:

| Property | Type | Contract |
| --- | --- | --- |
| `width` | `number` | Output width in pixels. |
| `height` | `number` | Output height in pixels. |
| `mimeType` | `string` | `image/png` or `image/jpeg`. |
| `format` | `string` | `png` or `jpeg`. |
| `bytesWritten` | `number` | Total encoded bytes delivered to callbacks. |
| `chunkCount` | `number` | Number of successful callback invocations. |

Call `result.free()` after reading the metadata. The API does not expose a
`ReadableStream`, asynchronous callback, or incremental-input contract.

## Utility exports

```ts
function streamthumbVersion(): string;
function wasmMemoryBytes(): number;
```

`streamthumbVersion` returns the package version embedded at build time.
`wasmMemoryBytes` returns the current WebAssembly linear-memory size in bytes.
WebAssembly linear memory only grows, so this value is a process-local
high-water observation intended for diagnostics and benchmarks, not the exact
memory cost of one thumbnail operation.

## Color handling

Resampling uses premultiplied alpha, then returns straight-alpha RGBA8. Color
channels are averaged in their encoded sample space; the pipeline does not
convert sRGB values to linear light. PNG `sRGB`, `gAMA`, `cHRM`, and ICC color
metadata do not affect resampling and are not copied to encoded output. The
API is therefore deterministic but is not a color-managed image pipeline.

## Accepted PNG input

The decoder accepts valid non-interlaced and Adam7 PNG images in the color types and bit depths listed in the project [README](../README.md), including supported `tRNS` transparency. Animated PNG control chunks are rejected rather than silently processing only one frame.

The encoded byte limit is checked before parsing. Declared dimensions, pixels, output geometry, and estimated working memory are checked before image-data processing and large working allocations.

## Errors and resource limits

The initializer rejects its promise when WebAssembly loading or compilation
fails. After successful initialization, all three thumbnail functions throw
synchronously for:

- invalid option types, unsupported option values, zero operational dimensions, or out-of-range numbers;
- malformed, unsupported, truncated, or animated PNG input;
- input, output, or working-memory limit violations;
- checked arithmetic overflow, allocation failure, or internal consistency failure.

`planThumbnailPng` reports a working-memory limit shortfall through
`withinMemoryLimit: false` rather than throwing. Execution with the same options
still throws. Other resource-limit and validation failures throw normally.

`thumbnailPngToChunks` additionally rejects raw RGBA output. A callback may
throw any JavaScript value; that value is propagated without replacing it with
a streamthumb error.

The API currently has no stable machine-readable error codes. Callers should catch `Error` and treat its message as diagnostic text, not as a version-stable identifier.

Resource limits bound the documented buffers and calculations, but they are not an execution deadline. See [SECURITY.md](../SECURITY.md) for the complete memory boundary and the remaining CPU-time limitation.
