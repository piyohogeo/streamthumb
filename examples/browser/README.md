# Browser example

Build the WebAssembly package at the repository root:

```text
wasm-pack build crates/streamthumb-wasm --target web --out-dir ../../pkg
```

Then serve the repository root with any static HTTP server and open `examples/browser/index.html`. The thumbnail work runs in a module Web Worker.

Open `examples/browser/smoke.html` to run a deterministic browser smoke test. It passes only after the module Web Worker has created a PNG and the browser has decoded the result.

Run the Rust WebAssembly tests in installed browsers with:

```text
wasm-pack test --headless --chrome crates/streamthumb-wasm
wasm-pack test --headless --firefox crates/streamthumb-wasm
```
