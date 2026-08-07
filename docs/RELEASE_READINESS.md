# Release readiness audit

This document records the pre-publication audit for version 0.1.0 after the
centered-cover and direct-writer extensions. It is evidence that release inputs
can be assembled and verified; it is not authorization to publish, tag, or
create a release.

## Automated gates

- Normal CI checks formatting, clippy with warnings denied, 115 native tests,
  the wasm32 build, Chrome and Firefox tests, installed npm tarball consumers
  in browsers, Node.js, and Deno, benchmark tooling, and all Rust packages.
- Scheduled and manually dispatched fuzzing covers row decoding, all thumbnail
  output codecs and fit modes, and ordered-versus-sparse area resampling.
- The release-candidate workflow uses Rust 1.85.0, Node.js 24.14.1, npm 11.11.0,
  and wasm-pack 0.15.0. It records the source revision, byte size, and SHA-256
  digest and verifies them before uploading an unpublished artifact.

## Rust package audit

`cargo package --workspace` assembles and build-verifies the five workspace
packages through Cargo's temporary local registry. Internal dependencies carry
both a path for workspace development and the exact `0.1.0` version needed in
published manifests.

The audited package archives contain only normalized manifests, lockfiles,
VCS metadata, crate source, and the WebAssembly crate's README and generated
license file. Generated corpora, benchmark output, fuzz artifacts, examples,
and repository-level design documents are excluded.

Rust package publication is not part of the current npm release procedure.
Publishing these crates would require a separate explicit decision and the
dependency order `core`, `encode`, `png`, then `cli` and `wasm`.

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

The package has no npm runtime dependencies. The local stable-toolchain audit
measured 160,698 packed bytes and 372,019 unpacked bytes. These values are
descriptive; the pinned release-candidate manifest is authoritative for any
future release.

## License and dependency audit

Every workspace package declares `MIT OR Apache-2.0`. Cargo metadata reports no
registry dependency with missing license metadata. All direct and transitive
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
