# Node.js example

Install `@streamthumb/wasm` in your application and run:

```text
npm install @streamthumb/wasm
node thumbnail.mjs input.png output.png
```

The example initializes the package with explicit WebAssembly bytes so it does not depend on Node.js `fetch()` support for `file:` URLs. See [`docs/WASM_API.md`](../../docs/WASM_API.md) for the complete API contract.

Repository CI copies this source into an isolated consumer, installs the release-candidate tarball, and verifies the generated PNG.
