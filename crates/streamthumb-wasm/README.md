# streamthumb-wasm

WebAssembly bindings for the memory-bounded `streamthumb` PNG thumbnail pipeline.

## Build

```text
node scripts/build-npm-package.mjs
node scripts/check-npm-package.mjs
```

Run these commands from the repository root. They create the unpublished
`@streamthumb/wasm` package in `target/npm-package` and verify the exact files
that npm would include.

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
result.free();
```

`output: "png"` returns encoded PNG bytes with MIME type `image/png`. `output: "rgba"` returns tightly packed, straight-alpha RGBA8 pixels with MIME type `application/octet-stream`.

The package exports `ThumbnailOptions`, `ThumbnailFit`, `ThumbnailFilter`, and
`ThumbnailOutputFormat` TypeScript types. Every option is optional, and
`thumbnailPng` also accepts an omitted or `null` options value.

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
