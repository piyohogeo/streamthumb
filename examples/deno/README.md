# Deno example

Run the example in a project where `@streamthumb/wasm` is available:

```text
deno add npm:@streamthumb/wasm
deno run --node-modules-dir=manual --allow-read --allow-write thumbnail.ts input.png output.png
```

The example reads the WebAssembly file explicitly, then converts an encoded PNG. See [`docs/WASM_API.md`](../../docs/WASM_API.md) for the complete API contract.

Repository CI copies this source into an isolated consumer, installs the release-candidate tarball, and verifies the generated PNG.
