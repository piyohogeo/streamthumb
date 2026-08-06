# Browser example

Build the WebAssembly package at the repository root:

```text
wasm-pack build crates/streamthumb-wasm --target web --out-dir ../../pkg
```

Then serve the repository root with any static HTTP server and open `examples/browser/index.html`. The thumbnail work runs in a module Web Worker.

