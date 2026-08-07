# Release readiness audit

This document records the pre-publication audit for version 0.1.0 after the
centered-cover, direct-writer, seekable-input, native benchmark, and WebAssembly
chunk-output extensions. It is evidence that release inputs can be assembled
and verified; it is not authorization to publish, tag, or create a release.

## Automated gates

- Normal CI checks formatting, clippy with warnings denied, 126 native unit and
  integration tests, one compiled Rustdoc example, runnable seekable-reader
  PNG/JPEG examples, the wasm32 build, Chrome and Firefox tests, installed npm
  tarball consumers in browsers, Node.js, and Deno, benchmark tooling, all Rust
  packages, and two excluded incremental-input spike tests.
- Scheduled and manually dispatched fuzzing covers row decoding, all thumbnail
  output codecs and fit modes, and ordered-versus-sparse area resampling.
- The release-candidate workflow uses Rust 1.85.0, Node.js 24.14.1, npm 11.11.0,
  and wasm-pack 0.15.0. It records the source revision, byte size, and SHA-256
  digest and verifies them before uploading an unpublished artifact.

## Rust package audit

`cargo package --workspace --no-verify` assembles the five workspace package
archives. The normal CI job separately compiles and tests the complete local
workspace. Archive verification is deferred because Cargo removes internal
path dependencies before verification and these exact `0.1.0` packages are not
yet available from crates.io. Internal dependencies carry both a path for
workspace development and the exact version needed in published manifests.

The audited package archives contain only normalized manifests, lockfiles,
VCS metadata, crate source, crate-owned examples, and the WebAssembly crate's
README and generated license file. The `streamthumb-png` archive contains the
runnable `examples/native_reader.rs`; generated corpora, benchmark output, fuzz
artifacts, repository-level examples, and design documents are excluded.

Rust package publication is not part of the current npm release procedure.
Publishing these crates would require a separate explicit decision and the
dependency order `core`, `encode`, `png`, then `cli` and `wasm`. Each downstream
archive must be verified after its internal dependencies are available from
crates.io.

## npm package audit

The local audit tarball contains exactly eight files:

- `LICENSE-APACHE`
- `LICENSE-MIT`
- `README.md`
- `package.json`
- `streamthumb_wasm.js`
- `streamthumb_wasm.d.ts`
- `streamthumb_wasm_bg.wasm`
- `streamthumb_wasm_bg.wasm.d.ts`

The package has no npm runtime dependencies. The current local stable-toolchain
audit measured 194,780 packed bytes and 497,659 unpacked bytes. Its optimized
WebAssembly file measured 454,856 bytes with SHA-256
`b813aab4ee219f1f11a5a4b722b9ba3ebd6ab24e013598a46bca93d600d1e18c`.

The most recent pinned Rust 1.85 release-candidate build measured 202,905
packed bytes and 544,090 unpacked bytes with tarball SHA-256
`c6764031da5b5fa9f211dfed76671ee4dfd7f933a5a5b9bdfddb10582a9de9fd`.
Package inspection enforces a 550,000-byte ceiling. Native `Read + Seek` support
is separated from WebAssembly slice monomorphization so the package remains
within that ceiling. These values are descriptive; the pinned
release-candidate manifest is authoritative for any future release.

## License and dependency audit

Every workspace package declares `MIT OR Apache-2.0`. The current Cargo
metadata audit inspected 67 packages and found no registry dependency with
missing license metadata. All direct and transitive
normal or development dependencies report permissive alternatives composed of
MIT, Apache-2.0, 0BSD, Zlib, Unlicense, IJG, or Unicode-3.0 terms. The npm
tarball includes both project license texts.

This inventory is a release engineering check, not legal advice. Dependency
changes require the metadata audit to be repeated.

## Remaining maintainer actions

Before publication, the maintainer must choose the final version and date,
replace `Unreleased` in `CHANGELOG.md`, verify the pinned release-candidate
artifact, and explicitly authorize signing, tagging, GitHub Release creation,
and npm publication. No automated workflow performs those actions.
