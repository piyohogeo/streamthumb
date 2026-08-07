# Cloudflare Worker example

Run `node scripts/build-npm-package.mjs` at the repository root, install this example's dependencies, and run `npm run dev`. Send an encoded PNG as the body of a `POST` request. The response is a bounded 512-pixel PNG thumbnail.

The compatibility date is an example baseline and should be updated deliberately when deploying.

This repository does not validate the example against a live Cloudflare account. Local package and source checks still run in CI.
