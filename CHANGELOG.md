# Changelog

All notable changes to this project will be documented in this file. The
project follows Semantic Versioning.

## [Unreleased]

### Added

- Rust preflight planners that return the complete ordered or Adam7 working-memory
  estimate without weakening execution-time enforcement of the configured limit.
- PNG header inspection metadata and buffered or direct-writer preflight APIs for
  the planned browser demo.
- A `planThumbnailPng` WebAssembly API with plain-object input metadata, output
  geometry, complete memory breakdown, and typed memory-limit status.
- A GitHub Pages demo with local-file and bundled-sample input, every current
  WebAssembly option, typed memory preflight, worker-based PNG/JPEG/RGBA output,
  previews, downloads, and automated Chrome coverage.
- An isolated browser File input spike proving that `Blob.slice()` and
  `FileReaderSync` can drive the production seekable reader path without a
  complete encoded-input copy in Chrome and Firefox workers, with format,
  failure, multi-chunk, and performance evidence for provisional API adoption.
- `thumbnailPngFromSeekable` and `thumbnailPngFromSeekableToChunks` WebAssembly
  APIs for synchronous bounded range reads in dedicated browser workers.
- A `planThumbnailPngFromSeekable` WebAssembly API for header-only planning
  through the same bounded synchronous range-reader contract.

### Changed

- The GitHub Pages demo now passes `File` and `Blob` inputs to its dedicated
  worker and uses the seekable planning and execution APIs, avoiding complete
  JavaScript and WebAssembly encoded-input copies.
- The Pages header is compact, its privacy note is next to file selection, its
  UI memory limit defaults to 4 MiB, and its generated 2048-square RGBA sample
  combines large decoded dimensions with a small encoded download.
- The Pages working-memory control now accepts 128 KiB increments and includes
  low-memory presets for demonstrating preflight rejection before decoding.
- Pages builds now attach the source revision to CSS, JavaScript, worker,
  WebAssembly, manifest, metadata, and sample requests so cached assets from
  different deployments cannot mix incompatible UI units and logic.
- Release WebAssembly packages now use `wasm-opt -Oz` explicitly so the pinned
  Rust 1.85 build remains within the existing package-size budget.
- Release builds use fat LTO, one code-generation unit, and aborting panics to
  keep the expanded WebAssembly API within that unchanged size budget.

## [0.1.0] - 2026-08-07

### Added

- Seekable `Read + Seek` PNG input APIs, native CLI integration, a runnable
  file-to-file example, and native Peak RSS comparisons against slice input.
- Bounded direct-to-writer PNG and JPEG APIs used by CLI file output.
- Synchronous WebAssembly chunk callbacks for bounded PNG and JPEG delivery.
- Failure-safe CLI destination replacement through same-directory staging.
- Browser boundary coverage for multi-chunk PNG/JPEG output and callback exceptions.
- Process-level CLI coverage for preserving existing output after encode failure.
- An isolated incremental-input spike proving bounded native seekable reads and
  documenting why JavaScript `ReadableStream` input remains deferred.
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
- Centered cover cropping across Rust, CLI, and WebAssembly APIs, with exact
  fractional crop boundaries shared by ordered PNG and Adam7 processing.
