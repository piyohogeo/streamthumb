# WebAssembly API contract

The `@streamthumb/wasm` package exposes a runtime-neutral byte-array API for bounded PNG thumbnail generation. It does not depend on DOM, Canvas, filesystem, threads, `SharedArrayBuffer`, or runtime-specific APIs.

## Initialization

Import the default initializer and call it once before using the named exports:

```js
import init, { thumbnailPng } from "@streamthumb/wasm";

await init();
```

Browsers can let the initializer fetch the adjacent WebAssembly file. Filesystem runtimes should resolve the package module, read `streamthumb_wasm_bg.wasm`, and pass its bytes as `module_or_path`. The [Node.js](../examples/node) and [Deno](../examples/deno) examples show that pattern.

Initialization is asynchronous. `thumbnailPng` is synchronous after initialization.

## Function

```ts
function thumbnailPng(
  input: Uint8Array,
  options?: ThumbnailOptions | null,
): ThumbnailResult;
```

`input` contains one encoded PNG. Passing it into WebAssembly copies the input bytes. An omitted, `undefined`, or `null` options value selects every default below.

## Options

All numeric options must be non-negative JavaScript safe integers. Operational dimensions and limits must be greater than zero. Width and height values must also fit in an unsigned 32-bit integer.

| Property | Default | Contract |
| --- | ---: | --- |
| `maxWidth` | `512` | Requested bounding-box width. |
| `maxHeight` | `512` | Requested bounding-box height. |
| `fit` | `"contain"` | Preserves aspect ratio inside the requested bounding box. This is the only supported fit mode. |
| `filter` | `"area"` | Uses the area resampling filter. This is the only supported filter. |
| `allowUpscale` | `false` | When false, neither output dimension exceeds the corresponding input dimension. |
| `output` | `"png"` | Selects encoded `"png"` or raw `"rgba"` output. |
| `maxInputBytes` | `67,108,864` (64 MiB) | Maximum encoded input length. |
| `maxInputWidth` | `100,000` | Maximum width declared by the PNG header. |
| `maxInputHeight` | `100,000` | Maximum height declared by the PNG header. |
| `maxInputPixels` | `500,000,000` | Maximum input width multiplied by height. |
| `maxOutputWidth` | `8,192` | Maximum planned output width. |
| `maxOutputHeight` | `8,192` | Maximum planned output height. |
| `maxOutputPixels` | `16,777,216` | Maximum planned output width multiplied by height. |
| `maxMemoryBytes` | `33,554,432` (32 MiB) | Maximum conservative working-memory estimate. It covers decoder storage, resize storage, encoder state, and bounded encoded output. It excludes caller-owned input, JavaScript memory, WebAssembly runtime overhead, and allocator slack. |

The requested bounding box and all applicable resource limits must pass. Setting a limit lower than the requested or calculated operation does not clamp the output; it rejects the operation.

## Result

`thumbnailPng` returns a `ThumbnailResult` with these getters:

| Property | Type | Contract |
| --- | --- | --- |
| `bytes` | `Uint8Array` | A copy of the encoded PNG or raw RGBA bytes. |
| `width` | `number` | Output width in pixels. |
| `height` | `number` | Output height in pixels. |
| `mimeType` | `string` | `image/png` for PNG or `application/octet-stream` for RGBA. |
| `format` | `string` | `png` or `rgba`. |

PNG output is a complete encoded PNG. RGBA output is tightly packed, row-major, straight-alpha RGBA8 data whose length is `width * height * 4`.

Reading `bytes` copies the output from WebAssembly. Read all required properties, then call `result.free()` to release the result's WebAssembly allocation promptly. The copied `Uint8Array` remains valid after `free()`.

## Accepted PNG input

The decoder accepts valid non-interlaced and Adam7 PNG images in the color types and bit depths listed in the project [README](../README.md), including supported `tRNS` transparency. Animated PNG control chunks are rejected rather than silently processing only one frame.

The encoded byte limit is checked before parsing. Declared dimensions, pixels, output geometry, and estimated working memory are checked before image-data processing and large working allocations.

## Errors and resource limits

The initializer rejects its promise when WebAssembly loading or compilation fails. After successful initialization, `thumbnailPng` throws a JavaScript `Error` synchronously for:

- invalid option types, unsupported option values, zero operational dimensions, or out-of-range numbers;
- malformed, unsupported, truncated, or animated PNG input;
- input, output, or working-memory limit violations;
- checked arithmetic overflow, allocation failure, or internal consistency failure.

The API currently has no stable machine-readable error codes. Callers should catch `Error` and treat its message as diagnostic text, not as a version-stable identifier.

Resource limits bound the documented buffers and calculations, but they are not an execution deadline. See [SECURITY.md](../SECURITY.md) for the complete memory boundary and the remaining CPU-time limitation.
