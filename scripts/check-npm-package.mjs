import { readFile } from "node:fs/promises";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const packageDirectory = path.resolve(
  root,
  process.argv[2] ?? "target/npm-package",
);
const manifest = JSON.parse(
  await readFile(path.join(packageDirectory, "package.json"), "utf8"),
);
const declarations = await readFile(
  path.join(packageDirectory, "streamthumb_wasm.d.ts"),
  "utf8",
);

const expectedManifest = {
  name: "@streamthumb/wasm",
  license: "MIT OR Apache-2.0",
  type: "module",
  main: "./streamthumb_wasm.js",
  module: "./streamthumb_wasm.js",
  types: "./streamthumb_wasm.d.ts",
};

for (const [key, value] of Object.entries(expectedManifest)) {
  if (manifest[key] !== value) {
    throw new Error(`package.json ${key} must be ${JSON.stringify(value)}`);
  }
}
if (manifest.publishConfig?.access !== "public") {
  throw new Error("The scoped package must set publishConfig.access to public.");
}
const expectedStructuredMetadata = {
  repository: {
    type: "git",
    url: "git+https://github.com/piyohogeo/streamthumb.git",
    directory: "crates/streamthumb-wasm",
  },
  exports: {
    ".": {
      types: "./streamthumb_wasm.d.ts",
      import: "./streamthumb_wasm.js",
    },
  },
};
for (const [key, value] of Object.entries(expectedStructuredMetadata)) {
  if (JSON.stringify(manifest[key]) !== JSON.stringify(value)) {
    throw new Error(`package.json ${key} does not match the release contract.`);
  }
}
if (!/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(manifest.version)) {
  throw new Error(`package.json version is not a supported semantic version: ${manifest.version}`);
}

const declarationContract = [
  "export interface ThumbnailOptions",
  'export type ThumbnailFit = "contain";',
  'export type ThumbnailFilter = "area";',
  'export type ThumbnailOutputFormat = "png" | "rgba";',
  "options?: ThumbnailOptions | null,",
];
for (const expected of declarationContract) {
  if (!declarations.includes(expected)) {
    throw new Error(`TypeScript declarations are missing: ${expected}`);
  }
}
if ((declarations.match(/export function thumbnailPng\(/g) ?? []).length !== 1) {
  throw new Error("TypeScript declarations must export exactly one thumbnailPng signature.");
}

const npmCommand = process.platform === "win32"
  ? process.env.ComSpec ?? "cmd.exe"
  : "npm";
const npmArguments = process.platform === "win32"
  ? ["/d", "/s", "/c", "npm.cmd pack --dry-run --json"]
  : ["pack", "--dry-run", "--json"];
const result = spawnSync(npmCommand, npmArguments, {
  cwd: packageDirectory,
  encoding: "utf8",
  env: {
    ...process.env,
    npm_config_cache: path.join(root, "target", "npm-cache"),
  },
});
if (result.error) {
  throw result.error;
}
if (result.status !== 0) {
  process.stderr.write(result.stderr);
  throw new Error(`npm pack exited with status ${result.status}`);
}

const [report] = JSON.parse(result.stdout);
const expectedFiles = [
  "LICENSE-APACHE",
  "LICENSE-MIT",
  "README.md",
  "package.json",
  "streamthumb_wasm.d.ts",
  "streamthumb_wasm.js",
  "streamthumb_wasm_bg.wasm",
  "streamthumb_wasm_bg.wasm.d.ts",
];
const actualFiles = report.files.map(({ path: file }) => file).sort();

if (JSON.stringify(actualFiles) !== JSON.stringify(expectedFiles)) {
  throw new Error(
    `Unexpected package contents:\n${actualFiles.map((file) => `- ${file}`).join("\n")}`,
  );
}
if (report.unpackedSize > 500_000) {
  throw new Error(`Unpacked package exceeds 500,000 bytes: ${report.unpackedSize}`);
}

console.log(
  `Verified ${report.id}: ${report.size} packed bytes, ${report.unpackedSize} unpacked bytes`,
);
