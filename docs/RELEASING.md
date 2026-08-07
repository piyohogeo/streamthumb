# Release process

This project publishes the browser-targeted WebAssembly package as
`@streamthumb/wasm`. The package is not published automatically by CI.

## Prepare a release

1. Choose a semantic version and update `workspace.package.version` in the
   root `Cargo.toml`.
2. Move the relevant entries in `CHANGELOG.md` from `Unreleased` to a heading
   for the new version and release date.
3. Run the full Rust and WebAssembly test suite.
4. Create `target/npm-artifacts`, then build and inspect the npm package:

   ```text
   node scripts/build-npm-package.mjs
   node scripts/check-npm-package.mjs
   npm pack ./target/npm-package --pack-destination target/npm-artifacts
   ```

5. Commit the version and changelog, push it, explicitly dispatch CI, and wait
   for both the push-triggered and manually dispatched runs to pass.
6. Download the `npm-package` artifact and inspect the tarball before creating
   a signed `vX.Y.Z` tag and GitHub release.

## Publish to npm

Publishing is an explicit maintainer action. Authenticate with an npm account
that can publish the `@streamthumb` scope, then publish the already inspected
tarball:

```text
npm publish target/npm-artifacts/streamthumb-wasm-X.Y.Z.tgz \
  --access public
```

Confirm the package name, version, integrity, and public visibility on npm.
Never rebuild between artifact inspection and publication.
