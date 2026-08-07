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
const packageReadme = await readFile(
  path.join(packageDirectory, "README.md"),
  "utf8",
);
const cargoToml = await readFile(path.join(root, "Cargo.toml"), "utf8");

const expectedManifest = {
  name: "@streamthumb/wasm",
  license: "MIT OR Apache-2.0",
  type: "module",
  description:
    "Memory-bounded PNG thumbnail generation with PNG and JPEG output for WebAssembly",
  main: "./streamthumb_wasm.js",
  module: "./streamthumb_wasm.js",
  types: "./streamthumb_wasm.d.ts",
  homepage: "https://github.com/piyohogeo/streamthumb#readme",
  bugs: "https://github.com/piyohogeo/streamthumb/issues",
};

for (const [key, value] of Object.entries(expectedManifest)) {
  if (manifest[key] !== value) {
    throw new Error(`package.json ${key} must be ${JSON.stringify(value)}`);
  }
}
const workspaceSection = cargoToml.match(
  /^\[workspace\.package\]\s*$([\s\S]*?)(?=^\[|(?![\s\S]))/m,
)?.[1];
const workspaceVersion = workspaceSection?.match(
  /^version\s*=\s*"([^"]+)"\s*$/m,
)?.[1];
if (!workspaceVersion || manifest.version !== workspaceVersion) {
  throw new Error(
    `package.json version ${manifest.version} does not match workspace version ${workspaceVersion}`,
  );
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
const expectedKeywords = ["png", "jpeg", "thumbnail", "webassembly", "wasm", "image"];
if (JSON.stringify(manifest.keywords) !== JSON.stringify(expectedKeywords)) {
  throw new Error("package.json keywords do not match the release contract.");
}

for (const expected of [
  "npm install @streamthumb/wasm",
  "result.free();",
  "https://github.com/piyohogeo/streamthumb/blob/main/docs/WASM_API.md",
  "## Node.js and Deno",
  "does not perform linear-light conversion",
]) {
  if (!packageReadme.includes(expected)) {
    throw new Error(`Published README is missing: ${expected}`);
  }
}

for (const license of ["LICENSE-MIT", "LICENSE-APACHE"]) {
  const [sourceLicense, packageLicense] = await Promise.all([
    readFile(path.join(root, license)),
    readFile(path.join(packageDirectory, license)),
  ]);
  if (!sourceLicense.equals(packageLicense)) {
    throw new Error(`${license} does not match the repository license text.`);
  }
}

const declarationContract = [
  "export interface ThumbnailOptions",
  'export type ThumbnailFit = "contain" | "cover";',
  'export type ThumbnailFilter = "area";',
  'export type ThumbnailOutputFormat = "png" | "jpeg" | "rgba";',
  'export type PngColorMode = "auto" | "rgba8" | "rgb8" | "grayscale-alpha8" | "grayscale8";',
  'export type PngCompression = "none" | "fastest" | "fast" | "balanced" | "high";',
  'export type PngFilter = "default" | "none" | "sub" | "up" | "average" | "paeth" | "adaptive" | "min-entropy";',
  "export interface PngOptions",
  'export type JpegSubsampling = "420" | "422" | "444";',
  "export interface JpegOptions",
  'export type OutputDelivery = "buffered" | "chunks";',
  "export interface ThumbnailPlanInput",
  "export interface ThumbnailPlanOutput",
  "export interface ThumbnailMemoryPlan",
  "export interface ThumbnailPlan",
  "delivery?: OutputDelivery | null,",
  "options?: ThumbnailOptions | null,",
  "export type ThumbnailChunkCallback = (chunk: Uint8Array) => void;",
  "onChunk: ThumbnailChunkCallback,",
  "export class ThumbnailResult",
  "export class ChunkedThumbnailResult",
  "readonly bytesWritten: number;",
  "readonly chunkCount: number;",
  "[Symbol.dispose](): void;",
  "export function streamthumbVersion(): string;",
  "export function wasmMemoryBytes(): number;",
  "export function initSync(",
  "export default function __wbg_init",
];
for (const expected of declarationContract) {
  if (!declarations.includes(expected)) {
    throw new Error(`TypeScript declarations are missing: ${expected}`);
  }
}
if ((declarations.match(/export function thumbnailPng\(/g) ?? []).length !== 1) {
  throw new Error("TypeScript declarations must export exactly one thumbnailPng signature.");
}
if ((declarations.match(/export function planThumbnailPng\(/g) ?? []).length !== 1) {
  throw new Error("TypeScript declarations must export exactly one planThumbnailPng signature.");
}
if ((declarations.match(/export function thumbnailPngToChunks\(/g) ?? []).length !== 1) {
  throw new Error("TypeScript declarations must export exactly one thumbnailPngToChunks signature.");
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
if (report.unpackedSize > 550_000) {
  throw new Error(`Unpacked package exceeds 550,000 bytes: ${report.unpackedSize}`);
}

console.log(
  `Verified ${report.id}: ${report.size} packed bytes, ${report.unpackedSize} unpacked bytes`,
);
