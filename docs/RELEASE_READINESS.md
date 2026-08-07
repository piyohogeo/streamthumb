# Version 0.2.0 release readiness audit

This document records the release-readiness evidence for version 0.2.0 after
the browser seekable-input, memory-preflight, and GitHub Pages extensions.
Version 0.2.0 is an unreleased GitHub-only candidate. npm and crates.io
publication remain outside its scope.

## Automated gates

- Normal CI checks formatting, clippy with warnings denied, the native unit and
  integration suite, one compiled Rustdoc example, runnable seekable-reader
  PNG/JPEG examples, the wasm32 build, 22 Chrome and Firefox WebAssembly tests,
  installed npm tarball consumers in browsers, Node.js, and Deno, benchmark
  tooling, all Rust packages, and the excluded input-feasibility spikes.
- Pages CI generates the 2048-square RGBA sample, stamps every runtime asset
  with the source revision, verifies seekable range reads, exercises every
  output format, proves 128 KiB preflight rejection and 4 MiB recovery, and
  deploys only after the Chrome smoke test succeeds.
- Scheduled and manually dispatched fuzzing covers row decoding, all thumbnail
  output codecs and fit modes, and ordered-versus-sparse area resampling.
- The release-candidate workflow uses Rust 1.85.0, Node.js 24.14.1, npm 11.11.0,
  and wasm-pack 0.15.0. It records the source revision, byte size, and SHA-256
  digest and verifies them before uploading an unpublished artifact.

## Rust package audit

`cargo package --workspace --no-verify` assembles the five workspace package
archives. The normal CI job separately compiles and tests the complete local
workspace. Archive verification is deferred because Cargo removes internal
path dependencies before verification and these exact `0.2.0` packages are not
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

The package has no npm runtime dependencies. The local pinned Rust 1.85
v0.2.0 audit measured 208,377 packed bytes and 537,110 unpacked bytes. Its
optimized WebAssembly file measured 486,783 bytes with SHA-256
`7156230b3427060ee87a25c1efd6d75d853353250bade9726d9a39059dc99034`.
Package inspection enforces the unchanged 550,000-byte ceiling. Slice and
seekable planning and execution share internal reader drivers so the expanded
API remains within that ceiling. These local values are descriptive; the
post-commit pinned release-candidate manifest and checksum are authoritative
for the release.

## License and dependency audit

Every workspace package declares `MIT OR Apache-2.0`. The current Cargo
metadata audit inspected 67 packages and found no registry dependency with
missing license metadata. All direct and transitive
normal or development dependencies report permissive alternatives composed of
MIT, Apache-2.0, 0BSD, Zlib, Unlicense, IJG, or Unicode-3.0 terms. The npm
tarball includes both project license texts.

This inventory is a release engineering check, not legal advice. Dependency
changes require the metadata audit to be repeated.

## Candidate outcome

Version 0.2.0 has not been tagged or published. Before release, the exact
version commit must pass push-triggered CI plus explicitly dispatched CI, Fuzz,
Benchmarks, Release Candidate, and Pages workflows. The candidate artifact must
then be downloaded and verified against its source revision, manifest, byte
size, and SHA-256 before a signed tag or GitHub Release is created.

Version 0.1.0 remains published as a
[GitHub Release](https://github.com/piyohogeo/streamthumb/releases/tag/v0.1.0)
from signed tag `v0.1.0`. The tag points to commit
`ab1fc3e2efd6c1628130242df7941434eab7c4e8`, and GitHub reports its SSH
signature as verified. The release contains the exact pinned candidate
tarball, its checksum file, and `release-manifest.json`; the tarball is 202,905
bytes with SHA-256
`9ef1b152c000afd5c32cf5312bd197aadf942abf46441726b62c40374263d216`.

The published artifact came from
[Release Candidate run 31163726619](https://github.com/piyohogeo/streamthumb/actions/runs/31163726619).
After the tag push, the same tagged revision passed explicitly dispatched
[CI](https://github.com/piyohogeo/streamthumb/actions/runs/31164747729),
[Fuzz](https://github.com/piyohogeo/streamthumb/actions/runs/31164749698),
[Benchmarks](https://github.com/piyohogeo/streamthumb/actions/runs/31164751914),
and [Release Candidate](https://github.com/piyohogeo/streamthumb/actions/runs/31164753915)
workflows. That historical artifact is not reused for v0.2.0. npm and crates.io
publication remain deferred and require separate explicit authorization.
