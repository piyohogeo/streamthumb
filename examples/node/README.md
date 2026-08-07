# Node.js example

Download `streamthumb-wasm-0.2.0.tgz` from the
[v0.2.0 GitHub Release](https://github.com/piyohogeo/streamthumb/releases/tag/v0.2.0),
install the local tarball in your application, and run:

```sh
npm install ./streamthumb-wasm-0.2.0.tgz
node thumbnail.mjs input.png output.png
```

The package is not currently published to the npm registry. The installed
package name remains `@streamthumb/wasm`.

The example initializes the package with explicit WebAssembly bytes so it does not depend on Node.js `fetch()` support for `file:` URLs. See [`docs/WASM_API.md`](../../docs/WASM_API.md) for the complete API contract.

Repository CI copies this source into an isolated consumer, installs the release-candidate tarball, and verifies the generated PNG.
