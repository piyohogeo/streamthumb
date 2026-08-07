# GitHub Pages demo

This directory contains the static source for the streamthumb GitHub Pages
demo. It processes selected PNG files in a module Web Worker and never sends
their contents to a server. The main thread passes each `File` or sample `Blob`
to the worker without materializing a complete `ArrayBuffer`. The worker uses
`FileReaderSync`, `Blob.slice()`, and the seekable WebAssembly APIs so planning
and execution copy only the encoded ranges requested by the PNG decoder.

Build the unpublished WebAssembly package and the site from the repository
root:

```text
node scripts/build-npm-package.mjs
node scripts/check-npm-package.mjs
node scripts/build-pages-demo.mjs
node scripts/test-pages-demo.mjs /path/to/chrome
```

The generated site is written to `target/pages`. It must be served over HTTP;
module workers and WebAssembly loading are not supported reliably from a
`file:` URL. All production asset URLs are relative so the site works below
the `/streamthumb/` GitHub Pages project path.

The bundled sample is generated deterministically as a highly compressible
2048 x 2048 RGBA PNG. It provides large decoded dimensions without adding a
large download to the static site.
