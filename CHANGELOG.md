# Changelog

All notable changes to this project will be documented in this file. The
project follows Semantic Versioning.

## [0.1.0] - Unreleased

### Added

- Memory-bounded PNG thumbnail generation for Rust and WebAssembly.
- Static PNG support for all standard color types and legal bit depths,
  including Adam7 interlacing and applicable `tRNS` transparency.
- Encoded PNG and raw RGBA output with explicit resource limits.
- Native, browser, and comparative memory benchmark tooling.
- An npm tarball consumer test covering package installation, browser import,
  automatic WebAssembly loading, thumbnail generation, and PNG decoding.
- Strict TypeScript declarations for thumbnail options and literal values,
  validated through a pinned TypeScript and esbuild consumer build.
- A pinned release-candidate workflow that records and verifies the source
  revision, package size, and SHA-256 checksum before any release action.
- Installed-tarball consumer tests for Node.js and Deno, including bare package
  resolution, explicit WebAssembly initialization, and PNG generation.
- Runnable browser, Node.js, and Deno examples, a complete WebAssembly API
  contract, and CI smoke tests that execute the public example sources.
- Release-candidate artifact verification and an explicit, no-rebuild release
  procedure for the npm package and attached GitHub assets.
- A design-completion traceability audit, documented color-space limitations,
  and expanded WebAssembly boundary tests for RGBA output and invalid options.
- A codec-independent RGBA row-sink architecture with a compatibility
  collector, immediate ordered-row emission, and row-wise Adam7 finalization.
- Direct row-streamed PNG encoding for ordered and Adam7 inputs, removing the
  complete resized RGBA frame from encoded-output memory while preserving the
  raw RGBA API and bounded encoded-byte result.
- Typed PNG color, compression, and filter settings across Rust, WebAssembly,
  and CLI APIs, including metadata-safe automatic color selection.
- Baseline sequential JPEG output across Rust, WebAssembly, and CLI APIs, with
  quality, alpha-compositing background, and 4:2:0/4:2:2/4:4:4 controls.
- A shared bounded encoder crate and MCU-row JPEG segmentation using standard
  restart markers, avoiding a complete resized RGBA or RGB frame.
