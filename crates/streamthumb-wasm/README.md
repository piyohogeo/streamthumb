# streamthumb-wasm

WebAssembly bindings for the memory-bounded `streamthumb` PNG thumbnail pipeline.

## Build

```text
wasm-pack build --target web
```

## API

```js
import init, { thumbnailPng } from "@streamthumb/wasm";

await init();

const result = thumbnailPng(inputBytes, {
  maxWidth: 512,
  maxHeight: 512,
  fit: "contain",
  filter: "area",
  allowUpscale: false,
  output: "png",
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
```

`output: "png"` returns encoded PNG bytes with MIME type `image/png`. `output: "rgba"` returns tightly packed, straight-alpha RGBA8 pixels with MIME type `application/octet-stream`.

The API has no dependency on DOM, Canvas, filesystem, threads, `SharedArrayBuffer`, or Node-specific APIs. Passing and returning byte arrays currently copies them across the JavaScript/WebAssembly boundary.

