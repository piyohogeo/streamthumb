# GitHub Pages demo

This directory contains the static source for the streamthumb GitHub Pages
demo. It processes selected PNG files in a module Web Worker and never sends
their contents to a server.

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

The bundled smoke sample is copied from the PNG Suite fixture
`fuzz/corpus/thumbnail_png/pngsuite_basn6a08.png`. Its license is copied into
the generated `samples` directory and linked from `sample-manifest.json`.
