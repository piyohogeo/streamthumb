# streamthumb-wasm

WebAssembly bindings for the memory-bounded `streamthumb` PNG-input thumbnail pipeline.

## Installation

```text
npm install @streamthumb/wasm
```

## API

```js
import init, {
  thumbnailPng,
  thumbnailPngFromSeekable,
  thumbnailPngToChunks,
} from "@streamthumb/wasm";

await init();

const result = thumbnailPng(inputBytes, {
  maxWidth: 512,
  maxHeight: 512,
  fit: "contain",
  filter: "area",
  allowUpscale: false,
  output: "png",
  png: {
    color: "auto",
    compression: "balanced",
    filter: "default",
  },
  maxInputBytes: 64 * 1024 * 1024,
  maxInputWidth: 100_000,
  maxInputHeight: 100_000,
  maxInputPixels: 500_000_000,
  maxOutputWidth: 8_192,
  maxOutputHeight: 8_192,
  maxOutputPixels: 16_777_216,
  maxMemoryBytes: 32 * 1024 * 1024,
});

console.log(result.width, result.height, result.mimeType, result.format);
const bytes = result.bytes;
result.free();
```

Encoded output can instead be consumed synchronously in chunks of at most 64
KiB. This avoids retaining the complete encoded result in WebAssembly:

```js
const chunks = [];
const info = thumbnailPngToChunks(
  inputBytes,
  (chunk) => chunks.push(chunk),
  { maxWidth: 512, maxHeight: 512, output: "jpeg" },
);

console.log(info.width, info.height, info.mimeType, info.bytesWritten);
info.free();
```

Each chunk is a new JavaScript-owned `Uint8Array`. The callback is synchronous,
has no asynchronous backpressure, and may throw to abort processing. Raw RGBA
output remains available only through `thumbnailPng`.

Browser `File` and `Blob` inputs can avoid a complete `ArrayBuffer` and
WebAssembly input copy when processing runs in a dedicated worker:

```js
const reader = new FileReaderSync();
const readAt = (offset, length) => new Uint8Array(
  reader.readAsArrayBuffer(file.slice(offset, offset + length)),
);
const result = thumbnailPngFromSeekable(file.size, readAt, {
  maxWidth: 512,
  maxHeight: 512,
  output: "png",
});
```

The callback is synchronous, must return exactly the requested bytes, and
cannot return a Promise. `thumbnailPngFromSeekableToChunks` combines this input
model with bounded encoded-output chunks. `FileReaderSync` is worker-only.

`output: "png"` returns encoded PNG bytes with MIME type `image/png`.
`output: "jpeg"` returns baseline JPEG bytes with MIME type `image/jpeg`; a
nested `jpeg` object configures quality, RGB alpha-compositing background, and
`"420"`, `"422"`, or `"444"` subsampling. `output: "rgba"` returns tightly
packed, straight-alpha RGBA8 pixels with MIME type `application/octet-stream`.

`fit: "contain"` preserves the complete source image inside the requested box.
`fit: "cover"` fills the requested aspect ratio and crops equal margins from
the left and right or the top and bottom. The centered crop is fused into area
resampling, so it does not allocate an intermediate resized image.

Area filtering uses premultiplied alpha but averages color channels in their
encoded sample space. The package does not perform linear-light conversion or
ICC color management, and encoded output does not inherit source PNG color
metadata.

The package exports `ThumbnailOptions`, `ThumbnailFit`, `ThumbnailFilter`,
`ThumbnailOutputFormat`, `PngOptions`, `PngColorMode`, `PngCompression`,
`PngFilter`, `JpegOptions`, `JpegSubsampling`, `ThumbnailChunkCallback`, and
`SeekableReadAt` TypeScript types. Every option is optional, and every
thumbnail function accepts an omitted or `null` options value. PNG output
remains RGBA8 by default;
the nested `png` object can select automatic or explicit 8-bit color output,
compression, and scanline filtering.

## Node.js and Deno

Browsers can call `init()` and let the module fetch its adjacent WebAssembly
file. Filesystem consumers should resolve the package entry point and pass the
WebAssembly bytes explicitly.

Node.js:

```js
import { readFile } from "node:fs/promises";
import init, { thumbnailPng } from "@streamthumb/wasm";

const packageModule = import.meta.resolve("@streamthumb/wasm");
const wasm = await readFile(
  new URL("streamthumb_wasm_bg.wasm", packageModule),
);
await init({ module_or_path: wasm });
```

Deno:

```ts
import init, { thumbnailPng } from "@streamthumb/wasm";

const packageModule = import.meta.resolve("@streamthumb/wasm");
const wasm = await Deno.readFile(
  new URL("streamthumb_wasm_bg.wasm", packageModule),
);
await init({ module_or_path: wasm });
```

The filesystem call belongs to the consumer; the package itself does not
import Node.js or Deno APIs.

The API has no dependency on DOM, Canvas, filesystem, threads, `SharedArrayBuffer`, or Node-specific APIs. Passing and returning byte arrays currently copies them across the JavaScript/WebAssembly boundary.

See the [WebAssembly API contract](https://github.com/piyohogeo/streamthumb/blob/main/docs/WASM_API.md)
for the complete option, result, input, and error contracts.

Runnable source examples are available for
[browsers](https://github.com/piyohogeo/streamthumb/tree/main/examples/browser),
[Node.js](https://github.com/piyohogeo/streamthumb/tree/main/examples/node), and
[Deno](https://github.com/piyohogeo/streamthumb/tree/main/examples/deno).

## Repository development

Run these commands from the repository root to build and inspect the
unpublished package:

```text
node scripts/build-npm-package.mjs
node scripts/check-npm-package.mjs
```

The package is created in `target/npm-package`. Normal CI installs its tarball
into isolated browser, Node.js, and Deno consumers before retaining it as an
artifact.
