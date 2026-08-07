# Release process

This project distributes the browser-targeted WebAssembly package as a verified
GitHub Release artifact. Publishing `@streamthumb/wasm` to npm is an optional,
separate maintainer action and is not part of the version 0.1.0 release plan.
Nothing is published automatically by CI.

See [the release readiness audit](RELEASE_READINESS.md) for the current package,
dependency, license, and automated-gate inventory.

The Rust workspace crates are assembled and build-verified together with
`cargo package --workspace` in normal CI. Their internal path dependencies also
carry exact workspace-version requirements so Cargo can replace paths during
packaging. This is a readiness check only; crates.io publication is outside the
current GitHub Release procedure and requires a separate explicit maintainer
decision and dependency-ordered publication plan.

## Prepare a release

1. Choose a semantic version and update `workspace.package.version` in the
   root `Cargo.toml`.
2. Move the relevant entries in `CHANGELOG.md` under a matching
   `## [X.Y.Z] - Unreleased` heading. When preparing the final candidate,
   replace `Unreleased` with its `YYYY-MM-DD` release date before committing.
3. Run the full Rust and WebAssembly test suite.
4. Create `target/npm-artifacts`, then build and inspect the npm package:

   ```text
   node scripts/build-npm-package.mjs
   node scripts/check-npm-package.mjs
   npm pack ./target/npm-package --pack-destination target/npm-artifacts
   ```

5. Commit the version and changelog, push it, explicitly dispatch CI, and wait
   for both the push-triggered and manually dispatched runs to pass.
6. Explicitly dispatch the `Release Candidate` workflow for the same commit.
   This workflow uses pinned Rust, Node.js, npm, and wasm-pack versions and
   produces one `npm-release-candidate` artifact.
7. Empty `target/npm-artifacts`, download the release candidate files into it,
   and verify the source revision, manifest, file size, and SHA-256 checksum.
   `RUN_ID` must identify the successful workflow run from step 6:

   ```text
   gh run download RUN_ID --name npm-release-candidate \
     --dir target/npm-artifacts
   node scripts/release-manifest.mjs check \
     --artifact-only \
     --source-revision FULL_COMMIT_SHA
   ```

8. Inspect the verified tarball. Confirm that the version, documentation,
   licenses, declarations, JavaScript, and WebAssembly files are the intended
   release contents.
9. Create a signed `vX.Y.Z` tag at the exact verified source revision, verify
   it locally, and push that tag explicitly:

   ```text
   git tag -s vX.Y.Z FULL_COMMIT_SHA -m "streamthumb X.Y.Z"
   git verify-tag vX.Y.Z
   git push origin vX.Y.Z
   ```

10. Create the GitHub release from that existing tag and attach the same
    tarball, manifest, and checksum files downloaded in step 7. Do not rebuild
    any release asset:

    ```text
    gh release create vX.Y.Z \
      target/npm-artifacts/streamthumb-wasm-X.Y.Z.tgz \
      target/npm-artifacts/streamthumb-wasm-X.Y.Z.tgz.sha256 \
      target/npm-artifacts/release-manifest.json \
      --verify-tag --title "streamthumb X.Y.Z" --generate-notes
    ```

## Optional npm publication

Publishing is outside the version 0.1.0 GitHub-only release plan and requires a
new explicit maintainer decision. If that decision is made later, authenticate
with an npm account that can publish the `@streamthumb` scope, then publish the
already inspected tarball:

```text
npm publish target/npm-artifacts/streamthumb-wasm-X.Y.Z.tgz \
  --access public
```

Confirm the package name, version, integrity, and public visibility on npm.
Never rebuild between artifact inspection and publication.
